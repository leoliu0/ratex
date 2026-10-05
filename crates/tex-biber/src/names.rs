//! BibTeX name parsing and Biber's base LaTeX recoding rules.
use crate::model::{Name, NameList};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;
use regex::Regex;
use std::{borrow::Cow,sync::LazyLock};

const PARTS: [&str; 4] = ["family", "given", "prefix", "suffix"];

pub fn parse(input: &str) -> NameList {
    let decoded = decode_latex(input);
    let mut list = NameList::default();
    for raw in split_names(&decoded) {
        if raw.trim().is_empty() { continue; }
        if let Some((key,value))=raw.split_once('=') {
            let key=key.trim().to_lowercase();let value=value.trim();
            if namelist_option(&key) && !value.chars().any(char::is_whitespace) && !value.contains(',') {
                store_option(&mut list.options,key,if value.is_empty(){"true"}else{value});
                continue;
            }
        }
        let mut name = parse_one(raw.trim());
        if format!("{}{}{}{}",name.family,name.given,name.prefix,name.suffix).to_lowercase()=="others" {list.others=true;continue;}
        name.hash = hash_string(&name_hash_key(&name));
        list.names.push(name);
    }
    let mut key: String = list.names.iter().map(name_hash_key).collect();
    if list.others && list.options.get("nohashothers").is_none_or(|v| v!="true" && v!="1") { key.push('+'); }
    list.hash = hash_string(&key);
    list.fullhash = if list.others && list.options.get("nohashothers").is_some_and(|v|v=="true"||v=="1") {key.push('+');hash_string(&key)}else{list.hash.clone()};
    list.bibnamehash = list.hash.clone();
    list
}

fn split_names(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped { escaped = false; continue; }
        if c == '\\' { escaped = true; continue; }
        if c == '{' { depth += 1; }
        if c == '}' { depth = depth.saturating_sub(1); }
        if depth == 0 && s.get(i..i+3).is_some_and(|v|v.eq_ignore_ascii_case("and")) &&
            (i == 0 || s[..i].chars().next_back().is_some_and(char::is_whitespace)) &&
            (i+3 == s.len() || s[i+3..].chars().next().is_some_and(char::is_whitespace)) {
            result.push(s[start..i].trim()); start = i+3;
        }
    }
    result.push(s[start..].trim()); result
}

fn split_top(s: &str, comma: bool) -> Vec<&str> {
    let mut out = Vec::new(); let mut start = 0; let mut depth = 0usize;
    let mut escaped = false; let mut quoted = false;
    for (i,c) in s.char_indices() {
        if escaped { escaped=false; continue; }
        if c=='\\' { escaped=true; continue; }
        if comma && c=='"' && depth==0 { quoted=!quoted; }
        if c=='{' { depth+=1; } else if c=='}' { depth=depth.saturating_sub(1); }
        if depth==0 && !quoted && (if comma { c==',' } else { c.is_whitespace() || c=='~' }) {
            if i>start { out.push(&s[start..i]); } else if comma { out.push(""); }
            start=i+c.len_utf8();
        }
    }
    if start<s.len() { out.push(&s[start..]); } else if comma {out.push("");}
    out
}

pub fn has_outer(s: &str) -> bool {
    if !s.starts_with('{') || !s.ends_with('}') { return false; }
    let mut depth=0usize; let mut escaped=false;
    for (i,c) in s.char_indices() {
        if escaped {escaped=false;continue;}
        if c=='\\' {escaped=true;continue;}
        if c=='{' {depth+=1;} else if c=='}' {depth=depth.saturating_sub(1);if depth==0 && i+1<s.len(){return false;}}
    }
    depth==0
}

fn lower_token(s: &str) -> bool {
    // Ordinary protected tokens do not contribute to BibTeX's von detection.
    if s.starts_with('{') { return false; }
    s.chars().find(|c| c.is_lowercase() || c.is_uppercase()).is_some_and(char::is_lowercase)
}

