#!/usr/bin/env python3
"""Regenerate accepted scalar-collation option fixtures with the Biber oracle."""
import argparse
import concurrent.futures
import pathlib
import subprocess
import tempfile
from xml.sax.saxutils import quoteattr
p=argparse.ArgumentParser();p.add_argument('oracle',type=pathlib.Path);a=p.parse_args()
root=pathlib.Path(__file__).resolve().parent/'fixtures'/'biber'
profiles={
 'level1':{'level':'1'},'level3':{'level':'3'},
 'shifted':{'variable':'shifted'},'blanked':{'variable':'blanked'},'shift-trimmed':{'variable':'shift-trimmed'},
 'backwards':{'backwards':'2'},'ignore-secondary':{'ignore_level2':'1'},'katakana-first':{'katakana_before_hiragana':'1'},
 'normalization-nfd':{'normalization':'NFD'},'normalization-nfc':{'normalization':'NFC'},
 'normalization-nfkd':{'normalization':'NFKD'},'normalization-nfkc':{'normalization':'NFKC'},
 'normalization-none':{'normalization':None},'identical':{'identical':'1'},
 'highest-minimal':{'highestFFFF':'1','minimalFFFE':'1'},'hangul-terminator':{'hangul_terminator':'30000'},
 'cjk-disabled':{'overrideCJK':'0'},'hangul-implicit':{'overrideHangul':None},
 'alternate':{'alternate':'shifted'},'long-contraction':{'long_contraction':'1'},
 'table-ignored':{'table':'does-not-exist.txt'},
}
profiles.update(('uca-'+str(v),{'UCA_Version':str(v)}) for v in [8,9,11,14,16,18,20,22,24,26,28,30,32,34,36,38,40,41,43])

def generate(row):
 name,options=row
 config='<config><collate_options>'+''.join('<option name='+quoteattr(k)+(' value='+quoteattr(v) if v is not None else '')+'/>' for k,v in options.items())+'</collate_options></config>\n'
 source=root/('collation-locale-ja' if name=='cjk-disabled' else 'collation-locale-en')
 target=root/('collation-options-'+name)
 with tempfile.TemporaryDirectory(prefix='biber-collopts-',dir='/home/leo/rv-build/tmp') as tmp:
  tmp=pathlib.Path(tmp)
  for filename in ['main.tex','main.bcf','refs.bib']:(tmp/filename).write_bytes((source/filename).read_bytes())
  (tmp/'biber.conf').write_text(config)
  result=subprocess.run([str(a.oracle),'--quiet','--configfile=biber.conf','main'],cwd=tmp,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  if result.returncode:raise RuntimeError(name+': '+result.stdout.decode(errors='replace'))
  target.mkdir(exist_ok=True)
  for filename in ['main.tex','main.bcf','refs.bib','biber.conf']:(target/filename).write_bytes((tmp/filename).read_bytes())
  (target/'expected.bbl').write_bytes((tmp/'main.bbl').read_bytes())
 return name
with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
 for name in pool.map(generate,profiles.items()):print(name,flush=True)
