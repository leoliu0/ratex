#!/usr/bin/env python3
"""Regenerate Biber 2.22 locale oracle fixtures, using system TeX only."""
import argparse
import concurrent.futures
import pathlib
import re
import subprocess
import tempfile

p=argparse.ArgumentParser()
p.add_argument('collate',type=pathlib.Path)
p.add_argument('oracle',type=pathlib.Path)
p.add_argument('--only')
a=p.parse_args()
root=pathlib.Path(__file__).resolve().parent/'fixtures'/'biber'
aliases={'de_phone':'de__phonebook','de_at_ph':'de_AT_phonebook','es_trad':'es__traditional','fr_ca':'fr_CA','fi_phone':'fi__phonebook','si_dict':'si__dictionary','sv_refo':'sv__reformed','ug_cyrl':'ug_Cyrl','zh_big5':'zh__big5han','zh_gb':'zh__gb2312han','zh_pin':'zh__pinyin','zh_strk':'zh__stroke','zh_zhu':'zh__zhuyin'}
class Probe:
    def __init__(self,stem,locale,options=''):
        self.stem,self.locale,self.options=stem,locale,options
    def read_text(self):return ''
variants=[Probe(x,x) for x in ['de','de-1996','ru','el','fr','en','bs','bs_Cyrl','sr_Latn','es-trad','de-phonebook','german','swedish','turkish']]
variants.extend(Probe('case-%s-upper-%s'%(case,upper),'en','sortcase=%s,sortupper=%s,'%(case,upper)) for case in ['true','false'] for upper in ['true','false'])
variants.append(Probe('multiple-contexts','en'))
translits={'translit-iast':'\\DeclareSortTranslit{\\translit{title}{IAST}{Devanagari}}','translit-russian-ala':'\\DeclareSortTranslit{\\translit{title}{Russian}{ALA-LC}}','translit-russian-bgn':'\\DeclareSortTranslit{\\translit{title}{Russian}{BGN/PCGN-Standard}}','translit-langid':'\\DeclareSortTranslit{\\translit[russian]{title}{Russian}{BGN/PCGN-Standard}}','translit-scope':'\\DeclareSortTranslit{\\translit{title}{Russian}{ALA-LC}}\\DeclareSortTranslit[misc]{\\translit{title}{IAST}{Devanagari}}'}
variants.extend(Probe(stem,'en') for stem in translits)

def generate(file):
    stem=file.stem
    locale=getattr(file,'locale',aliases.get(stem,stem))
    words=['a','A','ae','Ae','ä','Ä','a\u0327\u0308','b','B','ch','Ch','CH','cz','h','i','I','ı','İ','y','z','Z','å','æ','ø','ö','é','e\u0301','cote','coté','côte','côté','Dzs','dzs','ll','l·l','中文','北京','東京','一','阿','あ','ア','亜','가','각','不','ㄱ','ѝ','и','й','е','ё','і','ї','ґ','ω','Ω','α','ά']
    tailored=[]
    for line in file.read_text().splitlines():
        m=re.match(r'^([0-9A-Fa-f ]+)\s*;',line)
        if m:
            word=''.join(chr(int(x,16)) for x in m[1].split())
            if word not in tailored and not any(c in word for c in '{}\\\n\r%') and not any(0xfdd0<=ord(c)<=0xfdef or ord(c)&0xfffe==0xfffe for c in word):
                tailored.append(word)
    if tailored:
        stride=max(1,len(tailored)//24)
        words.extend(tailored[::stride][:24])
    if stem in translits:words.extend(['Пётр','Елена','Екатерина','Алексей','Маяковский','Подъезд','Соловьёв','Щербина','Юрий','Яков','oṃ','śāstra','kṛṣṇa','buddha','aṅga','dharma','Ṛgveda','ātmā'])
    words=list(dict.fromkeys(words))
    bib=''.join('@misc{k%03d,title={%s},year={2000}%s}\n'%(i,word,',langid={'+['russian','english','Russian'][i%3]+'}' if stem=='translit-langid' else '') for i,word in enumerate(words))
    tex='\\documentclass{article}\n\\usepackage[%sbackend=biber,style=numeric]{biblatex}\n\\addbibresource{refs.bib}\n'%getattr(file,'options','')
    if stem in translits:tex+=translits[stem]+'\n'
    contexts=[locale] if stem!='multiple-contexts' else ['de','de__phonebook','de-1996','sv','da','nb','fi','es__traditional','fr_CA','cs','pl','tr','lt','hu','et','ru','uk','el','ja','zh__pinyin','ko']
    for i,loc in enumerate(contexts):
        name='localeprobe' if len(contexts)==1 else 'localeprobe'+str(i)
        tex+='\\DeclareSortingTemplate[locale=%s]{%s}{\\sort{\\field{title}}\\sort{\\field{entrykey}}}\n'%(loc,name)
    if stem=='multiple-contexts':
        tex+='\\DeclareSortingTemplate[locale=en]{itemprobe}{\\sort[locale=sv,sortcase=true,sortupper=false]{\\field{title}}\\sort[locale=tr,sortcase=false,sortupper=true]{\\field{entrykey}}}\n'
    tex+='\\begin{document}\n\\nocite{*}\n'
    for i in range(len(contexts)):
        name='localeprobe' if len(contexts)==1 else 'localeprobe'+str(i)
        tex+='\\newrefcontext[sorting=%s]\n\\printbibliography\n'%name
    if stem=='multiple-contexts':tex+='\\newrefcontext[sorting=itemprobe]\n\\printbibliography\n'
    tex+='\\end{document}\n'
    target=root/('collation-locale-'+stem.replace('_','-'))
    with tempfile.TemporaryDirectory(prefix='biber-locale-',dir='/home/leo/rv-build/tmp') as tmp:
        tmp=pathlib.Path(tmp)
        (tmp/'main.tex').write_text(tex)
        (tmp/'refs.bib').write_text(bib)
        result=subprocess.run(['/usr/bin/pdflatex','-interaction=batchmode','-halt-on-error','main.tex'],cwd=tmp,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        if result.returncode:raise RuntimeError(stem+': TeX failed '+(tmp/'main.log').read_text()[-1500:])
        result=subprocess.run([str(a.oracle),'--quiet','main'],cwd=tmp,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        if result.returncode:raise RuntimeError(stem+': oracle failed '+result.stdout.decode(errors='replace'))
        target.mkdir(exist_ok=True)
        for name,source in [('main.tex','main.tex'),('refs.bib','refs.bib'),('main.bcf','main.bcf'),('expected.bbl','main.bbl')]:
            (target/name).write_bytes((tmp/source).read_bytes())
    return stem,len(words)
files=[f for f in [*sorted((a.collate/'Locale').glob('*.pl')),*variants] if not a.only or f.stem==a.only]
with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
    for stem,n in pool.map(generate,files):print(stem,n,flush=True)
