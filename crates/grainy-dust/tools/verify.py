"""Run required Rust checks and record measured errors without shell encoding artifacts."""
from pathlib import Path
import json
import os
import re
import subprocess
import sys
import time

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[3]
CRATE = ROOT / 'crates' / 'grainy-dust'
os.environ['PATH'] = str(Path.home()/'.cargo'/'bin') + os.pathsep + os.environ['PATH']


def run(command,filename):
    start=time.monotonic()
    with (CRATE/filename).open('w',encoding='utf-8',newline='\n') as stream:
        result=subprocess.run(command,cwd=ROOT,stdout=stream,stderr=subprocess.STDOUT,check=False)
    # Child processes write directly to the descriptor; normalize their Windows newlines.
    output=(CRATE/filename).read_text(encoding='utf-8')
    (CRATE/filename).write_text(output.rstrip()+'\n' if output else '',encoding='utf-8',newline='\n')
    print(' '.join(command), 'exit',result.returncode, 'seconds',round(time.monotonic()-start,2),flush=True)
    if result.returncode:
        print((CRATE/filename).read_text(encoding='utf-8')[-12000:])
        raise SystemExit(result.returncode)


run(['cargo','fmt','-p','grainy-dust','--','--check'],'fmt.log')
run(['cargo','clippy','-p','grainy-dust','--all-targets','--','-D','warnings'],'clippy.log')
run(['cargo','test','-p','grainy-dust','--','--nocapture'],'validation.log')
run([str(Path(os.environ['GRAINY_ORACLE'])/'.venv'/'Scripts'/'python.exe'),'-B','tools/golden/gen_dust.py','--check-rust'],'roundtrip.log')
log=(CRATE/'validation.log').read_text(encoding='utf-8')
records=[]
for line in log.splitlines():
    m=re.fullmatch(r'(.+): max abs error ([\deE.+-]+) \(tolerance ([\deE.+-]+)\)',line)
    if m:records.append(dict(case=m[1],maximum_error=float(m[2]),tolerance=float(m[3])))
    m=re.fullmatch(r'(.+): equal \(tolerance 0\)',line)
    if m:records.append(dict(case=m[1],maximum_error=0,tolerance=0))
(CRATE/'golden_errors.json').write_text(json.dumps(records,indent=2)+'\n',encoding='utf-8',newline='\n')
print('Recorded',len(records),'comparisons. All checks passed.',flush=True)
