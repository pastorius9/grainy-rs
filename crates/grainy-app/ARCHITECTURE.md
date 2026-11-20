# grainy-app architecture

The egui window of Grainy. Everything a feature needs is on one object, the **application context**
(`context::AppContext`); a feature lives in its own files and is registered with one line.

```
main.rs ──► app::GrainyApp ──owns──► AppContext (cx) ◄── every feature works on &mut AppContext
                 │                    ├─ doc: Box<dyn Document>     photos, selection, EditState, commits
                 │                    ├─ jobs: Jobs<AppContext>     background work, progress, cancel, status
                 │                    ├─ renderer: Box<dyn Renderer> preview + thumbnail lanes (worker.rs)
                 │                    ├─ dialogs: Vec<Box<dyn Dialog>>   the dialog stack
                 │                    └─ view state, Develop column state, status line, state::<T>()
                 ├─ panels: Vec<Box<dyn Panel>>       panels::registry()
                 └─ tools:  Vec<Box<dyn CanvasTool>>  tools::registry()
commands::REGISTRY ── static table of Command { id, label, enabled, run } (menus, keys, buttons call by id)
```

Every frame (`GrainyApp::show`): finished jobs run their `done` with the context → the document's listing job is
started when a refresh was asked → native file dialogs and dropped files → shortcut keys run commands → the
document stores the edit journal (`flush`) → renderer replies and new preview/thumbnail requests → menus, header,
library column, Develop column (panels), centre (photo view with the active tool, or the grid) → dialogs →
`flush` again.

## The ownership rule

**A feature owns its files plus one registration line.** Do not add fields to `AppContext`, `match` arms to shared
files or private methods on the window. What a feature keeps between frames goes in a type of its own file,
stored in the context with `cx.state::<MyState>()`. The registration points:

| Kind | Write | Register |
|---|---|---|
| command | `commands/<area>.rs`: `pub const COMMANDS: &[Command]` | one line in `commands::REGISTRY` (+ `pub mod`) |
| menu item | (a command) | flip its `""` to the command id in `menus.rs` (the item is already there, in the oracle's order) |
| panel body | `panels/<key>.rs`: a type implementing `Panel` | one line in `panels::registry()` (+ `pub mod`) |
| canvas tool | `tools/<name>.rs`: a type implementing `CanvasTool` | one line in `tools::registry()` (+ `pub mod`) |
| dialog / window | `dialogs/<name>.rs`: a type implementing `Dialog` | `pub mod` line; opened from a command with `cx.open_dialog(Box::new(..))` |
| background job | inside the feature: `cx.jobs.spawn(kind, label, work, done)` | nothing |
| new strings | Korean source text in the code; English: `text.rs::ENGLISH` when the oracle's `en.json` lacks it | one line each |

If a package truly needs a shared file changed (a renderer option, a new `Document` method), make the smallest
additive change, keep it in one commit and say so in the report.

## What the context offers

* `cx.doc` (`document::Document`): `photos()`, `current()`, `current_id()`, `select(i)`, `step(d)`,
  `toggle_selected(i)`, `selected_indices()`, `edit()` / `edit_mut()` (the open photo's `state::EditState`:
  `set(key, v)` live, `commit(label)` one undo step, `undo()`, `redo()`, `replace(settings, label)`,
  `switch_panel`, `reset`), `settings_of(i)`, `history()`, `restore_history(i)`, `copy()`, `paste(targets)`,
  `set_label_field(Rating|Flag|Label, v)`, `virtual_copies()`, `remove_selected()`, `query()` / `set_query(q)`
  (filters, search, folder, collection, sort, stacks), `folders()`, `collections()`, `library_folder()` (the
  writable folder, `None` for the session and read-only libraries), `read_only()`.
  Never write the catalogue yourself for an edit of the open photo: change `edit_mut()`; the context calls
  `doc.flush()` every frame, which stores the journal with the oracle's semantics (`commit` → `Catalog.edit` +
  `undo:{id}`; undo/redo → `Catalog.set_settings` + `undo:{id}`). For operations on other photos add a
  `Document` method (catalogue: `document/catalog.rs`, session: `session.rs`).
