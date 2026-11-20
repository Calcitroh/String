# Installs the build in app-new\ over app\ (build it first: see README.md).
# A running String is asked to save and close first (app-new\String.exe --quit), then reopened.
# If an older copy ignores that request, files it has open are renamed to *.old (Windows allows that) and the new copy
# takes their place; the new version then starts the next time String opens, and it deletes the *.old leftovers itself:
# only those listed in app\update-old.txt (never other files that happen to end in .old).
# app\update-files.txt lists what this script installed; a file listed there that a newer build no longer has is removed
# (nothing else in app\ is: files this script didn't put there are left alone).
# The first install also adds String to the Start menu (search "String", pin it to the taskbar from there) and opens it.
$src = Join-Path $PSScriptRoot 'app-new'
$dst = Join-Path $PSScriptRoot 'app'
if (-not (Test-Path (Join-Path $src 'String.exe'))) { 'nothing to install: app-new\String.exe is missing (build it first)'; exit 1 }
# a build missing its parts would leave String without them: every build step first (README.md)
foreach ($need in 'wwwroot\index.html', 'wwwroot\string.js', 'presence\string-presence.exe') {
    if (-not (Test-Path (Join-Path $src $need))) { "app-new is missing $need`: run every build step first (see README.md)"; exit 1 }
}
$first = -not (Test-Path (Join-Path $dst 'String.exe'))
# only the String running from this app\ folder counts (not a self-test, nor a copy installed somewhere else)
function Get-OurString {
    $prefix = $dst.TrimEnd('\') + '\'
    Get-Process String -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) }
}
$wasRunning = [bool](Get-OurString)
if ($wasRunning) {
    Start-Process (Join-Path $src 'String.exe') '--quit' -Wait
    for ($i = 0; $i -lt 40 -and (Get-OurString); $i++) { Start-Sleep -Milliseconds 250 }
}
# a name for a file in use, moved aside: <file>.old, or <file>.2.old and so on if that's taken (never over a file there)
function FreeOld([string]$path) { $o = "$path.old"; for ($i = 2; Test-Path -LiteralPath $o; $i++) { $o = "$path.$i.old" }; $o }$copied = 0; $renamed = 0; $same = 0; $removed = 0
$olds = New-Object System.Collections.Generic.List[string]
$installed = New-Object System.Collections.Generic.List[string]
foreach ($f in Get-ChildItem $src -Recurse -File) {
    $rel = $f.FullName.Substring($src.Length + 1)
    $installed.Add($rel)
    $to = Join-Path $dst $rel
    if ((Test-Path $to) -and (Get-FileHash $to).Hash -eq (Get-FileHash $f.FullName).Hash) { $same++; continue }
    New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
    try { Copy-Item $f.FullName $to -Force -ErrorAction Stop; $copied++ }
    catch {
        $old = FreeOld $to
        Rename-Item $to (Split-Path $old -Leaf) -ErrorAction Stop
        $olds.Add($old.Substring($dst.Length + 1))
        Copy-Item $f.FullName $to -Force -ErrorAction Stop
        $renamed++
    }
}
# files the last install put here that this build doesn't have any more
$listFile = Join-Path $dst 'update-files.txt'
if (Test-Path -LiteralPath $listFile) {
    $now = @{}; foreach ($r in $installed) { $now[$r.ToLowerInvariant()] = 1 }
    $root = $dst.TrimEnd('\') + '\'
    foreach ($rel in Get-Content -LiteralPath $listFile) {
        $rel = $rel.Trim()
        if (-not $rel -or $now.ContainsKey($rel.ToLowerInvariant())) { continue }
        $full = [IO.Path]::GetFullPath((Join-Path $dst $rel))
        if (-not $full.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) -or -not (Test-Path -LiteralPath $full -PathType Leaf)) { continue }
        try { Remove-Item -LiteralPath $full -Force -ErrorAction Stop; $removed++ }
        catch { try { $old = FreeOld $full; Rename-Item -LiteralPath $full (Split-Path $old -Leaf) -ErrorAction Stop; $olds.Add($old.Substring($dst.Length + 1)); $removed++ } catch { } } # in use: goes at the next start
    }
}
Set-Content -LiteralPath $listFile -Value $installed -Encoding UTF8
if ($olds.Count) { Add-Content -LiteralPath (Join-Path $dst 'update-old.txt') -Value $olds -Encoding UTF8 }
# Start Menu entry, so it can be found by name and pinned to the taskbar
$lnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'String.lnk'
if (-not (Test-Path $lnk)) {
    $s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
    $s.TargetPath = Join-Path $dst 'String.exe'
    $s.WorkingDirectory = $dst
    $s.Description = 'String: your own songs, played like a streaming app'
    $s.Save()
}
$stillRunning = [bool](Get-OurString)
if (($wasRunning -or $first) -and -not $stillRunning) { Start-Process (Join-Path $dst 'String.exe') }
"updated $copied files, swapped $renamed in-use files, removed $removed old files, $same already current; " + $(if ($first) { 'first install: added to the Start menu and opened' } elseif (-not $wasRunning) { 'it was not running' } elseif ($stillRunning) { 'String is still open (it may be asking about unsaved changes): close it, then reopen' } else { 'reopened' })
