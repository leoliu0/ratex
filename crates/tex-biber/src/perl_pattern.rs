//! Perl pattern syntax lowered to the public fancy-regex expression tree.
//! Capture indices in references are Perl indices; Group traversal indices are physical.
use fancy_regex::{AstNode, CaptureGroupTarget, Expr, LookAround};
use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

#[derive(Clone, Debug)]
pub enum ControlVerb {
    Accept, Fail, Commit(Option<String>),
    Prune(Option<String>), Skip(Option<String>), Then(Option<String>), Mark(String),
    ConditionRecursion(Option<usize>), Grapheme,
    WordBoundary { negative: bool, ascii: bool, locale: bool },
    ExtendedBoundary { kind: String, negative: bool },
    ScriptRunStart, ScriptRunEnd,
    AsciiInsensitiveLiteral(String), AsciiInsensitiveBackref(usize),
    AsciiInsensitiveBackrefs(Vec<usize>),
    LocaleInsensitiveLiteral(String), LocaleUnicodeLiteral(String), LocaleInsensitiveBackrefs(Vec<usize>),
    CharacterClass { ranges: Vec<(u32, u32)>, negated: bool, casei: bool, ascii_restrict: bool, locale: bool, locale_pattern: bool },
}

#[derive(Clone, Debug)]
pub struct Parsed {
    pub expr: Expr,
    pub capture_aliases: Vec<usize>,
    pub controls: BTreeMap<usize, ControlVerb>,
    pub capture_count: usize,
}

#[derive(Clone, Copy, Default)]
struct Flags { i: bool, m: bool, s: bool, x: u8, n: bool, ascii: u8, strict: bool, locale: bool, locale_unicode: bool }
struct Parser<'a> {
    text: &'a str, pos: usize, flags: Flags,
    logical: usize, aliases: Vec<usize>, controls: BTreeMap<usize, ControlVerb>,
    names: BTreeMap<String, Vec<usize>>, pending_recursion: BTreeMap<usize, String>,
    pending_ascii_backrefs: BTreeMap<usize, (CaptureGroupTarget, bool)>,
    script_depth: usize,
}

pub fn parse(pattern: &str) -> Result<Parsed, String> {
    let mut p = Parser { text: pattern, pos: 0, flags: Flags::default(),
        logical: 0, aliases: vec![0], controls: BTreeMap::new(), names: BTreeMap::new(),
        pending_recursion: BTreeMap::new(), pending_ascii_backrefs: BTreeMap::new(), script_depth:0 };
    let mut expr = p.alternation(false)?;
    p.space();
    if p.pos != pattern.len() { return Err(p.error("Unmatched closing parenthesis")); }
    p.resolve(&mut expr)?;
    for (physical, name) in &p.pending_recursion {
        let slot = *p.names.get(name).and_then(|v|v.first()).ok_or_else(||format!("Unknown recursion group {name}"))?;
        p.controls.insert(*physical, ControlVerb::ConditionRecursion(Some(slot)));
    }
    for (physical, (target, locale)) in &p.pending_ascii_backrefs {
        let slots=p.slots(target)?;
        p.controls.insert(*physical, if *locale {ControlVerb::LocaleInsensitiveBackrefs(slots)}else if slots.len()==1 {ControlVerb::AsciiInsensitiveBackref(slots[0])}else{ControlVerb::AsciiInsensitiveBackrefs(slots)});
    }
    Ok(Parsed {expr, capture_aliases:p.aliases, controls:p.controls, capture_count:p.logical})
}
fn c_locale() -> Result<bool,String> {
    let locale=["LC_ALL","LC_CTYPE","LANG"].iter().find_map(|name|std::env::var(name).ok().filter(|v|!v.is_empty())).unwrap_or_else(||"C".into());
    if matches!(locale.as_str(),"C"|"POSIX") {return Ok(true);}
    let normalized=locale.to_ascii_lowercase().replace('-',"");
    if normalized.contains("utf8") {return Ok(false);}
    Err(format!("Perl /l requires a UTF-8 or C/POSIX character locale; unsupported locale {locale:?}"))
}
fn c_locale_ranges(ranges:&[(u32,u32)]) -> Vec<(u32,u32)> {
    intersection(ranges,&[(0,127),(256,0x10ffff)])
}

