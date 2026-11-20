# Removes String for this Windows user: the files the installer put in its folder (setup\files.txt lists them), its
# shortcuts and its entry in Settings > Apps. Anything else in that folder stays, removal never goes through a folder
# link, and if a file can't be removed yet, the uninstaller and its Apps entry stay so it can simply be run again.
# Song files are never touched; String files the person edited are copied to "<install folder> edits\" first. Likes,
# playlists and listening stats (%LOCALAPPDATA%\String) are deleted only if the person types yes when asked. Settings > Apps > String > Uninstall runs it. The parameters are only for testing.
# Keep this file plain ASCII and Windows PowerShell 5.1 compatible.
param(
    [string]$To = (Split-Path $PSScriptRoot -Parent),
    [string]$Menu = [Environment]::GetFolderPath('Programs'),
    [string]$Desktop = [Environment]::GetFolderPath('Desktop'),
    [string]$AppsKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\String',
    [string]$Data = (Join-Path $env:LOCALAPPDATA 'String'),
    [ValidateSet('ask', 'keep')][string]$DataChoice = 'ask'
)
$ErrorActionPreference = 'Stop'
$To = [IO.Path]::GetFullPath($To).TrimEnd('\')
$Data = [IO.Path]::GetFullPath($Data).TrimEnd('\')
$exe = Join-Path $To 'String.exe'
$list = Join-Path $To 'setup\files.txt'
$self = Join-Path $To 'setup\uninstall.ps1'
$mark = Join-Path $To 'setup\installed.txt'

function Say([string]$text) { Write-Host "  $text" }
function Done([string]$text, [int]$code) { Write-Host ''; Say $text; Write-Host ''; if ($DataChoice -eq 'ask') { Read-Host '  Press Enter to close' | Out-Null }; exit $code }
function Has([string]$path) { Test-Path -LiteralPath $path }
function Under([string]$path, [string]$dir) { $path -and $path.StartsWith($dir + '\', [StringComparison]::OrdinalIgnoreCase) }
function IsLink([string]$path) { (Has $path) -and ([IO.File]::GetAttributes($path) -band [IO.FileAttributes]::ReparsePoint) -ne 0 }
function Inside([string]$root, [string]$rel) {
    if (-not $rel -or [IO.Path]::IsPathRooted($rel)) { return $null }
    try { $full = [IO.Path]::GetFullPath((Join-Path $root $rel)) } catch { return $null }
    if (Under $full $root) { return $full } else { return $null }
}
function NoLinks([string]$root, [string]$path) {
    if (IsLink $root) { return $false }
    for ($d = Split-Path $path -Parent; Under $d $root; $d = Split-Path $d -Parent) { if (IsLink $d) { return $false } }
    return $true
}
function Remove-Tree([string]$dir) {
    if (IsLink $dir) { [IO.Directory]::Delete($dir, $false); return }
    foreach ($e in [IO.Directory]::GetFileSystemEntries($dir)) {
        $a = [IO.File]::GetAttributes($e)
        if ($a -band [IO.FileAttributes]::Directory) { if ($a -band [IO.FileAttributes]::ReparsePoint) { [IO.Directory]::Delete($e, $false) } else { Remove-Tree $e } }
        else { [IO.File]::SetAttributes($e, [IO.FileAttributes]::Normal); [IO.File]::Delete($e) }
    }
    [IO.Directory]::Delete($dir, $false)
}
function Remove-Empty([string]$dir) {
    if (-not (Has $dir) -or (IsLink $dir)) { return }
    try { $subs = [IO.Directory]::GetDirectories($dir) } catch { return }
    foreach ($d in $subs) { Remove-Empty $d }
    try { [IO.Directory]::Delete($dir, $false) } catch { }
}
function Running { @(Get-Process -Name String -ErrorAction SilentlyContinue | Where-Object { Under $_.Path $To }) }
$sha = [Security.Cryptography.SHA256]::Create()
function Sha256([string]$path) {
    $s = [IO.File]::OpenRead($path)
    try { [BitConverter]::ToString($sha.ComputeHash($s)) -replace '-', '' } finally { $s.Dispose() }
}

# Only ever a copy String's installer made: its setup folder (not a link) holds this uninstaller, the file list and the
# installer's mark naming this folder. The unzipped copy has no mark. String.exe itself may already be gone when an
# earlier try stopped halfway.
$madeByInstaller = $false
try {
    $setup = Join-Path $To 'setup'
    $madeByInstaller = (Has $self) -and (Has $list) -and (Has $mark) -and -not (IsLink $To) -and -not (IsLink $setup) -and
        -not (IsLink $list) -and -not (IsLink $mark) -and ([IO.File]::ReadAllText($mark).Trim() -ieq $To) -and
        (@([IO.File]::ReadAllLines($list) | Where-Object { $_ -match '^[0-9A-F]{64}\t\d+\t' }).Count -ge 10)
}
catch { }
if (-not $madeByInstaller) { Done "This isn't an installed copy of String, so nothing was removed: $To" 1 }

Write-Host ''
Say 'Uninstalling String...'
if ((Running).Count) {
    Say 'Closing String first (it saves everything)...'
    $psi = New-Object Diagnostics.ProcessStartInfo $exe, '--quit'
    $psi.UseShellExecute = $false
    try { [void][Diagnostics.Process]::Start($psi).WaitForExit(15000) } catch { }
    for ($i = 0; $i -lt 60 -and (Running).Count; $i++) { Start-Sleep -Milliseconds 250 }
    if ((Running).Count) { Done 'String is still open. Close it, then uninstall again.' 1 }
}

# String's listed files, keeping the uninstaller, its list and the Apps entry until every one of them is gone. First,
# every listed file the person edited is copied to "<install folder> edits\<date time> (uninstalled)\"; if any copy
# fails, nothing is removed.
Set-Location -LiteralPath ([IO.Path]::GetTempPath())   # not inside the folder that is about to go
$targets = @(); $changed = @()
foreach ($line in [IO.File]::ReadAllLines($list)) {
    $p = $line.Split("`t")
    if ($p.Count -ne 3) { continue }
    $full = Inside $To $p[2]
    if (-not $full -or $full -ieq $self -or -not [IO.File]::Exists($full) -or -not (NoLinks $To $full)) { continue }
    $t = New-Object PSObject -Property @{ Full = $full; Rel = $full.Substring($To.Length + 1); Size = [int64]$p[1]; Hash = $p[0]; Copy = $null }
    $targets += $t
    try { if ((New-Object IO.FileInfo $full).Length -ne $t.Size -or (Sha256 $full) -ne $t.Hash) { $changed += $t } }
    catch { $changed += $t }   # unreadable: try to copy it, which says why if it can't
}
$stamp = (Get-Date -Format 'yyyy-MM-dd HH-mm-ss') + ' (uninstalled)'
$edits = Join-Path "$To edits" $stamp
for ($n = 2; Has $edits; $n++) { $edits = Join-Path "$To edits" "$stamp ($n)" }
foreach ($t in $changed) {
    $dst = Join-Path $edits $t.Rel
    try { [void][IO.Directory]::CreateDirectory((Split-Path $dst -Parent)); [IO.File]::Copy($t.Full, $dst, $false); $t.Copy = $dst }
    catch { Done "Your change to $($t.Rel) couldn't be saved to $edits, so nothing was removed. ($($_.Exception.Message))" 1 }
}
$saved = $changed.Count
if ($saved) { Say "Your own changes to String's files ($saved) are saved in: $edits" }
# Each file is checked again right before it goes: String's own unchanged, or identical to its saved copy. One that
# changed in the meantime (an editor saving it) stays.
$stuck = @()
foreach ($t in $targets) {
    try {
        $len = (New-Object IO.FileInfo $t.Full).Length
        $h = Sha256 $t.Full
        $same = $len -eq $t.Size -and $h -eq $t.Hash
        if (-not $same -and $t.Copy) { $same = (New-Object IO.FileInfo $t.Copy).Length -eq $len -and (Sha256 $t.Copy) -eq $h }
        if ($same) { [IO.File]::SetAttributes($t.Full, [IO.FileAttributes]::Normal); [IO.File]::Delete($t.Full) }
    }
    catch { }
    if ([IO.File]::Exists($t.Full)) { $stuck += $t.Rel }
}
if ($stuck.Count) {
    $where = ''; if ($saved) { $where = " Your changes so far are saved in: $edits" }
    Done "Some of String's files are still in use or were just changed ($(($stuck | Select-Object -First 3) -join ', ')), so String isn't fully removed yet. Close what uses them (or restart the PC), then uninstall it again from Settings > Apps.$where" 1
}

# then the shortcuts and the Apps entry (only String's own, pointing at this folder), then the uninstaller itself
$shell = New-Object -ComObject WScript.Shell
foreach ($dir in @($Menu, $Desktop)) {
    if (-not $dir) { continue }
    $lnk = Join-Path $dir 'String.lnk'
    try { if ((Has $lnk) -and ($shell.CreateShortcut($lnk).TargetPath -ieq $exe)) { Remove-Item -LiteralPath $lnk -Force } } catch { }
}
try {
    if ((Split-Path $AppsKey -Leaf) -eq 'String' -and (Has $AppsKey)) {
        $k = Get-ItemProperty -LiteralPath $AppsKey -ErrorAction SilentlyContinue
        if ($k.DisplayName -eq 'String' -and $k.InstallLocation -and [IO.Path]::GetFullPath($k.InstallLocation).TrimEnd('\') -ieq $To) { Remove-Item -LiteralPath $AppsKey -Force }
    }
}
catch { }
foreach ($f in $self, $list, $mark) { try { [IO.File]::Delete($f) } catch { } }
Remove-Empty $To
$left = ''
if (Has $To) { $left = " Files that weren't part of String (maybe your own) were left in: $To" }

# String's saved data: a folder named String holding its state.json or library.json, outside the program folder
$isData = (Split-Path $Data -Leaf) -eq 'String' -and -not (Under $Data $To) -and $Data -ine $To -and -not (IsLink $Data) -and ((Has (Join-Path $Data 'state.json')) -or (Has (Join-Path $Data 'library.json')))
$deleteData = $false
if ($DataChoice -eq 'ask' -and $isData) {
    Write-Host ''
    Say 'String is removed. Your song files stay where they are.'
    Say 'String also kept your likes, playlists, listening stats, lyrics, your look folder,'
    Say 'and the originals of song files whose names were written in. They are in:'
    Say "  $Data"
    $answer = Read-Host '  Delete those too? Type yes to delete them, or just press Enter to keep them'
    $deleteData = "$answer".Trim() -ieq 'yes'
}
if ($deleteData) {
    try { Remove-Tree $Data } catch { Done "String is uninstalled.$left Some of its saved data couldn't be removed; delete this folder yourself: $Data" 1 }
    Done "String is uninstalled, with your likes, playlists and stats.$left (Older daily copies stay in Documents\String Backup; delete that folder too if you want them gone.)" 0
}
Done "String is uninstalled.$left Your likes, playlists and stats are kept, in case you install it again." 0
