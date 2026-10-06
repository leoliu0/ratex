//! Unicode::Collate 1.31 semantics and the exact Biber 2.22 bundled tables.
use crate::model::{Control, DataList, SortItem};
use std::fmt::Write;
#[path = "collation_normalization.rs"]
pub(crate) mod normalization;
use normalization::canonical_combining_class;
#[path = "collation_numeric.rs"]
mod numeric;
pub(crate) use numeric::integer_sort_value;

type Mapping = (usize, usize, usize, usize);
type Element = [u16; 5];
include!("uca_table.rs");
include!("locale_language.rs");

#[derive(Clone)]
pub(crate) struct Collator {
    table: &'static [Mapping], suppress: &'static [u32], level: usize,
    backwards: u8, upper: bool, kana: bool, ignore_secondary: bool,
    variable: String, normalization: String, identical: bool,
    highest: bool, minimal: bool, version: u8, terminator: u16, rearrange: Vec<u32>,
    long: bool, cjk: &'static [Mapping], cjk_defined: bool, implicit_hangul: bool,
}
fn truth(s: &str) -> bool { !matches!(s,""|"0"|"\0") }
// Perl's numeric conversion consumes a decimal prefix, unlike Rust's parse().
fn number(s:&str)->f64 {
    let s=s.trim_start_matches(|c:char|c.is_ascii_whitespace());
    if let Some(n)=special_number(s.trim_end_matches(|c:char|c.is_ascii_whitespace())){return n;}
    let bytes=s.as_bytes();let mut i=usize::from(matches!(bytes.first(),Some(b'+')|Some(b'-')));
    let mut digits=0;
    while bytes.get(i).is_some_and(u8::is_ascii_digit){i+=1;digits+=1;}
    if bytes.get(i)==Some(&b'.'){i+=1;while bytes.get(i).is_some_and(u8::is_ascii_digit){i+=1;digits+=1;}}
    if digits==0{return 0.0;}
    if matches!(bytes.get(i),Some(b'e')|Some(b'E')){let begin=i;i+=1;if matches!(bytes.get(i),Some(b'+')|Some(b'-')){i+=1;}let start=i;while bytes.get(i).is_some_and(u8::is_ascii_digit){i+=1;}if i==start{i=begin;}}
    s[..i].parse().unwrap_or(0.0)
}
// Perl 5.38 grok_infnan, including C99 payload and legacy MSVC spellings.
fn special_number(s:&str)->Option<f64>{
    let negative=s.starts_with('-');let mut s=s.strip_prefix(['+','-']).unwrap_or(s);let mut legacy=false;
    if let Some(rest)=s.strip_prefix("1.#").or_else(||s.strip_prefix("1#")){s=rest;legacy=true;}
    if s.eq_ignore_ascii_case("infinity")||s.eq_ignore_ascii_case("inf")||legacy&&s.get(..3).is_some_and(|s|s.eq_ignore_ascii_case("inf"))&&s[3..].bytes().all(|b|b==b'0'){
        return Some(if negative{f64::NEG_INFINITY}else{f64::INFINITY});
    }
    if legacy&&s.get(..3).is_some_and(|s|s.eq_ignore_ascii_case("ind"))&&s[3..].bytes().all(|b|b==b'0'){return Some(f64::NAN);}
    if s.starts_with(['s','S','q','Q']){s=&s[1..];}
    if !s.get(..3).is_some_and(|s|s.eq_ignore_ascii_case("nan")){return None;}s=&s[3..];
    if s.starts_with(['s','S','q','Q']){s=&s[1..];}
    if s.is_empty(){return Some(f64::NAN);}
    let payload=s.strip_prefix('(')?.strip_suffix(')')?.trim_end_matches(|c:char|c.is_ascii_whitespace());
    let (digits,base)=if let Some(digits)=payload.strip_prefix("0x").or_else(||payload.strip_prefix("0X")){(digits,16)}else if let Some(digits)=payload.strip_prefix("0b").or_else(||payload.strip_prefix("0B")){(digits,2)}else{(payload,10)};
    if digits.is_empty(){return None;}let mut value=0u64;let mut previous_digit=false;
    for c in digits.chars(){if c=='_'&&base!=10&&previous_digit{previous_digit=false;continue;}let digit=c.to_digit(base).filter(|_|c.is_ascii())?;value=value.checked_mul(base as u64)?.checked_add(digit as u64)?;previous_digit=true;}
    previous_digit.then_some(f64::NAN)
}
/// Scalar::Util::looks_like_number followed by Sort::Key's signed IV coercion.
/// Perl preserves the exact decimal integer part even when a fraction is present.
pub(crate) fn integer_sort_key(s:&str)->i64 {
    let s=s.trim_matches(|c:char|c.is_ascii_whitespace());
    if s=="0 but true"{return 0;}
    let special=special_number(s);let n=if let Some(n)=special{n}else{let Ok(n)=s.parse::<f64>()else{return 2_000_000_000;};n};
    if n.is_nan(){return 0;}
    if special.is_none()&&!s.contains(['e','E']){
        let negative=s.starts_with('-');let unsigned=s.strip_prefix(['+','-']).unwrap_or(s);let integer=unsigned.split('.').next().unwrap_or("");
        let value=if integer.is_empty(){0}else{integer.parse::<u64>().unwrap_or(u64::MAX)};
        return if negative{if value>i64::MAX as u64{i64::MIN}else{-(value as i64)}}else{value as i64};
    }
    if n<0.0{n as i64}else{(n as u64) as i64}
}
/// Validate the options passed to Unicode::Collate::change, not its constructor.
pub(crate) fn validate_options(c:&Control)->Result<(),String> {
    for (key,value) in &c.options {
        let Some(name)=key.strip_prefix("collate_options.") else{continue;};
        if matches!(name,"entry"|"mapping"|"maxlength"|"contraction"|"ignoreChar"|"ignoreName"|"undefChar"|"undefName"|"rewrite"|"versionTable"|"alternateTable"|"backwardsTable"|"forwardsTable"|"rearrangeTable"|"variableTable"|"derivCode"|"normCode"|"rearrangeHash"|"backwardsFlag"|"suppress"|"suppressHash"|"__useXS"){
            return Err(format!("change of {name} via change() is not allowed!"));
        }
        if name=="table"&&(!truth(value)||c.options.get("collate_options.locale").is_some_and(|s|truth(s))){
            return Err("change of table via change() is not allowed!".into());
        }
        if matches!(name,"preprocess"|"overrideCJK"|"overrideHangul"|"overrideOut")&&truth(value){
            return Err(format!("Collation option '{name}' requires a Perl code callback, which the self-contained Rust Biber does not execute"));
        }
        if name=="level"&&c.options.get("sortcase").map(|s|truth(s)).unwrap_or(true)||name=="backwards"&&value!="\0"{
            let n=number(value);let shown=if n<0.0{n as i64}else{(n as u64) as i64};
            if n.is_nan()||n<1.0{return Err(format!("Illegal level {shown} (in value for key '{name}') lower than 1."));}
            if n>4.0{return Err(format!("Unsupported level {shown} (in value for key '{name}') higher than 4."));}
        }
        if name=="UCA_Version"&&!matches!(value.as_str(),"8"|"9"|"11"|"14"|"16"|"18"|"20"|"22"|"24"|"26"|"28"|"30"|"32"|"34"|"36"|"38"|"40"|"41"|"43"){
            return Err(format!("Illegal UCA version (passed {value})."));
        }
        if name=="rearrange"&&value!="\0"{return Err("Unicode::Collate: list for rearrangement must be store in ARRAYREF".into());}
        if name=="normalization"&&!matches!(value.as_str(),"\0"|"prenormalized"|"NFD"|"D"|"NFC"|"C"|"NFKD"|"KD"|"NFKC"|"KC"|"FCD"|"FCC"){
            return Err(format!("Unicode::Collate unknown normalization form name: {value}"));
        }
    }
    let variable=c.options.get("collate_options.alternate").filter(|s|truth(s)).or_else(||c.options.get("collate_options.variable").filter(|s|truth(s))).map(|s|s.to_lowercase());
    if let Some(variable)=variable{if !matches!(variable.as_str(),"blanked"|"non-ignorable"|"shifted"|"shift-trimmed"){return Err(format!("Unicode::Collate unknown variable parameter name: {variable}"));}}
    Ok(())
}
/// Defined, scalar locale defaults reported by Biber::UCollate::change.
/// Perl iterates this set in hash order; emit a deterministic order instead.
pub(crate) fn tool_log_info(c:&Control,d:&DataList)->Vec<String>{
    let locale=c.options.get("sortlocale_override").or_else(||c.sorting_locales.get(&d.sorting)).or_else(||c.options.get("sortlocale")).map(String::as_str).unwrap_or("en_US");
    let explicit=c.options.get("collate_options.locale").filter(|s|truth(s));let coll_locale=explicit.map(String::as_str).unwrap_or(locale);
    let file=locale_file(coll_locale);let row=LOCALES.iter().find(|r|r.0==file);let mut info=Vec::new();
    if explicit.is_none(){if let Some(table)=c.options.get("collate_options.table").filter(|s|truth(s)){info.push(format!("Ignoring collation table '{table}' as locale is set ({locale})"));}}
    let case=c.options.get("sortcase").map(|s|truth(s)).unwrap_or(true);
    let upper=c.options.get("sortupper").map(|s|truth(s)).unwrap_or(true);
    let level=if case{c.options.get("collate_options.level").map(String::as_str).unwrap_or("4")}else{"2"};
    let variable=c.options.get("collate_options.alternate").filter(|s|truth(s)).or_else(||c.options.get("collate_options.variable")).map(String::as_str).unwrap_or("non-ignorable");
    let normalization=c.options.get("collate_options.normalization").map(String::as_str).unwrap_or("prenormalized");
    for (key,old,value) in [("level","4",Some(level)),("normalization","NFD",Some(normalization)),("variable","shifted",Some(variable)),("UCA_Version","43",c.options.get("collate_options.UCA_Version").map(String::as_str)),("long_contraction","",c.options.get("collate_options.long_contraction").map(String::as_str))]{
        if let Some(value)=value{let value=if value=="\0"{""}else{value};if old!=value{info.push(format!("Overriding locale '{coll_locale}' defaults '{key} = {old}' with '{key} = {value}'"));}}
    }
    if let Some(row)=row{
        if row.3&&!upper{info.push(format!("Overriding locale '{coll_locale}' defaults 'upper_before_lower = 1' with 'upper_before_lower = 0'"));}
        if row.2>0{if let Some(backwards)=c.options.get("collate_options.backwards"){if backwards!=&row.2.to_string(){info.push(format!("Overriding locale '{coll_locale}' defaults 'backwards = {}' with 'backwards = {}'",row.2,if backwards=="\0"{""}else{backwards}));}}}
    }else{info.push(format!("No sort tailoring available for locale '{locale}'"));}
    info
}
fn locale_file(locale: &str) -> String {
    let s=language_locale(locale).to_lowercase().replace(['-', ' ', '.'], "_");
    let mut codes:Vec<&str>=s.split('_').collect();
    let alias=match codes.last().copied().unwrap_or("") {"phone"|"phonebk"=>"phonebook","dict"=>"dictionary","reform"=>"reformed","trad"=>"traditional","big5"=>"big5han","gb2312"=>"gb2312han",x=>x};
    if let Some(last)=codes.last_mut(){*last=alias;}
    let lan=codes.first().copied().unwrap_or("");
    let rest=&codes[1.min(codes.len())..];
    if lan=="bs" {return if rest.contains(&"cyrl"){"sr"}else{"hr"}.into();}
    if lan=="sr"&&rest.contains(&"latn"){return "hr".into();}
    let variant=rest.iter().rev().find(|x|matches!(**x,"phonebook"|"traditional"|"dictionary"|"reformed"|"big5han"|"gb2312han"|"pinyin"|"stroke"|"zhuyin"));
    let special=match (lan,variant.copied()) {
        ("de",Some("phonebook"))=>Some(if rest.contains(&"at"){"de_at_ph"}else{"de_phone"}),
        ("es",Some("traditional"))=>Some("es_trad"),("fi",Some("phonebook"))=>Some("fi_phone"),
        ("si",Some("dictionary"))=>Some("si_dict"),("sv",Some("reformed"))=>Some("sv_refo"),
        ("zh",Some("big5han"))=>Some("zh_big5"),("zh",Some("gb2312han"))=>Some("zh_gb"),
        ("zh",Some("pinyin"))=>Some("zh_pin"),("zh",Some("stroke"))=>Some("zh_strk"),
        ("zh",Some("zhuyin"))=>Some("zh_zhu"),("fr",_) if rest.contains(&"ca")=>Some("fr_ca"),
        ("ug",_) if rest.contains(&"cyrl")=>Some("ug_cyrl"),_=>None,
    };
    special.unwrap_or(lan).into()
}
impl Collator {
    fn new(locale: &str) -> Self {
        let file=locale_file(locale);
        let (_,table,backwards,upper,suppress,cjk)=LOCALES.iter().find(|x|x.0==file).copied().unwrap_or(("",&[],0,false,&[],&[]));
        Self {table,suppress,level:4,backwards:if backwards>0{1<<backwards}else{0},upper,kana:false,ignore_secondary:false,variable:"shifted".into(),normalization:"NFD".into(),identical:false,highest:false,minimal:false,version:43,terminator:0,rearrange:Vec::new(),long:false,cjk,cjk_defined:true,implicit_hangul:false}
    }
    pub(crate) fn initial(c: &Control,d: &DataList) -> Self {
        let locale=c.sorting_locales.get(&d.sorting).map(String::as_str).unwrap_or_else(||c.options.get("sortlocale").map(String::as_str).unwrap_or("en_US"));
        let mut this=Self::new(locale);this.level=1;this
    }
    pub(crate) fn sorting(c: &Control,d: &DataList,item: Option<&SortItem>) -> Self {
        let locale=item.and_then(|s|s.locale.as_deref()).or_else(||c.options.get("sortlocale_override").filter(|s|!s.is_empty()).map(String::as_str)).or_else(||c.sorting_locales.get(&d.sorting).map(String::as_str)).or_else(||c.options.get("sortlocale").map(String::as_str)).filter(|s|!s.is_empty()).unwrap_or("en_US");
        let locale=c.options.get("collate_options.locale").filter(|s|truth(s)).map(String::as_str).unwrap_or(locale);
        let mut this=Self::new(locale);
        this.variable="non-ignorable".into();this.normalization="prenormalized".into();
        for (key,value) in &c.options {if let Some(key)=key.strip_prefix("collate_options."){if key!="alternate"{this.change(key,value);}}}
        if let Some(alt)=c.options.get("collate_options.alternate").filter(|s|truth(s)){this.variable=alt.to_lowercase();}
        let case=c.options.get("sortcase").map(|s|truth(s)).unwrap_or(true);
        if !case{this.level=2;}
        this.upper=c.options.get("sortupper").map(|s|truth(s)).unwrap_or(true);
        if let Some(item)=item {if let Some(sc)=item.sortcase{if sc!=case{this.level=if sc{4}else{2};}}if let Some(su)=item.sortupper{this.upper=su;}}
        this
    }
    pub(crate) fn signature<'a>(c:&'a Control,d:&DataList,item:Option<&'a SortItem>)->(Option<&'a str>,Option<bool>,Option<bool>) {
        let locale=c.options.get("sortlocale_override").or_else(||c.sorting_locales.get(&d.sorting)).or_else(||c.options.get("sortlocale")).map(String::as_str).unwrap_or("en_US");
        let local=item.and_then(|i|i.locale.as_deref()).map(language_locale).filter(|&l|l!=language_locale(locale));
        let case=c.options.get("sortcase").map(|s|truth(s)).unwrap_or(true);
        let upper=c.options.get("sortupper").map(|s|truth(s)).unwrap_or(true);
        (local,item.and_then(|i|i.sortcase).filter(|&sc|sc!=case),item.and_then(|i|i.sortupper).filter(|&su|su!=upper))
    }
    pub(crate) fn item_key(&mut self,base:&Self,c:&Control,d:&DataList,item:Option<&SortItem>,s:&str)->Vec<u16> {
        let (locale,sc,su)=Self::signature(c,d,item);
        if locale.is_some(){return Self::sorting(c,d,item).key(s);}
        if sc.is_none()&&su.is_none(){self.level=base.level;self.upper=base.upper;}
        else {if let Some(sc)=sc{self.level=if sc{4}else{2};}if let Some(su)=su{self.upper=su;}}
        self.key(s)
    }
    fn change(&mut self,key: &str,value: &str) {
        match key {
            "level"=>self.level=number(value) as usize,
            "variable"=>self.variable=if truth(value){value.to_lowercase()}else{"shifted".into()},
            "normalization"=>self.normalization=value.into(),
            "upper_before_lower"=>self.upper=truth(value),"katakana_before_hiragana"=>self.kana=truth(value),
            "ignore_level2"=>self.ignore_secondary=truth(value),"identical"=>self.identical=truth(value),
            "highestFFFF"=>self.highest=truth(value),"minimalFFFE"=>self.minimal=truth(value),
            "UCA_Version"=>self.version=value.parse().unwrap_or(43),
            "hangul_terminator"=>self.terminator=number(value) as i64 as u16,
            "backwards"=>self.backwards=if value=="\0"{0}else{1<<(number(value) as u8)},
            "rearrange"=>self.rearrange.clear(),
            "long_contraction"=>self.long=truth(value),
            "overrideCJK"=>{if !truth(value){self.cjk=&[];self.cjk_defined=value!="\0";}},
            "overrideHangul"=>self.implicit_hangul=value=="\0",
            _=>{},
        }
    }
    fn find(&self,seq: &[u32]) -> Option<&'static [Element]> {
        fn search(table: &'static [Mapping],seq: &[u32])->Option<&'static [Element]>{let i=table.binary_search_by(|&(off,len,_,_)|SEQUENCES[off..off+len].cmp(seq)).ok()?;let (_,_,off,len)=table[i];Some(&WEIGHTS[off..off+len])}
        search(self.table,seq).or_else(||if seq.len()>1&&self.suppress.contains(&seq[0]){None}else{search(DUCET,seq)}).or_else(||if seq.len()==1&&is_ideograph(seq[0],self.version){search(self.cjk,seq)}else{None})
    }
    fn max_len(&self,cp: u32) -> usize {
        [self.table,DUCET].into_iter().map(|table|{let start=table.partition_point(|&(off,_,_,_)|SEQUENCES[off]<cp);table[start..].iter().take_while(|&&(off,_,_,_)|SEQUENCES[off]==cp).map(|x|x.1).max().unwrap_or(1)}).max().unwrap_or(1)
    }
    fn prefix(&self,seq: &[u32]) -> bool {
        [self.table,DUCET].into_iter().any(|table|{let start=table.partition_point(|&(off,len,_,_)|SEQUENCES[off..off+len].cmp(seq).is_lt());table.get(start).is_some_and(|&(off,len,_,_)|len>seq.len()&&SEQUENCES[off..off+len].starts_with(seq))})
    }
    fn implicit(&self,cp: u32,out: &mut Vec<Element>) {
        if cp==0xffff&&self.highest {out.push([0xfffe,0x20,5,0xffff,0]);return;}
        if cp==0xfffe&&self.minimal {out.push([1,0x20,5,0xfffe,0]);return;}
        let v=self.version;
        if v==8&&self.cjk_defined&&self.cjk.is_empty()&&is_ideograph(cp,0){out.push([cp as u16,0x20,2,cp as u16,0]);return;}
        let bound=if v>=43{0x9ffc}else if v>=38{0x9fef}else if v>=36{0x9fea}else if v>=32{0x9fd5}else if v>=24{0x9fcc}else if v>=20{0x9fcb}else if v>=18{0x9fc3}else if v>=14{0x9fbb}else{0x9fa5};
        let basic=(0x4e00..=bound).contains(&cp)||matches!(cp,0xfa0e|0xfa0f|0xfa11|0xfa13|0xfa14|0xfa1f|0xfa21|0xfa23|0xfa24|0xfa27|0xfa28|0xfa29);
        let ext=(0x3400..=if v>=43{0x4dbf}else{0x4db5}).contains(&cp)||(0x20000..=if v>=43{0x2a6dd}else{0x2a6d6}).contains(&cp)||v>=20&&(0x2a700..=0x2b734).contains(&cp)||v>=22&&(0x2b740..=0x2b81d).contains(&cp)||v>=32&&(0x2b820..=0x2cea1).contains(&cp)||v>=36&&(0x2ceb0..=0x2ebe0).contains(&cp)||v>=43&&(0x30000..=0x3134a).contains(&cp);
        let tangut=v>=34&&((0x17000..=if v>=40{0x187f7}else if v>=38{0x187f1}else{0x187ec}).contains(&cp)||(0x18800..=if v>=43{0x18aff}else{0x18af2}).contains(&cp)||v>=43&&(0x18d00..=0x18d08).contains(&cp));
        let nushu=v>=36&&(0x1b170..=0x1b2fb).contains(&cp);let khitan=v>=43&&(0x18b00..=0x18cd5).contains(&cp);
        let base=if tangut{0xfb00}else if nushu{0xfb01}else if khitan{0xfb02}else if basic{0xfb40}else if ext{0xfb80}else{0xfbc0};
        let first=if v==8{0xff80+(cp>>15)}else if tangut||nushu||khitan{base}else{base+(cp>>15)};
        let second=(if tangut{cp-0x17000}else if nushu{cp-0x1b170}else if khitan{cp-0x18b00}else{cp&0x7fff})|0x8000;
        out.push([first as u16,if v==8{2}else{0x20},if v==8{1}else{2},cp as u16,0]);out.push([second as u16,0,0,cp as u16,0]);
    }
    fn normalize<'a>(&self,s:&'a str)->std::borrow::Cow<'a,str> {
        normalization::normalize(s,&self.normalization)
    }
    fn elements(&self,s: &str) -> Vec<Element> {
        let mut chars:Vec<u32>=s.chars().map(|c|c as u32).collect();
        if matches!(self.version,9|11){for cp in &mut chars{if self.find(&[*cp]).is_some_and(|w|w.is_empty()){*cp=u32::MAX;}}}
        let mut i=0;while i+1<chars.len(){if self.rearrange.contains(&chars[i]){chars.swap(i,i+1);i+=1;}i+=1;}
        let mut out=Vec::with_capacity(chars.len());let mut seq=Vec::new();let mut best=Vec::new();let mut i=0;let mut previous_hangul=0;
        while i<chars.len(){let cp=chars[i];if cp==u32::MAX{i+=1;continue;}
            if self.version<=20&&(cp&0xfffe==0xfffe||(0xfdd0..=0xfdef).contains(&cp)){i+=1;continue;}
            let hst=hangul_type(cp);if self.terminator!=0&&previous_hangul!=0&&(hst==0||previous_hangul==1&&hst==3||previous_hangul==2&&hst==1||previous_hangul==3&&matches!(hst,1|2)){out.push([self.terminator,0,0,0,0]);}previous_hangul=hst;
            seq.clear();seq.push(cp);best.clear();best.push(cp);let mut end=i+1;
            let max=self.max_len(cp);
            for p in i+1..chars.len(){if chars[p]==u32::MAX{continue;}if seq.len()>=max{break;}seq.push(chars[p]);if self.find(&seq).is_some(){best.clone_from(&seq);end=p+1;}}
            if max>1&&truth(&self.normalization) {
                let mut previous=0;let mut previous_long=0;let mut long=if self.long{best.clone()}else{Vec::new()};let mut removed=Vec::new();let mut removed_long=Vec::new();
                for p in end..chars.len(){if chars[p]==u32::MAX{continue;}let class=char::from_u32(chars[p]).map(canonical_combining_class).unwrap_or(0);if class==0{break;}
                    best.push(chars[p]);if class!=previous&&self.find(&best).is_some(){removed.push(p);}else{best.pop();previous=class;}
                    if self.long{long.push(chars[p]);if class!=previous_long&&(self.find(&long).is_some()||self.prefix(&long)){removed_long.push(p);}else{long.pop();previous_long=class;}}
                }
                if !removed_long.is_empty()&&self.find(&long).is_some(){best=long;removed=removed_long;}
                for p in removed{chars[p]=u32::MAX;}
            }
            if let Some(weights)=self.find(&best){out.extend_from_slice(weights);}else if !self.implicit_hangul&&(0xac00..=0xd7a3).contains(&cp){let index=cp-0xac00;for j in [0x1100+index/588,0x1161+(index%588)/28,if index%28!=0{0x11a7+index%28}else{u32::MAX}]{if j!=u32::MAX{if let Some(w)=self.find(&[j]){out.extend_from_slice(w);}else{self.implicit(j,&mut out);}}}}else{self.implicit(cp,&mut out);}i=end;
        }
        if self.terminator!=0&&previous_hangul!=0{out.push([self.terminator,0,0,0,0]);}out
    }
    pub(crate) fn key(&self,s: &str)->Vec<u16> {
        let normalized=self.normalize(s);
        let elements=self.elements(&normalized);let mut levels:[Vec<u16>;4]=std::array::from_fn(|_|Vec::with_capacity(elements.len()));let mut last_variable=false;
        for mut w in elements {
            if self.ignore_secondary&&w[0]==0&&w[1]!=0{w[1]=0;w[2]=0;}
            if self.variable!="non-ignorable" {
                if w[4]!=0 {last_variable=true;if self.variable.starts_with('s'){w[3]=w[0];}w[0]=0;w[1]=0;w[2]=0;}
                else {if w[0]!=0{last_variable=false;}else if last_variable&&self.version>=9{continue;}if self.variable=="shifted"{w[3]=if w[0]==1{1}else if self.version>=36&&w[1]==0&&w[2]==0{0}else if w[..3].iter().any(|&x|x!=0){0xffff}else{0};}else if self.variable=="shift-trimmed"{w[3]=0;}}
            }
            if self.upper {w[2]=match w[2]{2..=6=>w[2]+6,8..=12=>w[2]-6,0x1c=>0x1d,0x1d=>0x1c,x=>x};}
            if self.kana {w[2]=match w[2]{0xf..=0x13=>w[2]-2,0xd..=0xe=>w[2]+5,x=>x};}
            for l in 0..self.level {if w[l]!=0{levels[l].push(w[l]);}}
        }
        let mut key=Vec::with_capacity(levels.iter().map(Vec::len).sum::<usize>()+4+s.len()*usize::from(self.identical));
        for (l,weights) in levels.iter_mut().enumerate(){if self.backwards&(1<<(l+1))!=0{weights.reverse();}key.append(weights);if l<3{key.push(0);}}
        if self.identical||self.version>=26&&self.level==4{key.push(0);if self.identical{for cp in normalized.chars().map(|c|c as u32){key.push((cp>>16)as u16);key.push(cp as u16);}}}key
    }
    pub(crate) fn view(&self,s: &str)->String {
        let key=self.key(s);let mut view=String::from("[");let mut first=true;
        for w in key {if w==0{view.push_str(" |");first=true;}else{view.push(' ');let _=write!(view,"{w:04X}");first=false;}}
        if view.as_bytes().get(1)==Some(&b' '){view.remove(1);}if !first{view.push(' ');}view.push(']');view
    }
}
fn hangul_type(cp:u32)->u8 {match cp{0x1100..=0x115f|0xa960..=0xa97c=>1,0x1160..=0x11a7|0xd7b0..=0xd7c6=>2,0x11a8..=0x11ff|0xd7cb..=0xd7fb=>3,0xac00..=0xd7a3=>if (cp-0xac00)%28==0{2}else{3},_=>0}}
fn is_ideograph(cp:u32,v:u8)->bool {
    let bound=if v>=43{0x9ffc}else if v>=38{0x9fef}else if v>=36{0x9fea}else if v>=32{0x9fd5}else if v>=24{0x9fcc}else if v>=20{0x9fcb}else if v>=18{0x9fc3}else if v>=14{0x9fbb}else{0x9fa5};
    (0x4e00..=bound).contains(&cp)||matches!(cp,0xfa0e|0xfa0f|0xfa11|0xfa13|0xfa14|0xfa1f|0xfa21|0xfa23|0xfa24|0xfa27|0xfa28|0xfa29)||(0x3400..=if v>=43{0x4dbf}else{0x4db5}).contains(&cp)||v>=8&&(0x20000..=if v>=43{0x2a6dd}else{0x2a6d6}).contains(&cp)||v>=20&&(0x2a700..=0x2b734).contains(&cp)||v>=22&&(0x2b740..=0x2b81d).contains(&cp)||v>=32&&(0x2b820..=0x2cea1).contains(&cp)||v>=36&&(0x2ceb0..=0x2ebe0).contains(&cp)||v>=43&&(0x30000..=0x3134a).contains(&cp)
}
