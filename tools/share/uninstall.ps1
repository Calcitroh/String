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
    [ValidateSet('ask', 'keep')][string]$DataChoice = 'ask',
    [switch]$BackupFunctionsOnly
)
$backupDirs = [Collections.Generic.Dictionary[string,IDisposable]]::new([StringComparer]::Ordinal)
$backupFiles = [Collections.Generic.List[IDisposable]]::new()
function Close-Backups {
    foreach ($file in $backupFiles) { $file.Dispose() }
    $backupFiles.Clear()
    foreach ($dir in $backupDirs.Values) { $dir.Dispose() }
    $backupDirs.Clear()
}
# Pin from the root down. OPEN_REPARSE_POINT opens the directory itself;
# denying write/delete sharing keeps it from being redirected after the check.
function Pin-BackupDir([string]$dir) {
    if ($backupDirs.ContainsKey($dir)) { return }
    $parent = [IO.Path]::GetDirectoryName($dir)
    if ($parent) {
        Pin-BackupDir $parent
        [void][IO.Directory]::CreateDirectory($dir)
    }
    $handle = [StringSetup.Backup]::CreateFileW($dir, 0x81, 1, [IntPtr]::Zero, 3, 0x02200000, [IntPtr]::Zero)
    if ($handle.IsInvalid) { $handle.Dispose(); throw "Cannot secure the backup folder $dir." }
    $backupDirs.Add($dir, $handle)
    $attrs = [StringSetup.Backup]::Attributes($handle)
    if (($attrs -band 0x410) -ne 0x10) { throw "$dir is not a regular directory or is a folder link." }
}

function Initialize-Backup {
    if ('StringSetup.Backup' -as [type]) { return }
    Add-Type -Namespace StringSetup -Name Backup -MemberDefinition @'
[DllImport("kernel32.dll", CharSet=CharSet.Unicode, ExactSpelling=true, SetLastError=true)]
public static extern Microsoft.Win32.SafeHandles.SafeFileHandle CreateFileW(string path, uint access, uint share,
    IntPtr security, uint disposition, uint flags, IntPtr template);

[DllImport("kernel32.dll", SetLastError=true)]
private static extern bool GetFileInformationByHandleEx(Microsoft.Win32.SafeHandles.SafeFileHandle file,
    int kind, out long info, uint size);

public static uint Attributes(Microsoft.Win32.SafeHandles.SafeFileHandle file) {
    long info;
    if (!GetFileInformationByHandleEx(file, 9, out info, 8))
        throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    return (uint)info;
}

[StructLayout(LayoutKind.Sequential)]
private struct IoStatus { public IntPtr status, information; }

[DllImport("ntdll.dll", ExactSpelling=true)]
private static extern int NtSetInformationFile(Microsoft.Win32.SafeHandles.SafeFileHandle file,
    out IoStatus io, byte[] buffer, int length, int kind);
[DllImport("ntdll.dll", ExactSpelling=true)]
private static extern uint RtlNtStatusToDosError(int status);

// MoveFile resolves the destination by opening its parent for write. During a
// guarded install that parent is pinned. A leaf-only FileRenameInformation with
// no root handle renames within the source directory and never replaces a target.
public static void RenameSibling(string source, string destination) {
    if (!String.Equals(System.IO.Path.GetDirectoryName(source), System.IO.Path.GetDirectoryName(destination),
            StringComparison.Ordinal)) throw new System.IO.IOException("Expected sibling directories.");
    byte[] name = System.Text.Encoding.Unicode.GetBytes(System.IO.Path.GetFileName(destination));
    int lengthOffset = 2 * IntPtr.Size;
    byte[] info = new byte[lengthOffset + 4 + name.Length + 2];
    BitConverter.GetBytes(name.Length).CopyTo(info, lengthOffset);
    name.CopyTo(info, lengthOffset + 4);
    using (var handle = CreateFileW(source, 0x10080, 7, IntPtr.Zero, 3, 0x02200000, IntPtr.Zero)) {
        if (handle.IsInvalid) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        if ((Attributes(handle) & 0x410) != 0x10) throw new System.IO.IOException("Expected a regular directory.");
        IoStatus io;
        int status = NtSetInformationFile(handle, out io, info, info.Length, 10);
        if (status < 0) throw new System.ComponentModel.Win32Exception((int)RtlNtStatusToDosError(status));
    }
}

// Under .NET Framework, File.Open does not accept a named stream pathname.
public static System.IO.FileStream OpenStream(string path, bool create) {
    var handle = CreateFileW(path, create ? 0xc0000000u : 0x80000000u, 1, IntPtr.Zero,
        create ? 1u : 3u, 0x00200000, IntPtr.Zero);
    if (handle.IsInvalid) {
        int error = Marshal.GetLastWin32Error();
        handle.Dispose();
        throw new System.ComponentModel.Win32Exception(error);
    }
    try {
        if ((Attributes(handle) & 0x410) != 0) throw new System.IO.IOException("Expected a regular file.");
        return new System.IO.FileStream(handle, create ? System.IO.FileAccess.ReadWrite : System.IO.FileAccess.Read);
    } catch { handle.Dispose(); throw; }
}
'@
}

