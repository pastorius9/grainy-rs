"""Record/audit the scalar JPEG/zlib namespace using MSVC's archive symbol table.

Run --generate against the original build once; ordinary builds use the checked-in
header and need no Python. Run --audit against the final build to reject leaks.
"""
from pathlib import Path
import argparse
import re
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('dumpbin', type=Path)
parser.add_argument('out', type=Path)
parser.add_argument('--generate', action='store_true')
args = parser.parse_args()
crate = Path(__file__).resolve().parents[1]
symbols = set()
# Inline UCRT wrappers are COMDATs owned by the compiler's standard headers.
crt = {'_snprintf_s', '_vfprintf_l', '_vsnprintf_s_l', '_vsscanf_s_l',
       'fprintf', 'sscanf_s', 'vsscanf_s'}
for library in (['grainy_jpeg', 'grainy_zlib'] if args.generate else ['grainy_raw_jpeg', 'grainy_raw_zlib']):
    text = subprocess.check_output([str(args.dumpbin), '/nologo', '/linkermember:1', str(args.out / (library + '.lib'))], text=True)
    text = text.split('public symbols', 1)[1].split('Summary', 1)[0]
    found = set(re.findall(r'^\s+[0-9A-F]+ ([A-Za-z_][A-Za-z_0-9]*)\s*$', text, re.M))
    # MSVC owns __isa_available_default, vector constants, and other CRT/COMDATs.
    found = {name for name in found if not name.startswith('__') and name not in crt}
    assert found, library
    symbols.update(found)
    if not args.generate:
        assert all(name.startswith('grainy_raw_') for name in found), sorted(found)
        print(f'{library}: {len(found)} C symbols, all prefixed grainy_raw_')
if args.generate:
    header = '// Scalar libjpeg-turbo 2.1.5.1 and zlib 1.3.1 external symbols.\n'
    header += '// Compiler-forced include; retained upstream sources are unmodified.\n'
    header += '#ifndef GRAINY_RAW_NATIVE_SYMBOLS_H\n#define GRAINY_RAW_NATIVE_SYMBOLS_H\n'
    header += ''.join(f'#define {name} grainy_raw_{name}\n' for name in sorted(symbols))
    header += '#endif\n'
    (crate / 'src/native_symbols.h').write_text(header, encoding='utf8', newline='\n')
    print(f'Wrote {len(symbols)} namespace mappings')
