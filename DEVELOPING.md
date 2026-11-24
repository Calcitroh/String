# Developing String

String is a WPF + WebView2 app (.NET 10, Windows): a small host window (`src\App.cs`) around a web page
(`src\wwwroot\`) that is the whole player. The page talks to the host through `window.belt` (`src\wwwroot\host.js`).
Dark and monochrome on purpose: contrast and weight do the work colour would do.

## Layout

- `src\App.cs`: the host. Single instance (`StringApp.*` mutex and events; `--quit` asks a running copy to save and
  close), window placement, dark title bar, no white flash. Serves the page at `https://string.example/` and
  `/_media/<path>`: a sound or picture from any folder, read-only, Range answered with 206. Range bodies always go out
  through `SliceStream`: WebView2 rewinds a seekable stream before reading it. The Range header is read before the
  file is opened (a number too big is a 416). `load` reads state.json and listening.json together; a read that fails
  (a lock, broken JSON) is an error, never empty defaults: the page shows "Your library could not open" with Try again,
  and the host refuses `save` / `store` until the page has sent `profileReady` after a good load (so nothing overwrites
  good data).
  - **Saving:** `state.json` and other `<name>.json` (listening stats) are written one after another off the window's
    thread, with a daily copy in `Documents\String Backup\` (60 kept), and every write is answered
    (`{ kind: 'saved', name, seq, ok, error }`). `host.js` keeps a write that failed (a full or locked disk), toasts
    once, retries every 10 s and says when it's saved again. Closing waits for the writes; if one failed, String stays
    open and the page asks (`closeBlocked`: "Keep String open" or "Close anyway" → `closeAnyway`). A page that never
    answers the flush is closed regardless. `WriteAtomic`: one write per file at a time, each with its own temporary
    name. Unexpected errors and failed saves go to `<data>\errors.log`.
  - **Calls for the page** (`belt.native.*`): paths, list, stats, readText, writeText and copy (only into the data
    folder), mkdir, pickers, reveal, dropped files, run (only `.exe`s in String's own folder or the data folder, with
    Python told to speak UTF-8), recycle (song files to the Recycle Bin via SHFileOperation with FOF_ALLOWUNDO, only
    from the Duplicates page after the person's yes; in a self-test into the test's data folder instead), replaceSong
    (an existing song file swapped for new bytes with File.Replace, the first original kept in
    `<data>\tag-backup\<path hash> <name>`).
  - **Taskbar thumbnail buttons** (previous, play/pause, next under String's taskbar preview): WPF `TaskbarItemInfo`;
    a press goes to the page as `{ kind: 'media' }` (`belt.onMedia`), and the page's `belt.setPlaying` swaps
    play/pause; glyphs turn dark if Windows' taskbar is light.
  - The `*.old` files a developer update left (in use while it ran) are deleted at start: only those
    `install-update.ps1` listed in `app\update-old.txt`.
  - `--smoke-test` runs the page's self-test off-screen (below).
  - **Hidden or minimised:** the host hides the WebView2 control (so Chromium stops drawing), sets its
    `MemoryUsageTargetLevel` to Low until it's shown again, and tells the page (`hide` / `show`): the visualizers, the
    seek bar, the lyrics and every CSS animation (`:root.string-hidden`) stop. The browser runs with
    `--disable-background-timer-throttling` (a hidden page's timers are otherwise slowed to once a minute after five
    minutes unless it's audibly playing, and muted playback still needs the 30 ms loop check and its hand-overs) and
    `--js-flags=--optimize-for-size` (the page's JavaScript takes about a third of the memory).
- **Your own look:** the host serves `<data>\look\` at `/_look/[v<n>/]<file>` (css, pictures, fonts only; never
  outside the folder or through a link inside it; each file read whole and closed, so an editor's save never finds it
  held; a missing look.css is an empty sheet), watches it (and the data folder for the look folder itself appearing or
  being replaced) and posts `{ kind: 'look' }`. `host.js` appends the link to the head, after String's own styles, in
  the background (so a slow `@import` never holds up the player; not in index.html, so a person's edited index.html
  survives updates) unless `localStorage['string.lookOff']`; each reload uses a new `v<n>` so same-named pictures load
  fresh, and swaps the sheet only once the new one has loaded. Settings → Look: Open folder (`lookFolder`: makes it
  with `wwwroot\look-starter.css`, examples all commented out) and "Use my look.css" (`set.look`).
- **Updates** (`src\Updater.cs`): an installed copy (`setup\installed.txt` names its own folder) asks GitHub for the
  newest release of `UpdateRepo` (`/releases/latest`; drafts and pre-releases don't count) once a day, unless Settings
  → Updates turns that off (`set.updates`). Installing downloads `String.zip` (progress as `{ kind: 'update' }`),
  checks `String.zip.sig` (ECDSA P-256 over SHA-256 of the whole zip) against `UpdateKey`, unpacks it into
  `<data>\updates\`, checks `app\setup\version.txt` inside says the version the release is named for (so an older
  signed release can't be passed off as new), and runs that release's own `app\setup\install.ps1 -To <this folder>` in
  a console window of its own (a generated `update.cmd`). The installer closes String (which saves first), keeps the
  person's changes, and opens the new String. A copy built from source never updates itself. `--update-feed` and
  `--update-key` (self-tests only) point it at `tests\fake-github.js`.
- `src\wwwroot\host.js`: `window.belt`, the page's calls to the host. Also the toast and the glyph set.
- `src\wwwroot\`:
  - `tags.js`: the pure tag reader (ID3v2/v1, FLAC, MP4, Ogg Vorbis/Opus, WAV; reads only the bytes it needs;
    `readLyrics` for lyrics inside a song: USLT, TXXX:LYRICS and TXXX:USLT (how FFmpeg writes them), Vorbis LYRICS, MP4
    ©lyr).
  - `library.js`: scanning and the tag cache (`library.json`). A folder that can't be listed keeps the songs already
    known under it; a song whose tags couldn't be read (`readError`) is read again at the next scan.
  - `player.js`: two decks (gapless hand-over, crossfade, fades), the queue, presence.json, listening ticks, and the
    loop (`N.S.loops[track id] = { a, b }`: set by right-clicking the seek bar or `[` `]`, remembered per song,
    checked every 30 ms while playing, a 40 ms sound dip at the jump, the heard time counted up to the loop's end; at
    least a second long, and a shorter saved one counts as none; a looped song never preloads or crossfades). At the
    end of a list play goes on (not with Repeat on, a loop, or `set.autoplay === false`): the next album or artist in
    their page order, Songs from the top, and after Liked, a playlist or a search the library's other songs (same
    artist first). That list is picked near the end of the last song (`continuation`, held in memory and never
    saved) so preload, gapless and crossfade work across it, and joins the queue when it starts. `q.listStart` is where
    the list now playing begins and `q.past` the earlier lists' starts and contexts: Repeat All repeats only the list
    now playing, Previous can still go back. `P.context(i)` is the list a queue place belongs to. Hidden songs never
    play. `P.history` is this run's last 80 songs that really started (the queue's Recent tab).
  - `lyrics.js`: lyrics for the song playing: a .lrc next to it, then the song's own, then LRCLIB (only after the
    person's yes, `set.lyricsOnline`), kept in `lyrics\` in the data folder for good. Be gentle with LRCLIB (it runs
    on donations): a song is asked about once (`asked` in its cache file), "nothing found" again only after 30 days,
    only once a song has stayed 1.2 s in the lyrics view (skipping asks nothing), and an ask that failed (offline,
    busy) is never remembered. LRC parsing, the current line and LRCLIB match picking are pure (Node-tested).
  - `translate.js`: lyric translation by a model on this PC through Ollama (Translate in the lyrics header, or T;
    `set.translate`, `trLang`, `trModel`, default the best downloaded: Qwen 3 14B first): the whole song in one request,
    repeated lines once, `think: false` for thinking models, the reply streamed as "N| text" lines into the view, kept
    in `translations\`.
  - `dupes.js`: `duplicates()` for the Duplicates page (route `dupes`, Library → Find duplicates): copies are *sure*
    (same title once "(Official Video)"-like brackets are gone, same artist, length within 2 s) or *maybe* (same title
    before any brackets / " - " / "feat.", same length, another or no credit); best copy first (lossless, bitrate, then
    the page's `prefer`: the folder holding most songs, then liked / in a playlist / played). The page lets you play
    each copy, pick the one to keep (sure copies of it are ticked, maybe ones not), then Hide in String (`S.hidden`, the
    file stays) or Move to the Recycle Bin (asks first). Either way `mergeCopy` gives the kept copy the dropped one's
    like, plays, loop, listening time (`N.stats.merge`), playlist places and queue spot; a copy playing when recycled
    hands over to the kept one at the same moment. "Not the same" (maybe groups) stores the pairs in `S.notDupes`.
  - `stats.js`: listening time (`listening.json`: per day, hour, artist and song, and `p`: the part heard on a
    phone; the model is pure and Node-tested). `phone.js`: phone listening through Navidrome on this PC, whose
    `scrobbles` table is its play history; each new play (`S.phone.after` = the last play number counted) becomes the
    whole song in Stats at the time it was played, under String's names when String has the file. A moment after
    start, every 5 min, and when the window comes back; `set.phoneStats === false` stops it.
  - `viz.js` the visualizers: drawn only while String is in view and music plays, or while the bars fall and the
    record coasts to a stop after a pause; the audio mixer (Web Audio) sleeps after a pause's fade and wakes on play.
    `art.js` covers: thumbnails per size (176, 420, 960 px), identical embedded pictures share one (SHA-256 of the
    bytes), at most two full-size pictures decoded at a time, album and artist cards past the first 24 draw their
    stand-in pattern only when scrolled near. `string.js` the screens, `hud.css` the HUD look (tints Mono / Arc /
    Ember), `string.css`, `selftest.js`. The filter on Songs, Liked and playlists keeps its text while the same list
    redraws and clears when you leave it; a filtered list keeps `positions` (each shown row's place in the whole
    list), so Delete, menus and playing act on the rows shown, and a playlist can't be reordered while filtered.
- `presence\`: the Discord helper `string-presence.exe` (published into `app-new\presence\`). It follows
  `presence.json` and talks to Discord's `discord-ipc-N` pipe with the person's own application ID: activity type 2,
  `status_display_type` from the setting, at most one update per 4.2 s, cleared on pause and when a stale song's time
  bar has run out; optional covers (song + artist only, off by default) from Apple's iTunes search, then Deezer's,
  remembered in `discord-covers.json` ("v2" keys). String closes its stdin at once, so it watches its parent process
  to stop with String.
- The host's `lrclib` call (`belt.native.lrclib(path)`) is the one door to LRCLIB: only `/api/` paths, with String's
  User-Agent; `--lrclib <url>` (self-tests only) points it at `tests\fake-lrclib.js`.
- Ollama (`ollamaModels`, `ollamaChat`: a streamed chat delivered like a program's output lines): the host starts
  `%LOCALAPPDATA%\Programs\Ollama\ollama.exe serve` if Ollama isn't running, and stops that one when String closes (an
  Ollama the person runs is left alone); requests ask `keep_alive: 3m`, so the model leaves the graphics card soon.
  `--ollama <url>` (self-tests only) points it at `tests\fake-ollama.js`; a self-test never starts the real one.
- The host's `navidromePlays(after)` reads Navidrome's database read-only with Microsoft.Data.Sqlite (its path from
  `DataFolder` in `C:\Program Files\Navidrome\navidrome.ini`, else `C:\ProgramData\Navidrome\navidrome.db`): scrobbles
  joined to media_file and library, as full paths. `--navidrome-db <file>` (only with --smoke-test) points it at the
  database `tests\fake-navidrome.js` makes; a self-test never reads a real one.
- The icon is String's logo, by awepollyon: `native\icon\logo.png`. `python native\icon\logo-to-icon.py` makes
  `src\Assets\String.ico` / `.png`, String 2.0's copies and `src\wwwroot\string-icon-credit.png` from it (Pillow).
  The first time String starts with it, a card above the player bar credits its maker until closed (`iconCredit` in
  string.js, `#stringIconCredit`, its layout on the element because an update keeps a person's edited string.css;
  `set.iconCreditSeen`, shared with String 2.0). The installer then tells Windows icons changed, without waiting.

