"""Where the release scripts read and write: the repository, its version, and the sibling output folders.

The output folders sit beside the repository (`<projects>/grainy-rs-dist`, `<projects>/grainy-rs-public`), where
`<projects>` is the folder that holds the repository, or, for a worktree in `<projects>/grainy-rs-wt/<name>`, the
folder that holds `grainy-rs-wt`. `GRAINY_RELEASE_DIST` and `GRAINY_RELEASE_PUBLIC` replace them.
"""

from __future__ import annotations

import os
import re
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