* `cx.settings()` – the open photo's settings; `cx.set_and_commit(key, value, label)`; `cx.apply_row(key, event)`;
  `cx.slider(ui, spec, enabled)`, `cx.sliders(ui, place)`, `cx.list_slider(..)`, `cx.setting_checkbox(..)`;
  `panels::controls::{choice, placeholder, hint, list_spec}`.
* `cx.run(id)` / `cx.run_with(id, &Invocation)` – run any command; `commands::enabled(cx, id)`.
* `cx.jobs` – `spawn`, `running(kind)`, `cancel(id)`, `cancel_kind(kind)`, `status()` (the status bar shows the
  oldest running job's progress); `cx.status` / `cx.set_status(text)` for messages.
* View and column state: `cx.mode()` / `set_mode`, `cx.tool()` / `open_tool`, `zoom` / `set_zoom`, `before`,
  `clip_*`, `expanded_panels`, `open_foldout` / `set_foldout`, `raw_view()`, `cx.egui` (the egui context),
  `cx.frame()` (the picture on screen with its histogram, clipping and scale), `cx.photo_rect()`.
* `cx.open_dialog`, `dialog_open(id)`, `close_dialog(id)`, `choose(Purpose)` (native file dialogs: answers reach
  dialogs through `Dialog::receive`).
* `cx.state::<T>()` – a feature's own state.
* `imagespace` – screen ↔ frame (`ViewTransform`) and frame ↔ source (`CoordinateMap`: the oracle's
  `source_point`, `source_points`, `frame_affine`, `gradient_point`, `screen_point`; tested against the oracle).

## How to add …

**A command.** In `commands/<area>.rs`:

```rust
pub const COMMANDS: &[Command] = &[
    Command { id: "cull.quick_collection", label: "빠른 컬렉션에 추가·제외", enabled: has_photos, run: quick },
];
fn quick(cx: &mut AppContext, invocation: &Invocation) { /* … */ }
```

Use the shortcut table's id when the oracle has a key for the action (`grainy_i18n::shortcuts::TABLE`): the key
then works without any other line. `invocation.key` tells which key ran it (`3` for three stars), `value` carries a
menu's value. Add the module to `commands/mod.rs` and its `COMMANDS` to `REGISTRY`.

**A panel.** `panels/<key>.rs` with `pub struct X; impl Panel for X { fn key(&self) -> &'static str { "<key>" }
fn ui(&mut self, ui, cx) { … } }`, then `Box::new(<key>::X)` in `panels::registry()` (replacing a `Placeholder`
when there is one). Panel order, titles, switches and edited dots come from `panels::table`; do not draw headers.

**A canvas tool.** `tools/<name>.rs` implementing `CanvasTool`: `pointer(&PointerEvent, cx)` gets `frame` (clamped)
and `free` coordinates of the displayed frame; convert to the source with `imagespace::CoordinateMap` (build it
once per settings change, keep it in your tool struct); `paint(painter, view, cx)` draws the overlay with
`view.to_screen(p)`; `key()` gets Enter, Esc, `[`, `]`… while the tool is active. Replace the `Pending` line of
its strip entry in `tools::registry()`, or add one with `active()` for a picker (a button of a panel).

**A dialog.** `dialogs/<name>.rs` implementing `Dialog` (`show` returns `false` when closed; `modal()`; `receive`
for native file dialog answers). Use `dialogs::frame()` and `egui::Modal` or `egui::Window` (non-modal windows:
return `false` from `modal()`).

**A job.** `cx.jobs.spawn("kind", tr("label"), move |handle| { handle.progress(i, n, text); … Ok(result) },
|cx: &mut AppContext, result| { … })`. The work runs on its own thread with only what it was given (clone the
settings, paths; open a second `Catalog` connection for catalogue work: never use `cx.doc` there). `done` runs on
the window's thread; check that the photo is still open (`cx.doc.current_id()` and `cx.doc.edit().version()`)
before applying a result (see `commands/auto.rs`). Use `handle.cancelled()` at safe points.

## Data safety

The app uses its own data folder (`paths::Locations::data_folder()`: `%LOCALAPPDATA%\GrainyRS`,
`GRAINY_RS_HOME`, `--library`). `CatalogDocument::open` refuses the Python Grainy's library and settings folders
(`paths::is_oracle_folder`); the Python Grainy's library opens only through `file.open_grainy_library`, read-only
(`Catalog::open_reader`; listings run on a snapshot copy in the data folder; edits, labels and pastes stay in
memory; thumbnails are rendered but never published). Original photo files are never written, moved or deleted
(imports add in place; removal from the library leaves files alone). Tests use temporary folders only.

## Network

Feedback (보내기 in 피드백 보내기) and updates use HTTP, each behind a trait: `dialogs::feedback::Transport`,
`dialogs::update::Backend` and `commands::settings::UpdateOpener`. Their defaults answer "not available in this
build" (`startup::unavailable`); the real ones (`network.rs`, on `grainy_platform::net`: `ureq` with the system's
TLS) are installed only by `GrainyApp::from_arguments`, at the program's start. Updates read only the Rust build's
own repository (`updater::RS_FEED`, `pastorius9/grainy-rs`), never the Python Grainy's (`ORACLE_*`, goldens only),
and compare with the Rust version (`grainy_platform::RS_VERSION`, the crate version; `APP_VERSION` 0.6.8 is kept
only for data shared with the Python Grainy). Windows swaps the release files of the program folder
(`updater::RS_ITEMS`); macOS swaps the whole `Grainy RS.app`, staged from the release's disk image
(`updater::stage_disk_image`: `hdiutil` and `ditto` behind `updater::DiskImageTools`, which tests fake). The whole
rule is in `docs/STATUS.md` ("Packaging: update rule").

