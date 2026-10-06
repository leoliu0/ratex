#!/usr/bin/env python3
"""Extract Biber 2.22's three Lingua::Translit tables without running Perl."""
import argparse
import json
import pathlib
import re
p=argparse.ArgumentParser();p.add_argument('tables',type=pathlib.Path);p.add_argument('output',type=pathlib.Path);a=p.parse_args()
source=a.tables.read_text()
out='// Exact Lingua::Translit tables bundled in Biber 2.22; Perl Artistic License/GPL.\n'
for name,symbol in [('ala-lc_rus','ALA_LC_RUS'),('bgn/pcgn_rus_standard','BGN_PCGN_RUS_STANDARD'),('iast_devanagari','IAST_DEVANAGARI')]:
    table=source.split('  "'+name+'" => {',1)[1]
    body=table.split('    "rules" => [',1)[1].split('\n    ],',1)[0]
    body=re.sub(r'\\x\{([0-9a-fA-F]+)\}',lambda m:chr(int(m[1],16)),body).replace('=>',':')
    rules=json.loads('['+body+']')
    out+='static '+symbol+': &[(&str,&str)] = &[\n'
    for rule in rules:
        pattern=str(rule['from'])
        context=rule.get('context',{})
        if 'after' in context:pattern='(?<='+context['after']+')'+pattern
        if 'before' in context:pattern+='(?='+context['before']+')'
        out+='('+json.dumps(pattern,ensure_ascii=False)+','+json.dumps(str(rule['to']),ensure_ascii=False)+'),\n'
    out+='];\n'
a.output.write_text(out)
print('Extracted three bundled tables')
