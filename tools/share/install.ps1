# Installs String for this Windows user (no admin needed) from the unzipped String.zip: the program in
# %LOCALAPPDATA%\Programs\String, a Start menu and a desktop shortcut, and an entry in Settings > Apps to uninstall it.
# "Install String.cmd" runs it. Running it again updates String; songs, likes, playlists and stats stay where they are
# (%LOCALAPPDATA%\String). The parameters are only for testing it somewhere harmless.
#
# Safe by design:
# - setup\files.txt (made with the zip) lists every file with its size and SHA-256. The unzipped files are checked
#   against it first, so a half-finished unzip never replaces a working String.
# - The new copy is made and checked in a folder beside the old one, then swapped in. A copy that fails (a full disk,
#   a locked file) leaves the installed String exactly as it was.
# - Only files listed in setup\files.txt are ever removed (by the next update or the uninstaller), only inside String's
#   folder, and never through a folder link (junction or symlink). Anything else stays.
# - The person's own changes survive an update: files they edited or added in the installed copy are first copied to
#   "<install folder> edits\<date time>\", then carried into the new copy. Where the update changed the same file:
#   the look (CSS, pictures, fonts) stays theirs, with the update's version saved in "<date time> (the update's
#   versions)\"; anything else (code) takes the update's version, since an old one could break String. The old copy
#   is then removed file by file, only what is String's unchanged or identical to its saved copy.
# Keep this file plain ASCII and Windows PowerShell 5.1 compatible: that is the PowerShell every Windows PC has.
param(
    [string]$To = (Join-Path $env:LOCALAPPDATA 'Programs\String'),
    [string]$Menu = [Environment]::GetFolderPath('Programs'),
    [string]$Desktop = [Environment]::GetFolderPath('Desktop'),
    [string]$AppsKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\String',
    [switch]$NoStart
)
$ErrorActionPreference = 'Stop'
$from = [IO.Path]::GetFullPath((Split-Path $PSScriptRoot -Parent)).TrimEnd('\')   # the "app" folder from the zip
$To = [IO.Path]::GetFullPath($To).TrimEnd('\')
$exe = Join-Path $To 'String.exe'
$tag = [Guid]::NewGuid().ToString('N').Substring(0, 8)
$new = "$To.new-$tag"
$old = "$To.old-$tag"
$unzipAgain = 'String.zip did not unzip completely. Unzip it again (right-click it, Extract All) and run "Install String" from the new folder.'

function Say([string]$text) { Write-Host "  $text" }
function Fail([string]$text) { Write-Host ''; Write-Host "  $text" -ForegroundColor Yellow; Write-Host ''; exit 1 }
function Has([string]$path) { Test-Path -LiteralPath $path }
function Under([string]$path, [string]$dir) { $path -and $path.StartsWith($dir + '\', [StringComparison]::OrdinalIgnoreCase) }
function IsLink([string]$path) { (Has $path) -and ([IO.File]::GetAttributes($path) -band [IO.FileAttributes]::ReparsePoint) -ne 0 }
# a relative path from a file list, as a full path that stays inside $root (null otherwise)
function Inside([string]$root, [string]$rel) {
    if (-not $rel -or [IO.Path]::IsPathRooted($rel)) { return $null }
    try { $full = [IO.Path]::GetFullPath((Join-Path $root $rel)) } catch { return $null }
    if (Under $full $root) { return $full } else { return $null }
}
# true if no folder between $root and $path (or $root itself) is a link
function NoLinks([string]$root, [string]$path) {
    if (IsLink $root) { return $false }
    for ($d = Split-Path $path -Parent; Under $d $root; $d = Split-Path $d -Parent) { if (IsLink $d) { return $false } }
    return $true
}
# The entries of a folder's setup\files.txt: hash, size, relative path (cleaned up), full path; only paths inside the
# folder. With -Strict (the unzipped copy), any odd line, an entry outside the folder, or a missing or wrong "#end"
# count line means the list itself is damaged, and nothing is returned.
function Listed([string]$root, [switch]$Strict) {
    $out = @(); $end = -1
    foreach ($line in [IO.File]::ReadAllLines((Join-Path $root 'setup\files.txt'))) {
        $p = $line.Split("`t")
        if ($p.Count -eq 2 -and $p[0] -eq '#end') { $end = [int]$p[1]; continue }
        if ($p.Count -ne 3 -or $p[0] -notmatch '^[0-9A-F]{64}$' -or $p[1] -notmatch '^\d+$') { if ($Strict) { return @() } else { continue } }
        $full = Inside $root $p[2]
        if (-not $full) { if ($Strict) { return @() } else { continue } }
        $out += New-Object PSObject -Property @{ Hash = $p[0]; Size = [int64]$p[1]; Rel = $full.Substring($root.Length + 1); Full = $full }
    }
    if ($Strict -and $end -ne $out.Count) { return @() }
    return $out
}
# deletes a folder this script made itself; a link inside is removed as a link, never followed
function Remove-Tree([string]$dir) {
    if (IsLink $dir) { [IO.Directory]::Delete($dir, $false); return }
    foreach ($e in [IO.Directory]::GetFileSystemEntries($dir)) {
        $a = [IO.File]::GetAttributes($e)
        if ($a -band [IO.FileAttributes]::Directory) { if ($a -band [IO.FileAttributes]::ReparsePoint) { [IO.Directory]::Delete($e, $false) } else { Remove-Tree $e } }
        else { [IO.File]::SetAttributes($e, [IO.FileAttributes]::Normal); [IO.File]::Delete($e) }
    }
    [IO.Directory]::Delete($dir, $false)
}
# every file under a folder, as paths relative to $root; folder links are not entered, file links are left out
function Walk([string]$root, [string]$dir) {
    foreach ($f in [IO.Directory]::GetFiles($dir)) { if (-not (IsLink $f)) { $f.Substring($root.Length + 1) } }
    foreach ($d in [IO.Directory]::GetDirectories($dir)) { if (-not (IsLink $d)) { Walk $root $d } }
}
# "a, b, c and 4 more"
function Short([string[]]$names) {
    $s = ($names | Select-Object -First 4) -join ', '
    if ($names.Count -gt 4) { $s += " and $($names.Count - 4) more" }
    return $s
}
# empty folders go, bottom up; links and folders with anything in them stay
function Remove-Empty([string]$dir) {
    if (-not (Has $dir) -or (IsLink $dir)) { return }
    try { $subs = [IO.Directory]::GetDirectories($dir) } catch { return }
    foreach ($d in $subs) { Remove-Empty $d }
    try { [IO.Directory]::Delete($dir, $false) } catch { }
}
# SHA-256 through .NET: Get-FileHash is a script cmdlet in Windows PowerShell 5.1 and goes missing when PowerShell is
# started with another PowerShell's module path
$sha = [Security.Cryptography.SHA256]::Create()
function Sha256([string]$path) {
    $s = [IO.File]::OpenRead($path)
    try { [BitConverter]::ToString($sha.ComputeHash($s)) -replace '-', '' } finally { $s.Dispose() }
}
# String copies running from the install folder or from the unzipped folder (another copy elsewhere is left alone)
function Running { @(Get-Process -Name String -ErrorAction SilentlyContinue | Where-Object { (Under $_.Path $To) -or (Under $_.Path $from) }) }
function WebView2Version {
    $guid = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    foreach ($k in "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$guid", "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$guid", "HKCU:\Software\Microsoft\EdgeUpdate\Clients\$guid") {
        $v = (Get-ItemProperty -LiteralPath $k -Name pv -ErrorAction SilentlyContinue).pv
        if ($v -and $v -ne '0.0.0.0') { return $v }
    }
    return $null
}

Write-Host ''
Say 'Installing String...'
foreach ($f in 'String.exe', 'wwwroot', 'setup\uninstall.ps1', 'setup\files.txt') { if (-not (Has (Join-Path $from $f))) { Fail $unzipAgain } }
# The build is for x64 PCs. Windows 11 on ARM runs x64 programs; Windows 10 on ARM doesn't.
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
$build = [Environment]::OSVersion.Version.Build
if ($arch -ne 'AMD64' -and -not ($arch -eq 'ARM64' -and $build -ge 22000)) { Fail "String needs 64-bit Windows 10 or 11 on an Intel or AMD PC, or Windows 11 on an ARM PC. This PC is $arch, Windows build $build." }
if ($from -ieq $To -or (Under $from $To) -or (Under $To $from)) { Fail "Run the installer from the unzipped folder, not from $To." }
if ((Has $To) -and -not (Has (Join-Path $To 'setup\files.txt')) -and [IO.Directory]::GetFileSystemEntries($To).Count) { Fail "$To already holds something that isn't String, so nothing was changed." }
if (IsLink $To) { Fail "$To is a folder link, so nothing was changed." }

Say 'Checking the unzipped files...'
try {
    $files = @(Listed $from -Strict)
    if ($files.Count -lt 10 -or -not ($files | Where-Object { $_.Rel -eq 'String.exe' })) { Fail $unzipAgain }
    foreach ($f in $files) {
        if (-not [IO.File]::Exists($f.Full) -or (New-Object IO.FileInfo $f.Full).Length -ne $f.Size -or (Sha256 $f.Full) -ne $f.Hash) { Fail $unzipAgain }
    }
}
catch { Fail "$unzipAgain ($($_.Exception.Message))" }

# An open String saves and closes when asked (String.exe --quit); started directly, so Windows asks nothing.
$running = Running
if ($running.Count) {
    Say 'Closing String first (it saves everything)...'
    $psi = New-Object Diagnostics.ProcessStartInfo $running[0].Path, '--quit'
    $psi.UseShellExecute = $false
    try { [void][Diagnostics.Process]::Start($psi).WaitForExit(15000) } catch { }
    for ($i = 0; $i -lt 60 -and (Running).Count; $i++) { Start-Sleep -Milliseconds 250 }
    if ((Running).Count) { Fail 'String is still open. Close it (right-click it on the taskbar, Close window), then run "Install String" again.' }
}

Say "Copying String to $To"
try {
    foreach ($f in $files) {
        $dst = Join-Path $new $f.Rel
        [void][IO.Directory]::CreateDirectory((Split-Path $dst -Parent))
        [IO.File]::Copy($f.Full, $dst, $true)
        if ((New-Object IO.FileInfo $dst).Length -ne $f.Size) { throw "incomplete: $($f.Rel)" }
    }
    [IO.File]::Copy((Join-Path $from 'setup\files.txt'), (Join-Path $new 'setup\files.txt'), $true)
    # the mark of an installed copy (the unzipped one has none), so the uninstaller never runs anywhere else
    [IO.File]::WriteAllText((Join-Path $new 'setup\installed.txt'), $To)
}
catch {
    if (Has $new) { try { Remove-Tree $new } catch { } }
    Fail "Copying String failed, so nothing was changed. Make sure the disk has 1 GB free and nothing has String's files open, then try again. ($($_.Exception.Message))"
}
# Files unzipped from a download carry a "came from the internet" mark, and Windows would warn about String.exe on
# every start. The installed copy is the one the person chose to install, so the mark comes off there.
# A drive that can't hold the mark (or a file that won't let go of it) isn't a reason to stop: String still works.
$stillMarked = 0
foreach ($f in Get-ChildItem -LiteralPath $new -Recurse -File -Force) {
    try { Unblock-File -LiteralPath $f.FullName -ErrorAction Stop } catch { $stillMarked++ }
}
if ($stillMarked) { Say "Note: the downloaded-file mark stayed on $stillMarked file(s), so Windows may ask before String starts. That's harmless." }

# The person's own changes to the installed copy: every file that isn't String's, unchanged, is theirs. All of them
# are copied out first; any failure stops here, with nothing changed.
$keptMine = @(); $tookNew = @(); $edits = $null; $theirs = $null; $cur = @{}
$lookTypes = '.css', '.png', '.jpg', '.jpeg', '.gif', '.webp', '.avif', '.bmp', '.svg', '.ico', '.woff', '.woff2', '.ttf', '.otf'
if ((Has $To) -and (Has (Join-Path $To 'setup\files.txt'))) {
    $mine = @()
    try {
        foreach ($f in @(Listed $To)) { $cur[$f.Rel.ToLower()] = $f }
        $next = @{}; foreach ($f in $files) { $next[$f.Rel.ToLower()] = $f }
        foreach ($rel in @(Walk $To $To)) {
            $k = $rel.ToLower()
            if ($k -eq 'setup\files.txt' -or $k -eq 'setup\installed.txt' -or (Split-Path $k -Leaf) -in 'desktop.ini', 'thumbs.db') { continue }
            $was = $cur[$k]
            $path = Join-Path $To $rel
            if ($was -and (New-Object IO.FileInfo $path).Length -eq $was.Size -and (Sha256 $path) -eq $was.Hash) { continue }   # String's own, unchanged
            $mine += $rel
        }
    }
    catch { try { Remove-Tree $new } catch { }; Fail "String couldn't read the files of the copy you have, so nothing was changed. ($($_.Exception.Message))" }
    if ($mine.Count) {
        Say "Keeping your own changes ($($mine.Count) files)..."
        # a folder of its own for this run; the update's versions of files they edited go in a second one beside it
        $stamp = Get-Date -Format 'yyyy-MM-dd HH-mm-ss'
        $edits = Join-Path "$To edits" $stamp
        for ($n = 2; (Has $edits) -or (Has "$edits (the update's versions)"); $n++) { $edits = Join-Path "$To edits" "$stamp ($n)" }
        $theirs = "$edits (the update's versions)"
        try {
            foreach ($rel in $mine) {
                $dst = Join-Path $edits $rel
                [void][IO.Directory]::CreateDirectory((Split-Path $dst -Parent))
                [IO.File]::Copy((Join-Path $To $rel), $dst, $false)   # never over an earlier copy (another run at the same second stops here instead)
            }
            foreach ($rel in $mine) {
                $k = $rel.ToLower(); $was = $cur[$k]; $now = $next[$k]
                $dst = Join-Path $new $rel
                $updated = $now -and (-not $was -or $now.Hash -ne $was.Hash)   # the update brings its own, different file here
                if ($updated -and -not ($was -and $lookTypes -contains [IO.Path]::GetExtension($rel).ToLower())) { $tookNew += $rel; continue }
                if ($updated) {
                    $cmp = Join-Path $theirs $rel
                    [void][IO.Directory]::CreateDirectory((Split-Path $cmp -Parent))
                    [IO.File]::Copy($dst, $cmp, $false)
                }
                [void][IO.Directory]::CreateDirectory((Split-Path $dst -Parent))
                [IO.File]::Copy((Join-Path $To $rel), $dst, $true)
                $keptMine += $rel
            }
        }
        catch { try { Remove-Tree $new } catch { }; Fail "Your own changes to String couldn't be copied, so nothing was changed. ($($_.Exception.Message))" }
    }
}

# swap the new copy in; the old one comes back if that fails
if (Has $To) {
    try { [IO.Directory]::Move($To, $old) }
    catch { try { Remove-Tree $new } catch { }; Fail 'Windows is still using String''s files (a String window, or one of its folders open somewhere). Close it and run "Install String" again. Nothing was changed.' }
}
try { [IO.Directory]::Move($new, $To) }
catch {
    if (Has $old) { [IO.Directory]::Move($old, $To) }
    try { Remove-Tree $new } catch { }
    Fail 'Installing String failed, so the copy you had stays as it was. Try again in a moment.'
}
# The previous copy goes, file by file: a file is removed only when it is String's own and unchanged, or identical to
# its saved copy. Anything else (saved after the copy was made, a link, unreadable) stays there for the person.
if (Has $old) {
    foreach ($rel in @(Walk $old $old)) {
        $p = Join-Path $old $rel; $k = $rel.ToLower()
        try {
            $len = (New-Object IO.FileInfo $p).Length
            $was = $cur[$k]
            $go = $k -eq 'setup\files.txt' -or $k -eq 'setup\installed.txt' -or (Split-Path $k -Leaf) -in 'desktop.ini', 'thumbs.db'
            if (-not $go -and $was -and $len -eq $was.Size) { $go = (Sha256 $p) -eq $was.Hash }
            if (-not $go -and $edits) {
                $b = Join-Path $edits $rel
                $go = [IO.File]::Exists($b) -and (New-Object IO.FileInfo $b).Length -eq $len -and (Sha256 $b) -eq (Sha256 $p)
            }
            if ($go) { [IO.File]::SetAttributes($p, [IO.FileAttributes]::Normal); [IO.File]::Delete($p) }
        }
        catch { }
    }
    Remove-Empty $old
    if (Has $old) { Say "Some files of the copy you had weren't String's or in the saved copy, so they were left here: $old" }
}
if ($edits) {
    if ($keptMine.Count) { Say "Kept your own changes: $(Short $keptMine)" }
    if ($tookNew.Count) { Say "This update also changed $(Short $tookNew), so String uses its new version there. Yours is in the copy below." }
    Say "A copy of everything you changed is in: $edits"
    if (Has $theirs) { Say "The update's versions of the look files you changed are in: $theirs" }
}

$shell = New-Object -ComObject WScript.Shell
foreach ($dir in @($Menu, $Desktop)) {
    if (-not $dir) { continue }
    try {
        [void][IO.Directory]::CreateDirectory($dir)
        $s = $shell.CreateShortcut((Join-Path $dir 'String.lnk'))
        $s.TargetPath = $exe
        $s.WorkingDirectory = $To
        $s.IconLocation = "$exe,0"
        $s.Description = 'String: your own songs, played like a streaming app'
        $s.Save()
    }
    catch { Say "(Windows didn't allow a shortcut in $dir; String works without it.)" }
}

# Settings > Apps entry, so String can be uninstalled like any other program (never over another program's entry)
$k = if (Has $AppsKey) { Get-ItemProperty -LiteralPath $AppsKey -ErrorAction SilentlyContinue } else { $null }
$theirs = $k -and $k.DisplayName -ne 'String'
# an entry for another String install that still exists elsewhere stays that install's
$another = $k -and $k.DisplayName -eq 'String' -and $k.InstallLocation -and ([IO.Path]::GetFullPath($k.InstallLocation).TrimEnd('\') -ine $To) -and (Has (Join-Path $k.InstallLocation 'setup\files.txt'))
if ((Split-Path $AppsKey -Leaf) -ne 'String' -or $theirs) { Say "(Not added to Settings > Apps: $AppsKey belongs to something else.)" }
elseif ($another) { Say "(Not added to Settings > Apps: it already lists the String in $($k.InstallLocation).)" }
else {
    if (-not (Has $AppsKey)) { [void](New-Item -Path $AppsKey -Force) }
    $version = ([Diagnostics.FileVersionInfo]::GetVersionInfo($exe).ProductVersion -split '\+')[0]
    $kb = [int]((Get-ChildItem -LiteralPath $To -Recurse -File -Force | Measure-Object Length -Sum).Sum / 1KB)
    Set-ItemProperty -LiteralPath $AppsKey -Name DisplayName -Value 'String'
    Set-ItemProperty -LiteralPath $AppsKey -Name DisplayVersion -Value $version
    Set-ItemProperty -LiteralPath $AppsKey -Name DisplayIcon -Value "$exe,0"
    Set-ItemProperty -LiteralPath $AppsKey -Name InstallLocation -Value $To
    Set-ItemProperty -LiteralPath $AppsKey -Name UninstallString -Value ('powershell.exe -NoProfile -ExecutionPolicy Bypass -File "' + (Join-Path $To 'setup\uninstall.ps1') + '"')
    Set-ItemProperty -LiteralPath $AppsKey -Name EstimatedSize -Value $kb -Type DWord
    Set-ItemProperty -LiteralPath $AppsKey -Name NoModify -Value 1 -Type DWord
    Set-ItemProperty -LiteralPath $AppsKey -Name NoRepair -Value 1 -Type DWord
}

Write-Host ''
Say 'Done. String is in your Start menu and on your desktop.'
if (-not (WebView2Version)) {
    Write-Host ''
    Write-Host '  One more thing: String also needs Microsoft Edge WebView2 Runtime, which this PC' -ForegroundColor Yellow
    Write-Host '  does not seem to have. Your browser now downloads it from Microsoft: run that file,' -ForegroundColor Yellow
    Write-Host '  then open String from the Start menu.' -ForegroundColor Yellow
    Write-Host ''
    if (-not $NoStart) { Start-Process 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' }
    exit 2
}
if (-not $NoStart) {
    $psi = New-Object Diagnostics.ProcessStartInfo $exe
    $psi.WorkingDirectory = $To
    [void][Diagnostics.Process]::Start($psi)
}
exit 0
