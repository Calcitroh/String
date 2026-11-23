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
    Build 'release-native.ps1' @('-Full', '-Version', '2.0.0')
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
$keyPath = "HKCU:\Software\StringMigrationTest-$id"
function Setup([string]$script, [string]$zone, [switch]$Remove, [switch]$Legacy) {
    $start = [Diagnostics.ProcessStartInfo]::new($windowsPS)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.Environment['LOCALAPPDATA'] = $local
    [void]$start.Environment.Remove('PSModulePath')
    $argsPS = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $script,
        '-To', (Join-Path $zone 'Player'), '-Menu', (Join-Path $zone 'Menu'),
        '-Desktop', (Join-Path $zone 'Desktop'), '-AppsKey', $keyPath)
    $argsPS += if ($Remove) { '-Data', $data, '-DataChoice', 'keep' } else { '-NoStart' }
    foreach ($arg in $argsPS) { $start.ArgumentList.Add($arg) }
    $p = [Diagnostics.Process]::Start($start)
    try {
        $p.WaitForExit()
        # 1.x can finish installing but report a missing WebView2 runtime. This test never starts 1.x.
        Check ($p.ExitCode -eq 0 -or ($Legacy -and $p.ExitCode -eq 2)) "Setup exit $($p.ExitCode): $script"
    } finally { $p.Dispose() }
    Check (-not (Test-Path -LiteralPath $keyPath)) 'Setup took ownership of the unrelated test Apps key'
}

function CheckInstall([string]$zone, [string]$version) {
    $to = Join-Path $zone 'Player'
    $exe = Join-Path $to 'String.exe'
    Check ([IO.File]::ReadAllText((Join-Path $to 'setup\installed.txt')) -ieq $to) "Wrong install marker in $to"
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($exe)
    Check (($info.ProductVersion -split '\+')[0] -ceq $version) "Wrong exe product version: $($info.ProductVersion)"
    if ($version -eq '2.0.0') {
        Check ($info.FileVersion -ceq $version -and $info.ProductMajorPart -eq 2 -and $info.ProductMinorPart -eq 0 -and
            $info.ProductBuildPart -eq 0 -and $info.ProductPrivatePart -eq 0) 'Wrong native VERSIONINFO'
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

$old = Join-Path $run '1.x'
$new = Join-Path $run '2.0'
Extract $saved '1.3.1' $old
Extract $NativeZip '2.0.0' $new
$installer = Join-Path $new 'app\setup\install.ps1'
$clean = Join-Path $run 'Clean install'
Setup $installer $clean
CheckInstall $clean '2.0.0'
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
$edited = (Get-FileHash -LiteralPath (Join-Path $to $css)).Hash
[IO.File]::WriteAllText((Join-Path $to 'my notes.txt'), 'Kept across the move.')

Setup $installer $zone
CheckInstall $zone '2.0.0'
SameFiles $before (Snapshot $data) 'migrated profile'
$shipped = Snapshot (Join-Path $new 'app')
$installed = Snapshot $to
$retired = @($original.Keys | Where-Object { -not $shipped.ContainsKey($_) -and $_ -ne 'setup\installed.txt' })
foreach ($file in $retired) { Check (-not $installed.ContainsKey($file)) "Retired program file survived: $file" }
foreach ($dir in 'wwwroot', 'presence', 'runtimes') { Check (-not (Test-Path -LiteralPath (Join-Path $to $dir))) "1.x directory survived: $dir" }
foreach ($file in $shipped.Keys) { Check ($installed[$file] -ceq $shipped[$file]) "Installed payload differs: $file" }
$backups = @(Get-ChildItem -LiteralPath "$to edits" -Directory)
Check ($backups.Count -eq 1) 'Expected one edits backup'
$savedCss = Join-Path $backups[0].FullName $css
Check ((Get-FileHash -LiteralPath $savedCss).Hash -ceq $edited) 'Edited CSS was lost'
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
$uninstallerHash = (Get-FileHash -LiteralPath $uninstaller).Hash
Setup $uninstaller $zone -Remove
SameFiles $afterSmoke (Snapshot $data) 'uninstalled profile'
SameFiles $songFiles (Snapshot $media) 'song files'
Check ([IO.File]::Exists((Join-Path $to 'my notes.txt')) -and [IO.File]::Exists($savedCss)) 'Uninstall removed personal files'
$savedSetup = @(Get-ChildItem -LiteralPath "$to edits" -Recurse -File -Filter 'uninstall.ps1')
Check ($savedSetup.Count -eq 1 -and (Get-FileHash -LiteralPath $savedSetup[0].FullName).Hash -ceq $uninstallerHash) 'Uninstall lost an edit to its own script'
foreach ($path in 'Player\String.exe', 'Menu\String.lnk', 'Desktop\String.lnk') {
    Check (-not (Test-Path -LiteralPath (Join-Path $zone $path))) "Uninstall left $path"
}
"1.3.1 to 2.0.0 passed. Smoke report: $out\results.json"
