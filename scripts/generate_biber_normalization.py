#!/usr/bin/env python3
"""Extract Unicode 15 normalization mappings from Biber 2.22's bundled Perl."""
import argparse
import pathlib
import re
p = argparse.ArgumentParser()
p.add_argument('unicore', type=pathlib.Path)
p.add_argument('output', type=pathlib.Path)
a = p.parse_args()
def body(path):
    return path.read_text().split("return <<'END';\n", 1)[1].split('\nEND', 1)[0]
classes = []
for line in body(a.unicore/'CombiningClass.pl').splitlines():
    lo, hi, cls = line.split('\t')
    classes.append((int(lo, 16), int(hi or lo, 16), int(cls)))
bounds = [int(x) for x in body(a.unicore/'lib/CompEx/Y.pl').splitlines()[1:]]
def excluded(cp):
    return any(lo <= cp < hi for lo, hi in zip(bounds[::2], bounds[1::2]))
data = []
values = []
compositions = []
for line in body(a.unicore/'Decomposition.pl').splitlines():
    lo, hi, text = line.split('\t')
    compat = text.startswith('<')
    seq = [int(x, 16) for x in re.sub(r'<[^>]+>\s*', '', text).split()]
    off = len(values)
    values.extend(seq)
    for cp in range(int(lo, 16), int(hi or lo, 16) + 1):
        data.append((cp, compat, off, len(seq)))
        if not compat and len(seq) == 2 and not excluded(cp):
            compositions.append((seq[0], seq[1], cp))
out = '// Generated from Biber 2.22 bundled Perl Unicode 15.0.0.\n// Unicode data copyright Unicode, Inc.; https://www.unicode.org/license.txt\n'
out += 'static CLASSES: &[(u32,u32,u8)] = &[\n' + ',\n'.join(f'({lo},{hi},{cls})' for lo,hi,cls in classes) + '\n];\n'
out += 'static DECOMPOSITIONS: &[(u32,bool,usize,usize)] = &[\n' + ',\n'.join(f'({cp},{str(compat).lower()},{off},{n})' for cp,compat,off,n in data) + '\n];\n'
out += 'static DECOMPOSED: &[u32] = &[' + ','.join(map(str,values)) + '];\n'
out += 'static COMPOSITIONS: &[(u32,u32,u32)] = &[\n' + ',\n'.join(f'({x},{y},{cp})' for x,y,cp in sorted(compositions)) + '\n];\n'
a.output.write_text(out)
print(f'{len(data)} decompositions; {len(compositions)} compositions; {len(classes)} combining-class ranges')
numeric = []
for line in body(a.unicore/'To/Nv.pl').splitlines():
    lo, hi, text = line.split('\t')
    if '/' in text:
        numerator, denominator = text.split('/')
        value = f'({float(numerator)!r}/{float(denominator)!r})'
        step = 'false'
    else:
        value, step = repr(float(text)), 'true'
    numeric.append(f'({int(lo,16)},{int(hi or lo,16)},{value},{step})')
decimals = []
for line in body(a.unicore/'To/PerlDeci.pl').splitlines():
    lo, hi, _ = line.split('\t')
    decimals.append(f'({int(lo,16)},{int(hi or lo,16)})')
numeric_out = '// Generated from Biber 2.22 bundled Perl Unicode 15.0.0 numeric tables.\n// Unicode data copyright Unicode, Inc.; https://www.unicode.org/license.txt\n'
numeric_out += 'static NUMERIC: &[(u32,u32,f64,bool)] = &[\n' + ',\n'.join(numeric) + '\n];\n'
numeric_out += 'static DECIMAL: &[(u32,u32)] = &[\n' + ',\n'.join(decimals) + '\n];\n'
a.output.with_name('collation_numeric_data.rs').write_text(numeric_out)
print(f'{len(numeric)} numeric-value and {len(decimals)} decimal-digit ranges')
