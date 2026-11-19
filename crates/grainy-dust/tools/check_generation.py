"""Check all fixture bytes are reproducible, showing table differences when SQLite changes."""
import hashlib
from pathlib import Path
import sqlite3
import subprocess
import sys
from contextlib import closing

ROOT=Path(__file__).resolve().parents[3]
GOLDEN=ROOT/'golden'/'dust'

def tables():
    with closing(sqlite3.connect(GOLDEN/'batch'/'catalog.sqlite')) as db:
        names=[r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")]
        return {name:list(db.execute('SELECT * FROM "'+name+'"')) for name in names}

def hashes():
    return {str(p.relative_to(GOLDEN)):hashlib.sha256(p.read_bytes()).hexdigest() for p in GOLDEN.rglob('*') if p.is_file()}

before=tables();old=hashes()
subprocess.run([sys.executable,'-B','tools/golden/gen_dust.py'],cwd=ROOT,check=True,stdout=subprocess.DEVNULL)
after=tables();new=hashes()
changed=[name for name in new if old.get(name)!=new[name]]
print('Changed fixture files:',changed)
for name in before:
    if before[name]!=after[name]:
        print('Changed table:',name)
        for a,b in zip(before[name],after[name]):
            if a!=b:print('Changed columns:',[(i,x,y) for i,(x,y) in enumerate(zip(a,b)) if x!=y])
if changed:raise SystemExit(1)
print('All fixture files are byte-identical on repeat generation.')