Data: `%LOCALAPPDATA%\String\` (state.json, listening.json, library.json, presence.json, discord-covers.json,
window.json, errors.log, lyrics\, translations\, tag-backup\, look\ (the person's own look), addons\, updates\,
webview\).

## Add-ons

Extra features a person installs: a folder `<data>\addons\<id>\` (`id`: lowercase letters, digits, dashes) with an
`addon.json`:

```json
{ "id": "example", "name": "Example", "version": "1.0.0", "scripts": ["example.js"], "styles": ["example.css"] }
```

The host lists them (`belt.native.addons`, skipping a broken `addon.json` and any file named outside the add-on's own
folder) and serves their files at `/_addon/<id>/<file>` (scripts, styles, pictures, fonts; nothing outside the folder,
never through a link inside it). `host.js` adds their styles (before the person's own look, which stays on top) and
then their scripts in order, once the page's own scripts have run; String's start waits for them
(`belt.addonsReady`). A script uses `N.addons` (in `string.js`):

- `page(name, fn)`: a page at route `name`; `fn(viewElement, argument)` draws it.
- `sidebarButton({ id, title, icon, route })`: a small button by "Playlists" that opens a route.
- `librarySection(fn)`: `fn()` returns HTML for the Library page.
- `editInfoNote(fn)`: `fn(track)` returns a sentence for Edit info, or `''`.
- `tests`: async functions `fn(ctx)` the self-test runs (`ctx` has `check`, `sleep`, `waitFor`, `step`, `paths`,
  `media`, `lower`, `L`, `P`, `U`, `N`).

and hears what String does through `N.on`: `'cacheLoaded'`, `'booted'`, `'edited'` (a song's id), `'merged'`
(`{ from, to }`: a duplicate dropped for the copy kept), `'removed'` (the ids of song files moved to the Recycle Bin).
`N.ui.h` has small helpers for drawing (`$`, `$$`, `esc`, `attr`, `plural`, `enc`, `pageTitle`). Add-ons run as part
of String: only install ones from someone you trust.

## Build, test, install

FFmpeg makes the test songs: have `ffmpeg` on the PATH, or set `FFMPEG` to its path.

```
dotnet publish src\String.csproj -c Release -o app-new
dotnet publish presence\Presence.csproj -c Release -o app-new\presence
node tests\test-tags.js
node tests\test-stats.js
node tests\test-lyrics.js
node tests\test-translate.js
node tests\test-dupes.js
node tests\test-player-regressions.js
node tests\test-presence.js
node tests\test-profile-recovery.js
powershell -File tests\run-selftest.ps1
powershell -File tests\measure.ps1 [-LongHidden] [-Trace]
powershell -File install-update.ps1
```

- `tests\`: `test-tags.js` (makes test songs with FFmpeg in `test-runs\string-media`, some with lyrics and a .lrc),
  `test-stats.js`, `test-lyrics.js`, `test-translate.js` (including odd model output that must not freeze the page),
  `test-dupes.js`, `test-player-regressions.js` (page modules in a Node vm with a fake host and clock: queue inserts,
  legacy queue restore, a folder added mid-scan, hour-long saving, saves the disk refused, unreadable folders, loops,
  lyrics retries), `test-profile-recovery.js` (a copy of app-new in test-runs with locked state/listening files:
  nothing overwritten, Try again recovers; needs app-new), `test-presence.js` (a fake Discord on a pipe of its own,
  never a real Discord, and fake Apple and Deezer searches), `fake-lrclib.js`, `fake-ollama.js`, `fake-navidrome.js`
  (a WAL SQLite file with Navidrome's three tables, made with node:sqlite), `fake-github.js` (releases signed with a
  key of its own: a good one, a tampered one, one that lies about its version, an older one, none).
- `measure.ps1` (needs app-new; `-Build <folder>` measures another build, e.g. a release to compare against): a
  library of 1,200 short songs with full-size covers (`measure-library.js`, FFmpeg) and a copy of the build whose
  self-test is `measure-page.js`, run with `--smoke-test`: playing on Songs, on Albums, in Now Playing and in
  synced Lyrics, then paused, minimised and playing, and after opening every album.
  `test-runs\measure-<time>\processes.csv` has each process's private working set (MiB, median of three samples)
  and CPU (100% = one logical processor); `data\<phase>.json` and `<phase>-end.json` hold the visualizer's frame
  count and time, so frames per second is their difference. `-LongHidden` adds five minutes minimised with a muted
  loop: rerun it after changing WebView2's settings. `-Trace` (Node 22+) also records a Chromium performance trace of
  Albums, Now Playing and Lyrics (`measure-trace.js`): per redraw, the time in script, style, layout, paint, raster
  and the GPU process.
- `run-selftest.ps1`: the in-window self-test against the fakes, each run in its own `test-runs\selftest-<time>\`,
  muted and off-screen, with `results.txt` and a screenshot per step; it closes itself and exits 0 only on a finished
  run ending "N passed, 0 failed". It plants a small probe add-on (and a broken one, and one naming files outside its
  folder); `-Addon <folder>` tests a real add-on too, and `-Exe <String.exe>` another build. Tests never ask real
  online services.
- `install-update.ps1` (for a copy built from source): asks the String running from `app\` to save and close, refuses
  an app-new missing its parts, copies `app-new\` over `app\`, removes files its own last install put there that the
  new build no longer has (`app\update-files.txt`; nothing else in `app\` is touched), adds a Start menu entry on the
  first install, and reopens it.

## Releases

`pwsh -File tools\release.ps1 [-Version 1.2.0] [-Notes "..."] [-Publish]` makes `release\String.zip`: a self-contained
build (no .NET to install; the Discord helper self-contained and trimmed) with, from `tools\share\`: `Install
String.cmd` → `app\setup\install.ps1`, and the notes `Read me first.txt` and `Phone setup.txt`. Licences go in
`app\licenses\` (runtime and WebView2 from the NuGet packages, the rest from `tools\share\licenses\`).
`app\setup\version.txt` is the version; `app\setup\files.txt` lists every program file (SHA-256, size, path, then
`#end<TAB>count`). It refuses to zip data files, debug files, or anything holding the build PC's user or profile name.
With the release key at `%USERPROFILE%\.string-release\release-key.pem` (its public half must be `UpdateKey`), it
signs the zip (`release\String.zip.sig`); `-Publish` puts both on GitHub with the GitHub CLI.

