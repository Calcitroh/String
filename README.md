# String

A dark, monochrome local music player for Windows 10 and 11. Plays your music files right where they sit, without
changing them. No ads, no accounts, no cloud.

> Before anything else, yes this is vibe coded. It also works! (hopefully)
>
> Icon's humanmade though, (DC - awepollyon) Thanks Bro <3

## Features

- **Library:** Home, Songs, Albums, Artists, Liked, playlists, and search (Songs, Liked, and playlists also have a
  filter box). Add folders from Library (the gear at the top) or drop them onto the window.
- **Playing:** Queue (drag songs onto the queue button; queued songs play first), Recent tab, shuffle, repeat, gapless
  or crossfade, media keys, and taskbar controls. Now Playing has a spinning record and a spectrum ring that moves with
  the music. Keeps going with the next album or more of your songs when a list ends (can be turned off in Settings).
  Click a song's length to see time left.
- **Lyrics:** Hit `Y` or the lyrics button. Pulls from a `.lrc` file next to the song, embedded lyrics, or
  [LRCLIB](https://lrclib.net) if you opt in (saved locally after the first lookup). Synced lines light up as they play
  and you can click any line to jump there. Hit `T` to put a translation under each line, run locally through
  [Ollama](https://ollama.com) so nothing leaves your PC.
- **Loop:** Right-click the seek bar to set "Start the loop here" and "End the loop here". Each song remembers its loop
  until you clear it (chip by the seek bar, or `\`).
- **Stats:** Listening time for today, this week, this month, and this year, plus your top artists and songs. Plays
  from your phone through [Navidrome](https://www.navidrome.org) on the same PC count too.
- **Duplicates:** `Library → Find duplicates` catches tracks you have more than once. Pick the copy to keep and either
  hide or Recycle Bin the rest (it asks first). Your likes, plays, loops, and playlist spots carry over to the one you
  kept.
- **Custom look:** `Library → Look → Open folder` creates a `look.css` with examples for colors, a custom logo, a
  background image, and fonts. It loads on top of the default theme and updates won't touch it.
- **Discord:** Shows "Listening to" with the song, artist, and time bar. Set it up once in `Library → Discord`.
- **Updates:** Checks once a day for signed updates from this project and asks before installing (turn it off in
  `Library → Updates`).

Keys: `Space` play/pause · `Ctrl+←/→` prev/next · `←/→` seek · `Ctrl+↑/↓` volume · `M` mute · `S` shuffle · `R` repeat ·
`L` like · `Q` queue · `F` Now Playing · `Y` lyrics · `T` translate · `[` and `]` loop start/end · `\` stop loop ·
`/` search

## Install

1. Download `String.zip` from the latest release on the [Releases](../../releases/latest) page.
2. Right-click it → **Extract All...** → **Extract**.
3. Double-click `Install String` in the folder that opens.

Windows will probably warn you since it isn't signed with a paid cert: click **More info**, then **Run anyway**.
Installs for your Windows account only (no admin needed), adds Start menu and desktop shortcuts, and uninstalls from
Settings → Apps. `Read me first.txt` in the zip has the details, and `Phone setup.txt` covers listening on your phone.

## String 2.0 preview

String is getting rebuilt as a native Windows app in Rust (`native/`). Same player, about half the RAM and way less CPU
usage. There's a pre-release on the [Releases](../../releases) page: unzip it and open `String.exe`. It uses your
existing String library without messing with your main install (installed copies never auto-update to previews). Not in
yet: add-ons, custom CSS, installer, and auto-updates.

## What goes online

String doesn't upload your library or listening history. These features make online requests:

- **Lyrics** (off until you say yes): sends song title, artist, album, and length to [LRCLIB](https://lrclib.net).
- **Discord covers** (off until you turn it on): sends song and artist name to Apple's iTunes search, then Deezer as a
  fallback.
- **Updates** (on by default; disable in `Library → Updates`): checks GitHub once a day for the latest release.

Your library, likes, playlists, and stats stay on your PC in `%LOCALAPPDATA%\String`, with a daily backup in
`Documents\String Backup`.

## Add-ons & Building

Drop add-ons into `%LOCALAPPDATA%\String\addons\<name>\`. They run as part of String, so only install ones from people
you trust.

For [how add-ons work](DEVELOPING.md#add-ons) or building from source, check [DEVELOPING.md](DEVELOPING.md). String is
MIT-licensed (see [LICENSE](LICENSE)).
