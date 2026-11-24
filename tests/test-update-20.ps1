#Requires -Version 7
# Build both releases, or pass -LegacyZip and -NativeZip to replay already built archives.
param([string]$LegacyZip, [string]$NativeZip)
$ErrorActionPreference = 'Stop'
if ([bool]$LegacyZip -ne [bool]$NativeZip) { throw 'Supply both release zips, or omit both to build them.' }
$root = Split-Path $PSScriptRoot -Parent
$id = [guid]::NewGuid().ToString('N')
$run = Join-Path $root "test-runs\update-20-$id"
[void][IO.Directory]::CreateDirectory($run)
"Migration test files: $run"

function Check([bool]$pass, [string]$message) {
    if (-not $pass) { throw $message }
}
# The native release under test: the version the workspace builds (native\Cargo.toml).
$nativeVersion = [regex]::Match([IO.File]::ReadAllText((Join-Path $root 'native\Cargo.toml')),
    '(?m)^version = "([0-9]+\.[0-9]+\.[0-9]+)"').Groups[1].Value
Check ([bool]$nativeVersion) 'No version in native\Cargo.toml'
# The manual ZIP installer must import a script block made from verified bytes.
function CheckHelperImport {
    $installer = Join-Path $root 'tools\share\install.ps1'
    $ast = [Management.Automation.Language.Parser]::ParseFile($installer, [ref]$null, [ref]$null)
    $imports = @($ast.FindAll({ param($n) $n -is [Management.Automation.Language.CommandAst] -and
        $n.InvocationOperator -eq 'Dot' }, $true))
    Check ($imports.Count -eq 1 -and $imports[0].CommandElements[0].Extent.Text -ceq '$verifiedHelper') 'Setup imports the helper by pathname after checking it'
    $reader = $ast.Find({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Read-BackupHelper' }, $true)
    . ([scriptblock]::Create($reader.Extent.Text))
    $path = Join-Path $run 'helper.ps1'
    [IO.File]::WriteAllText($path, 'param([switch]$BackupFunctionsOnly) "verified"')
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $digest = [BitConverter]::ToString($sha.ComputeHash([IO.File]::ReadAllBytes($path))).Replace('-', '')
        $row = @{ Full = $path; Size = (Get-Item $path).Length; Hash = $digest }
        $helper = Read-BackupHelper $row
        [IO.File]::WriteAllText($path, '')
        Check ((& $helper.Script -BackupFunctionsOnly) -ceq 'verified') 'Import lost the verified helper after extraction changed'
        $refused = $false
        try { [void](Read-BackupHelper $row) } catch { $refused = $true }
        Check $refused 'Helper content inconsistent with its manifest row was accepted'
    } finally { $sha.Dispose() }
}
# Exercise just the backup functions on a private tree. Loading their definitions
# separately keeps the Windows setup entry point out of this sharing check.
function CheckBackupFunctions([string]$script, [string]$folder) {
    . $script -BackupFunctionsOnly -To $folder -Menu $folder -Desktop $folder -AppsKey 'HKCU:\unused' -Data $folder -DataChoice keep
    Initialize-Backup
    $sha = [Security.Cryptography.SHA256]::Create()
    $source = Join-Path $folder 'original'
    $dest = Join-Path $folder 'edits\saved'
    [void][IO.Directory]::CreateDirectory($folder)
    [IO.File]::WriteAllText($source, 'Edited content')
    Set-Content -LiteralPath $source -Stream 'Count' -Value 'user metadata' -NoNewline
    Set-Content -LiteralPath $source -Stream 'rating' -Value '5' -NoNewline
    try {
        $snapshot = Save-Backup $source $dest
        Check (Matches-Backup $source $snapshot) 'A complete backup failed verification'
        $writer = [StringSetup.Backup]::CreateFileW(($source + ':rating'), 2, 7, [IntPtr]::Zero, 3, 0, [IntPtr]::Zero)
        try {
            Check (-not $writer.IsInvalid) 'Could not open the fixture stream for editing'
            try { $retired = Remove-VerifiedFile $source $snapshot } catch { $retired = $false }
            Check (-not $retired -and [IO.File]::Exists($source)) 'A file with an open stream writer was retired'
        } finally { $writer.Dispose() }
        $dir = [IO.Path]::GetDirectoryName($dest)
        $probe = [StringSetup.Backup]::CreateFileW($dir, 0x10000, 7, [IntPtr]::Zero, 3, 0x02000000, [IntPtr]::Zero)
        try { $denied = $probe.IsInvalid } finally { $probe.Dispose() }
        $stream = Get-Content -LiteralPath $dest -Stream 'rating' -Raw -ErrorAction SilentlyContinue
        $first = [IO.Directory]::CreateDirectory((Join-Path $folder 'current')).FullName
        $second = Join-Path $folder 'previous'
        [StringSetup.Backup]::RenameSibling($first, $second)
        [void][IO.Directory]::CreateDirectory($first)
        $refused = $false
        try { [StringSetup.Backup]::RenameSibling($first, $second) } catch { $refused = $true }
        Check ($refused -and [IO.Directory]::Exists($first) -and [IO.Directory]::Exists($second)) 'Guarded swap replaced an existing directory'
        Set-Content -LiteralPath $source -Stream 'rating' -Value '6' -NoNewline
        Check (-not (Remove-VerifiedFile $source $snapshot)) 'A newer stream edit was eligible for retirement'
        Set-Content -LiteralPath $source -Stream 'rating' -Value '5' -NoNewline
        Set-Content -LiteralPath $source -Stream 'empty' -Value '' -NoNewline
        Check (-not (Remove-VerifiedFile $source $snapshot)) 'A new empty stream was eligible for retirement'
        Check ($denied -and $stream -ceq '5') "Backup contract failed (folder denies delete access: $denied, saved rating: '$stream')"
        Remove-Item -LiteralPath $source -Stream 'empty'
        # Inspect sharing during enumeration without changing the contents.
        $enumerate = ${function:Named-Streams}
        $probes = [Collections.Generic.List[bool]]::new()
        function Named-Streams([string]$path) {
            foreach ($suffix in '', ':rating', ':new') {
                $mode = if ($suffix -eq ':new') { 1 } else { 3 }
                $writer = [StringSetup.Backup]::CreateFileW(($path + $suffix), 2, 7, [IntPtr]::Zero, $mode, 0, [IntPtr]::Zero)
                try { $probes.Add($writer.IsInvalid) } finally { $writer.Dispose() }
            }
            & $enumerate $path
        }
        $retired = Remove-VerifiedFile $source $snapshot
        Check ($probes.Count -ge 3 -and -not $probes.Contains($false)) 'Verified streams allowed a writer during retirement'
        Check ($retired -and -not [IO.File]::Exists($source)) 'A complete matching backup did not allow retirement'
        [IO.File]::WriteAllText($source, 'Read only')
        $hash = [BitConverter]::ToString($sha.ComputeHash([IO.File]::ReadAllBytes($source))).Replace('-', '')
        [IO.File]::SetAttributes($source, [IO.FileAttributes]::ReadOnly)
        Check (Remove-VerifiedFile $source $null -OriginalHash $hash) 'A matching read-only file could not be retired'
    } finally { Close-Backups; $sha.Dispose() }
}
CheckBackupFunctions (Join-Path $root 'tools\share\uninstall.ps1') (Join-Path $run 'Backup functions')
CheckHelperImport

function Build([string]$script, [string[]]$options) {
    & (Join-Path $PSHOME 'pwsh.exe') -NoProfile -File (Join-Path $root "tools\$script") @options
    Check ($LASTEXITCODE -eq 0) "$script failed ($LASTEXITCODE)"
}
if (-not $LegacyZip) {
    Build 'release.ps1' @('-Version', '1.3.1')
    $LegacyZip = Join-Path $root 'release\String.zip'
}
# Both builders write release\String.zip. Save the original zip and its matching signature first.
$saved = Join-Path $run 'String-1.3.1.zip'
Copy-Item -LiteralPath $LegacyZip -Destination $saved
if (Test-Path -LiteralPath "$LegacyZip.sig") { Copy-Item -LiteralPath "$LegacyZip.sig" -Destination "$saved.sig" }
if (-not $NativeZip) {
    Build 'release-native.ps1' @('-Full', '-Version', $nativeVersion)
    $NativeZip = Join-Path $root 'release\String.zip'
}

function Extract([string]$zip, [string]$version, [string]$folder) {
    $zip = (Get-Item -LiteralPath $zip).FullName
    if (Test-Path -LiteralPath "$zip.sig") {
        $source = [IO.File]::ReadAllText((Join-Path $root 'src\Updater.cs'))
        $pub = [regex]::Match($source, 'const string UpdateKey = "([^"]+)"').Groups[1].Value
        $key = [Security.Cryptography.ECDsa]::Create()
        $stream = [IO.File]::OpenRead($zip)
        try {
            $key.ImportSubjectPublicKeyInfo([Convert]::FromBase64String($pub), [ref]$null)
            $sig = [Convert]::FromBase64String([IO.File]::ReadAllText("$zip.sig"))
            Check ($key.VerifyData($stream, $sig, [Security.Cryptography.HashAlgorithmName]::SHA256)) "Invalid signature on $zip"
        } finally { $key.Dispose(); $stream.Dispose() }
        "Verified the $version release signature."
    } else { "Unsigned $version build. Signature check skipped." }
    [IO.Compression.ZipFile]::ExtractToDirectory($zip, $folder)
    $inside = [IO.File]::ReadAllText((Join-Path $folder 'app\setup\version.txt')).Trim()
    Check ($inside -ceq $version) "Expected $version in $zip, got $inside"
    foreach ($name in 'Install String.cmd', 'Read me first.txt') {
        Check ([IO.File]::Exists((Join-Path $folder $name))) "Missing $name in $zip"
    }
}

# Windows PowerShell 5.1, as launched by the real updater. Every shell destination is private to this run.
$windowsPS = Join-Path ([Environment]::SystemDirectory) 'WindowsPowerShell\v1.0\powershell.exe'
$local = Join-Path $run 'Local'
$data = Join-Path $local 'String'
$testKey = "HKCU:\Software\StringMigrationTest-$id"
$keyPath = Join-Path $testKey 'String'
function Setup([string]$script, [string]$zone, [switch]$Remove, [switch]$Legacy, [string]$Refusal) {
    $start = [Diagnostics.ProcessStartInfo]::new($windowsPS)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.Environment['LOCALAPPDATA'] = $local
    [void]$start.Environment.Remove('PSModulePath')
    $argsPS = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $script,
        '-To', (Join-Path $zone 'Player'), '-Menu', (Join-Path $zone 'Menu'),
        '-Desktop', (Join-Path $zone 'Desktop'), '-AppsKey', $keyPath)
    $argsPS += if ($Remove) { '-Data', $data, '-DataChoice', 'keep' } else { '-NoStart' }
    foreach ($arg in $argsPS) { $start.ArgumentList.Add($arg) }
    $p = [Diagnostics.Process]::Start($start)
    try {
        $output = $p.StandardOutput.ReadToEnd()
        $p.WaitForExit()
        if ($Refusal) {
            Check ($p.ExitCode -eq 1 -and $output.Contains($Refusal)) "Expected setup to refuse '$Refusal', got exit $($p.ExitCode): $output"
            return
        }
        # 1.x can finish installing but report a missing WebView2 runtime. This test never starts 1.x.
        Check ($p.ExitCode -eq 0 -or ($Legacy -and $p.ExitCode -eq 2)) "Setup exit $($p.ExitCode): $output"
    } finally { $p.Dispose() }
    if ($Remove) { Check (-not (Test-Path -LiteralPath $keyPath)) 'Uninstall left the Apps entry' }
}

