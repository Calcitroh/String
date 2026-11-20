#Requires -Version 7
# Makes a String release: release\String.zip (a self-contained build anyone can install: no .NET to install), signed
# with String's release key when it's on this PC (release\String.zip.sig, which installed copies check before
# updating), plus release\Phone setup.txt to send on its own. With -Publish it also goes on GitHub as a release (the
# GitHub CLI, gh, signed in), where installed copies find it.
#   pwsh -File tools\release.ps1 [-Version 1.2.0] [-Notes "What changed"] [-Publish]
# The release key: %USERPROFILE%\.string-release\release-key.pem (or -Key). Its public half is UpdateKey in
# src\Updater.cs; without the private half a release can still be built, but not signed or published.
# Nothing personal goes in: a build has no data (each person's lives in their own %LOCALAPPDATA%\String), and every
# file is checked for this PC's user and profile name (and any words listed, one per line, in
# %USERPROFILE%\.string-release\private-words.txt) before it is zipped.
# Builds go through release\build (their own obj/bin), never mixed with app-new.
param(
    [string]$Version = '',
    [string]$Notes = '',
    [string]$Key = (Join-Path $env:USERPROFILE '.string-release\release-key.pem'),
    [switch]$Publish
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$out = Join-Path $root 'release'
$stage = Join-Path $out 'stage'
$app = Join-Path $stage 'app'
$notesDir = Join-Path $PSScriptRoot 'share'
if (-not $Version) { $Version = ([xml](Get-Content -LiteralPath (Join-Path $root 'src\String.csproj') -Raw)).Project.PropertyGroup.Version | Where-Object { $_ } | Select-Object -First 1 }
if ($Version -notmatch '^\d+\.\d+\.\d+$') { "the version must look like 1.2.0 (got '$Version')"; exit 1 }

if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }   # our own staging folder, made below
New-Item -ItemType Directory -Force $app | Out-Null
$obj = Join-Path $out 'build'

"building String $Version (self-contained)..."
dotnet publish (Join-Path $root 'src\String.csproj') -c Release -o $app --artifacts-path $obj --self-contained true -p:Version=$Version -p:DebugType=none -p:DebugSymbols=false -nologo -v:q
if ($LASTEXITCODE) { 'String build failed'; exit 1 }
'building the Discord helper (self-contained, trimmed)...'
dotnet publish (Join-Path $root 'presence\Presence.csproj') -c Release -o (Join-Path $app 'presence') --artifacts-path $obj --self-contained true -p:PublishTrimmed=true -p:EnableCompressionInSingleFile=true -p:DebugType=none -p:DebugSymbols=false -nologo -v:q
if ($LASTEXITCODE) { 'Discord helper build failed'; exit 1 }
Get-ChildItem -LiteralPath $app -Filter *.xml -File | Remove-Item   # the WebView2 package's API docs

# licences of what the build bundles: the .NET and WPF runtimes and the WebView2 SDK from the NuGet packages the
# build used (versions from its assets file), the rest from tools\share\licenses
'adding licences...'
$lic = Join-Path $app 'licenses'
New-Item -ItemType Directory -Force $lic | Out-Null
$assets = Get-Content (Join-Path $obj 'obj\String\project.assets.json') -Raw | ConvertFrom-Json
$pkgRoot = @($assets.packageFolders.PSObject.Properties.Name)[0]
$versions = @{}
foreach ($fw in $assets.project.frameworks.PSObject.Properties) { foreach ($d in $fw.Value.downloadDependencies) { $versions[$d.name.ToLower()] = ($d.version -replace '[\[\]\s]', '' -split ',')[0] } }
foreach ($l in $assets.libraries.PSObject.Properties) { $id, $v = $l.Name -split '/'; $versions[$id.ToLower()] = $v }
foreach ($x in @(
        @('microsoft.netcore.app.runtime.win-x64', 'LICENSE.TXT', 'dotnet-LICENSE.txt'),
        @('microsoft.netcore.app.runtime.win-x64', 'THIRD-PARTY-NOTICES.TXT', 'dotnet-THIRD-PARTY-NOTICES.txt'),
        @('microsoft.windowsdesktop.app.runtime.win-x64', 'LICENSE', 'WindowsDesktop-LICENSE.txt'),
        @('microsoft.web.webview2', 'LICENSE.txt', 'WebView2-LICENSE.txt'),
        @('microsoft.web.webview2', 'NOTICE.txt', 'WebView2-NOTICE.txt'))) {
    $f = Join-Path $pkgRoot ("{0}\{1}\{2}" -f $x[0], $versions[$x[0]], $x[1])
    if (-not $versions[$x[0]] -or -not (Test-Path -LiteralPath $f)) { "missing licence $f"; exit 1 }
    Copy-Item -LiteralPath $f (Join-Path $lic $x[2])
}
foreach ($f in 'NOTICES.txt', 'Apache-2.0.txt') { Copy-Item -LiteralPath (Join-Path $notesDir "licenses\$f") (Join-Path $lic $f) }
Copy-Item -LiteralPath (Join-Path $root 'LICENSE') (Join-Path $lic 'String-LICENSE.txt')

