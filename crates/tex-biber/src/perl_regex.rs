//! Biber's interpolated Perl qr// and Safe double-quoted replacement strings.
#[derive(Clone, Debug)]
pub struct Regex { pub inner: crate::perl_vm::Regex, perl_display: String }
impl Regex {
    pub fn compile(pattern:&str,insensitive:bool)->Result<Self,String>{
        let perl_display=format!("(?^u{}:{pattern})",if insensitive {"i"} else {""});
        let compiled=if insensitive {format!("(?i:{pattern})")} else {pattern.to_owned()};
        crate::perl_vm::Regex::new(&compiled).map(|inner|Self {inner,perl_display})
    }
    pub fn match_all(&self,value:&str,negative:bool)->Result<Vec<String>,String>{
        let mut result=Vec::new();
        for captures in self.inner.captures_iter(value){
            let captures=captures?;
            if negative {return Ok(Vec::new());}
            if captures.len()==1 {result.push(captures.get(0).unwrap().as_str().to_owned());}
            else {for i in 1..captures.len(){result.push(captures.get(i).map_or("",|m|m.as_str()).to_owned());}}
        }
        if negative {result.push("1".into());}
        Ok(result)
    }
    pub fn replace_all(&self,value:&str,replacement:&str)->Result<String,String>{
        let replacement=Replacement::compile(replacement);
        if let Replacement::Code(error)=&replacement {return Err(error.clone());}
        let mut result=String::with_capacity(value.len());
        let mut end=0;
        let mut evaluation=Evaluation::default();
        for captures in self.inner.captures_iter(value){
            let captures=captures?;
            let matched=captures.get(0).unwrap();
            result.push_str(&value[end..matched.start()]);
            replacement.append(&mut result,&captures,value,&self.perl_display,&mut evaluation)?;
            end=matched.end();
        }
        result.push_str(&value[end..]);
        Ok(result)
    }
}

/// Biber substitutes unescaped $digits, not Perl double-quoted expressions.
/// Entry keys and field values consume one digit; match patterns consume all.
pub fn saved_captures(value:&str,captures:&[String],all_digits:bool)->String{
    let mut out=String::with_capacity(value.len());
    let mut chars=value.chars().peekable();
    let mut previous=None;
    while let Some(c)=chars.next(){
        if c=='$'&&previous!=Some('\\')&&chars.peek().is_some_and(char::is_ascii_digit){
            let mut number=chars.next().unwrap().to_digit(10).unwrap() as usize;
            if all_digits {while let Some(digit)=chars.peek().and_then(|c|c.to_digit(10)){chars.next();number=number.saturating_mul(10).saturating_add(digit as usize);}}
            if let Some(value)=number.checked_sub(1).and_then(|i|captures.get(i)){out.push_str(value);}
        } else {out.push(c);}
        previous=Some(c);
    }
    out
}
pub(crate) fn has_saved_captures(value:&str)->bool{
    value.as_bytes().windows(2).enumerate().any(|(i,pair)|pair[0]==b'$'&&pair[1].is_ascii_digit()&&(i==0||value.as_bytes()[i-1]!=b'\\'))
}

