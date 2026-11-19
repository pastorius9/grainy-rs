"""Make the clean public copy of the repository: one fresh commit, no private history, no internal files.

    python tools/release/make_public.py [--rev main] [--out FOLDER] [--names FILE]

1. `--out` (default `grainy-rs-public` beside the repository, tools/release/layout.py) must not exist, or must be a
   previous output of this script (its `.git` holds the marker `grainy-make-public`); only then is it deleted and
   made again. Anything else is refused.
2. `git archive <rev>` is unpacked there (bytes as committed: `core.autocrlf` off for the archive).
3. Internal files are removed (the list is REMOVE below: orchestration notes, briefs, review reports of the
   private work, machine-specific docs and scripts, logs).
4. tools/release/scan_public.py scans the folder. `docs/REVIEW-2026-11.md` is kept only when it has no real hit;
   any other real hit, or a file over 50 MB, stops here (exit 1, nothing committed).
5. `git init`, local identity `pastorius9 <177642243+pastorius9@users.noreply.github.com>`,
   `core.autocrlf false`, one commit `Grainy RS 0.1.0`.

Nothing is pushed: the folder is only prepared.
"""

from __future__ import annotations

import argparse
import fnmatch
import io
import os
import shutil
import stat
import subprocess
import sys
import tarfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import layout  # noqa: E402
import scan_public  # noqa: E402

REPO = layout.REPO
MARKER = 'grainy-make-public'
NAME = 'pastorius9'
EMAIL = '177642243+pastorius9@users.noreply.github.com'
REVIEW = 'docs/REVIEW-2026-11.md'

# Removed from the public copy (glob patterns on the path relative to the root, `/` separated):
REMOVE = [
    'AGENTS.md',                 # the orchestration rules of the private work (machine paths)
    'docs/HANDOFF*.md',          # handoff notes between sessions
    'docs/STATUS.md',            # the private work's progress log (worktrees, workers, this PC's folders)
    'docs/FIELD_TEST.md',        # names this PC's sample folders
    'docs/PLAN.md',              # the private work plan (names the oracle's folder on this PC)
    '**/BRIEF.md', 'BRIEF.md',   # worker briefs
    '**/REVIEW_REPORT.md', 'REVIEW_REPORT.md', '**/REVIEW.md', 'REVIEW.md',  # raw review reports
    '**/*.log', '*.log',         # build and test logs
    'tools/field',               # orchestration and field-test scripts with machine paths
    '.scan-names.txt', '**/.scan-names.txt',
]


def on_rm_error(func, path, _exc):
    os.chmod(path, stat.S_IWRITE)
    func(path)


def prepare_folder(out: Path):
    out = out.resolve()
    if out == REPO or REPO in out.parents or out in REPO.parents:
        raise SystemExit(f'refused: {out} overlaps the repository')
    if out.exists():
        if not (out / '.git' / MARKER).is_file():
            raise SystemExit(f'refused: {out} exists and is not an output of make_public.py (no .git/{MARKER})')
        print(f'removing the previous output {out}')
        shutil.rmtree(out, onerror=on_rm_error)
    out.mkdir(parents=True)
    (out / '.git').mkdir()
    (out / '.git' / MARKER).write_text('made by tools/release/make_public.py\n', encoding='ascii')


def git(*args, cwd=REPO, capture=True, data=False):
    result = subprocess.run(['git', *args], cwd=cwd, capture_output=capture, check=False)
    if result.returncode != 0:
        sys.stderr.write((result.stderr or b'').decode('utf-8', 'replace'))
        raise SystemExit(f'git {" ".join(args)} failed')
    return result.stdout if data else (result.stdout or b'').decode('utf-8', 'replace').strip()


def unpack(rev: str, out: Path):
    commit = git('rev-parse', '--verify', rev + '^{commit}')
    archive = git('-c', 'core.autocrlf=false', 'archive', '--format=tar', commit, data=True)
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(out, filter='data')
    return commit


def matches(rel: str, pattern: str) -> bool:
    if pattern.startswith('**/'):
        return fnmatch.fnmatch(rel, pattern) or fnmatch.fnmatch(rel.rsplit('/', 1)[-1], pattern[3:])
    return fnmatch.fnmatch(rel, pattern) or rel == pattern or rel.startswith(pattern.rstrip('/') + '/')


def remove_internal(out: Path) -> list[str]:
    removed = []
    for path in sorted(out.rglob('*'), key=lambda p: len(p.parts)):
        if not path.exists():
            continue
        rel = path.relative_to(out).as_posix()
        if rel == '.git' or rel.startswith('.git/'):
            continue
        if any(matches(rel, p) for p in REMOVE):
            if path.is_dir():
                count = sum(1 for f in path.rglob('*') if f.is_file())
                shutil.rmtree(path, onerror=on_rm_error)
                removed.append(f'{rel}/ ({count} files)')
            else:
                path.unlink()
                removed.append(rel)
    return removed


def scan(out: Path, names: str, label: str):
    print(f'\n--- scan ({label}) ---', flush=True)
    code = scan_public.main([str(out), '--names', names])
    return code


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--rev', default='main', help='the commit to publish (default main)')
    parser.add_argument('--out', default=str(layout.public_folder()))
    parser.add_argument('--names', default=str(scan_public.DEFAULT_NAMES))
    parser.add_argument('--message', default='Grainy RS ' + layout.version())
    args = parser.parse_args(argv)
    out = Path(args.out)

    prepare_folder(out)
    commit = unpack(args.rev, out)
    print(f'unpacked {args.rev} ({commit[:12]}) into {out}')
    removed = remove_internal(out)
    print(f'removed {len(removed)} internal paths:')
    for r in removed:
        print(f'  {r}')

    code = scan(out, args.names, 'with ' + REVIEW if (out / REVIEW).exists() else 'full')
    if code != 0 and (out / REVIEW).exists():
        print(f'\n{REVIEW} is removed (kept only when the scan passes) and the folder scanned again')
        (out / REVIEW).unlink()
        code = scan(out, args.names, 'without ' + REVIEW)
    if code != 0:
        print('\nthe scan did not pass: nothing committed (fix the hits, then run this script again)')
        return 1

    git('init', '-q', '-b', 'main', cwd=out)
    git('config', 'user.name', NAME, cwd=out)
    git('config', 'user.email', EMAIL, cwd=out)
    git('config', 'core.autocrlf', 'false', cwd=out)
    git('add', '-A', cwd=out)
    git('commit', '-q', '-m', args.message, cwd=out)
    status = git('status', '--porcelain', cwd=out)
    head = git('log', '-1', '--format=%H %an <%ae> %s', cwd=out)
    files = git('ls-files', cwd=out).count('\n') + 1
    print(f'\ncommitted {files} files: {head}')
    if status:
        print('uncommitted leftovers:\n' + status)
        return 1
    print(f'public folder ready: {out} (from {args.rev} {commit[:12]}); nothing was pushed')
    return 0


if __name__ == '__main__':
    sys.exit(main())