fn concat(mut parts: Vec<Expr>) -> Expr {
    match parts.len() {0=>Expr::Empty, 1=>parts.pop().unwrap(), _=>Expr::Concat(parts)}
}
fn alt(mut parts: Vec<Expr>) -> Expr {
    if parts.len()==1 {parts.pop().unwrap()} else {Expr::Alt(parts)}
}
fn normalize(ranges: &mut Vec<(u32,u32)>) {
    ranges.sort_unstable();
    let mut out = 0;
    for i in 0..ranges.len() {
        let (a,b)=ranges[i];
        if out>0 && a<=ranges[out-1].1.saturating_add(1) {ranges[out-1].1=ranges[out-1].1.max(b);}
        else {ranges[out]=(a,b);out+=1;}
    }
    ranges.truncate(out);
}
fn complement(ranges: &[(u32,u32)]) -> Vec<(u32,u32)> {
    let mut out=Vec::new();let mut at=0;
    for &(a,b) in ranges {if at<a {out.push((at,a-1));}at=b+1;}
    if at<=0x10ffff {out.push((at,0x10ffff));}
    out
}
fn intersection(left:&[(u32,u32)],right:&[(u32,u32)]) -> Vec<(u32,u32)> {
    let mut result=Vec::new();let (mut a,mut b)=(0,0);
    while a<left.len()&&b<right.len() {
        let start=left[a].0.max(right[b].0);let end=left[a].1.min(right[b].1);
        if start<=end {result.push((start,end));}
        if left[a].1<right[b].1 {a+=1;}else{b+=1;}
    }
    result
}