#[derive(Clone,Copy)]
enum Case {Upper,Lower,Fold,Title}
#[derive(Clone,Copy)]
enum Scope {Case(Case),Quote}
#[derive(Clone,Copy)]
enum Capture {Number(usize),Whole,Before,After,Highest,LastClosed}
enum Token {
    Literal(String,Option<Case>,usize),
    Capture(Capture,Option<Case>,usize),
    Offsets(bool,Option<Case>,usize),
    Offset(bool,usize,Option<Case>,usize),
    Next(Case),
}
enum Replacement {Data(Vec<Token>),Rejected,Code(String)}
impl Replacement {
    fn compile(replacement:&str)->Self{
        if let Some(construct)=replacement_code_construct(replacement) {return Self::Code(format!("Perl executable replacement construct {construct} is not supported (code evaluation is disabled)"));}
        let mut chars=replacement.chars().peekable();
        let mut tokens=Vec::new();
        let mut literal=String::new();
        let mut scopes=Vec::<Scope>::new();
        let mut mode=None;let mut quotes=0usize;let mut content=false;
        macro_rules! flush {() => {if !literal.is_empty(){tokens.push(Token::Literal(std::mem::take(&mut literal),mode,quotes));}}}
        while let Some(c)=chars.next(){
            if c=='\\'{
                let Some(c)=chars.next() else {return Self::Rejected;};
                match c {
                    'U'|'L'|'F'=>{
                        flush!();
                        if matches!(scopes.last(),Some(Scope::Case(_))){if !content{return Self::Rejected;}scopes.pop();}
                        else if scopes.iter().any(|s|matches!(s,Scope::Case(_))){return Self::Rejected;}
                        let case=match c {'U'=>Case::Upper,'L'=>Case::Lower,_=>Case::Fold};
                        scopes.push(Scope::Case(case));mode=Some(case);content=false;continue;
                    }
                    'u'|'l'=>{flush!();tokens.push(Token::Next(if c=='u'{Case::Title}else{Case::Lower}));continue;}
                    'Q'=>{flush!();scopes.push(Scope::Quote);quotes+=1;continue;}
                    'E'=>{
                        flush!();
                        if let Some(scope)=scopes.pop(){match scope{Scope::Quote=>quotes-=1,Scope::Case(_)=>mode=scopes.iter().rev().find_map(|s|match s{Scope::Case(case)=>Some(*case),Scope::Quote=>None})}}
                        content=true;continue;
                    }
                    'N' if chars.peek()==Some(&'{')=>{
                        chars.next();let mut name=String::new();let mut closed=false;
                        for c in chars.by_ref(){if c=='}'{closed=true;break;}name.push(c);}
                        let Some(points)=name.strip_prefix("U+")else{return Self::Rejected;};
                        let point=points.replace('_',"");
                        if !closed||point.is_empty()||!point.chars().all(|c|c.is_ascii_hexdigit()){return Self::Rejected;}
                        let number=point.chars().fold(0u32,|n,c|n.saturating_mul(16).saturating_add(c.to_digit(16).unwrap()));
                        literal.push(char::from_u32(number).unwrap_or(char::REPLACEMENT_CHARACTER));
                        content=true;continue;
                    }
                    'o' if chars.peek()!=Some(&'{')=>return Self::Rejected,
                    _=>{}
                }
                let decoded=match c {
                    'n'=>Some('\n'),'r'=>Some('\r'),'t'=>Some('\t'),'f'=>Some('\x0c'),'b'=>Some('\x08'),'a'=>Some('\x07'),'e'=>Some('\x1b'),
                    'c'=>chars.next().map(|c|(c.to_ascii_uppercase() as u32^64) as u8 as char),
                    'x'|'o'=>{
                        let radix=if c=='x'{16}else{8};let braced=chars.peek()==Some(&'{');if braced{chars.next();}
                        let mut number=0u32;let mut count=0;let mut stopped=false;let mut content=false;let mut closed=!braced;
                        if braced{
                            for c in chars.by_ref(){
                                if c=='}'{closed=true;break;}content=true;
                                if c=='_'&&!stopped{continue;}
                                if let Some(digit)=c.to_digit(radix).filter(|_|!stopped){number=number.saturating_mul(radix).saturating_add(digit);}else{stopped=true;}
                            }
                            if !closed||(c=='o'&&!content){return Self::Rejected;}
                        }else{
                            while let Some(digit)=chars.peek().and_then(|c|c.to_digit(radix)){if count>=2{break;}chars.next();number=number*radix+digit;count+=1;}
                        }
                        Some(char::from_u32(number).unwrap_or(char::REPLACEMENT_CHARACTER))
                    }
                    '0'..='7'=>{
                        let mut number=c.to_digit(8).unwrap();let mut count=1;
                        while count<3{let Some(digit)=chars.peek().and_then(|c|c.to_digit(8)) else{break;};chars.next();number=number*8+digit;count+=1;}
                        char::from_u32(number)
                    }
                    _=>Some(c)
                };
                if let Some(c)=decoded{literal.push(c);content=true;}else{return Self::Rejected;}
            }else if c=='"'{return Self::Code("Perl executable replacement expressions are not supported (unescaped quote)".into());}
            else if c=='$'{
                flush!();
                let capture=match chars.peek().copied(){
                    Some('+')|Some('-')=>{
                        let symbol=chars.next().unwrap();
                        if chars.peek()==Some(&'{'){return Self::Rejected;}
                        if chars.peek()==Some(&'['){
                            chars.next();let mut index=String::new();let mut closed=false;
                            for c in chars.by_ref(){if c==']'{closed=true;break;}index.push(c);}
                            let Some(index)=index.parse::<usize>().ok().filter(|_|closed)else{return Self::Rejected;};
                            tokens.push(Token::Offset(symbol=='+',index,mode,quotes));None
                        }else if symbol=='+'{Some(Capture::Highest)}else{None}
                    }
                    Some('^')=>{chars.next();if chars.next()==Some('N'){Some(Capture::LastClosed)}else{None}}
                    Some('&')=>{chars.next();Some(Capture::Whole)}
                    Some('`')=>{chars.next();Some(Capture::Before)}
                    Some('\'')=>{chars.next();Some(Capture::After)}
                    Some('{')=>{
                        chars.next();
                        if chars.peek()==Some(&'\\'){return Self::Code("Perl code evaluation is required for replacement interpolation '${\\...}'".into());}
                        let mut name=String::new();let mut closed=false;for c in chars.by_ref(){if c=='}'{closed=true;break;}name.push(c);}
                        if !closed{return Self::Rejected;}
                        name.parse::<usize>().ok().filter(|n|*n!=0).map(Capture::Number)
                    }
                    Some(c) if c.is_ascii_digit()=>{
                        let mut number=0usize;while let Some(digit)=chars.peek().and_then(|c|c.to_digit(10)){chars.next();number=number.saturating_mul(10).saturating_add(digit as usize);}
                        if chars.peek()==Some(&'['){chars.next();let mut closed=false;for c in chars.by_ref(){if c==']'{closed=true;break;}}if !closed{return Self::Rejected;}None}
                        else if number==0{None}else{Some(Capture::Number(number))}
                    }
                    Some(c) if c.is_alphanumeric()||c=='_'=>{
                        while chars.peek().is_some_and(|c|c.is_alphanumeric()||matches!(c,'_'|':')){chars.next();}
                        if chars.peek()==Some(&'{')||chars.peek()==Some(&'['){let closing=if chars.next()==Some('{'){'}'}else{']'};for c in chars.by_ref(){if c==closing{break;}}}
                        None
                    }
                    _=>{literal.push('$');None}
                };
                if let Some(capture)=capture{tokens.push(Token::Capture(capture,mode,quotes));}
                content=true;
            }else if c=='@'{
                if chars.peek()==Some(&'{'){return Self::Code("Perl code evaluation is required for replacement interpolation '@{...}'".into());}
                if chars.peek().is_some_and(|c|matches!(c,'+'|'-')){flush!();tokens.push(Token::Offsets(chars.next()==Some('+'),mode,quotes));}
                else if chars.peek().is_some_and(|c|c.is_alphabetic()||matches!(c,'_'|':')){while chars.peek().is_some_and(|c|c.is_alphanumeric()||matches!(c,'_'|':')){chars.next();}}
                else if chars.peek()==Some(&'$'){
                    chars.next();
                    if chars.peek()==Some(&'{'){chars.next();for c in chars.by_ref(){if c=='}'{break;}}}
                    else{while chars.peek().is_some_and(|c|c.is_alphanumeric()||matches!(c,'_'|':')){chars.next();}}
                }
                else{literal.push('@');}
                content=true;
            }else{literal.push(c);content=true;}
        }
        flush!();Self::Data(tokens)
    }
    fn append(&self,out:&mut String,captures:&crate::perl_vm::Captures<'_>,value:&str,perl_display:&str,evaluation:&mut Evaluation)->Result<(),String>{
        let tokens=match self{Self::Data(tokens)=>tokens,Self::Rejected=>{out.push_str(perl_display);return Ok(());},Self::Code(error)=>return Err(error.clone())};
        evaluation.pending.clear();
        for token in tokens{
            match token{
                Token::Next(case)=>evaluation.pending.push(*case),
                Token::Literal(text,mode,quotes)=>emit(out,text,*mode,evaluation,*quotes),
                Token::Capture(capture,mode,quotes)=>{
                    let matched=captures.get(0).unwrap();
                    let text=match capture{
                        Capture::Number(n)=>captures.get(*n).map_or("",|m|m.as_str()),
                        Capture::Whole=>matched.as_str(),Capture::Before=>&value[..matched.start()],Capture::After=>&value[matched.end()..],
                        Capture::Highest=>(1..captures.len()).rev().find_map(|i|captures.get(i)).map_or("",|m|m.as_str()),
                        Capture::LastClosed=>captures.last_closed().map_or("",|m|m.as_str()),
                    };
                    emit(out,text,*mode,evaluation,*quotes);
                }
                Token::Offsets(end,mode,quotes)=>{
                    for i in 0..captures.len(){if let Some(m)=captures.get(i){let offset=if *end{m.end()}else{m.start()};emit_number(out,value[..offset].chars().count(),*mode,evaluation,*quotes);}}
                }
                Token::Offset(end,index,mode,quotes)=>{
                    if let Some(m)=captures.get(*index){let offset=if *end{m.end()}else{m.start()};emit_number(out,value[..offset].chars().count(),*mode,evaluation,*quotes);}
                }
            }
        }
        Ok(())
    }
}
// Safe::reval receives a double-quoted Perl expression, not a replacement
// language. Keep its data-only interpolation, but never interpret expressions.
fn replacement_code_construct(value:&str)->Option<&'static str> {
    let mut chars=value.chars().peekable();
    while let Some(c)=chars.next(){
        if c=='\\'{chars.next();continue;}
        if c=='"'{return Some("quote/operator expression");}
        if c!='@'&&c!='$'{continue;}
        if c=='$'&&matches!(chars.peek(),Some('+')|Some('-')){
            let mut named=chars.clone();named.next();
            let opening=named.next();
            if opening==Some('{'){
                let mut key=String::new();for c in named.by_ref(){if c=='}'{break;}key.push(c);}
                if !key.chars().all(|c|c.is_alphanumeric()||c=='_'){return Some("computed regex capture hash key");}
            }
            if opening==Some('['){
                let mut index=String::new();for c in named.by_ref(){if c==']'{break;}index.push(c);}
                if !index.chars().all(|c|c.is_ascii_digit()){return Some("computed subscript or dereference");}
            }
        }
        if chars.peek()==Some(&'{'){
            chars.next();let mut name=String::new();
            for c in chars.by_ref(){if c=='}'{break;}name.push(c);}
            if c=='@'{return Some("'@{...}'");}
            if !name.chars().all(|c|c.is_alphanumeric()||matches!(c,'_'|':'|'^')){return Some("'${...}' scalar expression");}
        }else{
            let named=chars.peek().is_some_and(|c|c.is_alphabetic()||matches!(c,'_'|':'));
            while chars.peek().is_some_and(|c|c.is_alphanumeric()||matches!(c,'_'|':')){chars.next();}
            if c=='$'&&!named&&chars.peek()==Some(&'['){
                let mut index=chars.clone();index.next();let mut expression=String::new();
                for c in index{if c==']'{break;}expression.push(c);}
                if !expression.chars().all(|c|c.is_ascii_digit()){return Some("computed subscript or dereference");}
            }
            let mut arrow=chars.clone();
            if named&&(matches!(chars.peek(),Some('[')|Some('{'))||(arrow.next()==Some('-')&&arrow.next()==Some('>'))){return Some("computed subscript or dereference");}
        }
    }
    None
}
#[derive(Default)]
struct Evaluation {pending:Vec<Case>,characters:Vec<char>,scratch:Vec<char>}
fn extend_case(out:&mut Vec<char>,c:char,case:Option<Case>){
    match case{
        Some(Case::Upper)=>out.extend(crate::perl_unicode::upper_chars(c)),
        Some(Case::Lower)=>out.extend(crate::perl_unicode::lower_chars(c)),
        Some(Case::Fold)=>out.extend(crate::perl_unicode::fold_chars(c)),
        Some(Case::Title)=>out.extend(crate::perl_unicode::title_chars(c)),
        None=>out.push(c),
    }
}
fn quote_char(out:&mut String,c:char,quotes:usize){
    if quotes>0&&crate::perl_unicode::needs_quoting(c){for _ in 0..(1usize.checked_shl(quotes as u32).unwrap_or(usize::MAX)).saturating_sub(1){out.push('\\');}}
    out.push(c);
}
fn emit(out:&mut String,text:&str,mode:Option<Case>,evaluation:&mut Evaluation,quotes:usize){
    for c in text.chars(){
        if evaluation.pending.is_empty(){
            if quotes==0{
                match mode{
                    Some(Case::Upper)=>crate::perl_unicode::append_upper(out,c),
                    Some(Case::Lower)=>crate::perl_unicode::append_lower(out,c),
                    Some(Case::Fold)=>crate::perl_unicode::append_fold(out,c),
                    Some(Case::Title)=>crate::perl_unicode::append_title(out,c),
                    None=>out.push(c),
                }
                continue;
            }
            evaluation.characters.clear();extend_case(&mut evaluation.characters,c,mode);
        }else{
            evaluation.characters.clear();extend_case(&mut evaluation.characters,c,mode);
            for case in evaluation.pending.iter().rev(){
                let first=evaluation.characters[0];
                evaluation.scratch.clear();extend_case(&mut evaluation.scratch,first,Some(*case));
                evaluation.scratch.extend(evaluation.characters.iter().skip(1).copied());
                std::mem::swap(&mut evaluation.characters,&mut evaluation.scratch);
            }
            evaluation.pending.clear();
        }
        for c in &evaluation.characters{quote_char(out,*c,quotes);}
    }
}
fn emit_number(out:&mut String,mut number:usize,mode:Option<Case>,evaluation:&mut Evaluation,quotes:usize){
    let mut bytes=[0u8;40];let mut start=bytes.len();
    loop{start-=1;bytes[start]=b'0'+(number%10) as u8;number/=10;if number==0{break;}}
    emit(out,std::str::from_utf8(&bytes[start..]).unwrap(),mode,evaluation,quotes);
}
#[cfg(test)]
mod tests {
    use super::Regex;
    #[test]
    fn replacement_code_is_rejected_even_without_a_match(){
        let regex=Regex::compile("(a)",false).unwrap();
        for replacement in [r"@{[uc($1)]}",r"${\uc($1)}",r#"" . uc($1) . ""#,r"$foo[uc($1)]",r"$+{uc($1)}"]{
            let error=regex.replace_all("zzz",replacement).unwrap_err();
            assert!(error.contains("Perl executable replacement construct")&&error.contains("code evaluation is disabled"),"{error}");
        }
        assert_eq!(regex.replace_all("a",r#"\"literal\""#).unwrap(),"\"literal\"");
    }
    #[test]
    fn data_replacements_follow_safe_captures_and_unicode_escapes(){
        let regex=Regex::compile("(?<first>A)(B)(C)?",false).unwrap();
        assert_eq!(regex.replace_all("AB",r"${2}-$1").unwrap(),"B-A");
        assert_eq!(regex.replace_all("AB",r"\x{41}\o{102}\N{U+0043}").unwrap(),"ABC");
        assert_eq!(regex.replace_all("AB",r"@+/@-").unwrap(),"212/001");
        // Safe disallows %+, even though qr// named groups themselves work.
        assert_eq!(regex.replace_all("AB",r"$+{first}").unwrap(),"(?^u:(?<first>A)(B)(C)?)");
        assert_eq!(regex.replace_all("AB",r"${1").unwrap(),"(?^u:(?<first>A)(B)(C)?)");
    }
}