function CheckInstall([string]$zone, [string]$version) {
    $to = Join-Path $zone 'Player'
    $exe = Join-Path $to 'String.exe'
    Check (Test-Path -LiteralPath $keyPath) "Missing $version Apps entry"
    $entry = Get-ItemProperty -LiteralPath $keyPath
    Check ($entry.DisplayName -ceq 'String' -and $entry.InstallLocation -ceq $to) 'Incorrect Apps install location'
    Check ($entry.DisplayVersion -ceq $version) "Incorrect Apps version: $($entry.DisplayVersion)"
    $command = 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "' + (Join-Path $to 'setup\uninstall.ps1') + '"'
    Check ($entry.UninstallString -ceq $command) "Incorrect Apps uninstall command: $($entry.UninstallString)"
    Check ([IO.File]::ReadAllText((Join-Path $to 'setup\installed.txt')) -ieq $to) "Wrong install marker in $to"
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($exe)
    Check (($info.ProductVersion -split '\+')[0] -ceq $version) "Wrong exe product version: $($info.ProductVersion)"
    if ($version -ceq $nativeVersion) {
        $parts = [version]$version
        Check ($info.FileVersion -ceq $version -and $info.ProductMajorPart -eq $parts.Major -and
            $info.ProductMinorPart -eq $parts.Minor -and $info.ProductBuildPart -eq $parts.Build -and
            $info.ProductPrivatePart -eq 0) 'Wrong native VERSIONINFO'
    }
    $shell = New-Object -ComObject WScript.Shell
    foreach ($dir in 'Menu', 'Desktop') {
        $link = Join-Path $zone "$dir\String.lnk"
        Check ([IO.File]::Exists($link)) "Missing shortcut $link"
        $s = $shell.CreateShortcut($link)
        Check ($s.TargetPath -ieq $exe -and $s.WorkingDirectory -ieq $to -and $s.IconLocation -ieq "$exe,0") "Wrong shortcut $link"
    }
}
function Snapshot([string]$folder) {
    $map = @{}
    foreach ($f in Get-ChildItem -LiteralPath $folder -File -Force -Recurse) {
        $map[$f.FullName.Substring($folder.Length + 1)] = (Get-FileHash -LiteralPath $f.FullName).Hash
    }
    return $map
}
function SameFiles($before, $after, [string]$label) {
    Check ($before.Count -eq $after.Count) "File count changed in $label"
    foreach ($name in $before.Keys) { Check ($after[$name] -ceq $before[$name]) "File changed in $label : $name" }
}

