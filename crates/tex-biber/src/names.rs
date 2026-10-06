//! BibTeX name parsing and Biber's base LaTeX recoding rules.
use crate::model::{namelist_id, Control, Name, NameList};
use crate::collation::normalization::normalize;
use regex::Regex;
use std::{borrow::Cow,sync::LazyLock};

const PARTS: [&str; 4] = ["family", "given", "prefix", "suffix"];

pub fn parse(control: &Control, input: &str) -> NameList {
    let mut list = NameList { id: namelist_id(), ..NameList::default() };
    let separator=control.options.get("xnamesep").map(String::as_str).unwrap_or("=");
    let marker=control.options.get("xdatamarker").map(String::as_str).unwrap_or("xdata");
    for raw in split_words(input,control.options.get("namesep").filter(|s|!s.is_empty()).map(String::as_str).unwrap_or("and")) {
        if raw.trim().is_empty() { continue; }
        if raw.trim().split_once(separator).is_some_and(|(key,_)|key.eq_ignore_ascii_case(marker)) {
            let mut name=Name::default();name.options.insert("xdata".into(),raw.trim().into());list.names.push(name);continue;
        }
        if let Some((key,value))=raw.split_once(separator) {
            let key=key.trim().to_lowercase();let value=value.trim();
            if (namelist_option(&key)||control.options.contains_key(&format!("optionscope.NAMELIST.{key}.datatype"))) && !value.chars().any(char::is_whitespace) && !value.contains(',') {
                store_option(control,"NAMELIST",&mut list.options,key,if value.is_empty(){"true"}else{value});
                continue;
            }
        }
        let mut name = parse_one(control, raw.trim());
        if format!("{}{}{}{}",name.family,name.given,name.prefix,name.suffix).to_lowercase()==control.options.get("others_string").map(String::as_str).unwrap_or("others") {list.others=true;continue;}
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

pub(crate) fn split_words<'a>(s: &'a str,separator:&str) -> Vec<&'a str> {
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped { escaped = false; continue; }
        if c == '\\' { escaped = true; continue; }
        if c == '{' { depth += 1; }
        if c == '}' { depth = depth.saturating_sub(1); }
        if !separator.is_empty() && depth == 0 && s.get(i..i+separator.len()).is_some_and(|v|v.eq_ignore_ascii_case(separator)) &&
            (i == 0 || s[..i].chars().next_back().is_some_and(char::is_whitespace)) &&
            (i+separator.len() == s.len() || s[i+separator.len()..].chars().next().is_some_and(char::is_whitespace)) {
            result.push(s[start..i].trim()); start = i+separator.len();
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
        n=> { let mut s=parts[0].to_owned(); s.push(if crate::gcstring::length(parts[0])<3 {'~'} else {' '}); s.push_str(&parts[1..n-1].join(" "));s.push('~');s.push_str(parts[n-1]);s }
    }
}