pub fn join_name_parts(parts: &[&str]) -> String {
    match parts.len() {
        0=>String::new(), 1=>parts[0].to_owned(), 2=>format!("{}~{}",parts[0],parts[1]),
        n=> { let mut s=parts[0].to_owned(); s.push(if parts[0].graphemes(true).count()<3 {'~'} else {' '}); s.push_str(&parts[1..n-1].join(" "));s.push('~');s.push_str(parts[n-1]);s }
    }
}

fn parse_one(raw: &str) -> Name {
    let mut n=Name::default();
    let raw=raw.replace("\\ ", " ").split_whitespace().collect::<Vec<_>>().join(" ");
    let extended=split_top(&raw,true).iter().any(|p| p.split_once('=').is_some_and(|(k,_)|PARTS.contains(&k.trim()) || PARTS.iter().any(|p|k.trim()==format!("{p}-i"))));
    if extended {
        for item in split_top(&raw,true) {
            let Some((k,v))=item.split_once('=') else {continue};
            let k=k.trim().to_lowercase();let v=v.trim();
            let v=if v.starts_with('"') && v.ends_with('"') && v.len()>1 {&v[1..v.len()-1]}else{v};
            if k=="id" {n.hashid=v.to_owned();}
            else if let Some(part)=k.strip_suffix("-i").filter(|p|PARTS.contains(p)) {n.initial_tokens.insert(part.to_owned(), explicit_initials(v));}
            else if PARTS.contains(&k.as_str()) {
                let protected=has_outer(v);
                let value=if protected {v[1..v.len()-1].to_owned()}else{join_name_parts(&split_top(v,false))};
                set_part(&mut n,&k,value);
                if protected {n.options.insert(format!("protected-{k}"),"true".to_owned());}
            } else {store_option(&mut n.options,k,v);}
        }
    } else {
        let mut whole=raw.as_str();
        while has_outer(whole) && has_outer(&whole[1..whole.len()-1]) {whole=&whole[1..whole.len()-1];}
        let commas=split_top(whole,true);
        if commas.len()==1 {
            let words=split_top(whole,false);let len=words.len();
            if len>0 {
                if let Some(first)=words[..len-1].iter().position(|w|lower_token(w)) {
                    let last=first+words[first..len-1].iter().take_while(|w|lower_token(w)).count()-1;
                    n.given=join_name_parts(&words[..first]);n.prefix=join_name_parts(&words[first..=last]);n.family=join_name_parts(&words[last+1..]);
                }else{n.given=join_name_parts(&words[..len-1]);n.family=words[len-1].to_owned();}
            }
        } else {
            let words=split_top(commas[0].trim(),false);let len=words.len();
            let boundary=if len>0 {words[..len-1].iter().take_while(|w|lower_token(w)).count()}else{0};
            n.prefix=join_name_parts(&words[..boundary]);n.family=join_name_parts(&words[boundary..]);
            if commas.len()>2 {n.suffix=join_name_parts(&split_top(commas[1].trim(),false));n.given=join_name_parts(&split_top(commas[2].trim(),false));}
            else{n.given=join_name_parts(&split_top(commas[1].trim(),false));}
        }
        for part in PARTS {
            let value=get_part(&n,part).to_owned();
            if has_outer(&value) {n.strip.insert(part.to_owned());set_part(&mut n,part,value[1..value.len()-1].to_owned());}
        }
    }
    for part in PARTS {
        let value=get_part(&n,part);
        if value.is_empty(){continue;}
        if !n.initial_tokens.contains_key(part) {
            let cleaned=strip_noinit(value);let value=cleaned.as_ref();
            let protected=n.strip.contains(part) || n.options.contains_key(&format!("protected-{part}"));
            let tokens=if protected && !extended {vec![initial(value)]}
                else if protected { value.split('~').map(initial).collect() }
                else {split_top(value,false).into_iter().map(initial).collect()};
            n.initial_tokens.insert(part.to_owned(),tokens);
        }
        n.initials.insert(part.to_owned(),render_initials(&n.initial_tokens[part]));
    }
    n
}