For 2.0, run `pwsh -File tools\release-native.ps1 -Full -Version 2.0.0`. It checks the version against
`string-app` and the finished executable. It writes the same `release\String.zip` and detached signature used by
installed 1.x and 2.x copies. The zip contains `Install String.cmd`, `Read me first.txt`, `app\String.exe`, the
native licences and the same `app\setup` scripts and manifest. Both updaters must recognize the signing key.
Without a key, the builder produces an unsigned zip for manual testing. Full native publishing is disabled.

The installer (per user, no admin): `%LOCALAPPDATA%\Programs\String`, Start menu and desktop shortcuts, an HKCU Apps
entry that runs `setup\uninstall.ps1`; the installed copy loses Windows' downloaded-file mark. It checks the unzipped
copy against `files.txt` (hashes through .NET: Get-FileHash vanishes when Windows PowerShell 5.1 inherits pwsh's
PSModulePath, so the .cmd clears it), copies into a sibling `String.new-*`, swaps, and writes `setup\installed.txt`;
the next update and the uninstaller remove only listed files inside the folder, never through a link, and the
uninstaller runs only where that mark names its folder. An update keeps the person's own changes: every file in the
installed copy that isn't listed-and-unchanged is copied to `<install> edits\<date time>\` first, then carried into
the new copy; where the update changed the same file, CSS/pictures/fonts stay theirs (the update's saved in
`<date time> (the update's versions)\`) and code takes the update's. The old copy goes file by file, only what is
String's unchanged or identical to its saved copy. The uninstaller copies every edited listed file out before removing
anything, and deletes data only after a typed `yes`. Those files stay plain ASCII and Windows PowerShell 5.1
compatible.

On the move from 1.x to 2.0, the installer retires the bundled .NET runtime, WebView2 files, web pages and
presence helper. Edits to retired files and additions inside the old web, runtime or helper folders are saved in the dated
edits folder. Other personal files carry forward. Only 1.x checks for the WebView2 runtime.

Testing the installer: never run it (or the uninstaller) with default parameters on a PC whose String you use (they
would take over its Start menu entry and its data), and never start an installed test copy without `--smoke-test`:
`--quit` and the single-instance signals are shared with a running String. Set `-To -Menu -Desktop -AppsKey`,
use `-NoStart` for installation and `-Data -DataChoice keep` for removal.

`pwsh -File tests\test-update-20.ps1` builds 1.3.1, saves its zip, then builds 2.0.0. To replay archives,
pass `-LegacyZip <1.3.1 zip> -NativeZip <2.0.0 zip>` with their adjacent `.sig` files. The test checks any
supplied signatures, performs a clean install and a migration with Windows PowerShell 5.1, compares the profile
and program files, checks the shortcuts and exercises uninstall. It runs the upgraded player's muted, off-screen
self-test with generated songs. Each run prints its folder under `test-runs\`, including `smoke\results.json`.

## String 2.0 (native)

`native\` is String rebuilt as a native program in Rust: a Cargo workspace on the MSVC toolchain
(`native\rust-toolchain.toml`; it needs Visual Studio Build Tools' C++ tools). It reads and writes the same data
folder and files as the web version, with the same single-instance names (`StringApp.*`), so only one of the two runs
at a time.

- `string-core`: the library scan and tag reading (lofty), the saved documents (state.json, listening.json,
  library.json: guarded reads, atomic writes, the daily backup and, the first time a profile opens each day, a copy as
  it was found in `<backup>\opened\<date>\`), the queue and the listening model. `stringctl` inspects, round-trips or
  scans a data folder. Unlike 1.x, Next pressed on a list's last song goes on to the next list even with Repeat One or
  a seek-bar loop on (those repeat a song that ends by itself); only Repeat All wraps, and with "keep playing" off it
  stops at 0:00.
- `string-audio`: decoding (Symphonia), the two decks, gapless and crossfade, loops, the output device.
- `string-lyrics`: lyrics (.lrc, the song's own, LRCLIB) and translation through Ollama.
- `string-extras`: duplicates, Edit info (writing tags), phone listening from Navidrome, and updates (the same feed and
  key as the web version).
- Installed copies check for a release one minute after startup and every six hours thereafter when the daily switch
  is on and the last successful check was at least 24 hours ago. Manual and automatic checks share the worker.
  Both save `update.at` in epoch milliseconds. A quiet check announces each new version once using `update.told`.
  Smoke runs skip automatic checks.
- Updates, LRCLIB and Discord cover lookups share one HTTP client. It verifies TLS with Windows and reads the
  user's manual Internet Settings proxy on first use, including protocol-specific proxies, wildcards and `<local>`.
  With the manual proxy off, it retains environment proxy support. Loopback stays direct. The 1.x Windows HTTP
  client also supports automatic proxy discovery and configuration scripts (WPAD/PAC). On a PAC-only network,
  the native client connects directly unless an environment proxy is configured.
- `string-platform`: Windows: one instance, media keys and taskbar buttons, Discord, files and the Recycle Bin,
  backups, the error log.
- `string-app`: the window and every screen (egui and wgpu), and `String.exe`. `native\string-app\SCREENS.md` says how
  the screens are put together. Its icon: `assets\String.ico` (and `.png` for the window and the icon's credit card,
  `ui\credit.rs`), made by `native\icon\logo-to-icon.py`. `build.rs` embeds the icon and the crate's Windows file and
  product versions. Settings > Apps reads the product version.

```
cd native
cargo build --release -p string-app
cargo test --workspace --release
target\release\String.exe --smoke-test --data <a folder> --out <a folder>
```

The self-test runs muted and off-screen with instance names of its own, and writes `results.json` and a screenshot
per step; `--data` takes a copy of a real profile too. Without `--smoke-test`, `--data <folder>` opens String on that
folder (its backups stay inside it), and it shares the single-instance names, so close any other String first.
Its `stats-spotify` step imports the export `STRING_SMOKE_SPOTIFY_EXPORT` names (else a small made-up one, unless the
profile already has a Spotify history) the way a drop on Stats does, and writes its numbers to `stats-spotify.json`.

Stats can include a Spotify history (Spotify's "Extended streaming history" export) from `<data>\spotify.json`, a
document of its own in `listening.json`'s shape: written only by an import, read on a worker at startup, never touched
by 1.x. Stats' "Spotify history" panel takes the export dropped on the page or chosen there (a drop is sorted on a
worker: a history to the import, music to the library); a newer export adds only later plays, and Remove sets the
file aside in the backup folder. Plays are matched when Stats reads them, not stored: normalized title and main artist
to a library song, artists merged by name, matched again when song names change.

`pwsh -File tools\release-native.ps1 -Version 2.0.0-preview.1 [-Notes "..."] [-Publish]` makes
`release\String-<version>.zip`: `String.exe`, `Read me first.txt` (from `tools\share\Preview read me.txt`) and
`licenses\` (every library it's built from with its licence texts, Rust's, and String's). It builds in
`release\native-build` with the source paths renamed (the libraries' to `/cargo`, String's to `/string`), runs the same
privacy check as `tools\release.ps1`, and `-Publish` makes a GitHub pre-release on the pushed commit it was built from.
Installed copies never update to a pre-release; the script checks that the newest full release is still the same
afterwards. For an installable build, use `-Full` as described in Releases. Both formats link the Visual C++ runtime
statically and check the executable's PE imports before packaging.

Your own look in 2.0 (`string-app\src\theme\look.rs`) uses the same `<data>\look\look.css`, Open folder and
`set.look` as 1.x. egui can't run CSS, so it reads what it can draw, with 1.x's meaning: `--hi` (a custom `Tint`;
without `!important` it wins over Mono only, as Arc and Ember's own rules are more specific), `.brand`'s picture,
height and padding and its hidden parts (`.brand > *`, `svg`, `b`), and `#main`'s picture (size, position, repeat,
`content-box`). The surfaces, text colours and fonts are left out: the colours are constants every screen draws
with, and egui panics on a font it can't parse. Files are read whole, only inside the folder and never through a
link. A worker looks at the files the look uses (four times a second, once a second without a look.css) and reads
them again once a change has held still for one look, so nothing is held open.

