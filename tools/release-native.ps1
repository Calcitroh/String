#Requires -Version 7
# Makes an installable native release, or a loose preview that installed copies skip.
#   pwsh -File tools\release-native.ps1 -Full -Version 2.0.0 [-Key <release key>] [-Notes "..."] [-Publish]
#   pwsh -File tools\release-native.ps1 -Version 2.0.0-preview.1 [-Notes "What it is"] [-Publish]
# Nothing personal goes in: the same check as tools\release.ps1 (this PC's user and profile name, and any words listed,
# one per line, in %USERPROFILE%\.string-release\private-words.txt).
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$Notes = '',
    [switch]$Publish,
    [switch]$Full,
    [string]$Key = (Join-Path $env:USERPROFILE '.string-release\release-key.pem')
)
$ErrorActionPreference = 'Stop'
if ($Full) {
    if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') { "a full version looks like 2.0.0 (got '$Version')"; exit 1 }
} elseif ($Version -notmatch '^\d+\.\d+\.\d+-preview\.\d+$') { "a preview's version looks like 2.0.0-preview.1 (got '$Version')"; exit 1 }
$root = Split-Path $PSScriptRoot -Parent
$native = Join-Path $root 'native'
$out = Join-Path $root 'release'
$stage = Join-Path $out $(if ($Full) { 'native-full-stage' } else { 'native-stage' })
$name = if ($Full) { 'String' } else { "String-$Version" }
$app = Join-Path $stage $(if ($Full) { 'app' } else { $name })
$tag = "v$Version"
# A published preview's tag goes on the exact commit it's built from, so that commit has to be complete (no change and
# no new file outside git's ignore list, since the build could pick one up), already on GitHub's main, and the tag
# either new or already on that same commit.
if ($Publish) {
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { 'publishing needs the GitHub CLI (gh), signed in'; exit 1 }
    if ("$(git -C $root status --porcelain --untracked-files=all)".Trim()) { 'commit your changes (and new files) first: a published zip has to match a commit'; exit 1 }
    $commit = "$(git -C $root rev-parse HEAD)".Trim()
    # the repository is origin's (fetched from and pushed to the same GitHub repository), named on every GitHub call,
    # so GH_REPO or gh's own default can't pick another
    $origin = "$(git -C $root remote get-url origin)".Trim()
    $pushTo = "$(git -C $root remote get-url --push origin)".Trim()
    if ($origin -notmatch '^(https://github\.com/|git@github\.com:|ssh://git@github\.com/)([^/\s]+/[^/\s]+?)(\.git)?/?$') { "origin isn't a GitHub repository ($origin): not published"; exit 1 }
    $repo = $Matches[2]
    if ($pushTo -ne $origin) { "origin pushes somewhere else ($pushTo) than it fetches from: not published"; exit 1 }
    $github = "https://github.com/$repo.git"
    git -C $root fetch --quiet origin main
    if ($LASTEXITCODE) { "couldn't reach GitHub: not published"; exit 1 }
    git -C $root merge-base --is-ancestor $commit origin/main
    if ($LASTEXITCODE) { "push $commit to main first: the release's tag goes on it"; exit 1 }
}
"building String $Version..."
# A Rust program keeps the source paths of everything it's built from (for its error messages): the libraries' under
# this PC's profile folder, String's own under wherever this repository is. The build names them /cargo and /string
# instead, and the linker keeps only the debug file's name. Separate targets keep the preview, full release
# and everyday build from overwriting one another.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$originalFlags = $env:CARGO_ENCODED_RUSTFLAGS
$target = Join-Path $out $(if ($Full) { 'native-full-build' } else { 'native-build' })
$triple = 'x86_64-pc-windows-msvc'   # named, so a configured default target can't send the build elsewhere
Push-Location $native
try {
    $meta = cargo metadata --format-version 1 --locked --filter-platform $triple | ConvertFrom-Json -Depth 100
    if ($LASTEXITCODE) { 'cargo metadata failed'; exit 1 }
    $crate = $meta.packages | Where-Object name -EQ 'string-app'
    if ($Full -and $Version -cne $crate.version) { throw "Full release $Version must match string-app $($crate.version)." }
    # Vendored registries can live outside both the Cargo home and this checkout.
    $registryDirs = $meta.packages | Where-Object { $_.source -like 'registry+*' } | ForEach-Object {
        [IO.Directory]::GetParent([IO.Path]::GetDirectoryName($_.manifest_path)).FullName
    } | Sort-Object -Unique
    $flags = @("--remap-path-prefix=$($cargoHome.TrimEnd('\'))=/cargo", "--remap-path-prefix=$root=/string")
    foreach ($dir in $registryDirs) { $flags += "--remap-path-prefix=$dir=/cargo" }
    # A clean install cannot assume the Visual C++ redistributable is installed.
    $flags += '-Ctarget-feature=+crt-static'
    $env:CARGO_ENCODED_RUSTFLAGS = ($flags + '-Clink-arg=/PDBALTPATH:%_PDB%') -join [char]0x1f
    cargo build --release --locked -p string-app --target $triple --target-dir $target
    if ($LASTEXITCODE) { 'String build failed'; exit 1 }
    $rustc = "$(rustc --version)".Trim()
    $sysroot = "$(rustc --print sysroot)".Trim()
} finally {
    Pop-Location
    $env:CARGO_ENCODED_RUSTFLAGS = $originalFlags
}
if (Test-Path -LiteralPath $stage) {
    $stagingDir = Get-Item -LiteralPath $stage -Force
    if ($stagingDir.Parent.FullName -ne $out -or $stagingDir.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "unexpected staging folder: $stage" }
    Remove-Item -LiteralPath $stagingDir.FullName -Recurse -Force
}
New-Item -ItemType Directory -Force (Join-Path $app 'licenses') | Out-Null
$exe = Join-Path $app 'String.exe'
Copy-Item -LiteralPath (Join-Path $target "$triple\release\String.exe") $exe
if ($Full -and [Diagnostics.FileVersionInfo]::GetVersionInfo($exe).ProductVersion -cne $Version) {
    throw "String.exe does not have product version $Version."
}