fn get_part<'a>(n:&'a Name,p:&str)->&'a str {match p {"family"=>&n.family,"given"=>&n.given,"prefix"=>&n.prefix,_=>&n.suffix}}
fn set_part(n:&mut Name,p:&str,v:String){match p {"family"=>n.family=v,"given"=>n.given=v,"prefix"=>n.prefix=v,_=>n.suffix=v}}
fn dash(c:char)->bool {matches!(c,'-'|'\u{2010}'..='\u{2015}'|'\u{2e17}'|'\u{2e1a}'|'\u{2e3a}'|'\u{2e3b}'|'\u{301c}'|'\u{3030}'|'\u{fe31}'|'\u{fe32}'|'\u{fe58}'|'\u{fe63}'|'\u{ff0d}')}
fn initial(s:&str)->String {
    if !has_outer(s) {
        let mut chars=s.chars();let mut previous=chars.next();let mut current=chars.next();
        let mut hyphenated=false;
        for next in chars {
            if previous!=Some('{') && current.is_some_and(dash) && next!='}' {hyphenated=true;break;}
            previous=current;current=Some(next);
        }
        if hyphenated {
            let mut result=String::new();
            for part in s.split(dash){if !result.is_empty(){result.push('-');}result.push_str(&initial(part));}
            return result;
        }
    }
    let unbraced=s.trim_start_matches('{');let mut graphemes=unbraced.graphemes(true);
    let first=graphemes.next().unwrap_or("");
    if DIACRITIC.is_match(first){format!("{}{}",first,graphemes.next().unwrap_or(""))}else{first.to_owned()}
}
fn explicit_initials(s:&str)->Vec<String> {
    let mut out=Vec::new();let mut grouped=String::new();let mut depth=0usize;
    for g in s.graphemes(true) {
        if g=="{" {if depth>0{grouped.push('{');}depth+=1;}
        else if g=="}" && depth>0 {depth-=1;if depth==0{out.push(std::mem::take(&mut grouped));}else{grouped.push('}');}}
        else if depth>0 {grouped.push_str(g);}else{out.push(g.to_owned());}
    }
    if !grouped.is_empty(){out.push(grouped);}out
}
pub fn render_initials(tokens:&[String])->String {
    if tokens.is_empty(){return String::new();}
    let mut s=tokens.join("\\bibinitperiod\\bibinitdelim ");s.push_str("\\bibinitperiod");
    s.chars().fold(String::new(),|mut out,c|{if dash(c){out.push_str("\\bibinithyphendelim ");}else{out.push(c);}out})
}
pub fn name_hash_key(n:&Name)->String {
    if !n.hashid.is_empty(){return n.hashid.clone();}
    format!("{}{}{}{}",n.family,n.given,n.prefix,n.suffix)
}
pub fn hash_string(s:&str)->String {
    let mut normalized=String::with_capacity(s.len());let mut chars=s.chars().peekable();
    while let Some(c)=chars.next(){
        if c=='\\' {
            let mut macro_name=String::new();while chars.peek().is_some_and(|c|c.is_alphabetic()){macro_name.push(chars.next().unwrap());}
            if !macro_name.is_empty(){normalized.push_str(&macro_name);normalized.push(':');}
            else if let Some(c)=chars.next(){normalized.push_str(&format!("{}:",c as u32));}
            while chars.peek().is_some_and(|c|c.is_whitespace()){chars.next();}
        }else if !matches!(c,'{'|'}'|'~'|'.')&&!c.is_whitespace(){normalized.push(c);}
    }
    crate::md5_hex(normalized.nfc().collect::<String>().as_bytes())
}