## Things learned the hard way

- WebView2 rewinds seekable response streams (see `SliceStream`).
- Web Audio is routed only once its context runs; a suspended context would play silence.
- The HUD kit gives every `svg` a stroke: charts set `.bars, .bars * { stroke: none }`.
- View transitions name `#scroll` (not `#view`), so a page sliding in stays clipped above the player bar.
- The saved queue is the whole queue with each path stored once (`snapshot()` in string.js: sorted, each path kept as
  what differs from the one before it; the queue and its original order as numbers into that list), or a 12,000-song
  shuffle would save megabytes. `N.save()` hands `belt.save` the function, so the snapshot is made once per burst.
- `-replace '\\?$', '\'` in PowerShell (and .NET Replace with `x?$`) adds the character twice when it's already there:
  the empty match at the very end counts too. Use `TrimEnd('\') + '\'`.
- Translation reply parsing streams: a pattern with two `\s*` around an optional part, or a lazy `(.*?)\s*$`, takes
  time in the square of a line's length (235 ms for one 40,000-character line, every streamed word). translate.js
  uses bounded quantifiers and plain trimming.
- Repeat-one must rewrite presence.json each time the song starts over, or Discord's time bar runs out and clears.
- Class names are global: `.now` and `.tr` (song rows) were each reused by mistake once, and the new element took the
  row's absolute layout. Prefix new classes (`ll-tr`, `ly-…`, `np-…`).