function Named-Streams([string]$path) {
    foreach ($entry in Get-Item -Force -LiteralPath $path -Stream * -ErrorAction Stop) {
        if ($entry.Stream -and $entry.Stream -cne ':$DATA') { $entry.Stream }
    }
}

# Keep a guarded, verified copy of every data stream, including empty ones.
function Save-Backup([string]$source, [string]$dest) {
    Pin-BackupDir ([IO.Path]::GetDirectoryName($dest))
    $hashes = [Collections.Generic.Dictionary[string,string]]::new([StringComparer]::Ordinal)
    foreach ($stream in @('') + @(Named-Streams $source)) {
        $suffix = if ($stream) { ':' + $stream } else { '' }
        $inputFile = [StringSetup.Backup]::OpenStream($source + $suffix, $false)
        try {
            $copy = [StringSetup.Backup]::OpenStream($dest + $suffix, $true)
            $backupFiles.Add($copy)
            $inputFile.CopyTo($copy)
            $copy.Flush($true)
            $copy.Position = 0
            $inputFile.Position = 0
            $digest = [BitConverter]::ToString($sha.ComputeHash($copy))
            if ($digest -cne [BitConverter]::ToString($sha.ComputeHash($inputFile))) {
                throw "Backup verification failed for $source."
            }
            $hashes.Add($stream, $digest)
        } finally { $inputFile.Dispose() }
    }
    return $hashes
}

function Matches-Backup([string]$path, $hashes) {
    $streams = @('') + @(Named-Streams $path)
    if ($streams.Count -ne $hashes.get_Count()) { return $false }
    foreach ($stream in $streams) {
        if (-not $hashes.ContainsKey($stream)) { return $false }
        $suffix = if ($stream) { ':' + $stream } else { '' }
        $file = [StringSetup.Backup]::OpenStream($path + $suffix, $false)
        try {
            if ([BitConverter]::ToString($sha.ComputeHash($file)) -cne $hashes[$stream]) { return $false }
        } finally { $file.Dispose() }
    }
    return $true
}

# The installer imports the backup functions from the verified incoming package.
if ($BackupFunctionsOnly) { return }