# Licences: every library String.exe is built from (what string-app depends on, not tools used only to build or
# test), each listed with its licence and the licence files it ships; identical texts are printed once.
'adding licences...'
$pkgs = @{}; foreach ($p in $meta.packages) { $pkgs[$p.id] = $p }
$nodes = @{}; foreach ($n in $meta.resolve.nodes) { $nodes[$n.id] = $n }
$members = [Collections.Generic.HashSet[string]]::new([string[]]$meta.workspace_members)
$seen = [Collections.Generic.HashSet[string]]::new()
$todo = [Collections.Generic.Queue[string]]::new()
$todo.Enqueue(($meta.packages | Where-Object name -EQ 'string-app').id)
while ($todo.Count) {
    $id = $todo.Dequeue()
    if (-not $seen.Add($id)) { continue }
    foreach ($d in $nodes[$id].deps) { if ($d.dep_kinds | Where-Object { -not $_.kind }) { $todo.Enqueue($d.pkg) } }
}
$used = @($seen | Where-Object { -not $members.Contains($_) } | ForEach-Object { $pkgs[$_] } | Sort-Object name, version)
# The standard texts, for libraries that name their licence but ship no file of it: Rust's own copies, and the rest from
# tools\share\licenses. A licence with no text here stops the build.
$rustDocs = Join-Path $sysroot 'share\doc\rust'
function StandardText([string]$id) {
    foreach ($dir in (Join-Path $rustDocs 'licenses'), (Join-Path $PSScriptRoot 'share\licenses')) {
        $f = Join-Path $dir "$id.txt"
        if (Test-Path -LiteralPath $f) { return $f }
    }
}
$standard = [ordered]@{}   # licence id -> its file, copied into licenses\
foreach ($id in 'MIT', 'Apache-2.0') { $standard[$id] = StandardText $id }
$texts = [ordered]@{}   # normalised text -> its number
$list = foreach ($p in $used) {
    $dir = Split-Path $p.manifest_path -Parent
    # licence files anywhere in the package but its tests and examples (fonts and bundled C code keep theirs in
    # subfolders), and the text that sits next to a font it embeds
    $files = @(Get-ChildItem -LiteralPath $dir -File -Recurse -Depth 3 | Where-Object {
            ($_.DirectoryName -eq $dir -and $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE|COPYRIGHT)') -or (
                $_.FullName -notmatch '\\(tests?|benches|examples|fuzz)\\' -and (
                    ($_.Extension -in '', '.txt', '.md', '.html' -and $_.Name -match '(?i)^(licen[cs]e|copying|notice|unlicense|copyright|ofl|ufl)|licen[cs]e') -or
                    ($_.Extension -eq '.txt' -and (Test-Path -LiteralPath (Join-Path $_.DirectoryName "$($_.BaseName).ttf")))))
        })
    if ($p.license_file) { $files += Get-Item -LiteralPath (Join-Path $dir $p.license_file) }
    $refs = foreach ($f in ($files | Sort-Object FullName -Unique)) {
        $t = ([IO.File]::ReadAllText($f.FullName) -replace "`r`n", "`n").Trim()
        if (-not $texts.Contains($t)) { $texts[$t] = $texts.Count + 1 }
        $texts[$t]
    }
    $licence = if ($p.license) { $p.license } else { 'see its licence file' }
    $where = if ($p.repository) { "  $($p.repository)" } else { '' }
    $have = if ($refs) {
        'texts ' + (($refs | Sort-Object -Unique) -join ', ')
    } else {
        $ids = @([regex]::Matches("$($p.license)", '[A-Za-z0-9.\-]+') | ForEach-Object Value | Where-Object { $_ -notin 'OR', 'AND', 'WITH' } | Select-Object -Unique)
        if (-not $ids) { throw "$($p.name) $($p.version) names no licence and ships no licence file" }
        foreach ($id in $ids) {
            if (-not $standard.Contains($id)) {
                $f = StandardText $id
                if (-not $f) { throw "$($p.name) $($p.version) ships no licence file, and there's no standard text of $id (add tools\share\licenses\$id.txt)" }
                $standard[$id] = $f
            }
        }
        'no licence file in the package; the standard text: ' + (($ids | ForEach-Object { "$_.txt" }) -join ', ')
    }
    "$($p.name) $($p.version)  ($licence)$where`n    $have"
}
$body = @("String is built from these libraries, each under its own licence. Their licence texts follow the list,",
    "each printed once and numbered; the standard licence texts named are files in this folder.", '',
    "The Rust standard library ($rustc): MIT OR Apache-2.0, https://github.com/rust-lang/rust",
    "    the standard text: MIT.txt, Apache-2.0.txt; and Rust-COPYRIGHT-library.html", '') + $list