# the installer and the notes, with Windows line endings (Notepad, cmd and Windows PowerShell all read them)
function CopyText([string]$from, [string]$to) {
    $text = [IO.File]::ReadAllText($from) -replace "`r?`n", "`r`n"
    if ($text -match '[^\x00-\x7F]') { throw "not plain ASCII: $from" }
    [IO.File]::WriteAllText($to, $text, (New-Object Text.UTF8Encoding $false))
}
New-Item -ItemType Directory -Force (Join-Path $app 'setup') | Out-Null
CopyText (Join-Path $notesDir 'install.ps1') (Join-Path $app 'setup\install.ps1')
CopyText (Join-Path $notesDir 'uninstall.ps1') (Join-Path $app 'setup\uninstall.ps1')
# the version, inside the signed zip: the updater checks a release is the version it says it is
[IO.File]::WriteAllText((Join-Path $app 'setup\version.txt'), "$Version`r`n")
foreach ($f in 'Install String.cmd', 'Read me first.txt', 'Phone setup.txt') { CopyText (Join-Path $notesDir $f) (Join-Path $stage $f) }

# setup\files.txt: every program file with its SHA-256 and size. The installer checks the unzipped files against it,
# and the uninstaller (and the next update) remove only what it lists.
'listing the files...'
$rows = foreach ($f in (Get-ChildItem -LiteralPath $app -Recurse -File -Force | Sort-Object FullName)) {
    "{0}`t{1}`t{2}" -f (Get-FileHash -LiteralPath $f.FullName -Algorithm SHA256).Hash, $f.Length, $f.FullName.Substring($app.Length + 1)
}
$rows = @($rows) + ("#end`t{0}" -f @($rows).Count)   # the installer refuses a list cut short
[IO.File]::WriteAllLines((Join-Path $app 'setup\files.txt'), [string[]]$rows)

