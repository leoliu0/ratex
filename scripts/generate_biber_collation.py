#!/usr/bin/env python3
"""Extract Unicode::Collate 1.31 tables from Biber's unpacked PAR payload."""
import argparse
import pathlib
import re

p = argparse.ArgumentParser()
p.add_argument('collate', type=pathlib.Path, help='PAR inc/lib/Unicode/Collate directory')
p.add_argument('output', type=pathlib.Path)
a = p.parse_args()
weights = []
sequences = []

def entries(text):
    out = {}
    for line in text.splitlines():
        m = re.match(r'^([0-9A-Fa-f ]+)\s*;\s*(.*?)\s*(?:#.*)?$', line)
        if not m:
            continue
        seq = tuple(int(x, 16) for x in m[1].split())
        ces = []
        for mark, body in re.findall(r'\[([.*])([^\]]+)\]', m[2]):
            w = [int(x, 16) for x in body.split('.')]
            w += [0] * (4-len(w))
            ces.append((*w, int(mark == '*')))
        if ces:
            out[seq] = ces if any(any(w[:3]) for w in ces) else []
    return out

def table(name, data):
    rows = []
    for seq, ws in sorted(data.items()):
        off, wo = len(sequences), len(weights)
        sequences.extend(seq)
        weights.extend(ws)
        rows.append(f'({off},{len(seq)},{wo},{len(ws)})')
    return f'static {name}: &[Mapping] = &[\n' + ',\n'.join(rows) + '\n];\n'

text = '// Generated from Biber 2.22 PAR: Unicode::Collate 1.31, DUCET 13.0.0, locale 1.31.\n// Unicode data copyright Unicode, Inc.; https://www.unicode.org/license.txt\n'
text += table('DUCET', entries((a.collate/'allkeys.txt').read_text()))
locales = []
for f in sorted((a.collate/'Locale').glob('*.pl')):
    src = f.read_text()
    data = entries(src)
    cjk_data = {}
    cjk = re.search(r'CJK::(\w+)::weight', src)
    if cjk:
        cjk_src = (a.collate/'CJK'/f'{cjk[1]}.pm').read_text()
        body = cjk_src.split('__DATA__\n')[1].split('__END__')[0]
        if cjk[1] == 'Korean':
            prims = dict((x, int(y,16)) for x,y in re.findall(r"'([0-9A-F]+)',\s*0x([0-9A-Fa-f]+)", cjk_src))
            prim = []
            for line in body.splitlines():
                if ':' in line:
                    prim = [prims[x] for x in line.split(':')[1].split('-')]
                    secondary = 0x20
                else:
                    for cp in line.split():
                        secondary += 1
                        u = int(cp,16)
                        cjk_data[(u,)] = [(prim[0],secondary,2,u & 0xffff,0)] + [(x,0x20,2,u & 0xffff,0) for x in prim[1:]]
        else:
            wt = int(re.search(r'my \$wt = 0x([0-9A-Fa-f]+)', cjk_src)[1],16)
            for token in body.split():
                if '-' not in token:
                    u = int(token,16)
                    cjk_data[(u,)] = [(wt,0x20,2,u & 0xffff,0)]
                wt += 1
    symbol = 'LOCALE_' + f.stem.upper()
    text += table(symbol, data)
    cjk_symbol = 'CJK_' + f.stem.upper()
    text += table(cjk_symbol, cjk_data)
    backwards = re.search(r'backwards\s*=>\s*(\d+)',src)
    suppress = re.search(r'suppress\s*=>\s*\[([^\]]+)\]',src)
    suppress_values = suppress[1] if suppress else ''
    upper = bool(re.search(r'upper_before_lower\s*=>\s*1',src))
    locales.append(f'("{f.stem}", {symbol}, {backwards[1] if backwards else 0}, {str(upper).lower()}, &[{suppress_values}], {cjk_symbol})')
text += 'static SEQUENCES: &[u32] = &[\n' + ','.join(map(str,sequences)) + '\n];\n'
text += 'static WEIGHTS: &[Element] = &[\n' + ',\n'.join('['+','.join(map(str,w))+']' for w in weights) + '\n];\n'
text += 'static LOCALES: &[(&str, &[Mapping], u8, bool, &[u32], &[Mapping])] = &[\n' + ',\n'.join(locales) + '\n];\n'
a.output.write_text(text)
print(f'{len(locales)} locale tables; {len(weights)} collation elements; {len(text)} source bytes')
