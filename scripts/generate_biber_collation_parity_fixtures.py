#!/usr/bin/env python3
"""Generate scoped collation/transliteration parity fixtures and rejected-option oracle evidence."""
import argparse
import json
import pathlib
import subprocess
import tempfile
from xml.sax.saxutils import quoteattr
p=argparse.ArgumentParser();p.add_argument('oracle',type=pathlib.Path);p.add_argument('--only');a=p.parse_args()
root=pathlib.Path(__file__).resolve().parent/'fixtures'/'biber'
profiles={
 'numeric-casts': ('', r'\DeclareSortingTemplate{parity}{\sort{\field{volume}}\sort{\field{entrykey}}}', ['-4294967297','-2147483649','-2147483648','-1.9','-.5','0','0.5','1.1','1.9','1e1','9.9','2147483647','2147483648','4294967296','9223372036854775807','9223372036854775808','1e30','NaN','Inf','-Inf','abc'], 'volume'),
 'numeric-unicode': ('', r'\DeclareSortingTemplate{parity}{\sort{\field{volume}}\sort{\field{entrykey}}}', ['١٢','۱۲','१٢','²','⅓','⅞','Ⅷ','VIII','IL','IC','IVI','VIV','MMMM','III','IIV','XIIII','ı','９','𝟜𝟚','𝟜2','-½','༳','⑩','〇','一','一二','12','½','𐧳','١٢٣٤٥٦٧٨٩٠١٢٣٤٥٦٧٨٩٠١'], 'volume'),
 'numeric-noroman': (r'\ExecuteBibliographyOptions{noroman=true}', r'\DeclareSortingTemplate{parity}{\sort{\field{volume}}\sort{\field{entrykey}}}', ['IV','Ⅳ','X','Ⅹ','II','Ⅱ','I','Ⅰ','IL','III','⅓','١٢'], 'volume'),
 'numeric-boundaries': ('', r'\DeclareSortingTemplate{parity}{\sort{\field{volume}}\sort{\field{entrykey}}}', ['0','0e0','-0','+0','-1e30','1e19','18446744073709551615','18446744073709551616','-9223372036854775809','9.223372036854775808e18','1.8446744073709551616e19','1e999','-1e999','nan','+nan','-nan','NaN(123)','NaN(0x123)','nan(foo)','infinity','+Infinity','-infinity','1_2','0x10','1e','1.2.3','1.#INF','1.#QNAN','  12  ','- 12','0.9999999999999999','-0.9999999999999999','9223372036854775807.0'], 'volume'),
 'translit-exclusions': (r'\DeclareSortTranslit{\translit{*}{Russian}{ALA-LC}}', r'\DeclareSortingTemplate{parity}{\sort{\field{url}}\sort{\field{title}}}', ['Я','А','Щ','Б','Ю','Е','Елена','Подъезд'], 'url'),
 'translit-fullfold': (r'\DeclareSortTranslit{\translit[STRASSE]{title}{Russian}{ALA-LC}}', r'\DeclareSortingTemplate{parity}{\sort{\field{title}}\sort{\field{entrykey}}}', ['Я','А','Щ','Б','Ю','Е'], 'title'),
 'mutable-cache': ('', r'\DeclareSortingTemplate{parity}{\sort[sortcase=false]{\field{title}}\sort[sortupper=false]{\field{subtitle}}\sort[sortcase=false]{\field{title}}\sort{\field{entrykey}}}', ['A','a','B','b','Ä','ä'], 'title'),
 'scalar-list': ('', r'\DeclareSortingTemplate{parity}{\sort{\field{location}}\sort{\field{title}}}', ['A and Z','A and B','B and A','Z','A'], 'location'),
 'pinned-normalization': ('', r'\DeclareSortingTemplate{parity}{\sort{\field{title}}\sort{\field{entrykey}}}', ['\U0001e030','\U0001e031','\U0001e08f','\U0001e4ec','\u0301','\u1f82','\u0344','\u0958','\u1100\u1161\u11a8','\uac01','\u212b','A\u030a'], 'title'),
}
for name,(extra,sorting,words,field) in profiles.items():
 if a.only and name!=a.only:continue
 with tempfile.TemporaryDirectory(prefix='biber-parity-collation-',dir='/home/leo/rv-build/tmp') as work:
  work=pathlib.Path(work)
  tex='\\documentclass{article}\n\\usepackage[backend=biber,style=numeric]{biblatex}\n\\addbibresource{refs.bib}\n'+extra+'\n'+sorting+'\n\\begin{document}\n\\nocite{*}\n\\newrefcontext[sorting=parity]\n\\printbibliography\n\\end{document}\n'
  bib=''.join('@misc{k%03d,%s={%s},%s}\n'%(i,field,word, ('title={'+word+'},' if field!='title' else '')+'subtitle={'+words[(i+1)%len(words)]+'},langid={'+('Straße' if i%2==0 else 'STRASSE')+'}') for i,word in enumerate(words))
  (work/'main.tex').write_text(tex);(work/'refs.bib').write_text(bib)
  result=subprocess.run(['/usr/bin/pdflatex','-interaction=batchmode','-halt-on-error','main.tex'],cwd=work,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  if result.returncode:raise RuntimeError(name+': '+(work/'main.log').read_text()[-2000:])
  if name=='pinned-normalization':(work/'biber.conf').write_text('<config><collate_options><option name="normalization" value="NFKC"/></collate_options></config>')
  command=[str(a.oracle),'--quiet']+(['--configfile=biber.conf'] if (work/'biber.conf').exists() else [])+['main']
  result=subprocess.run(command,cwd=work,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  if result.returncode:raise RuntimeError(name+': '+result.stdout.decode(errors='replace'))
  target=root/('collation-parity-'+name);target.mkdir(exist_ok=True)
  for filename in ['main.tex','main.bcf','refs.bib','biber.conf']:
   if (work/filename).exists():(target/filename).write_bytes((work/filename).read_bytes())
  (target/'expected.bbl').write_bytes((work/'main.bbl').read_bytes())
  print(name, ' '.join(__import__('re').findall(r'\\entry\{([^}]+)\}',(work/'main.bbl').read_text())),flush=True)
if a.only:raise SystemExit(0)
source=root/'collation-locale-en'
options={k:'1' for k in ['entry','ignoreChar','ignoreName','undefChar','undefName','suppress','mapping','maxlength','contraction','rewrite']}
options.update({'level':'0','UCA_Version':'42','backwards':'[2,3]','rearrange':'[3584,3585]','normalization':'nfd','variable':'invalid'})
evidence=[]
for key,value in options.items():
 with tempfile.TemporaryDirectory(prefix='biber-parity-option-',dir='/home/leo/rv-build/tmp') as work:
  work=pathlib.Path(work)
  for filename in ['main.bcf','refs.bib']:(work/filename).write_bytes((source/filename).read_bytes())
  (work/'biber.conf').write_text('<config><collate_options><option name='+quoteattr(key)+' value='+quoteattr(value)+'/></collate_options></config>')
  result=subprocess.run([str(a.oracle),'--quiet','--configfile=biber.conf','main'],cwd=work,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  evidence.append({'option':key,'value':value,'exit':result.returncode,'output':result.stdout.decode(errors='replace')})
(root/'collation-parity-invalid-options.json').write_text(json.dumps(evidence,ensure_ascii=False,indent=2)+'\n')
print('rejected option profiles',len(evidence),flush=True)