impl Parser<'_> {
    fn error(&self, message: &str) -> String {format!("{message} at byte {} in Perl pattern",self.pos)}
    fn rest(&self) -> &str {&self.text[self.pos..]}
    fn take(&mut self) -> Option<char> {let c=self.rest().chars().next()?;self.pos+=c.len_utf8();Some(c)}
    fn eat(&mut self, value: &str) -> bool {if self.rest().starts_with(value) {self.pos+=value.len();true}else{false}}
    fn require(&mut self, value: &str) -> Result<(),String> {if self.eat(value) {Ok(())}else{Err(self.error(&format!("Expected {value}")))}}
    fn until(&mut self, delimiter: char) -> Result<String,String> {
        let end=self.rest().find(delimiter).ok_or_else(||self.error("Unterminated token"))?;
        let result=self.rest()[..end].to_owned();self.pos+=end+delimiter.len_utf8();Ok(result)
    }
    fn space(&mut self) {
        if self.flags.x==0 {return;}
        loop {
            while self.rest().chars().next().is_some_and(|c|matches!(c,' '| '\t'|'\n'|'\r'|'\x0b'|'\x0c'|'\u{85}'|'\u{200e}'|'\u{200f}'|'\u{2028}'|'\u{2029}')) {self.take();}
            if self.eat("#") {while self.take().is_some_and(|c|c!='\n') {}}
            else {break;}
        }
    }
    fn marker(&mut self, verb: ControlVerb) -> Expr {
        let physical=self.aliases.len();self.aliases.push(usize::MAX);self.controls.insert(physical,verb);
        // Use a reserved, uniquely named capture in fancy's AST, never a magic character.
        Expr::parse_tree(&format!("(?<__biber_control_{physical}>)")).unwrap().expr
    }
    fn literal(&mut self, value: String) -> Expr {
        if self.flags.i&&self.flags.locale {return self.marker(ControlVerb::LocaleInsensitiveLiteral(value));}
        if self.flags.i&&self.flags.locale_unicode {return self.marker(ControlVerb::LocaleUnicodeLiteral(value));}
        if self.flags.i && self.flags.ascii==2 {self.marker(ControlVerb::AsciiInsensitiveLiteral(value))}
        else {Expr::Literal {val:value,casei:self.flags.i}}
    }
    fn alternation(&mut self, reset: bool) -> Result<Expr,String> {
        let base=self.logical;let mut maximum=base;let mut branches=Vec::new();
        loop {
            branches.push(self.sequence()?);maximum=maximum.max(self.logical);
            if !self.eat("|") {break;}
            if reset {self.logical=base;}
        }
        self.logical=maximum;
        Ok(alt(branches))
    }
    fn sequence(&mut self) -> Result<Expr,String> {
        let mut parts=Vec::new();
        loop {
            self.space();
            if self.rest().is_empty() || self.rest().starts_with(['|',')']) {break;}
            let atom=self.atom()?;
            self.space();
            parts.push(self.quantify(atom)?);
        }
        Ok(concat(parts))
    }
    fn quantify(&mut self, child: Expr) -> Result<Expr,String> {
        let (lo,hi)=if self.eat("*") {(0,usize::MAX)} else if self.eat("+") {(1,usize::MAX)}
        else if self.eat("?") {(0,1)} else if self.rest().starts_with('{') {
            let saved=self.pos;self.pos+=1;let start=self.pos;
            while self.rest().starts_with(|c:char|c.is_ascii_digit()) {self.pos+=1;}
            if start==self.pos {self.pos=saved;return Ok(child);}
            let lo=self.text[start..self.pos].parse().map_err(|_|self.error("Quantifier overflow"))?;
            let hi=if self.eat(",") {let start=self.pos;while self.rest().starts_with(|c:char|c.is_ascii_digit()) {self.pos+=1;}
                if start==self.pos {usize::MAX}else{self.text[start..self.pos].parse().map_err(|_|self.error("Quantifier overflow"))?}}
            else {lo};
            if !self.eat("}") {self.pos=saved;return Ok(child);}
            if hi<lo {return Err(self.error("Quantifier range is reversed"));}(lo,hi)
        } else {return Ok(child);};
        let greedy=!self.eat("?");
        let repeated=Expr::Repeat {child:Box::new(child),lo,hi,greedy};
        if self.eat("+") {Ok(Expr::AtomicGroup(Box::new(repeated)))}else{Ok(repeated)}
    }
    fn atom(&mut self) -> Result<Expr,String> {
        let c=self.take().ok_or_else(||self.error("Missing atom"))?;
        match c {
            '('=>self.group(), '['=>self.class(), '\\'=>self.escape(false),
            '.'=>Ok(Expr::Any {newline:self.flags.s,crlf:false}),
            '^'=>self.leaf(if self.flags.m {"(?m:^)"}else{"\\A"}),
            '$'=>self.leaf(if self.flags.m {"(?m:$)"}else{"(?=\\n?\\z)"}),
            '*'|'+'|'?'=>Err(self.error("Quantifier follows nothing")),
            _=>Ok(self.literal(c.to_string())),
        }
    }
    fn leaf(&self, text:&str) -> Result<Expr,String> {Expr::parse_tree(text).map(|t|t.expr).map_err(|e|e.to_string())}
    fn scoped_body(&mut self, reset: bool) -> Result<Expr,String> {let body=self.alternation(reset)?;self.require(")")?;Ok(body)}
    fn group(&mut self) -> Result<Expr,String> {
        let flags=self.flags;
        let result=self.group_inner();
        // Bare modifier groups affect the enclosing lexical scope, not a new group.
        if !matches!(&result,Ok((_,true))) {self.flags=flags;}
        result.map(|(expr,_)|expr)
    }
    fn group_inner(&mut self) -> Result<(Expr,bool),String> {
        if self.eat("*") {return self.verb().map(|e|(e,false));}
        if !self.eat("?") {
            if self.flags.n {return self.scoped_body(false).map(|e|(e,false));}
            self.logical+=1;self.aliases.push(self.logical);
            return self.scoped_body(false).map(|e|(Expr::Group(Arc::new(e)),false));
        }
        if self.eat("[") {
            let ranges=self.extended_ranges()?;
            return Ok((self.marker(ControlVerb::CharacterClass {ranges,negated:false,casei:false,ascii_restrict:false,locale:false,locale_pattern:false}),false));
        }
        if self.eat("{")||self.eat("?{") {return Err(self.error("Perl code-valued regex constructs are not supported"));}
        if self.eat("#") {self.until(')')?;return Ok((Expr::Empty,false));}
        if self.eat(":") {return self.scoped_body(false).map(|e|(e,false));}
        if self.eat("|") {return self.scoped_body(true).map(|e|(e,false));}
        if self.eat(">") {return self.scoped_body(false).map(|e|(Expr::AtomicGroup(Box::new(e)),false));}
        if self.eat("*")||self.eat("<*") {return Err(self.error("Non-atomic lookaround is not supported by Biber's Perl 5.38"));}
        for (prefix,kind) in [("=",LookAround::LookAhead),("!",LookAround::LookAheadNeg),("<=",LookAround::LookBehind),("<!",LookAround::LookBehindNeg)] {
            if self.eat(prefix) {return self.scoped_body(false).map(|e|(Expr::LookAround(Box::new(e),kind),false));}
        }
        if self.eat("(") {return self.condition().map(|e|(e,false));}
        if self.eat("R)") {return Ok((Expr::SubroutineCall(0),false));}
        if self.eat("&") || self.eat("P>") {let name=self.until(')')?;return Ok((Expr::AstNode(AstNode::SubroutineCall(CaptureGroupTarget::ByName(name)),self.pos),false));}
        if self.eat("P=") {let name=self.until(')')?;return Ok((self.reference(CaptureGroupTarget::ByName(name)),false));}
        if {let r=self.rest();r.starts_with(|c:char|c.is_ascii_digit())||r.strip_prefix(['-','+']).is_some_and(|r|r.starts_with(|c:char|c.is_ascii_digit()))} {
            let value=self.until(')')?;let target=self.target(&value,true)?;
            return Ok((Expr::AstNode(AstNode::SubroutineCall(target),self.pos),false));
        }
        let name=if self.eat("P<")||self.eat("<") {Some(self.until('>')?)}else if self.eat("'") {Some(self.until('\'')?)}else{None};
        if let Some(name)=name {
            if name.is_empty() || !name.chars().next().is_some_and(|c|c=='_'||c.is_alphabetic()) || !name.chars().all(|c|c=='_'||c.is_alphanumeric()) {return Err(self.error("Invalid Perl capture name"));}
            self.logical+=1;self.aliases.push(self.logical);
            let slots=self.names.entry(name).or_default();if !slots.contains(&self.logical) {slots.push(self.logical);}
            return self.scoped_body(false).map(|e|(Expr::Group(Arc::new(e)),false));
        }
        self.modifiers()
    }
    fn modifiers(&mut self) -> Result<(Expr,bool),String> {
        self.modifier_flags()?;
        if self.eat(")") {Ok((Expr::Empty,true))}else{self.require(":")?;self.scoped_body(false).map(|e|(e,false))}
    }
    fn modifier_flags(&mut self) -> Result<(),String> {
        let reset=self.eat("^");if reset {let strict=self.flags.strict;self.flags=Flags {strict,..Flags::default()};}
        let mut enabled=true;let mut any=reset;let mut charset=None;let mut x_count=0;
        loop {
            let Some(c)=self.rest().chars().next() else {return Err(self.error("Unclosed modifiers"));};
            if c==':'||c==')' {break;}
            self.take();
            if c=='-' {enabled=false;continue;}
            any=true;
            match c {
                'i'=>self.flags.i=enabled,'m'=>self.flags.m=enabled,'s'=>self.flags.s=enabled,'n'=>self.flags.n=enabled,
                'p'=>{},
                'x'=>{if enabled {x_count+=1;if x_count>2 {return Err(self.error("More than two /x modifiers"));}self.flags.x=x_count;}else{self.flags.x=0;}},
                'a' if enabled=>{if charset.is_some()&&charset!=Some('a') {return Err(self.error("Mutually exclusive character-set modifiers"));}let count=if charset==Some('a') {self.flags.ascii+1}else{1};if count>2 {return Err(self.error("More than two /a modifiers"));}self.flags.ascii=count;self.flags.locale=false;self.flags.locale_unicode=false;charset=Some('a');},
                'u'|'d'|'l' if enabled=>{if charset.is_some() {return Err(self.error("Mutually exclusive or repeated character-set modifiers"));}self.flags.ascii=0;self.flags.locale=c=='l'&&c_locale()?;self.flags.locale_unicode=c=='l'&&!self.flags.locale;charset=Some(c);},
                _=>return Err(self.error("Unknown or invalid Perl modifier")),
            }
        }
        if !any {return Err(self.error("Missing Perl modifier"));}
        Ok(())
    }
    fn target(&self, text:&str, subroutine:bool) -> Result<CaptureGroupTarget,String> {
        if text.starts_with(['-','+']) && text[1..].chars().all(|c|c.is_ascii_digit()) {
            let offset=text.parse::<isize>().map_err(|_|self.error("Relative capture overflow"))?;
            // -1 is the last opened group; +1 is the next group.
            let slot=self.logical as isize+offset+if offset<0 {1}else{0};
            if slot<=0 {return Err(self.error("Invalid relative capture"));}
            Ok(CaptureGroupTarget::ByNumber(slot as usize))
        }else if text.chars().all(|c|c.is_ascii_digit()) && !text.is_empty() {
            let slot=text.parse().map_err(|_|self.error("Capture number overflow"))?;
            if slot==0&&!subroutine {return Err(self.error("Invalid zero backreference"));}Ok(CaptureGroupTarget::ByNumber(slot))
        }else {Ok(CaptureGroupTarget::ByName(text.to_owned()))}
    }
    fn reference(&mut self, target:CaptureGroupTarget) -> Expr {
        if self.flags.i && (self.flags.ascii==2||self.flags.locale) {
            let physical=self.aliases.len();self.pending_ascii_backrefs.insert(physical,(target,self.flags.locale));
            self.marker(ControlVerb::AsciiInsensitiveBackref(0))
        }else{Expr::AstNode(AstNode::Backref {target,casei:self.flags.i,relative_recursion_level:None},self.pos)}
    }
    fn condition(&mut self) -> Result<Expr,String> {
        if self.eat("DEFINE)") {let definitions=self.sequence()?;self.require(")")?;return Ok(Expr::DefineGroup {definitions:Box::new(definitions)});}
        let condition=if self.eat("?=") {Expr::LookAround(Box::new(self.scoped_body(false)?),LookAround::LookAhead)}
        else if self.eat("?!") {Expr::LookAround(Box::new(self.scoped_body(false)?),LookAround::LookAheadNeg)}
        else if self.eat("?<=") {Expr::LookAround(Box::new(self.scoped_body(false)?),LookAround::LookBehind)}
        else if self.eat("?<!") {Expr::LookAround(Box::new(self.scoped_body(false)?),LookAround::LookBehindNeg)}
        else {
            let target=self.until(')')?;
            if target=="R" {self.marker(ControlVerb::ConditionRecursion(None))}
            else if let Some(name)=target.strip_prefix("R&") {let physical=self.aliases.len();self.pending_recursion.insert(physical,name.to_owned());self.marker(ControlVerb::ConditionRecursion(None))}
            else if target.starts_with('R')&&target[1..].chars().all(|c|c.is_ascii_digit()) {self.marker(ControlVerb::ConditionRecursion(Some(target[1..].parse().map_err(|_|self.error("Recursion number overflow"))?)))}
            else {
                let delimited=target.starts_with(['<','\'']);
                let target=target.trim_start_matches(['<','\'']).trim_end_matches(['>','\'']);
                if !delimited && target.chars().any(|c|!c.is_ascii_digit()&&c!='-'&&c!='+') {return Err(self.error("Unknown switch condition"));}
                Expr::AstNode(AstNode::BackrefExistsCondition {target:self.target(target,false)?,relative_recursion_level:None},self.pos)
            }
        };
        let true_branch=self.sequence()?;
        let false_branch=if self.eat("|") {self.sequence()?}else{Expr::Empty};
        if self.rest().starts_with('|') {return Err(self.error("Conditional has more than two branches"));}
        self.require(")")?;
        Ok(Expr::Conditional {condition:Box::new(condition),true_branch:Box::new(true_branch),false_branch:Box::new(false_branch)})
    }
    fn verb(&mut self) -> Result<Expr,String> {
        for (prefix,kind) in [("pla:",LookAround::LookAhead),("positive_lookahead:",LookAround::LookAhead),("nla:",LookAround::LookAheadNeg),("negative_lookahead:",LookAround::LookAheadNeg),("plb:",LookAround::LookBehind),("positive_lookbehind:",LookAround::LookBehind),("nlb:",LookAround::LookBehindNeg),("negative_lookbehind:",LookAround::LookBehindNeg)] {
            if self.eat(prefix) {return self.scoped_body(false).map(|e|Expr::LookAround(Box::new(e),kind));}
        }
        if self.eat("atomic:") {return self.scoped_body(false).map(|e|Expr::AtomicGroup(Box::new(e)));}
        if self.eat("napla:")||self.eat("non_atomic_positive_lookahead:")||self.eat("naplb:")||self.eat("non_atomic_positive_lookbehind:") {return Err(self.error("Non-atomic lookaround is not supported by Biber's Perl 5.38"));}
        let script=if self.eat("sr:")||self.eat("script_run:") {Some(false)}else if self.eat("asr:")||self.eat("atomic_script_run:") {Some(true)}else{None};
        if let Some(atomic)=script {
            let outer=self.script_depth==0;
            let start=if outer {self.marker(ControlVerb::ScriptRunStart)}else{Expr::Empty};
            self.script_depth+=1;
            let body=self.scoped_body(false)?;
            self.script_depth-=1;
            let end=if outer {self.marker(ControlVerb::ScriptRunEnd)}else{Expr::Empty};
            let body=if atomic {Expr::AtomicGroup(Box::new(body))}else{body};
            return Ok(concat(vec![start,body,end]));
        }
        let text=self.until(')')?;let (name,arg)=text.split_once(':').map_or((text.as_str(),None),|(name,arg)|(name,Some(arg.to_owned())));
        let verb=match name {
            "ACCEPT"=>ControlVerb::Accept,"FAIL"|"F"=>ControlVerb::Fail,"COMMIT"=>ControlVerb::Commit(arg),
            "PRUNE"=>ControlVerb::Prune(arg),"SKIP"=>ControlVerb::Skip(arg),"THEN"=>ControlVerb::Then(arg),
            "MARK"|""=>ControlVerb::Mark(arg.filter(|s|!s.is_empty()).ok_or_else(||self.error("MARK requires a name"))?),
            _=>return Err(self.error(&format!("Unknown Perl control verb {name}"))),
        };Ok(self.marker(verb))
    }
    fn escape(&mut self, in_class:bool) -> Result<Expr,String> {
        let c=self.take().ok_or_else(||self.error("Trailing backslash"))?;
        if !in_class {
            match c {
                // Runtime qr/$pattern/ does not perform lexical quotemeta interpolation.
                'Q'|'E'=>return Ok(self.literal(c.to_string())),
                'K'=>return Ok(Expr::KeepOut),'G'=>return Ok(Expr::ContinueFromPreviousMatchEnd),
                'X'=>return Ok(self.marker(ControlVerb::Grapheme)),'C'=>return Err(self.error("\\C is no longer supported in Perl 5.38")),
                'R'=>return Ok(Expr::GeneralNewline {unicode:true}),
                'N' if !self.rest().starts_with('{')=>return Ok(Expr::Any {newline:false,crlf:false}),
                'N' if self.rest().starts_with('{')=>{
                    let saved=self.pos;self.take();let name=self.until('}')?;
                    if name.chars().next().is_some_and(|c|c.is_ascii_digit()) &&
                        name.split(',').count()<=2 && name.split(',').all(|n|n.bytes().all(|b|b.is_ascii_digit())) {
                        self.pos=saved;return Ok(Expr::Any {newline:false,crlf:false});
                    }
                    if let Some(value)=crate::perl_unicode::named_sequence(&name) {return Ok(self.literal(value.to_owned()));}
                    self.pos=saved;
                },
                'b'|'B'=>{
                    if self.eat("{") {
                        let kind=self.until('}')?;
                        if !matches!(kind.as_str(),"wb"|"sb"|"gcb"|"g") {return Err(self.error("Unknown Unicode boundary type"));}
                        return Ok(self.marker(ControlVerb::ExtendedBoundary {kind,negative:c=='B'}));
                    }
                    return Ok(self.marker(ControlVerb::WordBoundary {negative:c=='B',ascii:self.flags.ascii>0,locale:self.flags.locale}));
                },
                'Z'=>return self.leaf("(?=\\n?\\z)"),'A'|'z'=>return self.leaf(&format!("\\{c}")),
                'g'|'k'=>{
                    let (open,close)=match self.rest().chars().next() {Some('{')=>('{','}'),Some('<')=>('<','>'),Some('\'')=>('\'','\''),_=>('\0','\0')};
                    let value=if open!='\0' {self.take();self.until(close)?}else if c=='g' {let start=self.pos;self.eat("-");while self.rest().starts_with(|c:char|c.is_ascii_digit()) {self.pos+=1;}self.text[start..self.pos].to_owned()}else{return Err(self.error("Malformed named backreference"));};
                    return Ok(self.reference(self.target(&value,false)?));
                },
                '1'..='9'=>{
                    let start=self.pos-1;while self.rest().starts_with(|c:char|c.is_ascii_digit()) {self.pos+=1;}
                    let text=&self.text[start..self.pos];let n=text.parse::<usize>().map_err(|_|self.error("Backreference overflow"))?;
                    // Perl interprets multi-digit forms above the captures opened so far as octal.
                    if text.len()>1 && n>self.logical && c<='7' {self.pos=start;return self.octal();}
                    return Ok(self.reference(CaptureGroupTarget::ByNumber(n)));
                },
                _=>{},
            }
        }
        if matches!(c,'w'|'W'|'d'|'D'|'s'|'S'|'h'|'H'|'v'|'V'|'p'|'P') {
            let (ranges,negative)=self.property_escape(c)?;
            return Ok(self.marker(ControlVerb::CharacterClass {ranges,negated:negative,casei:self.flags.i,ascii_restrict:self.flags.ascii==2,locale:self.flags.locale,locale_pattern:self.flags.locale||self.flags.locale_unicode}));
        }
        let value=self.escaped_char(c)?;
        Ok(self.literal(value.to_string()))
    }
    fn octal(&mut self) -> Result<Expr,String> {let start=self.pos;for _ in 0..3 {if self.rest().starts_with(|c:char|matches!(c,'0'..='7')) {self.pos+=1;}else{break;}}
        let value=u32::from_str_radix(&self.text[start..self.pos],8).map_err(|_|self.error("Invalid octal escape"))?;
        Ok(self.literal(char::from_u32(value).ok_or_else(||self.error("Invalid codepoint"))?.to_string()))}
    fn escaped_char(&mut self,c:char) -> Result<char,String> {
        match c {
            'n'=>Ok('\n'),'r'=>Ok('\r'),'t'=>Ok('\t'),'f'=>Ok('\x0c'),'b'=>Ok('\x08'),'a'=>Ok('\x07'),'e'=>Ok('\x1b'),
            'c'=>{let c=self.take().ok_or_else(||self.error("Missing control character"))?;char::from_u32((c.to_ascii_uppercase() as u32)^64).ok_or_else(||self.error("Invalid control escape"))},
            'x'|'o'=>{let radix=if c=='x' {16}else{8};let digits=if self.eat("{") {self.until('}')?}else if c=='o' {return Err(self.error("Octal escape requires braces"));}else{let start=self.pos;for _ in 0..2 {if self.rest().starts_with(|c:char|c.is_ascii_hexdigit()) {self.pos+=1;}else{break;}}self.text[start..self.pos].to_owned()};
                if self.flags.strict && c=='x' && !self.text[..self.pos].ends_with('}') && digits.len()!=2 {return Err(self.error("Extended classes require two hex digits"));}
                let value=if digits.is_empty() {0}else{u32::from_str_radix(&digits,radix).map_err(|_|self.error("Invalid character escape"))?};char::from_u32(value).ok_or_else(||self.error("Invalid Unicode scalar"))},
            '0'..='7'=>{let mut digits=c.to_string();for _ in 0..2 {if self.rest().starts_with(|c:char|matches!(c,'0'..='7')) {digits.push(self.take().unwrap());}else{break;}}let n=u32::from_str_radix(&digits,8).unwrap();char::from_u32(n).ok_or_else(||self.error("Invalid octal character"))},
            'N'=>{if !self.eat("{") {return Err(self.error("Bare \\N must be parsed as a class"));}let name=self.until('}')?;
                if let Some(hex)=name.strip_prefix("U+") {char::from_u32(u32::from_str_radix(hex,16).map_err(|_|self.error("Invalid named codepoint"))?).ok_or_else(||self.error("Invalid named codepoint"))}
                else {crate::perl_unicode::named_char(&name).ok_or_else(||self.error("Unknown Unicode character name"))}},
            _ if self.flags.strict && c.is_ascii_alphanumeric()=>Err(self.error("Unrecognized escape in extended character class")),
            _=>Ok(c),
        }
    }
    fn property_escape(&mut self,c:char) -> Result<(Vec<(u32,u32)>,bool),String> {
        let negative=c.is_ascii_uppercase();
        let name=match c.to_ascii_lowercase() {
            'p'=>if self.eat("{") {self.until('}')?}else{self.take().ok_or_else(||self.error("Missing property name"))?.to_string()},
            'w'=>"Word".into(),'d'=>"Digit".into(),'s'=>"Space".into(),'h'=>"HorizSpace".into(),'v'=>"VertSpace".into(),_=>unreachable!(),
        };
        let name=name.trim();let inverted=name.starts_with('^');let name=name.trim_start_matches('^');
        let mut ranges=crate::perl_unicode::property_ranges_casei(name,self.flags.i)?.to_vec();
        if self.flags.ascii>0 && matches!(c.to_ascii_lowercase(),'w'|'d'|'s') {
            ranges.retain(|&(a,_)|a<128);for (_,b) in &mut ranges {*b=(*b).min(127);}
        } else if self.flags.locale&&matches!(c.to_ascii_lowercase(),'w'|'d'|'s') {
            ranges=c_locale_ranges(&ranges);
        }
        normalize(&mut ranges);Ok((ranges,negative^inverted))
    }
    fn class_item(&mut self) -> Result<(Vec<(u32,u32)>,Option<u32>),String> {
        let c=self.take().ok_or_else(||self.error("Unclosed class"))?;
        if c=='['&&self.rest().starts_with(['=','.']) {return Err(self.error("POSIX collating elements and equivalence classes are not supported by Perl"));}
        if c=='['&&self.eat(":") {
            let negative=self.eat("^");let end=self.rest().find(":]").ok_or_else(||self.error("Unclosed POSIX class"))?;
            let name=self.rest()[..end].to_owned();self.pos+=end+2;
            if !matches!(name.as_str(),"alnum"|"alpha"|"ascii"|"blank"|"cntrl"|"digit"|"graph"|"lower"|"print"|"punct"|"space"|"upper"|"word"|"xdigit") {return Err(self.error("Unknown POSIX character class"));}
            let property=if name=="ascii" {name.clone()}else{format!("{}{}",if self.flags.ascii>0 {"Posix"}else{"XPosix"},name)};
            let mut ranges=crate::perl_unicode::property_ranges_casei(&property,self.flags.i)?.to_vec();
            if self.flags.ascii>0 {ranges.retain(|&(a,_)|a<128);for (_,b) in &mut ranges {*b=(*b).min(127);}}
            if self.flags.locale {ranges=c_locale_ranges(&ranges);}
            normalize(&mut ranges);return Ok((if negative {complement(&ranges)}else{ranges},None));
        }
        if c=='\\' {
            let e=self.take().ok_or_else(||self.error("Trailing class backslash"))?;
            if matches!(e,'w'|'W'|'d'|'D'|'s'|'S'|'h'|'H'|'v'|'V'|'p'|'P') {let (ranges,negative)=self.property_escape(e)?;return Ok((if negative {complement(&ranges)}else{ranges},None));}
            let value=self.escaped_char(e)? as u32;return Ok((vec![(value,value)],Some(value)));
        }
        Ok((vec![(c as u32,c as u32)],Some(c as u32)))
    }
    fn class(&mut self) -> Result<Expr,String> {
        let (ranges,negative)=self.class_ranges()?;
        Ok(self.marker(ControlVerb::CharacterClass {ranges,negated:negative,casei:self.flags.i,ascii_restrict:self.flags.ascii==2,locale:self.flags.locale,locale_pattern:self.flags.locale||self.flags.locale_unicode}))
    }
    fn class_ranges(&mut self) -> Result<(Vec<(u32,u32)>,bool),String> {
        let negative=self.eat("^");let mut ranges=Vec::new();let mut first=true;
        loop {
            if self.flags.x==2 {while self.rest().starts_with([' ','\t']) {self.take();}}
            if !first&&self.eat("]") {break;}
            let (mut item,start)=self.class_item()?;first=false;
            if self.flags.x==2 {while self.rest().starts_with([' ','\t']) {self.take();}}
            let trailing_dash=self.rest().strip_prefix('-').is_some_and(|rest|if self.flags.x==2 {rest.trim_start_matches([' ','\t']).starts_with(']')}else{rest.starts_with(']')});
            if self.rest().starts_with('-')&&!trailing_dash {
                self.take();if self.flags.x==2 {while self.rest().starts_with([' ','\t']) {self.take();}}let (end_ranges,end)=self.class_item()?;
                if let (Some(a),Some(b))=(start,end) {if a>b {return Err(self.error("Reversed character range"));}item=vec![(a,b)];}
                else {if self.flags.strict {return Err(self.error("Invalid range in extended character class"));}item.push(('-' as u32,'-' as u32));item.extend(end_ranges);}
            }
            ranges.extend(item);
            if self.rest().is_empty() {return Err(self.error("Unclosed character class"));}
        }
        normalize(&mut ranges);
        Ok((ranges,negative))
    }
    fn close_set(&self, mut ranges:Vec<(u32,u32)>) -> Vec<(u32,u32)> {
        if !self.flags.i {return ranges;}
        let mut folds:[BTreeSet<Vec<char>>;2]=[BTreeSet::new(),BTreeSet::new()];
        let ascii=self.flags.ascii==2;
        let member=|c:char|crate::perl_unicode::in_ranges(&ranges,c);
        let mut extra=Vec::new();
        for &(c,fold) in crate::perl_unicode::nonidentity_folds() {
            if member(c) {
                folds[usize::from(ascii&&c.is_ascii())].insert(fold.to_vec());
                if fold.len()==1&&(!ascii||c.is_ascii()==fold[0].is_ascii()) {extra.push((fold[0] as u32,fold[0] as u32));}
            }
        }
        for &(c,fold) in crate::perl_unicode::nonidentity_folds() {
            if (fold.len()==1&&member(fold[0])&&(!ascii||c.is_ascii()==fold[0].is_ascii()))
                || folds[usize::from(ascii&&c.is_ascii())].contains(fold) {extra.push((c as u32,c as u32));}
        }
        ranges.extend(extra);normalize(&mut ranges);ranges
    }
    fn extended_ranges(&mut self) -> Result<Vec<(u32,u32)>,String> {
        let saved=self.flags;self.flags.x=2;self.flags.strict=true;self.flags.locale=false;self.flags.locale_unicode=false;
        let result=self.set_expression(0);
        let result=result.and_then(|ranges|{self.space();self.require("])")?;Ok(ranges)});
        self.flags=saved;result
    }
    fn set_expression(&mut self, minimum:u8) -> Result<Vec<(u32,u32)>,String> {
        self.space();
        let mut left=if self.eat("!") {let value=self.set_expression(3)?;complement(&value)}
        else if self.eat("(?") {
            let flags=self.flags;self.modifier_flags()?;self.require(":")?;self.require("(?[")?;
            let value=self.extended_ranges()?;self.require(")")?;self.flags=flags;value
        }else if self.eat("(") {let value=self.set_expression(0)?;self.space();self.require(")")?;value}
        else if self.rest().starts_with("[:") {let (ranges,_)=self.class_item()?;self.close_set(ranges)}
        else if self.eat("[") {let (ranges,negative)=self.class_ranges()?;let ranges=self.close_set(ranges);if negative {complement(&ranges)}else{ranges}}
        else if self.rest().starts_with('\\') {let (ranges,_)=self.class_item()?;self.close_set(ranges)}
        else {return Err(self.error("Expected an extended character-class operand"));};
        loop {
            self.space();let Some(operator)=self.rest().chars().next() else {break;};
            let precedence=match operator {'&'=>2,'+'|'|'|'-'|'^'=>1,_=>break};
            if precedence<minimum {break;}
            self.take();let right=self.set_expression(precedence+1)?;
            left=match operator {
                '&'=>intersection(&left,&right),
                '+'|'|'=>{left.extend(right);normalize(&mut left);left},
                '-'=>intersection(&left,&complement(&right)),
                '^'=>{let mut ranges=intersection(&left,&complement(&right));ranges.extend(intersection(&right,&complement(&left)));normalize(&mut ranges);ranges},
                _=>unreachable!(),
            };
        }
        Ok(left)
    }
    fn slots(&self,target:&CaptureGroupTarget) -> Result<Vec<usize>,String> {
        match target {
            CaptureGroupTarget::ByNumber(n)=>if *n<=self.logical {Ok(vec![*n])}else{Err(self.error("Reference to nonexistent capture"))},
            CaptureGroupTarget::ByName(name)=>self.names.get(name).cloned().ok_or_else(||self.error(&format!("Unknown capture name {name}"))),
            CaptureGroupTarget::Relative(_)=>unreachable!("relative references resolved lexically"),
        }
    }
    fn resolve(&mut self,expr:&mut Expr) -> Result<(),String> {
        let replacement=match expr {
            Expr::AstNode(AstNode::SubroutineCall(target),_)=>Some(Expr::SubroutineCall(self.slots(target)?[0])),
            Expr::AstNode(AstNode::Backref {target,casei,..},_)=>{
                let slots=self.slots(target)?;let mut result=Expr::Backref {group:*slots.last().unwrap(),casei:*casei};
                for &group in slots[..slots.len()-1].iter().rev() {result=Expr::Conditional {condition:Box::new(Expr::BackrefExistsCondition {group,relative_recursion_level:None}),true_branch:Box::new(Expr::Backref {group,casei:*casei}),false_branch:Box::new(result)};}
                Some(result)
            },
            Expr::AstNode(AstNode::BackrefExistsCondition {target,..},_)=>{
                let slots=self.slots(target)?;Some(alt(slots.into_iter().map(|group|Expr::BackrefExistsCondition {group,relative_recursion_level:None}).collect()))
            },
            _=>None,
        };
        if let Some(replacement)=replacement {*expr=replacement;return Ok(());}
        for child in expr.children_iter_mut() {self.resolve(child)?;}
        Ok(())
    }
}
