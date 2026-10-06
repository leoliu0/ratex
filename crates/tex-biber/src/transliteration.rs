//! The three Lingua::Translit transforms accepted by Biber 2.22.
use crate::model::{Control,Entry};
use crate::perl_regex::Regex;
use std::sync::LazyLock;
include!("transliteration_data.rs");
type Rules=Vec<(Regex,&'static str)>;
fn compile(rows:&'static [(&'static str,&'static str)])->Rules {rows.iter().map(|&(pattern,to)|(Regex::compile(pattern,false).expect("bundled transliteration pattern"),to)).collect()}
static IAST:LazyLock<Rules>=LazyLock::new(||compile(IAST_DEVANAGARI));
static ALA:LazyLock<Rules>=LazyLock::new(||compile(ALA_LC_RUS));
static BGN:LazyLock<Rules>=LazyLock::new(||compile(BGN_PCGN_RUS_STANDARD));
pub(crate) fn apply(c:&Control,e:&Entry,target:&str,value:String)->String {
    // The oracle dispatches internal integers, verbatim/URI fields and verbatim
    // lists without _translit. Only editor types, names and literal/key data enter it.
    if matches!(target,"presort"|"citeorder"|"intciteorder"|"citecount"|"entrykey"|"citekey"|"entrytype"|"labelalpha"|"labelyear"|"labelmonth"|"labelday")||c.fields.get(target).is_some_and(|f|matches!(f.datatype.as_str(),"integer"|"datepart"|"verbatim"|"uri")){return value;}
    let rules=c.type_options.get(&e.kind).and_then(|o|o.get("translit_rules")).or_else(||c.options.get("translit_rules"));
    let Some(rules)=rules else{return value;};
    for rule in rules.lines(){let mut parts=rule.split('\t');let configured_target=parts.next().unwrap_or("");let from=parts.next().unwrap_or("").to_lowercase();let to=parts.next().unwrap_or("").to_lowercase();let langids=parts.next().unwrap_or("\0");
        if langids!="\0"{
            let Some(lang)=e.fields.get("langid").filter(|s|!s.is_empty()&&s.as_str()!="0")else{continue;};
            let folded=crate::perl_unicode::full_fold(lang);let mut ids=langids.split(',').peekable();let mut first=true;let mut matched=false;
            while let Some(id)=ids.next(){let id=if first{id}else{id.trim_start_matches(crate::perl_unicode::is_space)};let id=if ids.peek().is_some(){id.trim_end_matches(crate::perl_unicode::is_space)}else{id};first=false;if crate::perl_unicode::full_fold(id)==folded{matched=true;break;}}
            if !matched{continue;}
        }
        if !configured_target.eq_ignore_ascii_case("*")&&configured_target!=target&&!c.options.get(&format!("datafieldset.{configured_target}")).is_some_and(|set|set.split(',').any(|field|field==target)){continue;}
        let table=match (from.as_str(),to.as_str()){("iast","devanagari")=>&*IAST,("russian","ala-lc")=>&*ALA,("russian","bgn/pcgn-standard")=>&*BGN,_=>return value};
        let mut text=crate::collation::normalization::normalize(&value,"NFC").into_owned();
        for (re,to) in table {if re.inner.is_match(&text).expect("bundled transliteration matching"){text=re.replace_all(&text,to).expect("bundled transliteration replacement");}}
        return text;
    }
    value
}