foreach ($e in $texts.GetEnumerator()) { $body += @('', ('=' * 30) + " $($e.Value) " + ('=' * 30), '', $e.Key) }
[IO.File]::WriteAllText((Join-Path $app 'licenses\THIRD-PARTY.txt'), (($body -join "`n") -replace "`n", "`r`n"), (New-Object Text.UTF8Encoding $false))
foreach ($e in $standard.GetEnumerator()) {
    if (-not $e.Value) { "missing the standard licence text $($e.Key)"; exit 1 }
    Copy-Item -LiteralPath $e.Value (Join-Path $app "licenses\$($e.Key).txt")
}
$f = Join-Path $rustDocs 'COPYRIGHT-library.html'
if (-not (Test-Path -LiteralPath $f)) { 'missing the Rust licence COPYRIGHT-library.html'; exit 1 }
Copy-Item -LiteralPath $f (Join-Path $app 'licenses\Rust-COPYRIGHT-library.html')
Copy-Item -LiteralPath (Join-Path $root 'LICENSE') (Join-Path $app 'licenses\String-LICENSE.txt')
"$($used.Count) libraries, $($texts.Count) distinct licence texts"

function CopyText([string]$name, [string]$dest) {
    $text = [IO.File]::ReadAllText((Join-Path $PSScriptRoot "share\$name")) -replace "`r?`n", "`r`n"
    if ($text -match '[^\x00-\x7F]') { throw "not plain ASCII: $name" }
    [IO.File]::WriteAllText($dest, $text)
}
# Read the x64 PE's load-time imports, including the entry points that may be
# missing from an older version of an otherwise present Windows DLL.
function PeString($image, [int]$rva) {
    $reader = $image.GetSectionData($rva).GetReader()
    while ($reader.ReadByte() -ne 0) { }
    $length = $reader.Offset - 1
    $reader.Offset = 0
    $reader.ReadUTF8($length)
}
function NativeImports([string]$path) {
    $stream = [IO.File]::OpenRead($path)
    try {
        $image = [Reflection.PortableExecutable.PEReader]::new($stream)
        try {
            $header = $image.PEHeaders.PEHeader
            if ($header.Magic -ne 'PE32Plus' -or -not $header.ImportTableDirectory.Size) { throw 'Expected an x64 PE with imports.' }
            $table = $image.GetSectionData($header.ImportTableDirectory.RelativeVirtualAddress).GetReader()
            while ($true) {
                $lookup = $table.ReadInt32()
                $table.Offset += 8 # timestamp and forwarder chain
                $dllName = $table.ReadInt32()
                $addresses = $table.ReadInt32()
                if (-not $dllName) { break }
                $dll = PeString $image $dllName
                if (-not $lookup) { $lookup = $addresses }
                $names = $image.GetSectionData($lookup).GetReader()
                $symbols = @(while ($entry = $names.ReadInt64()) {
                    if ($entry -lt 0) { '#' + ($entry -band 0xffff) }
                    else { PeString $image ([int]$entry + 2) } # skip the hint
                })
                if (-not $symbols.Count) { throw "Empty import table for $dll." }
                "$dll $($symbols -join ' ')"
            }
        } finally { $image.Dispose() }
    } finally { $stream.Dispose() }
}
function ReleaseImports([string]$path) {
    $imports = @(NativeImports $path)
    if (-not $imports.Count) { throw 'Missing native imports.' }
    foreach ($row in $imports) {
        $dll = ($row -split ' ', 2)[0]
        if ($dll -match '^(?:vcruntime.*\.dll|msvcp.*\.dll|ucrtbase\.dll|api-ms-win-crt-.*)$') {
            throw "String.exe requires a separate C runtime ($dll)."
        }
    }
    $imports
}
$imports = @(ReleaseImports $exe)
if ($Full) {
    $setup = New-Item -ItemType Directory -Path (Join-Path $app 'setup')
    [IO.File]::WriteAllLines((Join-Path $setup.FullName 'imports.txt'), [string[]]$imports)
    foreach ($file in 'install.ps1', 'uninstall.ps1') { CopyText $file (Join-Path $setup.FullName $file) }
    [IO.File]::WriteAllText((Join-Path $setup.FullName 'version.txt'), "$Version`r`n")
    CopyText 'Install String.cmd' (Join-Path $stage 'Install String.cmd')
    CopyText 'Native read me.txt' (Join-Path $stage 'Read me first.txt')
    CopyText 'Phone setup.txt' (Join-Path $stage 'Phone setup.txt')
    $rows = @(foreach ($f in (Get-ChildItem -LiteralPath $app -Recurse -File -Force | Sort-Object FullName)) {
        "{0}`t{1}`t{2}" -f (Get-FileHash -LiteralPath $f.FullName -Algorithm SHA256).Hash, $f.Length, $f.FullName.Substring($app.Length + 1)
    })
    [IO.File]::WriteAllLines((Join-Path $setup.FullName 'files.txt'), [string[]]($rows + "#end`t$($rows.Count)"))
} else {
    CopyText 'Preview read me.txt' (Join-Path $app 'Read me first.txt')
}