**The switch: `--offline` or `GRAINY_OFFLINE=1`** (any value except empty and `0`) turns every network use off:
nothing is installed and `grainy_platform::net` refuses every request before opening a connection. The real AI
process launcher, sign-in browser opener and MCP listener also refuse to start while offline; fake systems and
binders remain usable by tests. HTTPS is required for both requests and redirects. Smoke runs
use it (`cargo run -p grainy-app -- --offline`). The tests' helpers (`common::language_lock`, `common::temp_dir`)
switch it on for the whole test process; tests use fake transports, fake address readers and fake launchers
(`tests/network.rs`) and never start an installer.

## Per system (Windows, macOS)

Code that differs per system keeps the system call small and behind `cfg`, and the logic in plain functions that
take the system as a parameter, so Windows tests cover the macOS branch too (`paths::Locations`, `system_open`,
`grainy_platform::monitor`, `ai::system::Os`).

* **Data**: `paths::Locations::data_folder()`: `%LOCALAPPDATA%\GrainyRS`, macOS
  `~/Library/Application Support/GrainyRS` (never the Python Grainy's `.../Grainy`, which `is_oracle_folder`
  compares without case and Unicode composition there). Library, settings, crash marker, MCP bridges (`mcp/`),
  sign-in and install scripts (`ai-login/`, `ai-install/`) all live in it; the log goes to standard error.
* **Trash**: `grainy_platform::recycle` (`updater::recycle`): the recycle bin, on macOS the Trash through
  `NSFileManager` (no Finder automation, no permission prompt). Nothing is deleted for good.
* **Opening**: `system_open`: folders in Explorer / Finder, files and addresses with their default program
  (`rundll32` / `/usr/bin/open`); the menu words follow the system (탐색기에서 열기 / Finder에서 열기).
* **File dialogs**: `dialogs::native`: one pattern for every `rfd` dialog: start it on the window's thread, wait for
  its answer on a helper thread (`await_answer`). macOS shows panels only from the main thread, as a sheet answered
  through the main run loop, so the window thread never blocks on one.
* **Monitor profile**: `display::watch_window`: Windows asks for the device name of the window's monitor, macOS for
  the CoreGraphics display under the window (egui's place × zoom = the system's points), named `'Name (id)'`;
  the profile itself is the system's file (Windows) or ICC data (macOS) through `grainy_platform::monitor`.
* **Menus, keys**: eframe/winit give macOS its application menu (About, Services, Hide, Hide Others, Show All,
  Quit ⌘Q, named after the bundle). Quit ends the event loop, which runs `on_exit` exactly as closing the window
  does. Shortcuts match `Modifiers::command` (⌘ on macOS) and their texts are written with ⌘ there
  (`grainy_i18n::keys`). A trackpad scroll pans the photo on macOS (`view::trackpad_pan`), pinching zooms.
