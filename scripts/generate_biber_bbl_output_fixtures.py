#!/usr/bin/env python3
"""Generate Biber 2.22 BBL recode, wrap, encoding, and destination fixtures."""
import argparse
import json
import pathlib
import re
import subprocess
import tempfile
p=argparse.ArgumentParser();p.add_argument('oracle',type=pathlib.Path);a=p.parse_args()
root=pathlib.Path(__file__).resolve().parent/'fixtures'/'biber'
profiles={
 'safe-base':{'output_safechars':True},'safe-full':{'output_safechars':True,'output_safecharsset':'full'},
 'latin1-auto':{'output_encoding':'ISO-8859-1'},'ascii-auto':{'output_encoding':'ascii'},
 'utf16le-auto':{'output_encoding':'UTF-16LE'},
 'wrap32':{'wraplines':32},'wrap64':{'wraplines':64},'wrap-safe':{'wraplines':32,'output_safechars':True},
 'stdout':{'output_file':'-','quiet':True},'custom-file':{'output_file':'custom.bbl','quiet':True},
 'output-directory':{'output_directory':'out','quiet':True},
}
tex='\\documentclass{article}\n\\usepackage[backend=biber,style=numeric]{biblatex}\n\\addbibresource{refs.bib}\n\\begin{document}\n\\nocite{*}\n\\printbibliography\n\\end{document}\n'
bib='''@preamble{"Préambule α – Ж 中文 😀"}
@misc{unicode,author={Müller, Émile},title={Café naïve Straße αβ Ω ≠ ± … © € Ж 中文 😀},subtitle={A field with enough separate words to force wrapping over several lines without changing protected {LaTeX} macros},note={SupercalifragilisticexpialidociousSupercalifragilisticexpialidocious},url={https://example.org/path?query=é},year={2001}}
@misc{ascii,title={ASCII question ? should not cause recoding},note={Short},year={2000}}
@misc{spaces,title={This title contains    consecutive spaces and an accented é with combining marks and many words},note={A long field with enough words and an explicit tab\tbetween some words to exercise unexpand},year={2002}}
'''
with tempfile.TemporaryDirectory(prefix='biber-bbl-output-source-',dir='/home/leo/rv-build/tmp') as work:
 work=pathlib.Path(work);(work/'main.tex').write_text(tex);(work/'refs.bib').write_text(bib)
 result=subprocess.run(['/usr/bin/pdflatex','-interaction=batchmode','-halt-on-error','main.tex'],cwd=work,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
 if result.returncode:raise RuntimeError((work/'main.log').read_text()[-2000:])
 control=(work/'main.bcf').read_bytes()
for name,options in profiles.items():
 with tempfile.TemporaryDirectory(prefix='biber-bbl-output-',dir='/home/leo/rv-build/tmp') as work:
  work=pathlib.Path(work);(work/'main.tex').write_text(tex);(work/'refs.bib').write_text(bib);(work/'main.bcf').write_bytes(control)
  directory=work/options.get('output_directory','');directory.mkdir(exist_ok=True)
  args=['--'+key.replace('_','-')+('' if value is True else '='+str(value)) for key,value in options.items()]
  result=subprocess.run([str(a.oracle),*args,'main'],cwd=work,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  if result.returncode:raise RuntimeError(name+': '+result.stdout.decode(errors='replace'))
  output=options.get('output_file','main.bbl')
  actual=result.stdout if output=='-' else (directory/output).read_bytes()
  logfile=directory/'main.blg'
  if not logfile.exists():raise RuntimeError(name+': missing expected logger '+str(logfile))
  warnings=[line.split('WARN - ',1)[1] for line in logfile.read_text().splitlines() if 'WARN - ' in line]
  target=root/('bbl-output-parity-'+name);target.mkdir(exist_ok=True)
  (target/'main.tex').write_text(tex);(target/'refs.bib').write_text(bib);(target/'main.bcf').write_bytes(control)
  (target/'options.json').write_text(json.dumps(options,indent=2)+'\n');(target/'expected.bbl').write_bytes(actual)
  (target/'expected.warnings').write_text('\n'.join(warnings)+('\n' if warnings else ''))
  print(name,len(actual),'bytes',len(warnings),'warnings','logger',logfile.relative_to(work),flush=True)