fn parse_one(control: &Control, raw: &str) -> Name {
    let mut n=Name::default();
    let raw=raw.replace("\\ ", " ").split_whitespace().collect::<Vec<_>>().join(" ");
    let parts:Vec<&str>=if control.nameparts.is_empty(){PARTS.to_vec()}else{control.nameparts.iter().map(String::as_str).collect()};
    let separator=control.options.get("xnamesep").map(String::as_str).unwrap_or("=");
    let extended=!control.options.get("noxname").is_some_and(|s|matches!(s.as_str(),"1"|"true"))&&split_top(&raw,true).iter().any(|p| p.split_once(separator).is_some_and(|(k,_)|parts.contains(&k.trim()) || parts.iter().any(|p|k.trim()==format!("{p}-i"))));
    if extended {
        for item in split_top(&raw,true) {
            let Some((k,v))=item.split_once(separator) else {continue};
            let k=k.trim().to_lowercase();let v=v.trim();
            let v=if v.starts_with('"') && v.ends_with('"') && v.len()>1 {&v[1..v.len()-1]}else{v};
            if k=="id" {n.hashid=v.to_owned();}
            else if let Some(part)=k.strip_suffix("-i").filter(|p|parts.contains(p)) {n.initial_tokens.insert(part.to_owned(), explicit_initials(v));}
            else if parts.contains(&k.as_str()) {
                let protected=has_outer(v);
                let value=if protected {v[1..v.len()-1].to_owned()}else{join_name_parts(&split_top(v,false))};
                set_part(&mut n,&k,value);
                if protected {n.options.insert(format!("protected-{k}"),"true".to_owned());}
            } else {store_option(control,"NAME",&mut n.options,k,v);}
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
        for &part in &parts {
            let value=get_part(&n,part).to_owned();
            if has_outer(&value) {n.strip.insert(part.to_owned());set_part(&mut n,part,value[1..value.len()-1].to_owned());}
        }
    }
    for &part in &parts {
        let value=get_part(&n,part);
        if value.is_empty(){continue;}
        if !n.initial_tokens.contains_key(part) {
            let protected=n.strip.contains(part) || n.options.contains_key(&format!("protected-{part}"));
            let initial_input=if protected {
                let mut input=String::with_capacity(value.len()+usize::from(!extended)*2);
                if !extended{input.push('{');}
                input.extend(value.chars().map(|c|if crate::perl_unicode::is_space(c){'_'}else{c}));
                if !extended{input.push('}');}
                Cow::Owned(input)
            }else{Cow::Borrowed(value)};
            let cleaned=strip_initial(control,&initial_input);let value=cleaned.as_ref();
            let tokens=if protected && !extended {vec![initial_grapheme(value)]}
                else if protected { value.split('~').map(initial).collect() }
                else {split_top(value,false).into_iter().map(initial).collect()};
            n.initial_tokens.insert(part.to_owned(),tokens);
        }
        n.initials.insert(part.to_owned(),render_initials(&n.initial_tokens[part]));
    }
    n
}

fn get_part<'a>(n:&'a Name,p:&str)->&'a str {n.part(p)}
fn set_part(n:&mut Name,p:&str,v:String){n.set_part(p,v)}
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
    initial_grapheme(s)
}
/// Utils::gen_initials leaf: first GCString cluster, two when it is `\p{Dia}`.
fn initial_grapheme(s:&str)->String {
    let unbraced=s.trim_start_matches('{');
    let first=crate::gcstring::prefix(unbraced,1);
    if first.chars().next().is_some_and(|c|crate::perl_unicode::property_contains("Dia",c).unwrap_or(false)){crate::gcstring::prefix(unbraced,2).to_owned()}else{first.to_owned()}
}
/// bibtex.pm `_split_initials`: Perl `\b{gcb}` pieces; braces toggle (not nest) a compound initial.
fn explicit_initials(s:&str)->Vec<String> {
    let mut out=Vec::new();let mut compound=String::new();let mut inside=false;
    for g in crate::perl_unicode::graphemes(s) {
        if g=="{" {inside=true;}
        else if g=="}" {inside=false;out.push(std::mem::take(&mut compound));}
        else if inside {compound.push_str(g);}else{out.push(g.to_owned());}
    }
    out
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
pub fn hash_string(s:&str)->String {crate::process::digest(&crate::process::hash_normalize(s))}

/// Biber 2.22's default `base` decode set, in Recode.pm substitution order.
/// The temporary brace markers distinguish literal protection from accent
/// grouping. Decoding a plain letter macro never removes its enclosing braces.
/// NFD is the output form, as Recode's default `normalization`.
pub fn decode_latex(input:&str)->String {
    let text=BOX_SPACE.replace_all(input, |c:&regex::Captures| {
        if crate::perl_unicode::graphemes(&c[2]).count()==1 {format!("\\{}{{{}}}",&c[1],&c[2])}else{c[0].to_owned()}
    });
    let text=CHAR_MACRO.replace_all(&text, |c:&regex::Captures| {
        let (digits,radix)=if let Some(v)=c.get(1){(v.as_str(),16)}else if let Some(v)=c.get(2){(v.as_str(),8)}else{(c.get(3).unwrap().as_str(),10)};
        u32::from_str_radix(digits,radix).ok().and_then(char::from_u32).map(|v|v.to_string()).unwrap_or_else(||c[0].to_owned())
    });
    let text=CONTROL_SPACE.replace_all(&text, r"$1{}$2");
    let text=CONTROL_PUNCT.replace_all(&text, r"$1{}$2");
    let text=decode_plain_macros(&text,false);
    let text=decode_plain_macros(&text,true);
    // Explicit single-grapheme braces are protected before accents are decoded.
    // Only a brace immediately following an accent command is an argument.
    let text=SIMPLE_GROUP.replace_all(&text, |c:&regex::Captures| {
        let start=c.get(0).unwrap().start();
        if crate::perl_unicode::graphemes(&c[1]).count()==1 && !accent_before(&text[..start]) {
            format!("\u{f}{}\u{e}",&c[1])
        }else{c[0].to_owned()}
    });
    let text=BRACED_ACCENT.replace_all(&text, |c:&regex::Captures| {
        if let Some(mark)=accent_macro(&c[2]) {
            format!("{}{}{}{}",if c[1].is_empty(){""}else{"\u{1f}"},&c[3],mark,if c[4].is_empty(){""}else{"\u{1e}"})
        }else{c[0].to_owned()}
    });
    let text=LETTER_GROUP.replace_all(&text, "\u{1f}$1\u{1e}");
    let text=decode_unbraced_accents(&text);
    let mut out=String::with_capacity(text.len());
    let mut i=0;
    while i<text.len() {
        let ch=text[i..].chars().next().unwrap();
        if matches!(ch,'{'|'\u{1f}') && !MACRO_ARGUMENT.is_match(&text[..i]) {
            let start=i+ch.len_utf8();
            if let Some(g)=crate::perl_unicode::graphemes(&text[start..]).next() {
                let end=start+g.len();
                if text[end..].starts_with(['}','\u{1e}']) {
                    out.push_str(g);i=end+1;continue;
                }
            }
        }
        out.push(ch);i+=ch.len_utf8();
    }
    normalize(&out.replace(['\u{1f}','\u{f}'],"{").replace(['\u{1e}','\u{e}'],"}").replace("ı\u{301}","í"),"NFD").into_owned()
}

static BOX_SPACE:LazyLock<Regex>=LazyLock::new(||Regex::new(r"\\(h?box)\s+\{([^{}]+)\}").unwrap());
static CHAR_MACRO:LazyLock<Regex>=LazyLock::new(||Regex::new(r#"\\char(?:"([0-9A-Fa-f]+)|'([0-9]+)|([0-9]+))"#).unwrap());
static CONTROL_SPACE:LazyLock<Regex>=LazyLock::new(||Regex::new(r"(\\[a-zA-Z]+)\\(\s+)").unwrap());
static CONTROL_PUNCT:LazyLock<Regex>=LazyLock::new(||Regex::new(r"([^{]\\\w)([;,.:%])").unwrap());
static PLAIN_MACRO:LazyLock<Regex>=LazyLock::new(||Regex::new(r"\\([A-Za-z]+|---|--)").unwrap());
static WORD_START:LazyLock<Regex>=LazyLock::new(||Regex::new(r"^\w").unwrap());
static SIMPLE_GROUP:LazyLock<Regex>=LazyLock::new(||Regex::new(r"\{([^{}]*)\}").unwrap());
static ACCENT_COMMAND:LazyLock<Regex>=LazyLock::new(||Regex::new(r#"\\([A-Za-z]+|[`'^~=."])$"#).unwrap());
static BRACED_ACCENT:LazyLock<Regex>=LazyLock::new(||Regex::new(r#"(\{?)\\([A-Za-z]+|[`'^~=."])\s*\{(\pL\pM*)\}(\}?)"#).unwrap());
static LETTER_GROUP:LazyLock<Regex>=LazyLock::new(||Regex::new(r"\{(\pL\pM*)\}").unwrap());
static LETTER_START:LazyLock<Regex>=LazyLock::new(||Regex::new(r"^\pL").unwrap());
static MARK_START:LazyLock<Regex>=LazyLock::new(||Regex::new(r"^\pM*").unwrap());
static MACRO_ARGUMENT:LazyLock<Regex>=LazyLock::new(||Regex::new(r"\\\pL+(?:\{[^{]+\})*$").unwrap());

fn accent_before(prefix:&str)->bool {
    ACCENT_COMMAND.captures(prefix).is_some_and(|c|accent_macro(&c[1]).is_some())
}

fn plain_letter(name:&str)->bool {
    // Punctuation and symbols have a subtly different terminator from letters:
    // their empty argument is consumed only when preceded by a literal space.
    !matches!(name,"--"|"---"|"textendash"|"textemdash"|"textquoteleft"|"textquoteright"|"quotesinglbase"|"textquotedblleft"|"textquotedblright"|"quotedblbase"|"dag"|"ddag"|"textbullet"|"textperthousand"|"textpertenthousand"|"guilsinglleft"|"guilsinglright"|"textreferencemark"|"textinterrobang"|"textoverline"|"langle"|"rangle"|"textquotedbl"|"textdollar"|"textpercent"|"textampersand"|"textquotesingle"|"textasteriskcentered"|"textless"|"textequals"|"textgreater"|"textbar"|"nobreakspace"|"textexclamdown"|"textcent"|"textsterling"|"pounds"|"textcurrency"|"textyen"|"textbrokenbar"|"textsection"|"S"|"textasciidieresis"|"textcopyright"|"copyright"|"textordfeminine"|"guillemotleft"|"textminus"|"textregistered"|"textasciimacron"|"textdegree"|"texttwosuperior"|"textthreesuperior"|"textasciiacute"|"textparagraph"|"P"|"textcentereddot"|"textperiodcentered"|"textasciicedilla"|"textonesuperior"|"textordmasculine"|"guillemotright"|"textonequarter"|"textonehalf"|"textthreequarters"|"textquestiondown")
}

fn decode_plain_macros(text:&str,letters:bool)->String {
    let mut out=String::with_capacity(text.len());
    let mut last=0;
    for c in PLAIN_MACRO.captures_iter(text) {
        let m=c.get(0).unwrap();
        if m.start()<last || plain_letter(&c[1])!=letters {continue;}
        let Some(value)=letter_macro(&c[1]) else{continue;};
        let rest=&text[m.end()..];
        let mut consumed=0;
        if letters && rest.starts_with("{}"){consumed=2;}
        else if !letters && rest.starts_with(" {}"){consumed=3;}
        else {
            for ch in rest.chars(){if !ch.is_whitespace(){break;}consumed+=ch.len_utf8();}
            if consumed==0 {
                let left=WORD_START.is_match(&c[1][c[1].len()-1..]);
                if left==WORD_START.is_match(rest){continue;}
            }
        }
        out.push_str(&text[last..m.start()]);out.push_str(value);last=m.end()+consumed;
    }
    out.push_str(&text[last..]);out
}

fn decode_unbraced_accents(text:&str)->String {
    let mut out=String::with_capacity(text.len());
    let mut i=0;
    while i<text.len() {
        let ch=text[i..].chars().next().unwrap();
        if ch!='\\'{out.push(ch);i+=ch.len_utf8();continue;}
        let start=i;i+=1;
        if i==text.len(){out.push('\\');break;}
        let next=text[i..].chars().next().unwrap();
        let begin=i;
        if next.is_ascii_alphabetic(){while i<text.len() && text.as_bytes()[i].is_ascii_alphabetic(){i+=1;}}
        else{i+=next.len_utf8();}
        let name=&text[begin..i];
        let Some(mark)=accent_macro(name) else{out.push_str(&text[start..i]);continue;};
        let after=i;
        while i<text.len() && text[i..].chars().next().unwrap().is_whitespace(){i+=text[i..].chars().next().unwrap().len_utf8();}
        let letter_command=name.as_bytes().last().unwrap().is_ascii_alphabetic();
        let valid=if !letter_command || i>after {LETTER_START.is_match(&text[i..])}
            else{text[i..].chars().next().is_some_and(|c|!c.is_ascii_alphabetic())};
        if !valid {
            // Perl's \s* can backtrack to its empty match. For a letter
            // accent this makes the first whitespace a non-ASCII-letter argument.
            if letter_command && i>after {i=after;}
            else{i=after;out.push_str(&text[start..i]);continue;}
        }
        let Some(arg)=text[i..].chars().next() else{i=after;out.push_str(&text[start..i]);continue;};
        let end=i+arg.len_utf8();
        let marks=MARK_START.find(&text[end..]).unwrap().end();
        out.push_str(&text[i..end+marks]);out.push_str(mark);i=end+marks;
    }
    out
}

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
fn store_option(control:&Control,scope:&str,options:&mut std::collections::BTreeMap<String,String>,key:String,value:&str){
    let raw=format!("{key}={value}");
    options.extend(expand_options(control,scope,&raw));
}
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

// Biber precompiles these expressions with bare qr//. The later /gxms
// substitution does not override the flags of that compiled expression.
thread_local! {
    static TEXT_RULES: std::cell::RefCell<std::collections::BTreeMap<String,crate::perl_regex::Regex>> = std::cell::RefCell::new(std::collections::BTreeMap::new());
}
pub(crate) fn text_rules<'a>(control:&'a Control,key:&str)->impl Iterator<Item=&'a str> {
    control.text_rules.get(key).into_iter().flat_map(|rules|rules.iter().map(String::as_str))
        .chain(control.options.get(key).filter(|_|!control.text_rules.contains_key(key)).into_iter().flat_map(|patterns|patterns.lines()))
}
pub(crate) fn validate_control_rules(control:&Control)->Result<(),String> {
    for key in control.options.keys() {
        if !matches!(key.as_str(),"noinit"|"nolabel"|"nolabelwidthcount") && !key.starts_with("nosort.") && !key.starts_with("nonamestring.") {continue;}
        for pattern in text_rules(control,key).filter(|s|!s.is_empty()) {
            TEXT_RULES.with(|cache| {
                let mut cache=cache.borrow_mut();
                if !cache.contains_key(pattern) {
                    let rule=crate::perl_regex::Regex::compile(pattern,false)
                        .map_err(|error|format!("Invalid {key} regular expression '{pattern}': {error}"))?;
                    cache.insert(pattern.to_owned(),rule);
                }
                Ok::<(),String>(())
            })?;
        }
    }
    Ok(())
}
pub(crate) fn with_text_rule<T>(pattern:&str,f:impl FnOnce(&crate::perl_regex::Regex)->T)->Option<T> {
    TEXT_RULES.with(|cache| {
        let mut cache=cache.borrow_mut();
        if !cache.contains_key(pattern) {
            let rule=crate::perl_regex::Regex::compile(pattern,false).ok()?;
            cache.insert(pattern.to_owned(),rule);
        }
        Some(f(&cache[pattern]))
    })
}
pub(crate) fn strip_rules<'a>(control:&Control,key:&str,value:&'a str)->Cow<'a,str> {
    let mut value=Cow::Borrowed(value);
    for pattern in text_rules(control,key).filter(|s|!s.is_empty()) {
        if let Some(Some(replaced))=with_text_rule(pattern,|rule| {
            let mut matches=rule.inner.find_iter(&value);
            let first=matches.next()?.ok()?;
            let mut out=String::with_capacity(value.len());out.push_str(&value[..first.start()]);
            let mut end=first.end();
            for found in matches {let found=found.ok()?;out.push_str(&value[end..found.start()]);end=found.end();}
            out.push_str(&value[end..]);Some(out)
        }) {value=Cow::Owned(replaced);}
    }
    value
}
pub(crate) fn strip_initial<'a>(control:&Control,value:&'a str)->Cow<'a,str> {
    if control.options.contains_key("noinit") {strip_rules(control,"noinit",value)}else{strip_noinit(value)}
}
pub fn initial_tokens_xml(_control:&Control,raw:&str)->Vec<String> {vec![initial(raw)]}
pub(crate) fn expand_options(control:&Control,scope:&str,value:&str)->std::collections::BTreeMap<String,String> {
    let mut options=std::collections::BTreeMap::new();
    for raw in crate::bib::xsv(control,value) {
        let (key,value)=raw.split_once('=').unwrap_or((&raw,"true"));let key=key.trim();let value=value.trim();
        let value=if control.options.get(&format!("optionscope.{scope}.{key}.datatype")).is_some_and(|s|s=="boolean") {
            if value.eq_ignore_ascii_case("true")||value=="1"{"true"}else if value.eq_ignore_ascii_case("false")||value=="0"{"false"}else{value}
        }else{value};
        let input=control.options.get(&format!("optionscope.{scope}.{key}.backendin"));
        if let Some(input)=input {
            for sub in input.split(',') {
                let sub=sub.trim();
                if let Some((sub,val))=sub.split_once('=') {
                    let sub=sub.trim();let val=val.trim();
                    if matches!(value,"0"|"false") {
                        if control.options.get(&format!("optionscope.{scope}.{sub}.datatype")).is_some_and(|s|s=="boolean"){options.insert(sub.into(),if matches!(val,"true"|"1"){"false"}else{"true"}.into());}
                    }else{options.insert(sub.into(),val.into());}
                }else{options.insert(sub.into(),value.into());}
            }
        }else if key=="nametemplates" {
            for sub in ["sortingnamekeytemplatename","uniquenametemplatename","labelalphanametemplatename","namehashtemplatename"]{options.insert(sub.into(),value.into());}
        }else{
            let value=if control.options.get(&format!("optionscope.{scope}.{key}.datatype")).is_some_and(|s|s=="boolean"){match value{"1"|"true"=>"true","0"|"false"=>"false",_=>value}}else{value};
            options.insert(key.into(),value.into());
        }
    }
    options
}
pub(crate) fn output_option(control:&Control,scope:&str,key:&str)->bool {
    control.options.get(&format!("optionscope.{scope}.{key}.backendout")).is_some_and(|s|matches!(s.as_str(),"1"|"true"))
}