# Nothing personal: no data or debug files, and no sign of this PC's user or profile folder name (or private words).
# As in tools\release.ps1: each file's bytes read as Latin-1, so a name's UTF-8 and UTF-16 bytes are found either way; a
# bare name counts only as a whole word. One exception, for the build machine's own words only by where they sit: the
# Vulkan library packs its extension names so tightly that a short word can appear between "VK_EXT_image_" and the
# next name. A word anywhere else, between underscores too, still stops the build.
$everything = @(Get-ChildItem -LiteralPath $stage -Recurse -File -Force)
$bad = @($everything | Where-Object { $_.Name -match '^(state|listening|library|presence|window|discord-covers)\.json$|\.(pdb|bak|log|old)$' })
if ($bad.Count) { "unexpected files: $(($bad | ForEach-Object FullName) -join ', ')"; exit 1 }
$latin = [Text.Encoding]::GetEncoding(28591)
$privateWords = Join-Path $env:USERPROFILE '.string-release\private-words.txt'
$names = @($env:USERNAME, (Split-Path $env:USERPROFILE -Leaf)) + @(if (Test-Path -LiteralPath $privateWords) { Get-Content -LiteralPath $privateWords | ForEach-Object { $_.Trim() } })
$names = $names | Where-Object { $_ -and $_.Length -ge 3 } | Select-Object -Unique
$rx = @(); $rxExe = @()
foreach ($n in $names) {
    $a = [regex]::Escape($latin.GetString([Text.Encoding]::UTF8.GetBytes($n)))
    $w = [regex]::Escape($latin.GetString([Text.Encoding]::Unicode.GetBytes($n)))
    $utf16 = [regex]::new("(?<![A-Za-z0-9]\x00)$w(?![A-Za-z0-9]\x00)", 'IgnoreCase')
    $rx += [regex]::new("(?<![A-Za-z0-9])$a(?![A-Za-z0-9])", 'IgnoreCase'), $utf16
    # String.exe only: not where the packed Vulkan names read exactly VK_EXT_image_<word>_float16_int8
    $rxExe += [regex]::new("(?<![A-Za-z0-9])(?!(?<=VK_EXT_image_)$a" + '_float16_int8\x00)' + "$a(?![A-Za-z0-9])", 'IgnoreCase'), $utf16
}
foreach ($p in [Text.Encoding]::UTF8, [Text.Encoding]::Unicode) {
    $profilePath = [regex]::new([regex]::Escape($latin.GetString($p.GetBytes($env:USERPROFILE.TrimEnd('\')))), 'IgnoreCase')
    $rx += $profilePath; $rxExe += $profilePath
}
foreach ($f in $everything) {
    $hay = $latin.GetString([IO.File]::ReadAllBytes($f.FullName))
    $checks = if ($f.Name -eq 'String.exe' -and $f.Directory.FullName -eq $app) { $rxExe } else { $rx }
    foreach ($r in $checks) { if ($r.IsMatch($hay)) { "something private is inside $($f.FullName)"; exit 1 } }
}
"checked all $($everything.Count) files for this PC's user and profile name$(if (Test-Path -LiteralPath $privateWords) { ' and the private words' }): not there"

'zipping...'
$zip = Join-Path $out "$name.zip"
$sigFile = "$zip.sig"
if ($Full -and (Test-Path -LiteralPath $sigFile)) { Remove-Item -LiteralPath $sigFile }
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip, [IO.Compression.CompressionLevel]::Optimal, $false)
"release\$name.zip: {0:N1} MB" -f ((Get-Item $zip).Length / 1MB)

if ($Full) {
    if (-not (Test-Path -LiteralPath $Key)) {
        "not signed: no release key at $Key"
        exit $(if ($Publish) { 1 } else { 0 })   # installed copies only take a signed release
    }
    $ecdsa = [Security.Cryptography.ECDsa]::Create()
    try {
        $ecdsa.ImportFromPem([IO.File]::ReadAllText($Key))
        $pub = [Convert]::ToBase64String($ecdsa.ExportSubjectPublicKeyInfo())
        $legacy = [IO.File]::ReadAllText((Join-Path $root 'src\Updater.cs'))
        $nativeCode = [IO.File]::ReadAllText((Join-Path $native 'string-extras\src\updates.rs'))
        if (-not $legacy.Contains("UpdateKey = `"$pub`"") -or -not $nativeCode.Contains("const KEY: &str = `"$pub`"")) {
            throw "This release key does not match both updaters."
        }
        $stream = [IO.File]::OpenRead($zip)
        try { $sig = $ecdsa.SignData($stream, [Security.Cryptography.HashAlgorithmName]::SHA256) }
        finally { $stream.Dispose() }
        [IO.File]::WriteAllText($sigFile, [Convert]::ToBase64String($sig))
        'signed: release\String.zip.sig'
    } finally { $ecdsa.Dispose() }
}

if ($Publish) {
    Push-Location $root
    try {
        # what installed copies update to (the newest full release): read before, and it must be the same after
        $before = "$(gh api "repos/$repo/releases/latest" --jq .tag_name)".Trim()
        if ($LASTEXITCODE -or -not $before) { "couldn't read the newest full release from GitHub: not published"; exit 1 }
        # The tag goes on the commit this zip was built from: pushed here if GitHub doesn't have it, and either way it
        # must then point (through an annotated tag too) at exactly that commit before the release is made on it.
        function TaggedCommit {
            $lines = @(git ls-remote $github "refs/tags/$tag" "refs/tags/$tag^{}")
            if ($LASTEXITCODE) { "couldn't read GitHub's tags: not published"; exit 1 }
            $peeled = $lines | Where-Object { $_ -match '\^\{\}$' } | Select-Object -First 1
            $line = if ($peeled) { $peeled } else { $lines | Select-Object -First 1 }
            if ($line) { ($line -split '\s+')[0] }
        }
        $tagged = TaggedCommit
        if (-not $tagged) {
            git push --quiet origin "${commit}:refs/tags/$tag"
            if ($LASTEXITCODE) { "couldn't put the tag $tag on ${commit}: not published"; exit 1 }
            $tagged = TaggedCommit
        }
        if ($tagged -ne $commit) { "the tag $tag on GitHub is on $tagged, not on ${commit}: not published"; exit 1 }
        $text = if ($Notes) { $Notes } else { "String $Version" }
        if ($Full) {
            # The newest full release is what every installed copy updates to: String.zip and its signature, as latest.
            if (-not (Test-Path -LiteralPath $sigFile)) { 'a full release has to be signed: not published'; exit 1 }
            gh release create $tag $zip $sigFile --repo $repo --verify-tag --latest --title "String $Version" --notes $text
            if ($LASTEXITCODE) { 'publishing failed'; exit 1 }
            "published $tag as the newest full release, on $commit"
            $after = "$(gh api "repos/$repo/releases/latest" --jq .tag_name)".Trim()
            if ($LASTEXITCODE -or $after -cne $tag) { "published, but GitHub's newest full release reads '$after', not ${tag}: check it on GitHub"; exit 1 }
            "installed copies (on $before) will update to $tag"
        } else {
            gh release create $tag $zip --repo $repo --verify-tag --prerelease --title "String $Version" --notes $text
            if ($LASTEXITCODE) { 'publishing failed'; exit 1 }
            "published $tag as a pre-release, on $commit"
            $after = "$(gh api "repos/$repo/releases/latest" --jq .tag_name)".Trim()
            if ($LASTEXITCODE -or -not $after) { "published, but couldn't read the newest full release back: check that it's still $before"; exit 1 }
            if ($after -cne $before) { "the newest full release is now $after, not ${before}: installed copies would update to it. Fix it on GitHub now"; exit 1 }
            "installed copies still update to $after"
        }
    } finally { Pop-Location }
}