# Model an older machine by adding a missing system import to a small private package.
# Its executable is inert. Only the installer runs, with the same manifest checks as a release.
function MissingDependencyPackage([string]$source, [string]$fixture, [string]$import) {
    $app = Join-Path $fixture 'app'
    $setup = [IO.Directory]::CreateDirectory((Join-Path $app 'setup')).FullName
    foreach ($name in 'install.ps1', 'uninstall.ps1', 'version.txt') {
        [IO.File]::Copy((Join-Path $source "setup\$name"), (Join-Path $setup $name))
    }
    [IO.File]::WriteAllText((Join-Path $app 'String.exe'), 'fixture')
    [IO.File]::WriteAllText((Join-Path $setup 'imports.txt'), $import)
    $rows = @(foreach ($f in Get-ChildItem -LiteralPath $app -Recurse -File) {
        "{0}`t{1}`t{2}" -f (Get-FileHash -LiteralPath $f.FullName).Hash, $f.Length, [IO.Path]::GetRelativePath($app, $f.FullName)
    })
    [IO.File]::WriteAllLines((Join-Path $setup 'files.txt'), [string[]]($rows + "#end`t$($rows.Count)"))
    return (Join-Path $setup 'install.ps1')
}

function CheckPreflight([string]$installer, [string]$zone, [string]$kind) {
    $to = Join-Path $zone 'Player'
    [void][IO.Directory]::CreateDirectory((Join-Path $to 'setup'))
    [IO.File]::WriteAllText((Join-Path $to 'setup\files.txt'), "#end`t0")
    [IO.File]::WriteAllText((Join-Path $to 'setup\version.txt'), '1.3.1')
    [IO.File]::WriteAllText((Join-Path $to 'String.exe'), 'old player fixture')
    $before = Snapshot $to
    if ($kind -in 'dependency', 'entry point') {
        $source = Split-Path (Split-Path $installer -Parent) -Parent
        $import = if ($kind -eq 'dependency') { 'string-test-missing.dll TestEntry' } else { 'icuuc.dll StringTestMissingEntry' }
        $message = if ($kind -eq 'dependency') { 'Windows could not load string-test-missing.dll' } else { 'icuuc.dll is missing StringTestMissingEntry' }
        $fixture = MissingDependencyPackage $source (Join-Path $zone 'Package') $import
        Setup $fixture $zone -Refusal $message
    } else {
        $target = [IO.Directory]::CreateDirectory((Join-Path $zone 'Empty')).FullName
        $link = if ($kind -eq 'parent') { "$zone alias" } else { "$to edits" }
        $dest = if ($kind -eq 'parent') { $zone } else { $target }
        [void](New-Item -ItemType Junction -Path $link -Target $dest)
        try {
            $via = if ($kind -eq 'parent') { $link } else { $zone }
            Setup $installer $via -Refusal 'folder link'
            Check ((Get-ChildItem -LiteralPath $target -Force).Count -eq 0) 'Setup wrote through the edits link'
        } finally { [IO.Directory]::Delete($link) }
    }
    SameFiles $before (Snapshot $to) "original player after $kind refusal"
}

