# Extending the String shell

## Screen entry points

Each route has a module in `src/screens/` with the same entry point.

```rust
pub fn show(ui: &mut egui::Ui, v: &mut crate::view::View<'_>) {
    super::heading(ui, "Library", "");
    if crate::ui::btn(ui, "All songs").clicked() {
        v.go(crate::route::Route::Songs);
    }
}
```

For an existing route, replace that file’s body. All twelve routes are already registered in `screens::show`. Record and Lyrics are overlays over the current history entry. Their modules receive the same view. For a new route, extend `Route` and its parser, path and title in `route.rs`, declare the module and add one dispatch arm in `screens/mod.rs`. The history stores typed routes. Album and artist keys round-trip through percent-encoded paths. `v.go(route)` also saves the current route, clears the previous table selection and closes overlays.

## State and playback

- `v.s.models` contains songs by ID, ordered song IDs, album and artist groups, liked IDs, and playlists. `Models::search` returns matching IDs. `Ids` is an immutable shared slice. Clone the handle before a playback action. The catalog includes track visibility and playability for queue decisions.
- `v.s` owns the loaded state, library and listening documents. Use `set_setting`, `set_field`, `toggle_like`, `add_playlist` and `add_to_playlist` for ordinary changes. A change to tag edits or hidden songs also needs `s.rebuild()` and `s.queue_changed(v.player)`. Changing the library cache itself requires `s.library_dirty = true`. Keep screen-only input and selection in `Views` or a screen-owned state struct. Use a generation or input change to invalidate expensive derived lists.
- `v.st` is the engine status for this frame. Start an ordered context with `v.s.play_ids(v.player, &ids, row as isize, v.context())`. Insert manual items with `s.insert`, or jump to an existing queue entry with `v.jump(index)`. Send queue changes through the session so the engine replans a preloaded track. The audio host owns speculative advance tickets and confirms playback from the active output.
- Read `v.s.listening` for statistics. `Song::lyrics_song()` supplies the lyric-service input. Perform long searches, file operations and service calls on a worker. Send the result back with its song ID or revision and call `ctx.request_repaint()`. The UI holds the session lock for the frame.
- The profile writer keeps the original document handles, schedules dirty snapshots and joins pending writes at close. A failed profile open leaves the shell on the retry screen. An external modification causes a save refusal and a close confirmation. Explicit profiles keep their backups in `backup/`.
- `App::logic` drives scanning, accounting and saves, including while minimized. `App::ui` paints the shell. Scan status reports elapsed time and location count during a scan. The core returns file totals when the scan completes.

## Widget kit

| API | Usage |
| --- | --- |
| `theme::{font,hud,mono,Tint}` | Use the shared UI, Strong, HUD and Mono faces, surface colours and selected tint. Sizes are logical points. |
| `ui::{btn,seg,icon_btn,play_big}` | Buttons return an egui response. Icon buttons take a rectangle, glyph name, accessible title, selected state and tint. |
| `ui::glyph::draw` | Paint a named HUD outline into a rectangle. The glyph inventory is in `ui/glyphs.json`. |
| `ui::songs::show` | Supply `Table`, a persistent `Selection`, the cover cache and a right-click menu callback. Handle the returned `Action` for play, queue, like or sort. `screens::library_table` is the complete wiring example. |
| `Selection::ids` | Return the selected songs from the current bound list. Rebinding retains surviving song IDs and identifies playlist occurrences by their added time and order. Drag payloads use the same `model::Ids` type. A target can accept `response.dnd_release_payload::<Ids>()`. |
| `v.covers.get(&song.cover, size, tint)` | Request a 176, 420 or 960 pixel image. Pass `None` for colour or `Some(v.tint)` for tinted art. Paint the optional result with `ui::sleeve`. Workers decode on cache misses and wake the UI. Large art includes grooved record and blurred backdrop textures. |
| `ui::{rail,hit,child,label}` | Seek or volume input, accessible click target, clipped child UI and clipped single-line text for custom layouts. |
| `v.views.toast.set`, `ui::confirm`, `ui::context_menu` | Transient feedback, a modal returning the user’s decision and a standard context menu. |

## Smoke checks

Build offline from `native/`. Point smoke mode at an isolated profile and output directory. From the repository root, a release run looks like this.

```powershell
.\native\target\release\String.exe --smoke-test --data .\test-runs\shell\profile --out .\test-runs\shell\out
```

A test profile can start with a version-one state document containing `roots` or `files` with absolute local paths. For the full check, use two or more decodable songs longer than five seconds each, repeat off, and an empty loop setting. The scan creates the library cache. On a second run the smoke driver checks that the previously saved track is cued and paused at its position. An empty library exercises the shell and reports the playback check as skipped.

Add a `smoke::Case` to `smoke::CASES` between Songs and Now Playing. Implement `prepare(&mut View)` to navigate and set up the case, then `ready(&mut View) -> bool` to check model and texture readiness. The driver saves the painted frame as `<case name>.png`. Keep the record case last for the playback performance sample. The JSON includes checks, errors, skipped work, elapsed time, UI frames and process CPU as a percentage of one logical core. The process exits nonzero on failure. The window is positioned outside the virtual desktop before first paint, cannot activate, and uses its own instance ID. Smoke mode forces mute in the audio engine for the entire process.

## Native integration

`platform.rs` is the integration point for placement, single-instance activation, media keys and taskbar transport. `configure` receives the viewport builder, `attach` receives the HWND and `update` publishes current play state and dispatches received commands. A native callback should send a `platform::Command` and wake egui with the creation context’s repaint handle. Keep the smoke identity and off-screen placement when replacing the platform adapter.

## Menus, playlists and the queue

`ui::menus` owns the song menu, the list menu and their dialogs. `shell::show` calls `menus::overlay` once per frame, so a screen only opens them.

```rust
use crate::ui::menus::{self, Origin};
// A song row: the selected ids, where the click landed, and the list they came from.
if let Some(at) = menus::menu_pos(&response) {
    menus::track_menu(selection.ids(), at, Origin::list(ids.clone(), row, v.context()));
}
// An album, artist or playlist card, or a page's "more" button.
menus::ctx_menu(json!({"kind":"album","key":key}), at, None);
```

- `Origin::list(ids, row, ctx)` makes Play start that list at `row` (the first selected row for several songs). `Origin::none()` plays just the songs. Set `Origin::playlist` to the playlist id and a clone of its bound selection to add "Remove from this playlist".
- `ctx_menu`'s last argument is the filtered list on screen, when there is one.
- The same actions are free functions for buttons: `play_ctx`, `shuffle_ctx`, `play_next`, `queue_tracks`, `add_to_playlist`, `toggle_like`, `new_playlist`, `rename_playlist`, `delete_playlist`, `playlist_picker`. Each says what it did in the toast. `ctx_name`, `ctx_route`, `same_ctx`, `plural`, `long` and `when` give the words; `caps`, `empty_note` and `eq_bars` draw the shared bits.
- `menus::hooks().edit_info = Some(fn(&mut View, &str))` adds "Edit info". `hooks().reveal` replaces the built-in Show in Explorer.
- A drag carries `model::Ids`. Sidebar playlists and the queue button take it with `dnd_release_payload::<Ids>()`; `overlay` draws the label that follows the pointer.
- Session additions live in `ui/menus.rs`: `enqueue`, `new_playlist`, `playlist_add`, `playlist_rename`, `playlist_delete`, `playlist_remove`, `playlist_move`, `playlist_entries`, `swap_names`.