- In `String.prototype.replace`, `$$` in the replacement is one `$`: scripted edits of this code (full of `$$(…)`)
  must use split/join, or an editor.
- Chromium reuses a picture it already has for the same address, even with `no-store`: reloading a stylesheet doesn't
  refetch `url(logo.png)`. A new folder in the address per reload (`/_look/v<n>/`) does.
- A response stream WebView2 still holds keeps the file open: an atomic save (File.Replace) of that file then fails.
  Files a person edits are served from memory.
- Nothing next to the seek bar may change its width (the loop chip floats above it): a right-click would otherwise land
  on a different time than the one under the pointer.
- Scripts an add-on adds after the page loads run only once the page's own scripts have (String waits for
  DOMContentLoaded before adding them), so they always find `N.addons` and `N.ui`.
- A visualizer drawn with requestAnimationFrame runs at the screen's refresh rate: on a 240 Hz screen that is 240
  frames a second, most of String's processor use while it's visible and playing. Making each frame cheaper is the
  lever, never a frame-rate cap. What a frame mostly costs is raster and compositing work in the GPU process, not
  script: the sidebar's animated wire, the player bar's progress line and the spinning disc each get their own layer
  (`will-change: transform`, only while playing and visible), so their redraws don't repaint what's behind them. On
  Albums that halved the processor use; in Now Playing and Lyrics the screen reaches more of the display's frames
  for about 20 MiB more GPU memory. Caching the whole spinning record as one layer looked softer and was dropped.
- A WebView2 inside a minimised WPF window keeps drawing unless the control is hidden, and a hidden, muted page has
  its timers throttled (see "Hidden or minimised" above).