$ErrorActionPreference = 'Stop'
$To = [IO.Path]::GetFullPath($To).TrimEnd('\')
$Data = [IO.Path]::GetFullPath($Data).TrimEnd('\')
$exe = Join-Path $To 'String.exe'
$list = Join-Path $To 'setup\files.txt'
$self = Join-Path $To 'setup\uninstall.ps1'
$mark = Join-Path $To 'setup\installed.txt'

function Say([string]$text) { Write-Host "  $text" }
function Done([string]$text, [int]$code) { Close-Backups; Write-Host ''; Say $text; Write-Host ''; if ($DataChoice -eq 'ask') { Read-Host '  Press Enter to close' | Out-Null }; exit $code }
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
    if ((Has $self) -and (Has $list) -and (Has $mark) -and -not (IsLink $To) -and -not (IsLink $setup) -and
        -not (IsLink $self) -and -not (IsLink $list) -and -not (IsLink $mark) -and ([IO.File]::ReadAllText($mark).Trim() -ieq $To)) {
        # A native release has far fewer files than the bundled web runtime. Check the manifest's own count.
        $lines = [IO.File]::ReadAllLines($list)
        $rows = @($lines -match '^[0-9A-F]{64}\t\d+\t.+$')
        $madeByInstaller = $lines.Count -eq $rows.Count + 1 -and $lines[-1] -ceq "#end`t$($rows.Count)" -and
            ($rows -match '\tString\.exe$') -and ($rows -match '\tsetup\\uninstall\.ps1$')
    }
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
foreach ($line in $rows) {
    $p = $line.Split("`t")
    if ($p.Count -ne 3) { continue }
    $full = Inside $To $p[2]
    if (-not $full -or -not [IO.File]::Exists($full) -or (IsLink $full) -or -not (NoLinks $To $full)) { continue }
    $t = New-Object PSObject -Property @{ Full = $full; Rel = $full.Substring($To.Length + 1); Size = [int64]$p[1]; Hash = $p[0]; Copy = $null }
    $targets += $t
    try { if ((New-Object IO.FileInfo $full).Length -ne $t.Size -or (Sha256 $full) -ne $t.Hash -or @(Named-Streams $full).Count) { $changed += $t } }
    catch { $changed += $t }   # unreadable: try to copy it, which says why if it can't
}
$stamp = (Get-Date -Format 'yyyy-MM-dd HH-mm-ss') + ' (uninstalled)'
$edits = Join-Path "$To edits" $stamp
for ($n = 2; Has $edits; $n++) { $edits = Join-Path "$To edits" "$stamp ($n)" }
foreach ($t in $changed) {
    $dst = Join-Path $edits $t.Rel
    try { Initialize-Backup; $t.Copy = Save-Backup $t.Full $dst }
    catch { Done "Your change to $($t.Rel) couldn't be saved to $edits, so nothing was removed. ($($_.Exception.Message))" 1 }
}
$saved = $changed.Count
if ($saved) { Say "Your own changes to String's files ($saved) are saved in: $edits" }
# Each file is checked again right before it goes: String's own unchanged, or identical to its saved copy. One that
# changed in the meantime (an editor saving it) stays.
function Remove-Verified($t) {
    try {
        $len = (New-Object IO.FileInfo $t.Full).Length
        $h = Sha256 $t.Full
        $same = $len -eq $t.Size -and $h -eq $t.Hash -and -not @(Named-Streams $t.Full).Count
        if (-not $same -and $t.Copy) { $same = Matches-Backup $t.Full $t.Copy }
        if ($same) { [IO.File]::SetAttributes($t.Full, [IO.FileAttributes]::Normal); [IO.File]::Delete($t.Full) }
    }
    catch { }
    return -not [IO.File]::Exists($t.Full)
}
$stuck = @()
foreach ($t in $targets) {
    if ($t.Full -ieq $self) { $selfFile = $t; continue }
    if (-not (Remove-Verified $t)) { $stuck += $t.Rel }
}
if (-not $stuck.Count -and $selfFile -and -not (Remove-Verified $selfFile)) { $stuck += $selfFile.Rel }
if ($stuck.Count) {
    $where = ''; if ($saved) { $where = " Your changes so far are saved in: $edits" }
    Done "Some of String's files are still in use or were just changed ($(($stuck | Select-Object -First 3) -join ', ')), so String isn't fully removed yet. Close what uses them (or restart the PC), then uninstall it again from Settings > Apps.$where" 1
}

# then the shortcuts and the Apps entry (only String's own, pointing at this folder)
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
foreach ($f in $list, $mark) {
    try { if (-not @(Named-Streams $f).Count) { [IO.File]::Delete($f) } } catch { }
}
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
