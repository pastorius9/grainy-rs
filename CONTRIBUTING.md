# Contributing

## Build and test

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Windows (MSVC toolchain) and macOS (Xcode command line tools) are built and tested by GitHub Actions on every
push (`.github/workflows/build.yml`: format, clippy, tests, release build, package, smoke run). A C/C++ compiler
builds the vendored LibRaw, Little CMS and libjpeg-turbo. Tests use generated data and temporary folders only and
never use the network.

The release packages are made by `tools/release/make_release.py` (Windows zip; checked with
`tools/release/check_imports.py` and `tools/release/smoke.py`) and `tools/release/make_macos.py` (macOS app bundle
and disk image, on a Mac; `--dry-run` shows what it makes on any system). `tools/release/scan_public.py` must find
nothing before anything is published.

The application's window tests compare rendered pictures with `crates/grainy-app/tests/snapshots/*.png`; to
replace them after an intended change, run the tests with `GRAINY_UPDATE_SNAPSHOTS=1` and look at every changed
picture (see `crates/grainy-app/ARCHITECTURE.md`). Pictures can differ between systems in font rendering and GPU
output: do not replace the checked-in snapshots with ones made on another system without looking at every
difference.

## Reference data (goldens)

Every ported function is tested against reference data computed by the Python Grainy 0.6.8 (the **oracle**).
The data lives in `golden/<crate>/<case>/` (`case.json` plus one `.npy` file per array) and is read by the tests
through `grainy-testkit`. The generators are `tools/golden/gen_<crate>.py`.

To regenerate goldens you need a checkout of the Python Grainy with its virtual environment. Name it with the
environment variable `GRAINY_ORACLE` and run a generator with the oracle's own interpreter, from this
repository's root:

```
# PowerShell
$env:GRAINY_ORACLE = "C:\path\to\grainy"
& "$env:GRAINY_ORACLE\.venv\Scripts\python.exe" tools/golden/gen_tone.py

# bash
export GRAINY_ORACLE=/path/to/grainy
"$GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_tone.py
```

A generator stops with a clear message when `GRAINY_ORACLE` is not set. `tools/golden/gen_app_lensdb.py` also
reads camera files from the folder named by `GRAINY_SAMPLES` (CC0 RAW samples). The oracle repository is only
read: generators never write into it.

Generators are deterministic. After a change, regenerate and check that `git diff -- golden/` shows only the
cases you meant to change. Machine-specific text (temporary folders, the checkout's folder) is replaced with
placeholders such as `<tmp>` and `<root>` before it is written.

## Changes

- Port the oracle's maths faithfully (same formulas, order of operations and float types) and prove it with
  goldens; state the tolerance of every comparison.
- No panics on bad input in library code; `unsafe` only in the FFI crates.
- Original photo files are never written, moved or deleted, by the application or by tests.
- `cargo fmt`, `clippy -D warnings` and the tests of the crates you changed must pass.
