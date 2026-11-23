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
#   is then removed file by file, only what is String's unchanged or identical to its saved copy. On moving to
#   the native player, edits to retired program files and web assets stay in the edits folder.
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
function Fail([string]$text) { if ($backupDirs) { Close-Backups }; Write-Host ''; Write-Host "  $text" -ForegroundColor Yellow; Write-Host ''; exit 1 }
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
# A backup may be the last copy of an edited file. Check even the ancestors
# outside the install folder, including a reparse point with a missing target.
function Check-BackupPath([string]$path) {
    while ($path) {
        try {
            if ([IO.File]::GetAttributes($path) -band [IO.FileAttributes]::ReparsePoint) { throw "$path is a folder link." }
        } catch [IO.FileNotFoundException] { } catch [IO.DirectoryNotFoundException] { }
        $path = [IO.Path]::GetDirectoryName($path)
    }
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

function CheckNativeImports([string]$list) {
    if (-not ('StringSetup.Loader' -as [type])) {
        Add-Type -Namespace StringSetup -Name Loader -MemberDefinition @'
[DllImport("kernel32.dll", CharSet=CharSet.Unicode, ExactSpelling=true)]
public static extern IntPtr LoadLibraryExW(string name, IntPtr file, uint flags);
[DllImport("kernel32.dll", CharSet=CharSet.Ansi, ExactSpelling=true)]
public static extern IntPtr GetProcAddress(IntPtr module, string name);
[DllImport("kernel32.dll", EntryPoint="GetProcAddress", ExactSpelling=true)]
public static extern IntPtr GetOrdinal(IntPtr module, IntPtr ordinal);
[DllImport("kernel32.dll")] public static extern bool FreeLibrary(IntPtr module);
'@
    }
    $rows = [IO.File]::ReadAllLines($list)
    if (-not $rows.Count) { throw 'Missing list of Windows components.' }
    foreach ($row in $rows) {
        $parts = $row.Split(' ')
        $dll = $parts[0]
        if ($parts.Count -lt 2 -or $dll -notmatch '^[a-z0-9_-][a-z0-9_.-]*\.dll$') { throw 'Invalid list of Windows components.' }
        # LOAD_LIBRARY_SEARCH_SYSTEM32 also resolves Windows API-set names.
        $module = [StringSetup.Loader]::LoadLibraryExW($dll, [IntPtr]::Zero, 0x800)
        if ($module -eq [IntPtr]::Zero) { throw "Windows could not load $dll." }
        try {
            foreach ($name in $parts[1..($parts.Count - 1)]) {
                $proc = if ($name -match '^#([0-9]+)$') {
                    [StringSetup.Loader]::GetOrdinal($module, [IntPtr][int]$Matches[1])
                } else { [StringSetup.Loader]::GetProcAddress($module, $name) }
                if ($proc -eq [IntPtr]::Zero) { throw "$dll is missing $name." }
            }
        } finally { [void][StringSetup.Loader]::FreeLibrary($module) }
    }
}

Write-Host ''
Say 'Installing String...'
# The build is for x64 PCs. Windows 11 on ARM runs x64 programs; Windows 10 on ARM doesn't.
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
$build = [Environment]::OSVersion.Version.Build
if ($arch -ne 'AMD64' -and -not ($arch -eq 'ARM64' -and $build -ge 22000)) { Fail "String needs 64-bit Windows 10 or 11 on an Intel or AMD PC, or Windows 11 on an ARM PC. This PC is $arch, Windows build $build." }
if ($from -ieq $To -or (Under $from $To) -or (Under $To $from)) { Fail "Run the installer from the unzipped folder, not from $To." }
if ((Has $To) -and -not (Has (Join-Path $To 'setup\files.txt')) -and [IO.Directory]::GetFileSystemEntries($To).Count) { Fail "$To already holds something that isn't String, so nothing was changed." }
if (IsLink $To) { Fail "$To is a folder link, so nothing was changed." }
try { Check-BackupPath "$To edits" } catch { Fail "$($_.Exception.Message) Nothing was changed." }

Say 'Checking the unzipped files...'
try {
    $files = @(Listed $from -Strict)
    foreach ($required in 'String.exe', 'setup\install.ps1', 'setup\uninstall.ps1', 'setup\version.txt') {
        if ($files.Rel -notcontains $required) { Fail $unzipAgain }
    }
    foreach ($f in $files) {
        if (-not [IO.File]::Exists($f.Full) -or (New-Object IO.FileInfo $f.Full).Length -ne $f.Size -or (Sha256 $f.Full) -ne $f.Hash) { Fail $unzipAgain }
    }
    $version = [version]([IO.File]::ReadAllText((Join-Path $from 'setup\version.txt')).Trim())
    $needsWebView = $version.Major -lt 2
    if ($needsWebView -and -not ($files.Rel -like 'wwwroot\*')) { Fail $unzipAgain }
}
catch { Fail "$unzipAgain ($($_.Exception.Message))" }

# Load the backup functions from the incoming, manifest-checked setup scripts.
# Reject an incomplete or mixed package before closing the old player.
$backupScript = Join-Path $from 'setup\uninstall.ps1'
if (-not (Get-Command -Name $backupScript).Parameters.ContainsKey('BackupFunctionsOnly')) { Fail $unzipAgain }
. $backupScript -BackupFunctionsOnly -To $To -Menu $Menu -Desktop $Desktop -AppsKey $AppsKey -Data $env:LOCALAPPDATA -DataChoice keep
Initialize-Backup

if (-not $needsWebView) {
    if ($files.Rel -notcontains 'setup\imports.txt') { Fail $unzipAgain }
    if (-not [Environment]::Is64BitProcess) {
        # A 32-bit shell would check the 32-bit Windows DLLs.
        $ps64 = Join-Path ([Environment]::GetFolderPath('Windows')) 'Sysnative\WindowsPowerShell\v1.0\powershell.exe'
        $options = @('-To', $To, '-Menu', $Menu, '-Desktop', $Desktop, '-AppsKey', $AppsKey)
        if ($NoStart) { $options += '-NoStart' }
        & $ps64 -NoProfile -ExecutionPolicy Bypass -File $PSCommandPath @options
        exit $LASTEXITCODE
    }
    try { CheckNativeImports (Join-Path $from 'setup\imports.txt') }
    catch { Fail "This Windows installation cannot load String $version. $($_.Exception.Message) Update Windows and try again. Your existing player has not been changed." }
}

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

$savedHashes = [Collections.Generic.Dictionary[string,object]]::new([StringComparer]::Ordinal)

# The person's own changes to the installed copy: every file that isn't String's, unchanged, is theirs. All of them
# are copied out first; any failure stops here, with nothing changed.
$keptMine = @(); $tookNew = @(); $edits = $null; $theirs = $null; $cur = @{}
$lookTypes = '.css', '.png', '.jpg', '.jpeg', '.gif', '.webp', '.avif', '.bmp', '.svg', '.ico', '.woff', '.woff2', '.ttf', '.otf'
if ((Has $To) -and (Has (Join-Path $To 'setup\files.txt'))) {
    $mine = @()
    try {
        foreach ($f in @(Listed $To)) { $cur[$f.Rel.ToLower()] = $f }
        $fromWebPlayer = -not $needsWebView -and ($cur.Keys -like 'wwwroot\*')
        $next = @{}; foreach ($f in $files) { $next[$f.Rel.ToLower()] = $f }
        foreach ($rel in @(Walk $To $To)) {
            $k = $rel.ToLower()
            if ($k -eq 'setup\files.txt' -or $k -eq 'setup\installed.txt' -or (Split-Path $k -Leaf) -in 'desktop.ini', 'thumbs.db') { continue }
            $was = $cur[$k]
            $path = Join-Path $To $rel
            if ($was -and (New-Object IO.FileInfo $path).Length -eq $was.Size -and (Sha256 $path) -eq $was.Hash -and -not @(Named-Streams $path).Count) { continue }   # String's own, unchanged
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
                $savedHashes.Add($rel, (Save-Backup (Join-Path $To $rel) $dst))
            }
            foreach ($rel in $mine) {
                $k = $rel.ToLower(); $was = $cur[$k]; $now = $next[$k]
                # The old runtime and web pages are no longer loaded. Their edits are safe in $edits.
                if ($fromWebPlayer -and -not $now -and ($was -or $k -match '^(wwwroot|runtimes|presence)\\')) { continue }
                $dst = Join-Path $new $rel
                $updated = $now -and (-not $was -or $now.Hash -ne $was.Hash)   # the update brings its own, different file here
                if ($updated -and -not ($was -and $lookTypes -contains [IO.Path]::GetExtension($rel).ToLower())) { $tookNew += $rel; continue }
                if ($updated) {
                    $cmp = Join-Path $theirs $rel
                    [void](Save-Backup $dst $cmp)
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
    try { [StringSetup.Backup]::RenameSibling($To, $old) }
    catch { try { Remove-Tree $new } catch { }; Fail 'Windows is still using String''s files (a String window, or one of its folders open somewhere). Close it and run "Install String" again. Nothing was changed.' }
}
try { [StringSetup.Backup]::RenameSibling($new, $To) }
catch {
    if (Has $old) { [StringSetup.Backup]::RenameSibling($old, $To) }
    try { Remove-Tree $new } catch { }
    Fail 'Installing String failed, so the copy you had stays as it was. Try again in a moment.'
}
# The previous copy goes, file by file: a file is removed only when it is String's own and unchanged, or identical to
# its saved copy. Anything else (saved after the copy was made, a link, unreadable) stays there for the person.
if (Has $old) {
    foreach ($rel in @(Walk $old $old)) {
        $p = Join-Path $old $rel; $k = $rel.ToLower()
        try {
            if ((IsLink $p) -or -not (NoLinks $old $p)) { continue }
            $len = (New-Object IO.FileInfo $p).Length
            $was = $cur[$k]
            $go = $k -eq 'setup\files.txt' -or $k -eq 'setup\installed.txt' -or (Split-Path $k -Leaf) -in 'desktop.ini', 'thumbs.db'
            if (-not $go -and $was -and $len -eq $was.Size) { $go = (Sha256 $p) -eq $was.Hash }
            if ($go -and @(Named-Streams $p).Count) { $go = $false }
            # The verified backup and its parent directories are still held open.
            if (-not $go -and $savedHashes.ContainsKey($rel)) { $go = Matches-Backup $p $savedHashes[$rel] }
            if ($go) { [IO.File]::SetAttributes($p, [IO.FileAttributes]::Normal); [IO.File]::Delete($p) }
        }
        catch { }
    }
    Remove-Empty $old
    if (Has $old) { Say "Some files of the copy you had weren't String's or in the saved copy, so they were left here: $old" }
}
Close-Backups
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
# An update can bring a new icon, and Windows keeps drawing a program's old one (shortcuts, Start, the taskbar) until
# it's told icons changed: the shell's "associations changed" notice, and Windows' own icon cache refresh. Neither
# changes anything else, nothing here is waited for (a slow or stuck shell can't hold up the update or String's
# reopening), and any failure, even finding the refresh tool, is ignored. A pinned taskbar button's menu can still keep
# the old icon until Windows restarts.
try {
    Add-Type -Namespace StringSetup -Name Shell -MemberDefinition '[DllImport("shell32.dll")] public static extern void SHChangeNotify(int eventId, uint flags, System.IntPtr item1, System.IntPtr item2);'
    [StringSetup.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
}
catch { }
try {
    $refresh = Join-Path ([Environment]::GetFolderPath('System')) 'ie4uinit.exe'
    # A helper that outlives setup must not hold the updater's extracted directory open.
    if (Has $refresh) { [void](Start-Process $refresh -ArgumentList '-show' -WorkingDirectory ([IO.Path]::GetTempPath()) -WindowStyle Hidden -PassThru) }
}
catch { }

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
if ($needsWebView -and -not (WebView2Version)) {
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