$old = Join-Path $run '1.x'
$new = Join-Path $run '2.0'
Extract $saved '1.3.1' $old
Extract $NativeZip $nativeVersion $new
$installer = Join-Path $new 'app\setup\install.ps1'
$imports = Get-Content -LiteralPath (Join-Path $new 'app\setup\imports.txt')
Check (($imports -match '^icuuc\.dll .*\bunorm2_normalize\b') -and
    ($imports -match '^icuin\.dll .*\bucol_open\b')) 'The release is missing the ICU load-time requirements'
try {
    foreach ($kind in 'dependency', 'entry point', 'edits', 'parent') {
        CheckPreflight $installer (Join-Path $run "Preflight $kind") $kind
    }
    $clean = Join-Path $run 'Clean install'
    Setup $installer $clean
    CheckInstall $clean $nativeVersion
    Setup (Join-Path $clean 'Player\setup\uninstall.ps1') $clean -Remove
    Check (-not (Test-Path -LiteralPath (Join-Path $clean 'Player'))) 'Clean uninstall left program files'
    "Clean native install and uninstall passed."

    $zone = Join-Path $run 'Upgrade'
    $to = Join-Path $zone 'Player'
    Setup (Join-Path $old 'app\setup\install.ps1') $zone -Legacy
    CheckInstall $zone '1.3.1'
    $original = Snapshot $to
    foreach ($required in 'wwwroot\string.css', 'String.runtimeconfig.json', 'coreclr.dll', 'WebView2Loader.dll') {
        Check ($original.ContainsKey($required)) "Missing 1.x fixture file $required"
    }
    $fixtures = Join-Path $root 'test-runs\string-media'
    $names = 'glass orbit.mp3', 'low tide.flac', 'northern static.m4a', 'Album One\01 ogg song.ogg', 'Album One\02 opus song.opus'
    if ($names | Where-Object { -not [IO.File]::Exists((Join-Path $fixtures $_)) }) {
        & node (Join-Path $PSScriptRoot 'test-tags.js')
        Check ($LASTEXITCODE -eq 0) 'Could not make the tagged test songs'
    }
    $media = Join-Path $run 'Music'
    $songs = @(foreach ($name in $names) {
        $dest = Join-Path $media $name
        [void][IO.Directory]::CreateDirectory((Split-Path $dest -Parent))
        [IO.File]::Copy((Join-Path $fixtures $name), $dest)
        $dest
    })
    $songFiles = Snapshot $media
    [void][IO.Directory]::CreateDirectory((Join-Path $data 'look'))
    [void][IO.Directory]::CreateDirectory((Join-Path $data 'addons\fixture'))
    $songId = $songs[0].ToLowerInvariant()
    $at = 1577923200000
    $playlist = @{ id = 'saved-list'; name = 'Saved songs'; created = $at; updated = $at; items = @(@{id = $songId; at = $at}) }
    $state = @{ v = 1; files = $songs; liked = @{$songId = $at}; set = @{ phoneStats = $false }; playlists = @($playlist) }
    [IO.File]::WriteAllText((Join-Path $data 'state.json'), ($state | ConvertTo-Json -Depth 6))
    $day = @{ s = 37; h = @(37) + (@(0) * 23); a = @{'0' = 37}; t = @{'0' = 37} }
    $listening = @{ v = 1; artists = @('Test Signal'); tracks = ,@($songId, 'Glass Orbit', 'Test Signal'); days = @{'2020-01-02' = $day} }
    [IO.File]::WriteAllText((Join-Path $data 'listening.json'), ($listening | ConvertTo-Json -Depth 6))
    [IO.File]::WriteAllText((Join-Path $data 'library.json'), '{"v":1,"tracks":[]}')
    [IO.File]::WriteAllText((Join-Path $data 'look\look.css'), '/* personal look */')
    [IO.File]::WriteAllBytes((Join-Path $data 'addons\fixture\sample.bin'), [byte[]](0, 255, 32, 100))
    $before = Snapshot $data
    $css = 'wwwroot\string.css'
    [IO.File]::AppendAllText((Join-Path $to $css), "`r`n/* personal edit */`r`n")
    Set-Content -LiteralPath (Join-Path $to $css) -Stream 'notes' -Value 'Preserved metadata' -NoNewline
    $streamOnly = 'String.runtimeconfig.json'
    Set-Content -LiteralPath (Join-Path $to $streamOnly) -Stream 'rating' -Value '5' -NoNewline
    $edited = (Get-FileHash -LiteralPath (Join-Path $to $css)).Hash
    [IO.File]::WriteAllText((Join-Path $to 'my notes.txt'), 'Kept across the move.')

    Setup $installer $zone
    CheckInstall $zone $nativeVersion
    SameFiles $before (Snapshot $data) 'migrated profile'
    $shipped = Snapshot (Join-Path $new 'app')
    $installed = Snapshot $to
    $retired = @($original.Keys | Where-Object { -not $shipped.ContainsKey($_) -and $_ -ne 'setup\installed.txt' })
    foreach ($file in $retired) { Check (-not $installed.ContainsKey($file)) "Retired program file survived: $file" }
    Check (@(Get-ChildItem -LiteralPath $zone -Directory -Filter 'Player.old-*').Count -eq 0) 'Verified originals remained in the previous installation folder'
    foreach ($dir in 'wwwroot', 'presence', 'runtimes') { Check (-not (Test-Path -LiteralPath (Join-Path $to $dir))) "1.x directory survived: $dir" }
    foreach ($file in $shipped.Keys) { Check ($installed[$file] -ceq $shipped[$file]) "Installed payload differs: $file" }
    $backups = @(Get-ChildItem -LiteralPath "$to edits" -Directory)
    Check ($backups.Count -eq 1) 'Expected one edits backup'
    $savedCss = Join-Path $backups[0].FullName $css
    Check ((Get-FileHash -LiteralPath $savedCss).Hash -ceq $edited) 'Edited CSS was lost'
    Check ((Get-Content -LiteralPath $savedCss -Stream 'notes' -Raw) -ceq 'Preserved metadata') 'Edited CSS lost its named stream'
    Check ((Get-Content -LiteralPath (Join-Path $backups[0].FullName $streamOnly) -Stream 'rating' -Raw) -ceq '5') 'A stream-only edit was lost'
    Check ([IO.File]::ReadAllText((Join-Path $to 'my notes.txt')) -ceq 'Kept across the move.') 'Personal file was lost'
    "Migrated $($before.Count) profile files. Removed $($retired.Count) retired program files. Verified edited CSS and both shortcuts."

    $out = Join-Path $run 'smoke'
    $start = [Diagnostics.ProcessStartInfo]::new((Join-Path $to 'String.exe'))
    $start.UseShellExecute = $false
    foreach ($arg in '--smoke-test', '--data', $data, '--out', $out) { $start.ArgumentList.Add($arg) }
    $p = [Diagnostics.Process]::Start($start)
    try {
        if (-not $p.WaitForExit(240000)) { $p.Kill(); $p.WaitForExit(); throw 'Installed smoke test timed out' }
        $result = Get-Content -LiteralPath (Join-Path $out 'results.json') -Raw | ConvertFrom-Json
        Check ($p.ExitCode -eq 0 -and $result.passed -eq $true -and $result.checks -contains '01-songs' -and
            $result.checks -contains '42-settings-updates') "Installed smoke test failed ($($p.ExitCode)): $($result.error)"
        Check ($result.performance.muted -eq $true -and $result.performance.off_screen -eq $true) 'Smoke did not report muted off-screen playback'
        "Installed smoke test: $($result.checks.Count) checks passed, $($result.skipped.Count) skipped."
    } finally { $p.Dispose() }
    $reopened = Get-Content -LiteralPath (Join-Path $data 'state.json') -Raw | ConvertFrom-Json -AsHashtable
    Check ($reopened.playlists.id -contains 'saved-list' -and $reopened.liked[$songId] -eq $at) 'Saved playlist or like missing after opening 2.0'
    $history = Get-Content -LiteralPath (Join-Path $data 'listening.json') -Raw | ConvertFrom-Json -AsHashtable
    Check ($history.days['2020-01-02'].s -eq 37) 'Existing listening time missing after opening 2.0'
    $afterSmoke = Snapshot $data
    $uninstaller = Join-Path $to 'setup\uninstall.ps1'
    [IO.File]::AppendAllText($uninstaller, "`r`n# Personal setup note.`r`n")
    Set-Content -LiteralPath $uninstaller -Stream 'notes' -Value 'Uninstall metadata' -NoNewline
    Set-Content -LiteralPath (Join-Path $to 'String.exe') -Stream 'rating' -Value '4' -NoNewline
    $uninstallerHash = (Get-FileHash -LiteralPath $uninstaller).Hash
    Setup $uninstaller $zone -Remove
    SameFiles $afterSmoke (Snapshot $data) 'uninstalled profile'
    SameFiles $songFiles (Snapshot $media) 'song files'
    Check ([IO.File]::Exists((Join-Path $to 'my notes.txt')) -and [IO.File]::Exists($savedCss)) 'Uninstall removed personal files'
    $savedSetup = @(Get-ChildItem -LiteralPath "$to edits" -Recurse -File -Filter 'uninstall.ps1')
    Check ($savedSetup.Count -eq 1 -and (Get-FileHash -LiteralPath $savedSetup[0].FullName).Hash -ceq $uninstallerHash) 'Uninstall lost an edit to its own script'
    Check ((Get-Content -LiteralPath $savedSetup[0].FullName -Stream 'notes' -Raw) -ceq 'Uninstall metadata') 'Uninstall lost a named stream'
    $savedExe = @(Get-ChildItem -LiteralPath "$to edits" -Recurse -File -Filter 'String.exe')
    Check ($savedExe.Count -eq 1 -and (Get-Content -LiteralPath $savedExe[0].FullName -Stream 'rating' -Raw) -ceq '4') 'Uninstall lost a stream-only edit'
    foreach ($path in 'Player\String.exe', 'Menu\String.lnk', 'Desktop\String.lnk') {
        Check (-not (Test-Path -LiteralPath (Join-Path $zone $path))) "Uninstall left $path"
    }
    "1.3.1 to $nativeVersion passed. Smoke report: $out\results.json"
} finally {
    if (Test-Path -LiteralPath $testKey) { Remove-Item -LiteralPath $testKey -Recurse -Force }
}
