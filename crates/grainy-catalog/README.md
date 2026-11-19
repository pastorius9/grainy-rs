# grainy-catalog

Storage port of the Python Grainy oracle, tested on generated catalogs only. Production code has no
Python, Qt, OpenCV, image import, or application-global state. `Catalog` is Send; a worker opens its own
read-only connection. Callers supply the catalog directory explicitly.

## Oracle coverage

| Oracle method(s) | Rust item(s) | Evidence |
|---|---|---|
| `Catalog.__init__`, `_migrate_photos`, `_migrate_browser`, `_migrate_profiles` | `Catalog::new`, private `schema::migrate` | Six historical starting DBs; tables, columns, indexes, triggers, backups, and second-open idempotence |
| `add`, `photo`, `photos`, `paths` | Same `Catalog` method names | Python-written SQLite reader fixture; generated Unicode, long, case-variant and virtual paths |
| `update`, `update_many`, `set_user_metadata` | Same names | Exact raw table deltas; unsupported fields, bulk rollback, metadata merge |
| `edit`, `set_settings`, `histories` | Same names, `&Settings` input | No-ops including Python numeric equality, 83 edits/80-history cap, timestamps, invalid legacy-profile repair |
| `save_preset`, `save_adobe_preset`, `presets`, `delete_preset` | Same names | Geometry reset, replacement/deletion, DCP assets, oracle Adobe translation supplied through adapter |
| `snapshot`, `snapshots`, `virtual_copy`, `stack` | Same names | Exact JSON, copy-of-copy source, absent stack inheritance, empty stack |
| `add_collection`, `collections`, `collection_add`, `collection_ids`, `remove_collection` | Same names | Regular/smart/nested collections, duplicate members, deleted-parent behavior |
| `folder_roots`, `register_folder`, `removed_folders`, `folder_photo_ids`, `remove_folder`, `restore_folder` | Same names | Roots/exclusions/watch preferences, recursive removal, re-registration |
| `preference`, `save_preference`, `remove_photos`, `_delete_photos` | Same names; private deletion helper | Exact undo/RAW/default/custom JSON, deletion dependencies, 1,005-row batching |
| `open_reader`, `backup`, `close` | Same names | Snapshot isolation, no mutation or sidecars left for a closed DB, readable SQLite backups |
| `folders.path_key`, `stored_path`, `in_folder`, `contains_folder`, `left_out`, `below`, `folder_nodes` | `folders::Platform` methods with the same names | Windows/macOS/POSIX goldens, NFC, UNC/drives, relative dots, POSIX double slash, direct/recursive counts |
| `library_query.sync_index`, `query`, `read_page`; `library.matches` | Same names in `query` | Dirty triggers and index refresh, 18 query configurations, 319 predicate cases, cancellation |
| `preview_store.signature`, `snapshot`, `files`, `inspect`, `publish`, `replace_file`, `store_render` | Same names in `preview`; `inspect_snapshot` accepts a captured state | 23 oracle validity scenarios; actual generated source changes, offline changes, stale worker rejection, atomic publication |
| `preview_store.load_offline` | `read_offline`, `load_offline`, `write_offline` | float16/float32 NPZ packets, legacy/native selection, exact metadata; Python opens Rust-written packets |
| Catalog-relevant `workflow_data` functions | Same names in `workflow` | Stack ordering, quick collection, metadata presets/keywords, export history/signatures |
| `command_batch.text_hash`, persisted v1/v2 records | `workflow::text_hash`, catalog preferences | Actual oracle-created batch records with 81 undo entries retain byte-identical JSON and hashes |
| `profile_store` schema and codec | Private `profiles` module and profile-assets migration | Inline/packed/nested DCP round trips, SHA-256/base64 validation, missing/corrupt/oversize/wrong-type assets |
| `dust_store.migrate` and stored records | Dust table migration/preservation | Existing session and compressed entry survive opening, migration, and backup |

The main scripted case runs **91 operations**, comparing return values and changed tables after every
step. The committed reference data is about 4 MB, including small real SQLite files. JSON strings in
columns are compared **as strings**, including order, spaces, escaping and float formatting. No JSON
canonicalization is used to hide differences. Random preview tokens alone are replaced with the agreed
marker in table comparisons; token changes/no-ops are separately tested with production randomness.
The clock and token source can be injected through `Sources`.

All golden comparisons have tolerance **0**, measured maximum error **0**. This includes the decoded
float16/float32 packet arrays. Color conversion and JPEG rendering are not claimed as pixel-tested here.

## Explicit integration boundaries

- `save_adobe_preset` invokes a caller-supplied translator with the original `_adobe_preset` record. Its
  result must contain `settings` and nonempty `mapped`, like `adobe_preset.translate`. The catalog stores
  the original record and translated settings exactly. Adobe XML/Lua parsing and Lightroom importing
  are outside this crate; there is no fallback that silently accepts an unvalidated source.
- `publish` / `store_render` accept an encoded JPEG packet. The codecs/rendering layer supplies the
  oracle's RGB thumbnail (320-pixel limit, JPEG quality 88 and ICC profile). Inspection returns the
  original JPEG bytes instead of a PIL/QImage; display resizing is a UI/codec concern.