# nothing personal: no data or debug files, and no sign of this PC's user or profile folder name (or private words)
$everything = @(Get-ChildItem -LiteralPath $stage -Recurse -File -Force)
$bad = @($everything | Where-Object { $_.Name -match '^(state|listening|library|presence|window|discord-covers)\.json$|\.(pdb|bak|log|old)$' })
if ($bad.Count) { "unexpected files: $(($bad | ForEach-Object FullName) -join ', ')"; exit 1 }
# Each file's bytes are read as Latin-1 (one char per byte), and so are the name's UTF-8 and UTF-16 bytes, so a name in
# any letters is found either way. A bare name counts only as a whole word, because the runtime's compression
# dictionary holds ordinary English words that may contain a short name.
$latin = [Text.Encoding]::GetEncoding(28591)
$privateWords = Join-Path $env:USERPROFILE '.string-release\private-words.txt'
$names = @($env:USERNAME, (Split-Path $env:USERPROFILE -Leaf)) + @(if (Test-Path -LiteralPath $privateWords) { Get-Content -LiteralPath $privateWords | ForEach-Object { $_.Trim() } })
$names = $names | Where-Object { $_ -and $_.Length -ge 3 } | Select-Object -Unique
$rx = @()
foreach ($n in $names) {
    $a = [regex]::Escape($latin.GetString([Text.Encoding]::UTF8.GetBytes($n)))
    $w = [regex]::Escape($latin.GetString([Text.Encoding]::Unicode.GetBytes($n)))
    $rx += [regex]::new("(?<![A-Za-z0-9])$a(?![A-Za-z0-9])", 'IgnoreCase'), [regex]::new("(?<![A-Za-z0-9]\x00)$w(?![A-Za-z0-9]\x00)", 'IgnoreCase')
}
# and the profile folder's full path, wherever it is (D:\Profiles\... too)
$rx += [regex]::new([regex]::Escape($latin.GetString([Text.Encoding]::UTF8.GetBytes($env:USERPROFILE.TrimEnd('\')))), 'IgnoreCase')
$rx += [regex]::new([regex]::Escape($latin.GetString([Text.Encoding]::Unicode.GetBytes($env:USERPROFILE.TrimEnd('\')))), 'IgnoreCase')
foreach ($f in $everything) {
    $hay = $latin.GetString([IO.File]::ReadAllBytes($f.FullName))
    foreach ($r in $rx) { if ($r.IsMatch($hay)) { "something private is inside $($f.FullName)"; exit 1 } }
}
"checked all $($everything.Count) files for this PC's user and profile name$(if (Test-Path -LiteralPath $privateWords) { ' and the private words' }): not there"

'zipping...'
$zip = Join-Path $out 'String.zip'
$sigFile = "$zip.sig"
foreach ($f in $zip, $sigFile) { if (Test-Path -LiteralPath $f) { Remove-Item -LiteralPath $f } }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip, [IO.Compression.CompressionLevel]::Optimal, $false)
Copy-Item (Join-Path $stage 'Phone setup.txt') (Join-Path $out 'Phone setup.txt') -Force
$unzipped = (Get-ChildItem -LiteralPath $stage -Recurse -File | Measure-Object Length -Sum).Sum
"release\String.zip: String $Version, {0:N0} MB (unzipped {1:N0} MB)" -f ((Get-Item $zip).Length / 1MB), ($unzipped / 1MB)

# the signature: ECDSA P-256 over SHA-256 of the whole zip, checked against UpdateKey in src\Updater.cs first
if (-not (Test-Path -LiteralPath $Key)) {
    "not signed: no release key at $Key"
    if ($Publish) { 'a release has to be signed before it is published'; exit 1 }
    exit 0
}
$ecdsa = [Security.Cryptography.ECDsa]::Create()
$ecdsa.ImportFromPem((Get-Content -LiteralPath $Key -Raw))
$pub = [Convert]::ToBase64String($ecdsa.ExportSubjectPublicKeyInfo())
$code = Get-Content -LiteralPath (Join-Path $root 'src\Updater.cs') -Raw
if ($code -notmatch [regex]::Escape("UpdateKey = `"$pub`"")) { 'this release key is not the one String checks for (UpdateKey in src\Updater.cs): not signed'; exit 1 }
$sig = $ecdsa.SignData([IO.File]::ReadAllBytes($zip), [Security.Cryptography.HashAlgorithmName]::SHA256)
[IO.File]::WriteAllText($sigFile, [Convert]::ToBase64String($sig))
'signed: release\String.zip.sig'

if ($Publish) {
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { 'publishing needs the GitHub CLI (gh), signed in'; exit 1 }
    # a release whose copies can't find the next one would strand everyone on it: UpdateRepo must be this repository
    $repo = if ($code -match 'const string UpdateRepo = "([^"]*)"') { $Matches[1] } else { '' }
    $here = "$(gh repo view --json nameWithOwner --jq .nameWithOwner 2>$null)".Trim()
    if (-not $repo -or $repo -ne $here) { "UpdateRepo in src\Updater.cs is '$repo', but this repository is '$here': set it, rebuild, then publish"; exit 1 }
    $tag = "v$Version"
    $text = if ($Notes) { $Notes } else { "String $Version" }
    gh release create $tag $zip $sigFile --title "String $Version" --notes $text
    if ($LASTEXITCODE) { 'publishing failed'; exit 1 }
    "published $tag"
}
