"""Verify the original five download URLs/hashes in memory; retain no archives."""
import hashlib
import urllib.request

archives = [
    ('libraw.tar.gz', 'https://github.com/LibRaw/LibRaw/archive/refs/tags/0.22.1.tar.gz',
     'e676248284075605aa2697a66eeed7dc258820bd1d4988c724d29edffd726726'),
    ('jpeg.tar.gz', 'https://github.com/libjpeg-turbo/libjpeg-turbo/archive/refs/tags/2.1.5.1.tar.gz',
     '61846251941e5791005fb7face196eec24541fce04f12570c308557529e92c75'),
    ('zlib.tar.gz', 'https://github.com/madler/zlib/archive/refs/tags/v1.3.1.tar.gz',
     '17e88863f3600672ab49182f217281b6fc4d3c762bde361935e436a95214d05c'),
    ('lcms.tar.gz', 'https://github.com/mm2/Little-CMS/archive/refs/tags/lcms2.19.tar.gz',
     '6acc3eb5fac194acc85331194133ad1d4a60419dad16b3bc225a7d27380a5b3a'),
    ('openjpeg.tar.gz', 'https://github.com/uclouvain/openjpeg/archive/refs/tags/v2.5.4.tar.gz',
     'a695fbe19c0165f295a8531b1e4e855cd94d0875d2f88ec4b61080677e27188a'),
]
for name, url, expected in archives:
    with urllib.request.urlopen(url, timeout=45) as response:
        actual = hashlib.sha256(response.read()).hexdigest()
    assert actual == expected, (name, actual, expected)
    print(f'{name}: SHA256 verified from {url}', flush=True)