/// Convert the default Biber `base` macro set; unknown TeX commands survive.
/// NFC here is the output form (Biber internally uses NFD).
pub fn decode_latex(input:&str)->String {decode_segment(input).0.replace("ı\u{301}","í").nfc().collect()}
fn decode_segment(s:&str)->(String,bool) {
    let mut out=String::with_capacity(s.len());let mut changed=false;let mut i=0;let mut macro_args=false;
    while i<s.len(){
        let c=s[i..].chars().next().unwrap();
        if c=='{' {
            if let Some(end)=group_end(s,i){let inner=&s[i+1..end];let (decoded,did)=decode_segment(inner);
                if did && !macro_args && decoded.graphemes(true).count()==1 {out.push_str(&decoded);}else{out.push('{');out.push_str(&decoded);out.push('}');}
                changed|=did;i=end+1;continue;
            }
        }
        if c!='\\' {out.push(c);macro_args=false;i+=c.len_utf8();continue;}
        let start=i;i+=1;if i==s.len(){out.push('\\');break;}
        let next=s[i..].chars().next().unwrap();let command;
        if next.is_ascii_alphabetic(){let begin=i;while i<s.len() && s.as_bytes()[i].is_ascii_alphabetic(){i+=1;}command=&s[begin..i];}
        else{command=&s[i..i+next.len_utf8()];i+=next.len_utf8();}
        macro_args=false;
        if command=="char" {
            let mut radix=10;if s[i..].starts_with('"'){radix=16;i+=1;}else if s[i..].starts_with('\''){radix=8;i+=1;}
            let begin=i;while i<s.len()&&s.as_bytes()[i].is_ascii_hexdigit()&&(radix==16||s.as_bytes()[i].is_ascii_digit()){i+=1;}
            if let Ok(v)=u32::from_str_radix(&s[begin..i],radix){if let Some(c)=char::from_u32(v){out.push(c);changed=true;continue;}}
            out.push_str(&s[start..i]);continue;
        }
        if let Some(mark)=accent_macro(command) {
            let after=i;while i<s.len()&&s[i..].chars().next().unwrap().is_whitespace(){i+=s[i..].chars().next().unwrap().len_utf8();}
            let arg;
            if i<s.len()&&s[i..].starts_with('{') {
                if let Some(end)=group_end(s,i){arg=decode_segment(&s[i+1..end]).0;i=end+1;}else{i=after;out.push_str(&s[start..i]);continue;}
            }else if i<s.len(){let c=s[i..].chars().next().unwrap();if !c.is_alphabetic(){i=after;out.push_str(&s[start..i]);continue;}arg=c.to_string();i+=c.len_utf8();}
            else{i=after;out.push_str(&s[start..i]);continue;}
            if arg.graphemes(true).count()!=1 || !arg.chars().next().is_some_and(char::is_alphabetic) {out.push_str(&s[start..i]);continue;}
            out.push_str(&arg);out.push_str(mark);changed=true;continue;
        }
        if let Some(value)=letter_macro(command) {
            out.push_str(value);changed=true;
            if s[i..].starts_with("{}"){i+=2;}else{while i<s.len()&&s[i..].chars().next().unwrap().is_whitespace(){i+=s[i..].chars().next().unwrap().len_utf8();}}
            continue;
        }
        out.push_str(&s[start..i]);macro_args=command.chars().all(|c|c.is_alphabetic());
    }
    (out,changed)
}
fn group_end(s:&str,start:usize)->Option<usize>{let mut depth=0usize;let mut escaped=false;for (j,c) in s[start..].char_indices(){if escaped{escaped=false;continue;}if c=='\\'{escaped=true;continue;}if c=='{'{depth+=1;}else if c=='}'{depth-=1;if depth==0{return Some(start+j);}}}None}