* **Fonts**: Malgun Gothic (Windows) or Apple SD Gothic Neo by weight (macOS, `fonts::face_weights`); the AI
  contact sheets' captions use Pillow's font search and, where nothing is found (macOS), Pillow's built-in font.

## Tests

* Model tests (`tests/model.rs`, `tests/worker.rs`, `tests/library.rs`, `tests/imagespace.rs`) need no window.
  Catalogue behaviour is proven against the oracle: `tools/golden/gen_app.py` builds a library with the Python
  Grainy's own code and runs scripted sessions through its `MainWindow` methods; the Rust test runs the same
  steps through the `Document` and compares every table (`tests/library.rs`). Extend the script for a new
  catalogue operation.
* Snapshot tests (`tests/snapshots.rs` and the window tests of the other files) render the real window headless
  with `egui_kittest` on wgpu and **compare** the picture with the committed `tests/snapshots/<name>.png`
  (`tests/common/snapshot.rs`, one function for all files; each file's `save` forwards to it). Pattern:

```rust
#[test]
fn my_feature() {
    render(Language::Korean, "my-feature-ko", |app| {   // test card, no engine
        app.open_panel("curve");
        app.edit_state_mut().set("curve", json!([[0, 0], [0.5, 0.6], [1, 1]]));
    });
}
```

  For real photos use `photo_harness(paths)` + `settle(&mut harness)` (waits until `idle()`: renderer, jobs,
  listings, thumbnails; use `common::snapshot::settle`, or `settle_frame` when the photo's first frame must have
  arrived), then `save(&mut harness, name)`; drive it with `harness.key_press`, `run(id)` or `state_mut()`. Take the
  `LANGUAGE` lock in every test that renders or compares translated text. The harness fails on a missing glyph, so
  every string you show is checked against the loaded fonts. Look at the PNG you wrote.

  **A snapshot must be reproducible**: the same picture in every run, on every machine.
  * Set `grainy_app::context::FROZEN_TIMING` in the builder (the status bar's render time reads `0 ms`). While it is
    set the app also previews on the CPU renderer only (no GPU adapter is used), because GPU frames may differ from
    CPU frames by a rounding step between machines.
  * Make every folder and file name that can be on screen fixed. `common::temp_dir(name)` returns `<temp>/grainy-app-tests/<name>-<pid>/<name>`
    (`<temp>` resolved to its real long form, `common::temp_root`: no 8.3 short names, macOS's `/private/var`):
    the unique part is the folder above, the folder you get is called `name`, and the folder tree starts at the
    first folder that holds photos. Never put the process id, a random part or a date into a name that is drawn.
  * Wait until the app is idle with `common::snapshot::settle`, not with a fixed number of frames.
  * The folder tree opens new folders by itself, once, as an import's listing grows, so which folders are open
    depends on timing: call `common::snapshot::open_folders(app, &["Trip"])` (drives are always opened), then settle
    again, before a picture that shows the tree.
  * The status line shows the message that arrived last, and an import's "complete" and the preview's size line
    come from independent jobs: pin it with `common::snapshot::pin_status(app, "")` when the status line is not the
    subject of the picture. Other clocks (dates, times) must be set to fixed values by the test.
  * Do not hide regions of the picture; fix the cause.

  **Comparing and updating.** A picture passes when it has the committed size and at most 32 pixels differ by more
  than 4 levels in any channel (anti-aliasing and rounding stay below that; a changed word, digit or icon does not).
  On a mismatch the test fails and writes the new picture next to the old one as `<name>.new.png` (ignored by git:
  `crates/grainy-app/.gitignore`) for inspection. The committed files are replaced only when the environment variable
  `GRAINY_UPDATE_SNAPSHOTS=1` is set (`GRAINY_UPDATE_SNAPSHOTS=1 cargo test -p grainy-app`); look at every changed
  picture before committing it. A snapshot that is not committed yet fails the same way: create it with the variable.
  **Pictures are compared on Windows only** (`common::snapshot::COMPARED`): the committed PNGs are Windows renders;
  on macOS the tests still build, drive and render the window, but its fonts and GPU draw differently, so the picture
  is neither compared nor written there.
