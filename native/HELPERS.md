# Writing a helper for String 2.0

String 2.0 can't load page add-ons the way String 1.x does. Instead an add-on can be a **helper**: a program of its
own, with its own window, that String starts from a button in its sidebar. A helper adds songs by running String
itself with `--add`.

Helpers run with your rights, like any program you start: only install ones from someone you trust.

## The folder

A helper is an add-on folder in String's data folder, `<data>\addons\<id>\`. `<data>` is `%LOCALAPPDATA%\String`, or
the folder given with `--data`. `<id>` is a lowercase letter or digit followed by up to 40 more lowercase letters,
digits or dashes. The folder holds an `addon.json`:

```json
{
  "id": "example",
  "name": "Example",
  "version": "1.0.0",
  "program": "example-helper.exe",
  "button": { "title": "Example: what it does", "glyph": "down" }
}
```

- `program`: the helper's `.exe`, a plain file name directly inside the add-on's own folder.
- `button.title`: the button's tooltip. Without it, the tooltip is `name`.
- `button.glyph`: the name of one of String's glyphs, from `native\string-app\src\ui\glyphs.json` (`down`, `music`,
  `search`, `globe`, `star`, `tray-down` and many more). A name String doesn't have shows as `plus`.
- `name` and `version` are for people. The folder's name is the add-on's id.

String 1.x's keys (`scripts`, `styles`) can sit in the same `addon.json`: 1.x ignores `program` and `button`, and 2.0
ignores `scripts` and `styles`, so one folder can serve both.

String turns a helper down, and draws no button for it, when:

- `addon.json` isn't valid JSON, isn't an object, or `program` isn't a string;
- `program` names anything other than a file directly inside the add-on's folder (a path, `..`, a drive, a folder);
- `program` isn't an `.exe`, or isn't there;
- the add-ons folder, the add-on's folder, `addon.json` or the program is a link (a symbolic link or a junction);
- there's a `program` but no `button` object.

Each one turned down is written once to `<data>\errors.log`, with why. An add-on with no `program` (a 1.x add-on) is
left alone, as is a folder whose name isn't an id or that has no `addon.json`.

String looks at the add-ons folder when it starts and again each time its window gets the focus, so a helper put in
or taken out shows up after switching back to String.

## The button

Each helper gets a small round button in the sidebar, in the Playlists heading, left of `+`. Three fit; with more,
the last place is a "more" button that lists the rest. While the copy of the helper String started is running, its
button is lit.

A click starts the program:

```
<program> --string-data "<data folder>" --string-exe "<full path of the running String.exe>" --string-version "<x.y.z>"
```

- Its working directory is its own add-on folder.
- String doesn't wait for it, and closing String doesn't close it.
- While the copy String started last is still running, a click doesn't start another: it brings that copy's window
  forward instead.
- In a self-test (`--smoke-test`) helpers are listed and their buttons drawn, but nothing is ever started.

Make the helper a Windows-subsystem program (no console window), and read the three arguments rather than guessing
where String is: `--string-exe` is the exact String to call back, and `--string-data` the data folder it is using.

## Adding songs

```
"<String.exe>" --add "<absolute path>" ["<absolute path>" ...]
```

- Files are added as single songs and folders as library folders, exactly as if they were dropped on String's window.
  Then String scans.
- Give absolute paths. Paths that don't exist are skipped; `--add` with none left does nothing and exits 0.
- If String is running, the paths go to it: it takes them even while minimised, and stays where it is (it doesn't
  come to the front over the helper's window). The `--add` process exits at once without opening a window: **0** once
  String has them, **1** if the String running didn't take them (String 1.x, which has no `--add`, or a String that
  is closing).
- If none is running, that `--add` process becomes String, with those paths added. So don't wait for it to exit:
  start it and go on.
- Everything after `--add` up to the next argument starting with `--` is a path. Windows keeps a command line under
  about 32,000 characters: for more paths than that, run `--add` more than once, or add their folder.

In Rust, for example:

```rust
use std::process::Command;

fn add_to_string(string_exe: &str, paths: &[std::path::PathBuf]) -> std::io::Result<()> {
    let mut child = Command::new(string_exe).arg("--add").args(paths).spawn()?;
    // String running: this ends at once with 0 (taken) or 1 (not taken). None running: this child is String now.
    std::thread::spawn(move || child.wait());
    Ok(())
}
```

## Trying a helper out

The self-test runs String muted and off-screen with instance names of its own, so it never meets your String:

```
String.exe --smoke-test --data <a test data folder> --out <a folder for the results>
```

Put the helper in `<test data folder>\addons\<id>\`: the screenshots show its button, `results.json` says whether the
helpers case passed, and `<test data folder>\errors.log` says why a helper was turned down. `--add <paths>` after the
other options adds those paths to the test library at the start.

## Not in 2.0 yet

Pages drawn inside String, notes in Edit info, and String's events (`merged`, `removed`) are 1.x add-on features
without a 2.0 equivalent so far. A helper has its own window.
