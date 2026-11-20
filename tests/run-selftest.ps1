# Runs String's in-window self-test (src\wwwroot\selftest.js), off-screen and muted, against a fake LRCLIB
# (tests\fake-lrclib.js), a fake Ollama (tests\fake-ollama.js) and a fake Navidrome database (tests\fake-navidrome.js) and a fake GitHub (tests\fake-github.js),
# so it never asks the real lrclib.net, never loads a real model and never reads a real Navidrome. Build app-new first (see DEVELOPING.md), and make the test songs with: node tests\test-tags.js
# Each run gets its own folder under test-runs\ (screenshots, results.txt); nothing is deleted.
# -Exe tests another build, such as the one tools\release.ps1 makes (release\stage\app\String.exe).
# -Addon tests an add-on too: its folder (addon.json, whose "id" names the folder it installs as) is copied into the
# test's data folder, and its checks join the run.
param([string]$Exe = '', [string]$Addon = '')
$root = Split-Path $PSScriptRoot -Parent
$exe = if ($Exe) { $Exe } else { Join-Path $root 'app-new\String.exe' }
$media = Join-Path $root 'test-runs\string-media'
if (-not (Test-Path -LiteralPath $exe)) { 'build it first: dotnet publish src\String.csproj -c Release -o app-new'; exit 1 }
if (-not (Test-Path $media)) { 'make the test songs first: node tests\test-tags.js'; exit 1 }
$run = Join-Path $root ('test-runs\selftest-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Force $run | Out-Null
# a folder link inside the test's look folder, pointing outside it: String must never serve through it
$outside = Join-Path $run 'outside-look'
New-Item -ItemType Directory -Force $outside, (Join-Path $run 'data\look') | Out-Null
Set-Content -LiteralPath (Join-Path $outside 'secret.css') 'body { color: red }'
cmd.exe /c mklink /J "$(Join-Path $run 'data\look\linked')" "$outside" | Out-Null
# add-ons: a small probe (a page, a button, a Library section, an Edit info line, its own checks), a broken one (bad
# addon.json: skipped), one naming files outside its folder (dropped), and a folder link inside the probe
$addons = Join-Path $run 'data\addons'
New-Item -ItemType Directory -Force (Join-Path $addons 'probe'), (Join-Path $addons 'broken'), (Join-Path $addons 'sneaky') | Out-Null
Set-Content -LiteralPath (Join-Path $outside 'secret.js') 'window.leaked = true'
Set-Content -LiteralPath (Join-Path $addons 'probe\addon.json') '{ "id": "probe", "name": "Self-test probe", "version": "1", "scripts": ["probe.js"], "styles": ["probe.css"] }'
Set-Content -LiteralPath (Join-Path $addons 'probe\probe.css') '.probe-mark { --probe: 1; }'
Set-Content -LiteralPath (Join-Path $addons 'probe\notes.txt') 'not served'
Set-Content -LiteralPath (Join-Path $addons 'probe\probe.js') @'
(function () {
  const N = window.N, heard = { cacheLoaded: 0, booted: 0, edited: [] };
  N.addons.page('probe', v => { v.innerHTML = '<div class="probe-mark" id="probeMark">probe page</div>'; });
  N.addons.sidebarButton({ id: 'probeBtn', title: 'Probe', icon: 'box', route: 'probe' });
  N.addons.librarySection(() => '<div id="probeLib">probe section</div>');
  N.addons.editInfoNote(t => t && /glass orbit/i.test(t.T) ? 'Probe note.' : '');
  N.on('cacheLoaded', () => heard.cacheLoaded++);
  N.on('booted', () => heard.booted++);
  N.on('edited', id => heard.edited.push(id));
  N.addons.tests.push(async ({ check, sleep, U, L, lower }) => {
    check('add-on: it heard String start', heard.cacheLoaded === 1 && heard.booted === 1, heard);
    document.getElementById('probeBtn').click(); await sleep(400);
    const mark = document.getElementById('probeMark');
    check('add-on: its button by Playlists opens its page, with its own style', U.route() === 'probe' && !!mark && getComputedStyle(mark).getPropertyValue('--probe').trim() === '1' && document.getElementById('probeBtn').classList.contains('on'));
    U.go('settings'); await sleep(400);
    check('add-on: its section on the Library page', !!document.getElementById('probeLib'));
    const glass = L.get(lower('glass orbit.mp3'));
    U.editInfo(glass); await sleep(300);
    check('add-on: its line in Edit info', /Probe note\./.test(document.querySelector('.scrim .dialog').textContent));
    document.querySelector('.scrim [data-k="ok"]').click(); await sleep(300);
    check('add-on: it hears an edit', heard.edited.includes(glass.id), heard.edited);
    const st = u => fetch(u).then(r => r.status);
    const own = await st('/_addon/probe/probe.js');
    const sneak = await Promise.all(['/_addon/probe/%2e%2e%5c%2e%2e%5cstate.json', '/_addon/probe/..%2F..%2Fstate.json', '/_addon/probe/C%3A%5CWindows%5Cwin.ini', '/_addon/probe/linked/secret.js', '/_addon/probe/notes.txt', '/_addon/Probe/probe.js', '/_addon/%2e%2e/state.json'].map(st));
    check('add-on files: its own are served; nothing outside its folder, through a link, or of another kind', own === 200 && sneak.every(s => s === 404) && !window.leaked, [own, sneak]);
  });
})();
'@
cmd.exe /c mklink /J "$(Join-Path $addons 'probe\linked')" "$outside" | Out-Null
Set-Content -LiteralPath (Join-Path $addons 'broken\addon.json') '{ "name": "Broken", "scripts": ['
Set-Content -LiteralPath (Join-Path $addons 'sneaky\addon.json') '{ "name": "Sneaky", "scripts": ["../probe/probe.js", "C:\\Windows\\x.js", "..\\probe\\probe.js"] }'
if ($Addon) {
    $manifest = Get-Content -LiteralPath (Join-Path $Addon 'addon.json') -Raw | ConvertFrom-Json
    if (-not $manifest.id) { "the add-on's addon.json has no id"; exit 1 }
    Copy-Item -LiteralPath $Addon -Destination (Join-Path $addons $manifest.id) -Recurse
}
# the fakes listen on fixed ports: a second run at the same time would talk to the first run's fakes
foreach ($port in 47913, 47914, 47915) {
    $l = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $port)
    try { $l.Start(); $l.Stop() } catch { "port $port is taken: is another self-test running? Let it finish first."; exit 1 }
}$navidromeDb = Join-Path $run 'navidrome\navidrome.db'
node --no-warnings (Join-Path $PSScriptRoot 'fake-navidrome.js') $navidromeDb $media | Out-Null
$fake = Start-Process node -ArgumentList ('"' + (Join-Path $PSScriptRoot 'fake-lrclib.js') + '"') -PassThru -WindowStyle Hidden
$fakeOllama = Start-Process node -ArgumentList ('"' + (Join-Path $PSScriptRoot 'fake-ollama.js') + '"') -PassThru -WindowStyle Hidden
# String's updater: two small fake releases (a good one, one whose inside says another version), zipped here and served
# with a release key of the fake GitHub's own (tests\fake-github.js)
$upd = Join-Path $run 'update'
foreach ($kind in 'good', 'liar') {
    $rel = Join-Path $upd "$kind-release"
    New-Item -ItemType Directory -Force (Join-Path $rel 'app\setup') | Out-Null
    Set-Content -LiteralPath (Join-Path $rel 'app\String.exe') 'not a real program'
    Set-Content -LiteralPath (Join-Path $rel 'app\setup\install.ps1') "'a stand-in installer that is never run'"
    Set-Content -LiteralPath (Join-Path $rel 'app\setup\version.txt') $(if ($kind -eq 'good') { '9.9.9' } else { '9.9.8' })
    Set-Content -LiteralPath (Join-Path $rel 'Install String.cmd') '@echo off'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory($rel, (Join-Path $upd "$kind.zip"))
}
$fakeGitHub = Start-Process node -ArgumentList ('"' + (Join-Path $PSScriptRoot 'fake-github.js') + '"'), ('"' + $upd + '"') -PassThru -WindowStyle Hidden
for ($i = 0; $i -lt 50 -and -not (Test-Path -LiteralPath (Join-Path $upd 'update-key.txt')); $i++) { Start-Sleep -Milliseconds 100 }
$updateKey = (Get-Content -LiteralPath (Join-Path $upd 'update-key.txt') -Raw).Trim()
try {
    Start-Sleep -Milliseconds 700
    $args = @('--smoke-test', '--data', "`"$run\data`"", '--out', "`"$run\out`"", '--test-media', "`"$media`"", '--lrclib', 'http://127.0.0.1:47913', '--ollama', 'http://127.0.0.1:47914', '--navidrome-db', "`"$navidromeDb`"", '--update-feed', 'http://127.0.0.1:47915/repos/good/releases/latest', '--update-key', $updateKey, '--timeout', '300')
    # started through .NET, not Start-Process, which reads brackets in a path as a wildcard
    $p = [Diagnostics.Process]::Start((New-Object Diagnostics.ProcessStartInfo $exe, ($args -join ' ')))
    # String closes itself after its own timeout; a window that never even starts the test is stopped after 10 minutes
    $hung = -not $p.WaitForExit(600000)
    if ($hung) { try { $p.Kill() } catch { }; 'the test window never finished: stopped after 10 minutes' }
}
finally {
    Stop-Process -Id $fake.Id -Force -ErrorAction SilentlyContinue
    Stop-Process -Id $fakeOllama.Id -Force -ErrorAction SilentlyContinue
    Stop-Process -Id $fakeGitHub.Id -Force -ErrorAction SilentlyContinue
}
$results = Join-Path $run 'out\results.txt'
if (-not (Test-Path $results)) { "no results: see $run"; exit 1 }
$text = Get-Content $results -Raw
$lines = ($text -replace '\\"', '"') -split '","'
$lines | Where-Object { $_ -match 'FAIL|ERROR|result:' }
"screenshots: $run\out"
# Passes only on a finished run whose last line is "result: N passed, 0 failed" (N > 0), with no error from the host.
# Anything else (a failed check, a timeout before the end, an exception, no result at all) exits 1, so a script that
# runs this can trust the exit code.
$final = (Get-Content $results | Where-Object { $_ -like 'final *' } | Select-Object -Last 1)
$state = $null
if ($final) { try { $state = $final.Substring(14) | ConvertFrom-Json } catch { } }
$summary = if ($state -and $state.log) { @($state.log) | Where-Object { $_ -match '^result: \d+ passed, \d+ failed$' } | Select-Object -Last 1 } else { $null }
$hostError = [bool](Get-Content $results | Where-Object { $_ -like 'error *' })
if (-not $hung -and $summary -match '^result: (\d+) passed, (\d+) failed$' -and [int]$Matches[1] -gt 0 -and [int]$Matches[2] -eq 0 -and $state.done -eq $true -and -not $hostError) { exit 0 }
"SELF-TEST FAILED: " + $(if ($hung) { 'the window hung and was stopped' } elseif (-not $state) { 'no final state' } elseif ($state.done -ne $true) { 'it did not finish (timeout?)' } elseif (-not $summary) { 'no result line' } elseif ($hostError) { 'the host reported an error' } else { $summary })
exit 1