- `load_offline` selects native versus legacy pixels, validates the source mode and adds provenance.
  Working-space conversion is an explicit callback; native sensor metadata is retained in `raw_info`.
  `rebuild` is not included because it calls image loading/developing/output conversion, all excluded
  from this storage brief. The NPZ reader accepts the C-order little-endian f16/f32 and Unicode-scalar
  formats the oracle's preview writer produces, with a 256 MiB per-array allocation limit.
- `workflow_data` numerical exposure/white-balance algorithms, command capture/apply/restore execution,
  dust detection/review UI and profile-cache/garbage-collection maintenance are not part of this port.
  Their stored preference/table formats are preserved. Quick-collection localization is supplied by
  the caller (`"빠른 컬렉션"` in the Korean oracle fixtures).

## Decisions and preserved quirks

- SQL schemas and trigger bodies are extracted from the oracle by the generator and committed as assets.
  There is no invented version number. Migrations retain the original backup filenames.
- Python `json.dumps` spacing, Unicode surrogate escapes, float exponent formatting, settings key order,
  and numeric/bool equality are private helpers. This is essential for compact batch history hashes and
  exact no-op edits. Ordinary JSON objects retain insertion order via serde_json `preserve_order`.
- Invalid *inline legacy* profile data can be replaced while its original history text is retained;
  broken internal references are rejected. Storage reads do not normalize history/snapshot/preset data.
- SQLite NOCASE remains ASCII-only; Windows path keys and search casefolding have their separate rules.
  macOS storage uses NFC while comparisons additionally lowercase. The odd `commonpath` behavior for
  relative `.` and POSIX `//` is retained. Duplicate saved stack-order ids are retained as the oracle does.
- Removing photos does not delete source files or old preview files. Saved batch records are not silently
  rewritten. Virtual copies do not inherit a stack id. History is capped only when read, not deleted.
- A valid cached online render remains useful after its source disappears, even if an offline packet is
  present; source/offline signature checks follow the oracle's branches exactly.
- Write methods use deferred transactions; `edit`, browser-index refresh and final preview publication
  use IMMEDIATE transactions. A closed WAL database is opened with SQLite `immutable=1` for read-only
  access without creating sidecars; an existing live WAL uses ordinary SQLite read-only snapshots.
- Query filtering is evaluated in Rust over lightweight metadata rows, preserving SQLite sort order and
  dirty-index semantics. It does not load edits. The oracle's optional SQL smart-rule optimization is
  not reproduced; cancellation also interrupts SQLite's sorter through a progress handler.

## Verification

Run from this worktree (PowerShell):

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$env:PYTHONDONTWRITEBYTECODE = '1'
$env:PYTHONIOENCODING = 'utf-8'
& "$env:GRAINY_ORACLE/.venv/Scripts/python.exe" -B tools/golden/gen_catalog.py
cargo fmt --all --check
cargo clippy -p grainy-catalog --all-targets -- -D warnings
cargo test -p grainy-catalog
& "$env:GRAINY_ORACLE/.venv/Scripts/python.exe" -B tools/golden/gen_catalog.py --check-rust target/catalog-tests/roundtrip
```

The last command uses the oracle's `Catalog.open_reader` to compare every public reader and all tables
of the Rust-produced database, then checks both Rust-written NPZ files with NumPy. It also checks that
the database bytes did not change. It refuses round-trip directories outside this worktree's `target/`.
The generator disables Python bytecode writes before importing the oracle and only creates generated
test data. Tests never inspect the owner's library, settings, or the oracle's `data/` directory.

There are 16 Rust integration tests. Additional failure tests check write atomicity, damaged references,
cancelled queries, no-op preview tokens and batch boundaries. `grainy-core`'s three existing golden tests
also pass. Cargo prints pre-existing manifest warnings about unused workspace `serde`/`repository`;
Clippy with `-D warnings` passes without Rust-code warnings.

## Dependencies

Existing shared dependencies: `grainy-core`, `grainy-testkit` (tests), `serde_json` with `preserve_order`,
and `thiserror`. Added dependencies and verified licenses:

| Dependency | Version tested | License |
|---|---|---|
| rusqlite (`bundled`, `backup`, `hooks`) | 0.38.0 | MIT |
| sha2 | 0.10.9 | MIT / Apache-2.0 |
| base64 | 0.22.1 | MIT / Apache-2.0 |
| unicode-normalization | 0.1.26 | MIT / Apache-2.0 and Unicode-3.0 |
| unicode-casefold | 0.2.0 | MIT / Apache-2.0 |
| chrono | 0.4.45 | MIT / Apache-2.0 |
| image (JPEG only) | 0.25.10 | MIT / Apache-2.0 |
| zip (deflate only) | 2.6.1 | MIT |
| half | 2.7.1 | MIT / Apache-2.0 |

Bundled SQLite is the native dependency explicitly authorized by the brief. The other additions are
Rust libraries. Cargo.lock is intentionally not committed.

## Requests for shared crates

1. Consider centralizing Python-compatible JSON serialization/equality and the oracle's default key
   order in `grainy-core` before implementing command-batch compact undo elsewhere. Private exact
   helpers are included here; no shared crate was modified.
2. Connect the Adobe translator, JPEG/ICC encoder and offline color-conversion callback when the
   corresponding import/codecs/color crates are available. Storage tests already exercise these boundaries
   with oracle-generated results. No other crate is needed to build or test this one.
