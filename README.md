# String

Your own songs, played like a streaming app. No ads, no accounts, no cloud: String plays the music files in the
folders you pick, where they are, and doesn't change them. Dark and monochrome, for Windows 10 and 11.

- **Library:** Home, Songs, Albums, Artists, Liked, playlists and search. Songs, Liked and playlists have a filter box.
  Add folders in Library (the gear at the top) or drop them on the window.
- **Playing:** a queue (songs you add play first; drag songs onto the queue button) with a Recent tab, shuffle, repeat,
  gapless or crossfaded songs, media keys, taskbar buttons, and Now Playing with a spinning record and a spectrum ring
  that moves with the music. When a list ends, play goes on with the next album or more of your songs (Settings can
  turn that off). Click a song's length to see the time left.
- **Lyrics:** the lyrics button (or Y). Synced lyrics light up line by line; click a line to go there. They come from a
  .lrc file next to the song, the song's own lyrics, or LRCLIB online (only if you say yes; each song is looked up once
  and kept). **Translate** (or T) puts each line's translation under it, made on your PC by a model in
  [Ollama](https://ollama.com): nothing leaves your computer.
- **Loop:** right-click the seek bar: "Start the loop here", then "End the loop here". Each song remembers its loop
  until you stop it (the chip by the seek bar, or \\).
- **Stats:** how long you've listened today, this week, this month and this year, with your top artists and songs.
  If your phone plays your library through [Navidrome](https://www.navidrome.org) on the same PC, those plays count too.
- **Duplicates:** Library → Find duplicates lists songs you have more than once. Pick the copy to keep, then hide the
  others in String or move them to the Recycle Bin (it asks first). The copy you keep takes over their likes, plays,
  loops and playlist places.
- **Your own look:** Library → Look → Open folder makes a `look.css` full of examples (colours, your own logo, a
  background picture, a font). It loads after String's own look and updates never touch it.
- **Discord:** "Listening to" with the song, the artist and a time bar. Set it up once in Library → Discord.
- **Updates:** String checks for a newer version once a day and offers to install it (you can turn that off in
  Library → Updates). Updates are signed: String only installs ones from this project.

Keys: Space play/pause · Ctrl+←/→ previous/next · ←/→ seek · Ctrl+↑/↓ volume · M mute · S shuffle · R repeat ·
L like · Q queue · F Now Playing · Y lyrics · [ and ] loop start and end · \\ stop the loop · / search.

## Install

1. Download **String.zip** from the newest release on the [Releases](../../releases/latest) page.
2. Right-click it, choose **Extract All...**, then **Extract**.
3. Double-click **Install String** in the folder that opens.

Windows may warn you, because String isn't signed with a paid certificate: "More info", then "Run anyway". String
installs for your Windows account only (no admin), with a Start menu and a desktop shortcut, and it uninstalls from
Settings → Apps. "Read me first.txt" in the zip has the details, and "Phone setup.txt" shows how to listen on your phone.

## String 2.0 preview

String is being rebuilt as a native Windows program (written in Rust, in `native/`): the same player, using about half
the memory and far less of the processor. A preview is on the [Releases](../../releases) page as a pre-release: unzip
it and open String.exe. It opens your String library and leaves your installed String alone (installed copies never
update to a preview). Your own look works in part: its tint, your logo and a background picture, not yet its colours
or font. Not in it yet: add-ons, installing and updating.

## What goes online

Nothing about you, and only when you've turned the feature on:

- **Lyrics** (off until you say yes): the song's title, artist, album and length go to [LRCLIB](https://lrclib.net).
- **Discord covers** (off until you turn them on): the song and artist name go to Apple's iTunes search, then Deezer's.
- **Updates** (on; Library → Updates turns it off): once a day String asks GitHub for this project's newest release.

Your library, likes, playlists and stats stay on your PC, in `%LOCALAPPDATA%\String`, with a daily copy in
`Documents\String Backup`.

## Add-ons

String can load add-ons: extra features someone gives you, in `%LOCALAPPDATA%\String\addons\<name>\`. They run as part
of String, so only install add-ons from someone you trust. How they work: [DEVELOPING.md](DEVELOPING.md#add-ons).

## Building it yourself

See [DEVELOPING.md](DEVELOPING.md). String is MIT-licensed: see [LICENSE](LICENSE).
