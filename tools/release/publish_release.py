"""Publish the Windows and macOS artifacts from the same successful Actions run."""

import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import layout


def gh(*args):
    return subprocess.check_output(['gh', *args], text=True, encoding='utf-8')


def checked_files(folder, version):
    files = {}
    for name in (layout.zip_name(version), layout.dmg_name(version)):
        package = folder / name
        checksum = folder / (name + '.sha256')
        if package.stat().st_size == 0:
            raise ValueError(f'empty package: {package}')
        with package.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        if checksum.read_text(encoding='ascii').splitlines() != [f'{digest} *{name}']:
            raise ValueError(f'checksum mismatch: {checksum}')
        files[package] = 'sha256:' + digest
        files[checksum] = 'sha256:' + hashlib.sha256(checksum.read_bytes()).hexdigest()
    if set(folder.iterdir()) != set(files):
        raise ValueError(f'unexpected files in {folder}')
    return files


def publish(folder):
    repo = os.environ['GITHUB_REPOSITORY']
    sha = os.environ['GITHUB_SHA']
    ref = os.environ['GITHUB_REF']
    version = layout.version()
    tag = 'v' + version
    if not re.fullmatch(r'(0|[1-9]\d*)(\.(0|[1-9]\d*)){2}', version):
        raise ValueError('expected a stable semantic version')
    if (repo != 'pastorius9/grainy-rs' or os.environ['GITHUB_EVENT_NAME'] != 'push'
            or ref not in {f'refs/tags/{tag}', f'refs/heads/release/{tag}'}):
        raise ValueError('release ref does not match this build')
    files = checked_files(folder, version)
    api = 'repos/' + repo
    releases = [item for page in json.loads(gh('api', '--paginate', '--slurp', api + '/releases?per_page=100'))
                for item in page]
    release = next((item for item in releases if item['tag_name'] == tag), None)
    if not release or release['draft']:
        for item in releases:
            old = re.fullmatch(r'v(\d+)\.(\d+)\.(\d+)', item['tag_name'])
            if (not item['draft'] and not item['prerelease'] and old
                    and tuple(map(int, old.groups())) > tuple(map(int, version.split('.')))):
                raise ValueError(f'a newer stable release already exists: {item["tag_name"]}')

    if ref.startswith('refs/heads/'):
        branch = ref.removeprefix('refs/')
        if json.loads(gh('api', f'{api}/git/ref/{branch}'))['object']['sha'] != sha:
            raise ValueError('the release branch has moved since this build')
    tags = json.loads(gh('api', f'{api}/git/matching-refs/tags/{tag}'))
    if not any(item['ref'] == f'refs/tags/{tag}' for item in tags):
        gh('api', '--method', 'POST', f'{api}/git/refs', '-f', f'ref=refs/tags/{tag}', '-f', f'sha={sha}')
    if json.loads(gh('api', f'{api}/commits/{tag}'))['sha'] != sha:
        raise ValueError(f'{tag} belongs to another commit')

    if not release:
        note_file = Path(__file__).resolve().parents[2] / 'docs' / 'releases' / f'{version}.md'
        notes = ['--notes-file', str(note_file)] if note_file.is_file() else ['--generate-notes']
        gh('release', 'create', tag, '--repo', repo, '--draft', '--verify-tag',
           '--title', f'Grainy RS {version}', *notes)
        release_id = gh('release', 'view', tag, '--repo', repo, '--json', 'databaseId',
                        '--jq', '.databaseId').strip()
        release = json.loads(gh('api', f'{api}/releases/{release_id}'))
    if release['prerelease']:
        raise ValueError('a prerelease with this tag already exists')
    release_url = f'{api}/releases/{release["id"]}'
    expected = {path.name: digest for path, digest in files.items()}
    remote = {asset['name']: asset['digest'] for asset in release['assets']}
    if any(expected.get(name) != digest for name, digest in remote.items()):
        raise ValueError('existing release assets differ from this build')
    missing = [str(path) for path in files if path.name not in remote]
    if missing:
        if not release['draft']:
            raise ValueError('published release has different assets')
        gh('release', 'upload', tag, '--repo', repo, *missing)
    uploaded = json.loads(gh('api', release_url))['assets']
    if {asset['name']: asset['digest'] for asset in uploaded if asset['state'] == 'uploaded'} != expected:
        raise ValueError('uploaded checksums differ from local artifacts')
    if release['draft']:
        gh('release', 'edit', tag, '--repo', repo, '--draft=false', '--latest')
    published = json.loads(gh('api', release_url))
    if published['draft'] or published['prerelease']:
        raise ValueError('publication did not complete')
    print(published['html_url'])


if __name__ == '__main__':
    publish(Path(sys.argv[1]))
