"""Where the release scripts read and write: the repository, its version, and the sibling output folders.

The output folders sit beside the repository (`<projects>/grainy-rs-dist`, `<projects>/grainy-rs-public`), where
`<projects>` is the folder that holds the repository, or, for a worktree in `<projects>/grainy-rs-wt/<name>`, the
folder that holds `grainy-rs-wt`. `GRAINY_RELEASE_DIST` and `GRAINY_RELEASE_PUBLIC` replace them.
"""

from __future__ import annotations

import os
import re
import shlex
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]


def projects() -> Path:
    parent = REPO.parent
    return parent.parent if parent.name == 'grainy-rs-wt' else parent


def dist_folder() -> Path:
    return Path(os.environ.get('GRAINY_RELEASE_DIST') or projects() / 'grainy-rs-dist')


def public_folder() -> Path:
    return Path(os.environ.get('GRAINY_RELEASE_PUBLIC') or projects() / 'grainy-rs-public')


def version() -> str:
    """The workspace version (`[workspace.package] version`), which is `grainy_platform::RS_VERSION`."""
    text = (REPO / 'Cargo.toml').read_text(encoding='utf-8')
    section = re.search(r'^\[workspace\.package\](.*?)(?=^\[|\Z)', text, re.MULTILINE | re.DOTALL)
    m = re.search(r'^version\s*=\s*"([^"]+)"', section.group(1) if section else '', re.MULTILINE)
    if not m:
        raise SystemExit('no [workspace.package] version in Cargo.toml')
    return m.group(1)


def rs_items() -> list[str]:
    """`updater::RS_ITEMS` as the code names it (the zip must hold exactly these)."""
    text = (REPO / 'crates/grainy-platform/src/updater.rs').read_text(encoding='utf-8')
    m = re.search(r'pub const RS_ITEMS: \[&str; \d+\] = \[([^\]]*)\]', text)
    if not m:
        raise SystemExit('updater::RS_ITEMS not found in crates/grainy-platform/src/updater.rs')
    return re.findall(r'"([^"]+)"', m.group(1))


def zip_name(v: str | None = None) -> str:
    return f'GrainyRS-{v or version()}-windows-x64.zip'


def dmg_name(v: str | None = None) -> str:
    return f'GrainyRS-{v or version()}-macos-universal.dmg'

def release_env(windows: bool) -> dict[str, str]:
    """Remap compiler paths even when a checkout or a user directory contains spaces."""
    env = dict(os.environ)
    flags = (env['CARGO_ENCODED_RUSTFLAGS'].split('\x1f') if env.get('CARGO_ENCODED_RUSTFLAGS')
             else shlex.split(env.get('RUSTFLAGS', '')))
    if windows:
        # Environment rustflags take precedence over the target setting in .cargo/config.toml.
        flags += ['-C', 'target-feature=+crt-static', '-C', 'link-arg=/PDBALTPATH:%_PDB%']
    home = Path.home()
    for path, name in ((home, '/build/home'), (Path(env.get('CARGO_HOME', home / '.cargo')), '/build/cargo'),
                       (REPO, '/build/grainy')):
        flags.append(f'--remap-path-prefix={path}={name}')
    env['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join(flags)
    return env


def check_binary_paths(path: Path, private: list[Path]):
    """Detect native, slash-normalized and UTF-16 paths embedded in a release file."""
    data = path.read_bytes().lower()
    for root in private:
        source = str(root)
        for value in {source, source.replace('\\', '/'), source.replace('/', '\\')}:
            for encoding in ('utf-8', 'utf-16le'):
                if value.encode(encoding).lower() in data:
                    raise SystemExit(f'{path.name} contains a local build path')