// Mapping data ported from Biber 2.22 recode_data.xml (base decode set).
fn letter_macro(name: &str) -> Option<&'static str> {
    match name {
        "AA" => Some("Å"),
        "AE" => Some("Æ"),
        "DH" => Some("Ð"),
        "O" => Some("Ø"),
        "Thorn" => Some("Þ"),
        "TH" => Some("Þ"),
        "ss" => Some("ß"),
        "aa" => Some("å"),
        "ae" => Some("æ"),
        "dh" => Some("ð"),
        "o" => Some("ø"),
        "textthorn" => Some("þ"),
        "textthornvari" => Some("þ"),
        "textthornvarii" => Some("þ"),
        "textthornvariii" => Some("þ"),
        "textthornvariv" => Some("þ"),
        "th" => Some("þ"),
        "DJ" => Some("Đ"),
        "dj" => Some("đ"),
        "textcrd" => Some("đ"),
        "textHbar" => Some("Ħ"),
        "textcrh" => Some("ħ"),
        "texthbar" => Some("ħ"),
        "i" => Some("ı"),
        "j" => Some("ȷ"),
        "IJ" => Some("Ĳ"),
        "ij" => Some("ĳ"),
        "textkra" => Some("ĸ"),
        "L" => Some("Ł"),
        "textbarl" => Some("ł"),
        "l" => Some("ł"),
        "NG" => Some("Ŋ"),
        "ng" => Some("ŋ"),
        "OE" => Some("Œ"),
        "oe" => Some("œ"),
        "textTbar" => Some("Ŧ"),
        "textTstroke" => Some("Ŧ"),
        "texttbar" => Some("ŧ"),
        "texttstroke" => Some("ŧ"),
        "textcrb" => Some("ƀ"),
        "textBhook" => Some("Ɓ"),
        "textOopen" => Some("Ɔ"),
        "textChook" => Some("Ƈ"),
        "textchook" => Some("ƈ"),
        "texthtc" => Some("ƈ"),
        "textDafrican" => Some("Ɖ"),
        "textDhook" => Some("Ɗ"),
        "textEreversed" => Some("Ǝ"),
        "textEopen" => Some("Ɛ"),
        "textFhook" => Some("Ƒ"),
        "textflorin" => Some("ƒ"),
        "textGammaafrican" => Some("Ɣ"),
        "texthvlig" => Some("ƕ"),
        "hv" => Some("ƕ"),
        "textIotaafrican" => Some("Ɩ"),
        "textKhook" => Some("Ƙ"),
        "textkhook" => Some("ƙ"),
        "texthtk" => Some("ƙ"),
        "textcrlambda" => Some("ƛ"),
        "textNhookleft" => Some("Ɲ"),
        "OHORN" => Some("Ơ"),
        "ohorn" => Some("ơ"),
        "textPhook" => Some("Ƥ"),
        "textphook" => Some("ƥ"),
        "texthtp" => Some("ƥ"),
        "textEsh" => Some("Ʃ"),
        "ESH" => Some("Ʃ"),
        "textlooptoprevesh" => Some("ƪ"),
        "textpalhookbelow" => Some("ƫ"),
        "textThook" => Some("Ƭ"),
        "textthook" => Some("ƭ"),
        "texthtt" => Some("ƭ"),
        "textTretroflexhook" => Some("Ʈ"),
        "UHORN" => Some("Ư"),
        "uhorn" => Some("ư"),
        "textVhook" => Some("Ʋ"),
        "textYhook" => Some("Ƴ"),
        "textyhook" => Some("ƴ"),
        "textEzh" => Some("Ʒ"),
        "texteturned" => Some("ǝ"),
        "textturna" => Some("ɐ"),
        "textscripta" => Some("ɑ"),
        "textturnscripta" => Some("ɒ"),
        "textbhook" => Some("ɓ"),
        "texthtb" => Some("ɓ"),
        "textoopen" => Some("ɔ"),
        "textopeno" => Some("ɔ"),
        "textctc" => Some("ɕ"),
        "textdtail" => Some("ɖ"),
        "textrtaild" => Some("ɖ"),
        "textdhook" => Some("ɗ"),
        "texthtd" => Some("ɗ"),
        "textreve" => Some("ɘ"),
        "textschwa" => Some("ə"),
        "textrhookschwa" => Some("ɚ"),
        "texteopen" => Some("ɛ"),
        "textepsilon" => Some("ɛ"),
        "textrevepsilon" => Some("ɜ"),
        "textrhookrevepsilon" => Some("ɝ"),
        "textcloserevepsilon" => Some("ɞ"),
        "textbardotlessj" => Some("ɟ"),
        "texthtg" => Some("ɠ"),
        "textscriptg" => Some("ɡ"),
        "textscg" => Some("ɢ"),
        "textgammalatinsmall" => Some("ɣ"),
        "textgamma" => Some("γ"),
        "textramshorns" => Some("ɤ"),
        "textturnh" => Some("ɥ"),
        "texthth" => Some("ɦ"),
        "texththeng" => Some("ɧ"),
        "textbari" => Some("ɨ"),
        "textiotalatin" => Some("ɩ"),
        "textiota" => Some("ι"),
        "textsci" => Some("ɪ"),
        "textltilde" => Some("ɫ"),
        "textbeltl" => Some("ɬ"),
        "textrtaill" => Some("ɭ"),
        "textlyoghlig" => Some("ɮ"),
        "textturnm" => Some("ɯ"),
        "textturnmrleg" => Some("ɰ"),
        "textltailm" => Some("ɱ"),
        "textltailn" => Some("ɲ"),
        "textnhookleft" => Some("ɲ"),
        "textrtailn" => Some("ɳ"),
        "textscn" => Some("ɴ"),
        "textbaro" => Some("ɵ"),
        "textscoelig" => Some("ɶ"),
        "textcloseomega" => Some("ɷ"),
        "textphi" => Some("ɸ"),
        "textturnr" => Some("ɹ"),
        "textturnlonglegr" => Some("ɺ"),
        "textturnrrtail" => Some("ɻ"),
        "textlonglegr" => Some("ɼ"),
        "textrtailr" => Some("ɽ"),
        "textfishhookr" => Some("ɾ"),
        "textlhti" => Some("ɿ"),
        "textscr" => Some("ʀ"),
        "textinvscr" => Some("ʁ"),
        "textrtails" => Some("ʂ"),
        "textesh" => Some("ʃ"),
        "texthtbardotlessj" => Some("ʄ"),
        "textraisevibyi" => Some("ʅ"),
        "textctesh" => Some("ʆ"),
        "textturnt" => Some("ʇ"),
        "textrtailt" => Some("ʈ"),
        "texttretroflexhook" => Some("ʈ"),
        "textbaru" => Some("ʉ"),
        "textupsilon" => Some("υ"),
        "textscriptv" => Some("ʋ"),
        "textvhook" => Some("ʋ"),
        "textturnv" => Some("ʌ"),
        "textturnw" => Some("ʍ"),
        "textturny" => Some("ʎ"),
        "textscy" => Some("ʏ"),
        "textrtailz" => Some("ʐ"),
        "textctz" => Some("ʑ"),
        "textezh" => Some("ʒ"),
        "textyogh" => Some("ʒ"),
        "textctyogh" => Some("ʓ"),
        "textglotstop" => Some("ʔ"),
        "textrevglotstop" => Some("ʕ"),
        "textinvglotstop" => Some("ʖ"),
        "textstretchc" => Some("ʗ"),
        "textbullseye" => Some("ʘ"),
        "textscb" => Some("ʙ"),
        "textcloseepsilon" => Some("ʚ"),
        "texthtscg" => Some("ʛ"),
        "textsch" => Some("ʜ"),
        "textctj" => Some("ʝ"),
        "textturnk" => Some("ʞ"),
        "textscl" => Some("ʟ"),
        "texthtq" => Some("ʠ"),
        "textbarglotstop" => Some("ʡ"),
        "textbarrevglotstop" => Some("ʢ"),
        "textdzlig" => Some("ʣ"),
        "textdyoghlig" => Some("ʤ"),
        "textdctzlig" => Some("ʥ"),
        "texttslig" => Some("ʦ"),
        "textteshlig" => Some("ʧ"),
        "texttesh" => Some("ʧ"),
        "texttctclig" => Some("ʨ"),
        "hamza" => Some("ʾ"),
        "ain" => Some("ʿ"),
        "ayn" => Some("ʿ"),
        "textprimstress" => Some("ˈ"),
        "textlengthmark" => Some("ː"),
        "textendash" => Some("–"),
        "--" => Some("–"),
        "textemdash" => Some("—"),
        "---" => Some("—"),
        "textquoteleft" => Some("‘"),
        "textquoteright" => Some("’"),
        "quotesinglbase" => Some("‚"),
        "textquotedblleft" => Some("“"),
        "textquotedblright" => Some("”"),
        "quotedblbase" => Some("„"),
        "dag" => Some("†"),
        "ddag" => Some("‡"),
        "textbullet" => Some("•"),
        "textperthousand" => Some("‰"),
        "textpertenthousand" => Some("‱"),
        "guilsinglleft" => Some("‹"),
        "guilsinglright" => Some("›"),
        "textreferencemark" => Some("※"),
        "textinterrobang" => Some("‽"),
        "textoverline" => Some("‾"),
        "langle" => Some("⟨"),
        "rangle" => Some("⟩"),
        "textquotedbl" => Some("\""),
        "textdollar" => Some("$"),
        "textpercent" => Some("%"),
        "textampersand" => Some("&"),
        "textquotesingle" => Some("'"),
        "textasteriskcentered" => Some("*"),
        "textless" => Some("<"),
        "textequals" => Some("="),
        "textgreater" => Some(">"),
        "textbar" => Some("|"),
        "nobreakspace" => Some(" "),
        "textexclamdown" => Some("¡"),
        "textcent" => Some("¢"),
        "textsterling" => Some("£"),
        "pounds" => Some("£"),
        "textcurrency" => Some("¤"),
        "textyen" => Some("¥"),
        "textbrokenbar" => Some("¦"),
        "textsection" => Some("§"),
        "S" => Some("§"),
        "textasciidieresis" => Some("¨"),
        "textcopyright" => Some("©"),
        "copyright" => Some("©"),
        "textordfeminine" => Some("ª"),
        "guillemotleft" => Some("«"),
        "textminus" => Some("−"),
        "textregistered" => Some("®"),
        "textasciimacron" => Some("¯"),
        "textdegree" => Some("°"),
        "texttwosuperior" => Some("²"),
        "textthreesuperior" => Some("³"),
        "textasciiacute" => Some("´"),
        "textparagraph" => Some("¶"),
        "P" => Some("¶"),
        "textcentereddot" => Some("·"),
        "textperiodcentered" => Some("·"),
        "textasciicedilla" => Some("¸"),
        "textonesuperior" => Some("¹"),
        "textordmasculine" => Some("º"),
        "guillemotright" => Some("»"),
        "textonequarter" => Some("¼"),
        "textonehalf" => Some("½"),
        "textthreequarters" => Some("¾"),
        "textquestiondown" => Some("¿"),
        _ => None,
    }
}
fn accent_macro(name: &str) -> Option<&'static str> {
    match name {
        "`" => Some("̀"),
        "'" => Some("́"),
        "^" => Some("̂"),
        "~" => Some("̃"),
        "=" => Some("̄"),
        "u" => Some("̆"),
        "." => Some("̇"),
        "\"" => Some("̈"),
        "h" => Some("̉"),
        "r" => Some("̊"),
        "H" => Some("̋"),
        "v" => Some("̌"),
        "U" => Some("̎"),
        "G" => Some("̏"),
        "M" => Some("̢"),
        "d" => Some("̣"),
        "c" => Some("̧"),
        "k" => Some("̨"),
        "b" => Some("̱"),
        "B" => Some("̵"),
        "t" => Some("̑"),
        "textvbaraccent" => Some("̍"),
        "textdoublevbaraccent" => Some("̎"),
        "textdotbreve" => Some("̐"),
        "textturncommaabove" => Some("̒"),
        "textcommaabove" => Some("̓"),
        "textrevcommaabove" => Some("̔"),
        "textcommaabover" => Some("̕"),
        "textsubgrave" => Some("̖"),
        "textsubacute" => Some("̗"),
        "textadvancing" => Some("̘"),
        "textretracting" => Some("̙"),
        "textlangleabove" => Some("̚"),
        "textrighthorn" => Some("̛"),
        "textsublhalfring" => Some("̜"),
        "textraising" => Some("̝"),
        "textlowering" => Some("̞"),
        "textsubplus" => Some("̟"),
        "textsubbar" => Some("̠"),
        "textsubminus" => Some("̠"),
        "textpalhookbelow" => Some("̡"),
        "textsubumlaut" => Some("̤"),
        "textsubring" => Some("̥"),
        "textcommabelow" => Some("̦"),
        "textsyllabic" => Some("̩"),
        "textsubbridge" => Some("̪"),
        "textsubw" => Some("̫"),
        "textsubwedge" => Some("̬"),
        "textsubcircnum" => Some("̭"),
        "textsubbreve" => Some("̮"),
        "textundertie" => Some("̮"),
        "textsubarch" => Some("̯"),
        "textsubtilde" => Some("̰"),
        "subdoublebar" => Some("͇"),
        "textsuperimposetilde" => Some("̴"),
        "textlstrokethru" => Some("̶"),
        "textsstrikethru" => Some("̷"),
        "textlstrikethru" => Some("̸"),
        "textsubrhalfring" => Some("̹"),
        "textinvsubbridge" => Some("̺"),
        "textsubsquare" => Some("̻"),
        "textseagull" => Some("̼"),
        "textovercross" => Some("̽"),
        "overbridge" => Some("͆"),
        "subdoublevert" => Some("͈"),
        "subcorner" => Some("͉"),
        "crtilde" => Some("͊"),
        "textoverw" => Some("͊"),
        "dottedtilde" => Some("͋"),
        "doubletilde" => Some("͌"),
        "spreadlips" => Some("͍"),
        "whistle" => Some("͎"),
        "textrightarrowhead" => Some("͐"),
        "textlefthalfring" => Some("͑"),
        "sublptr" => Some("͔"),
        "subrptr" => Some("͕"),
        "textrightuparrowhead" => Some("͖"),
        "textrighthalfring" => Some("͗"),
        "textdoublebreve" => Some("͝"),
        "textdoublemacron" => Some("͞"),
        "textdoublemacronbelow" => Some("͟"),
        "textdoubletilde" => Some("͠"),
        "texttoptiebar" => Some("͡"),
        "sliding" => Some("͢"),
        _ => None,
    }
}
fn namelist_option(k:&str)->bool{
    matches!(k,"nametemplates"|"sortingnamekeytemplatename"|"uniquenametemplatename"|"labelalphanametemplatename"|"namehashtemplatename"|"uniquelist"|"uniquename"|"familyinits"|"giveninits"|"prefixinits"|"suffixinits"|"terseinits"|"nohashothers"|"nosortothers"|"useprefix")
}
fn store_option(options:&mut std::collections::BTreeMap<String,String>,key:String,value:&str){
    if key=="nametemplates" {
        for k in ["sortingnamekeytemplatename","uniquenametemplatename","labelalphanametemplatename","namehashtemplatename"] {options.insert(k.to_owned(),value.to_owned());}
    }else{options.insert(key,value.to_owned());}
}
static DIACRITIC:LazyLock<Regex>=LazyLock::new(||Regex::new(r"^\p{Diacritic}").unwrap());
static NOINIT_PREFIX:LazyLock<Regex>=LazyLock::new(||Regex::new(r"\b\p{Ll}{2}\p{Pd}").unwrap());
static NOINIT_MACRO:LazyLock<Regex>=LazyLock::new(||Regex::new(r"\\[A-Za-z]+\s*(?:\{([^}]*)\})?").unwrap());
fn strip_noinit(s:&str)->Cow<'_,str>{
    let mut out=String::new();let mut last=0;
    for m in NOINIT_PREFIX.find_iter(s){
        if s[m.end()..].chars().next().is_some_and(|c|!c.is_whitespace()){
            out.push_str(&s[last..m.start()]);last=m.end();
        }
    }
    let mut value=if last>0{out.push_str(&s[last..]);Cow::Owned(out)}else{Cow::Borrowed(s)};
    if value.contains(['\u{2bf}','\u{2018}']){value=Cow::Owned(value.replace(['\u{2bf}','\u{2018}'],""));}
    let replaced=NOINIT_MACRO.replace_all(&value,"$1");
    if matches!(replaced,Cow::Borrowed(_)){drop(replaced);value}else{Cow::Owned(replaced.into_owned())}
}
