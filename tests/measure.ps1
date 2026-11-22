# Publish to app-new first. Results go to test-runs/measure-*/processes.csv and data/*-end.json.
# Median of three private working set samples in MiB; 100% CPU means one logical processor.
# -Trace captures Albums, Now Playing and Lyrics after their process samples (Node 22+).
param([string]$Build = 'app-new', [ValidateRange(5, 20)][int]$Seconds = 10, [switch]$LongHidden,
      [switch]$Trace, [ValidateRange(5, 25)][int]$SettleSeconds = 25)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$buildDir = (Resolve-Path -LiteralPath $Build).Path
if (!(Test-Path -LiteralPath (Join-Path $buildDir 'String.exe'))) { throw 'Publish String first.' }
$media = node (Join-Path $PSScriptRoot 'measure-library.js')
if ($LASTEXITCODE) { throw 'Could not prepare the measurement library.' }
$run = Join-Path $root ('test-runs\measure-' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff'))
$data = Join-Path $run 'data'
New-Item -ItemType Directory -Path $data | Out-Null
$app = Join-Path $run 'app'
Copy-Item -LiteralPath $buildDir -Destination $app -Recurse
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'measure-page.js') -Destination (Join-Path $app 'wwwroot\selftest.js')
@{ seconds = $Seconds; settleSeconds = $SettleSeconds; longHidden = [bool]$LongHidden } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $data 'measure.json') -Encoding Ascii
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class MeasureWindow {
    delegate bool Visit(IntPtr w, IntPtr p);
    [DllImport("user32")] static extern bool EnumWindows(Visit fn, IntPtr p);
    [DllImport("user32")] static extern uint GetWindowThreadProcessId(IntPtr w, out int id);
    [DllImport("user32", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr w, StringBuilder text, int len);
    [DllImport("user32")] static extern bool IsWindowVisible(IntPtr w);
    [DllImport("user32")] static extern bool ShowWindowAsync(IntPtr w, int cmd);
    public static void Show(int host, int cmd) {
        bool found = false;
        EnumWindows((w, p) => {
            int id; GetWindowThreadProcessId(w, out id);
            if (id != host || !IsWindowVisible(w)) return true;
            var title = new StringBuilder(256);
            if (GetWindowText(w, title, title.Capacity) == 0) return true;
            found = ShowWindowAsync(w, cmd);
            return !found;
        }, IntPtr.Zero);
        if (!found) throw new InvalidOperationException("Could not change the smoke window state.");
    }
}
'@
function Wait-Marker([string]$Name) {
    $deadline = [DateTime]::UtcNow.AddMinutes(3)
    while (!(Test-Path -LiteralPath (Join-Path $data "$Name.json"))) {
        $errFile = Join-Path $data 'measure-error.txt'
        if (Test-Path -LiteralPath $errFile) { throw (Get-Content -LiteralPath $errFile -Raw) }
        if ($p.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw "Did not reach $Name. See $run" }
        Start-Sleep -Milliseconds 120
    }
}
function Get-Roles {
    $roles = @{ $p.Id = 'host' }
    $queue = [Collections.Generic.Queue[int]]::new()
    $queue.Enqueue($p.Id)
    while ($queue.Count) {
        $parent = $queue.Dequeue()
        foreach ($child in (Get-CimInstance Win32_Process -Filter "ParentProcessId=$parent")) {
            $id = [int]$child.ProcessId
            if ($roles.ContainsKey($id)) { continue }
            $type = if ($child.CommandLine -match '--utility-sub-type=([^ .]+\.[^ .]+\.[^ "]+)') { $Matches[1] }
                elseif ($child.CommandLine -match '--type=([^ "]+)') { $Matches[1] }
                elseif ($child.Name -eq 'msedgewebview2.exe') { 'browser' } else { $child.Name }
            $roles[$id] = $type
            $queue.Enqueue($id)
        }
    }
    return $roles
}
function Get-Sample($Roles) {
    $filter = ($Roles.Keys | ForEach-Object { "IDProcess=$_" }) -join ' OR '
    $map = @{}
    foreach ($v in (Get-CimInstance Win32_PerfRawData_PerfProc_Process -Filter $filter)) { $map[[int]$v.IDProcess] = $v }
    return $map
}
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = Join-Path $app 'String.exe'
$start.UseShellExecute = $false
if ($Trace) { $start.EnvironmentVariables['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = '--remote-debugging-port=0' }
$timeout = if ($LongHidden) { 1000 } else { 700 }
$start.Arguments = "--smoke-test --data `"$data`" --out `"$run\out`" --test-media `"$media`" --timeout $timeout"
$p = [Diagnostics.Process]::Start($start)
$csv = Join-Path $run 'processes.csv'
try {
    foreach ($phase in 'playing','paused','minimised','browsed','albums','record','lyrics') {
        if ($phase -eq 'minimised') { Wait-Marker 'minimise'; [MeasureWindow]::Show($p.Id, 7) }
        if ($phase -eq 'browsed') { Wait-Marker 'restore'; [MeasureWindow]::Show($p.Id, 4) }
        Wait-Marker $phase
        for ($attempt = 1; $attempt -le 2; $attempt++) {
            $roles = Get-Roles
            $first = Get-Sample $roles
            Start-Sleep -Milliseconds ($Seconds * 500)
            $middle = Get-Sample $roles
            Start-Sleep -Milliseconds ($Seconds * 500)
            $last = Get-Sample $roles
            $endRoles = Get-Roles
            if (!(Compare-Object @($roles.Keys | Sort-Object) @($endRoles.Keys | Sort-Object))) { break }
            if ($attempt -eq 2) { throw "Process tree changed twice during $phase" }
            Write-Output "Process tree changed during $phase. Repeating the sample."
        }
        foreach ($id in $roles.Keys) {
            $a = $first[$id]; $b = $last[$id]
            if (!$a -or !$middle[$id] -or !$b) { throw "Missing counters for $id" }
            $cpu = 100.0 * ([double]$b.PercentProcessorTime - $a.PercentProcessorTime) / ([double]$b.Timestamp_Sys100NS - $a.Timestamp_Sys100NS)
            $memory = @($a.WorkingSetPrivate, $middle[$id].WorkingSetPrivate, $b.WorkingSetPrivate) | Sort-Object
            [pscustomobject]@{ Phase=$phase; PID=$id; Role=$roles[$id]; PrivateMiB=[math]::Round($memory[1] / 1MB, 2); CPU=[math]::Round($cpu, 2) } |
                Export-Csv -NoTypeInformation -Append -LiteralPath $csv
        }
        if ($Trace -and $phase -in 'albums','record','lyrics') {
            node (Join-Path $PSScriptRoot 'measure-trace.js') $run $phase
            if ($LASTEXITCODE) { throw "Could not trace $phase" }
        }
        Set-Content -LiteralPath (Join-Path $data "$phase.ack") -Value 'ok'
        Wait-Marker "$phase-end"
        Write-Output "Measured $phase"
    }
    $finishMs = if ($LongHidden) { 420000 } else { 20000 }
    if (!$p.WaitForExit($finishMs)) { throw 'Smoke window did not exit.' }
    $errFile = Join-Path $data 'measure-error.txt'
    if (Test-Path -LiteralPath $errFile) { throw (Get-Content -LiteralPath $errFile -Raw) }
    if ($LongHidden) { Get-Content -LiteralPath (Join-Path $data 'long-hidden.json') }
    Import-Csv -LiteralPath $csv | Sort-Object Role,Phase | Format-Table -AutoSize
    Write-Output $csv
} finally { if (!$p.HasExited) { $p.Kill(); $p.WaitForExit() } }
