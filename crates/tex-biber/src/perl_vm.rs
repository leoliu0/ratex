//! Perl's ordered, backtracking regex execution, independent of fancy-regex's VM.
//!
//! The parser supplies logical capture numbers separately from physical groups.
//! Execution uses immutable bytecode, a capture undo journal, and arena-indexed
//! continuation frames. In particular, subroutine calls are runtime calls, not
//! statically expanded expressions with a fixed recursion depth.

use std::collections::BTreeSet;

use fancy_regex::{Assertion, BacktrackingControlVerb, Expr, LookAround};

use crate::perl_pattern::{self, ControlVerb, Parsed};
use crate::perl_unicode;

#[derive(Clone, Debug)]
pub struct Regex {
    program: Vec<Instruction>,
    groups: Vec<Option<usize>>,
    capture_count: usize,
    boundary_start_class: bool,
}

#[derive(Clone, Debug)]
enum Instruction {
    End,
    Jump(usize),
    Split { second: usize, trie: bool },
    Literal { value: String, insensitive: bool, ascii_restrict: bool },
    LocaleLiteral(String),
    Class(CharacterClass),
    Delegate(regex::Regex),
    Any { newline: bool, crlf: bool },
    Newline { unicode: bool },
    Assert(Assertion),
    PreviousEnd,
    Keep,
    StartGroup { physical: usize, logical: usize },
    EndGroup { physical: usize, logical: usize },
    Call(usize),
    EnterAlt { end: usize, trie: bool },
    LeaveAlt,
    EnterAtomic { end: usize },
    LeaveAtomic,
    EnterRepeat { min: usize, max: usize, greedy: bool, test: usize, end: usize },
    RepeatTest,
    RepeatEnd,
    EnterLook { kind: LookAround, min: usize, max: usize, end: usize },
    LeaveLook,
    EnterCondition { yes: usize, no: usize },
    LeaveCondition,
    CaptureExists(usize),
    Backref { group: usize, insensitive: bool, ascii_restrict: bool },
    Verb(ControlVerb),
}

#[derive(Clone, Debug)]
struct CharacterClass {
    ranges: Vec<(u32, u32)>,
    extra: BTreeSet<char>,
    sequences: Vec<String>,
    negated: bool,
    ascii_restrict: bool,
    locale: bool,
}

fn contains(ranges: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    let index = ranges.partition_point(|&(_, last)| last < c);
    ranges.get(index).is_some_and(|&(first, last)| first <= c && c <= last)
}

impl CharacterClass {
    fn new(ranges: &[(u32, u32)], negated: bool, insensitive: bool, ascii_restrict: bool, locale: bool) -> Self {
        let mut extra = BTreeSet::new();
        let mut sequences = BTreeSet::new();
        if insensitive {
            // Only nonidentity folds need examining. No scalar-range expansion,
            // Unicode scan, or per-character string allocation at match time.
            let mut folds: [BTreeSet<Vec<char>>; 2] = [BTreeSet::new(), BTreeSet::new()];
            let bucket=|c:char|usize::from(if locale {c as u32<=255}else{ascii_restrict&&c.is_ascii()});
            let permitted=|a:char,b:char|(!ascii_restrict||a.is_ascii()==b.is_ascii())&&(!locale||(a as u32<=255)==(b as u32<=255));
            for &(c, fold) in perl_unicode::nonidentity_folds() {
                if locale&&c as u32<=255&&!c.is_ascii() {continue;}
                if contains(ranges, c) {
                    folds[bucket(c)].insert(fold.to_vec());
                    if fold.len() == 1 && permitted(c,fold[0]) {
                        extra.insert(fold[0]);
                    }
                    if fold.len() > 1 && !negated {
                        sequences.insert(c.to_string());
                    }
                }
            }
            for &(c, fold) in perl_unicode::nonidentity_folds() {
                if locale&&c as u32<=255&&!c.is_ascii() {continue;}
                let folded_member = fold.len() == 1 && contains(ranges, fold[0]) && permitted(fold[0],c);
                if folded_member || folds[bucket(c)].contains(fold) {
                    extra.insert(c);
                }
            }
        }
        Self { ranges: ranges.to_vec(), extra, sequences: sequences.into_iter().collect(), negated, ascii_restrict, locale }
    }

    fn endpoints(&self, value: &str, position: usize, endpoints: &mut Vec<usize>) {
        endpoints.clear();
        if !self.negated {
            for sequence in &self.sequences {
                let matched=if self.locale {perl_unicode::match_folded_locale(sequence,value,position)}else{match_literal(sequence,value,position,true,self.ascii_restrict)};
                if let Some(end) = matched {
                    endpoints.push(end);
                }
            }
        }
        if let Some(c) = value.get(position..).and_then(|s| s.chars().next()) {
            if (contains(&self.ranges, c) || self.extra.contains(&c)) != self.negated {
                endpoints.push(position + c.len_utf8());
            }
        }
        endpoints.sort_unstable_by(|a, b| b.cmp(a));
        endpoints.dedup();
    }
}

struct Compiler<'a> {
    parsed: &'a Parsed,
    program: Vec<Instruction>,
    groups: Vec<Option<usize>>,
    physical: usize,
}

impl Compiler<'_> {
    fn emit(&mut self, instruction: Instruction) -> usize {
        let pc = self.program.len();
        self.program.push(instruction);
        pc
    }

    fn expr(&mut self, expr: &Expr) -> Result<(), String> {
        match expr {
            Expr::Empty => {},
            Expr::Any { newline, crlf } => { self.emit(Instruction::Any { newline: *newline, crlf: *crlf }); },
            Expr::Assertion(assertion) => { self.emit(Instruction::Assert(*assertion)); },
            Expr::GeneralNewline { unicode } => { self.emit(Instruction::Newline { unicode: *unicode }); },
            Expr::Literal { val, casei } => { self.emit(Instruction::Literal { value: val.clone(), insensitive: *casei, ascii_restrict: false }); },
            Expr::Concat(children) => {
                // Perl optimizes adjacent folded literal atoms into one string:
                // ss and [s][s] can match sharp-s, but (s)(s) and s{2} cannot.
                let mut index = 0;
                while index < children.len() {
                    if let Some((mut text, casei, ascii, locale)) = self.literal_atom(&children[index]) {
                        index += 1;
                        loop {
                            let physical = self.physical;
                            let Some(next) = children.get(index).and_then(|child| self.literal_atom(child)) else { break; };
                            if next.1 != casei || next.2 != ascii || next.3 != locale { self.physical = physical; break; }
                            text.push_str(&next.0);
                            index += 1;
                        }
                        self.emit(if locale==1&&casei {Instruction::LocaleLiteral(text)}else{Instruction::Literal { value: text, insensitive: casei, ascii_restrict: ascii }});
                    } else {
                        self.expr(&children[index])?;
                        index += 1;
                    }
                }
            },
            Expr::Alt(children) => self.alternation(children)?,
            Expr::Group(child) => {
                self.physical += 1;
                let physical = self.physical;
                if let Some(verb) = self.parsed.controls.get(&physical) {
                    self.control(verb)?;
                } else {
                    let logical = *self.parsed.capture_aliases.get(physical).ok_or_else(|| "Missing physical capture alias".to_owned())?;
                    let entry = self.program.len();
                    if self.groups[logical].is_none() { self.groups[logical] = Some(entry); }
                    self.emit(Instruction::StartGroup { physical, logical });
                    self.expr(child)?;
                    self.emit(Instruction::EndGroup { physical, logical });
                }
            },
            Expr::LookAround(child, kind) => self.look(child, *kind)?,
            Expr::Repeat { child, lo, hi, greedy } => {
                let enter = self.emit(Instruction::EnterRepeat { min: *lo, max: *hi, greedy: *greedy, test: 0, end: 0 });
                let test = self.emit(Instruction::RepeatTest);
                self.expr(child)?;
                self.emit(Instruction::RepeatEnd);
                let end = self.program.len();
                self.program[enter] = Instruction::EnterRepeat { min: *lo, max: *hi, greedy: *greedy, test, end };
            },
            Expr::Delegate { inner, casei } => {
                let pattern = if *casei { format!("\\A(?i:{inner})\\z") } else { format!("\\A(?:{inner})\\z") };
                let regex = regex::Regex::new(&pattern).map_err(|error| error.to_string())?;
                self.emit(Instruction::Delegate(regex));
            },
            Expr::Backref { group, casei } => { self.emit(Instruction::Backref { group: *group, insensitive: *casei, ascii_restrict: false }); },
            Expr::AtomicGroup(child) => {
                let enter=self.emit(Instruction::EnterAtomic {end:0});
                self.expr(child)?;
                let end=self.emit(Instruction::LeaveAtomic);
                self.program[enter]=Instruction::EnterAtomic {end};
            },
            Expr::KeepOut => { self.emit(Instruction::Keep); },
            Expr::ContinueFromPreviousMatchEnd => { self.emit(Instruction::PreviousEnd); },
            Expr::BackrefExistsCondition { group, relative_recursion_level: None } => { self.emit(Instruction::CaptureExists(*group)); },
            Expr::Conditional { condition, true_branch, false_branch } => {
                let enter = self.emit(Instruction::EnterCondition { yes: 0, no: 0 });
                self.expr(condition)?;
                self.emit(Instruction::LeaveCondition);
                let yes = self.program.len();
                self.expr(true_branch)?;
                let jump = self.emit(Instruction::Jump(0));
                let no = self.program.len();
                self.expr(false_branch)?;
                let end = self.program.len();
                self.program[jump] = Instruction::Jump(end);
                self.program[enter] = Instruction::EnterCondition { yes, no };
            },
            Expr::SubroutineCall(group) => { self.emit(Instruction::Call(*group)); },
            Expr::DefineGroup { definitions } => {
                let jump = self.emit(Instruction::Jump(0));
                self.expr(definitions)?;
                self.program[jump] = Instruction::Jump(self.program.len());
            },
            Expr::BacktrackingControlVerb(verb) => self.control(&match verb {
                BacktrackingControlVerb::Accept => ControlVerb::Accept,
                BacktrackingControlVerb::Fail => ControlVerb::Fail,
                BacktrackingControlVerb::Commit => ControlVerb::Commit(None),
                BacktrackingControlVerb::Prune => ControlVerb::Prune(None),
                BacktrackingControlVerb::Skip => ControlVerb::Skip(None),
            })?,
            Expr::BackrefWithRelativeRecursionLevel { .. } | Expr::BackrefExistsCondition { .. } => return Err("Recursion-level-qualified references are not Perl syntax".to_owned()),
            Expr::Absent(_) => return Err("Absent operators are not Perl syntax".to_owned()),
            Expr::AstNode(..) => return Err("Unresolved regex parser node".to_owned()),
        }
        Ok(())
    }

    fn trie_prefix(&self, expr:&Expr, physical:usize)->Option<bool> {
        match expr {
            Expr::Literal {val,casei} if !val.is_empty()=>Some(*casei),
            Expr::Concat(parts)=>parts.first().and_then(|part|self.trie_prefix(part,physical)),
            Expr::Group(_)=>match self.parsed.controls.get(&(physical+1)) {
                Some(ControlVerb::CharacterClass {ranges,negated:false,casei,ascii_restrict:false,locale_pattern:false,..}) if ranges.len()==1&&ranges[0].0==ranges[0].1=>Some(*casei),
                _=>None,
            },
            _=>None,
        }
    }
    fn alternation(&mut self,children:&[Expr])->Result<(),String> {
        // Perl compiles adjacent compatible EXACT prefixes into TRIE nodes.
        // Those choices are not BRANCH frames: THEN skips the complete trie,
        // not its next word, including tries inside a larger alternation.
        let mut physical=self.physical;
        let keys:Vec<_>=children.iter().map(|child| {
            let key=self.trie_prefix(child,physical);
            widths(child,self.parsed,&mut physical);key
        }).collect();
        let trie=keys.len()>1&&keys[0].is_some()&&keys.iter().all(|key|*key==keys[0]);
        let mut units=Vec::new();let mut index=0;
        while index<children.len() {
            let mut end=index+1;
            if !trie&&keys[index].is_some() {while end<children.len()&&keys[end]==keys[index] {end+=1;}}
            units.push((index,end));index=end;
        }
        let enter=self.emit(Instruction::EnterAlt {end:0,trie});
        let mut jumps=Vec::new();
        for (index,&(start,end)) in units.iter().enumerate() {
            let split=(index+1<units.len()).then(||self.emit(Instruction::Split {second:0,trie}));
            if end-start>1 {self.alternation(&children[start..end])?;}else{self.expr(&children[start])?;}
            if let Some(split)=split {
                jumps.push(self.emit(Instruction::Jump(0)));
                self.program[split]=Instruction::Split {second:self.program.len(),trie};
            }
        }
        let end=self.emit(Instruction::LeaveAlt);
        self.program[enter]=Instruction::EnterAlt {end,trie};
        for jump in jumps {self.program[jump]=Instruction::Jump(end);}
        Ok(())
    }
    fn look(&mut self, child: &Expr, kind: LookAround) -> Result<(), String> {
        let mut physical = self.physical;
        let (min, max) = widths(child, self.parsed, &mut physical);
        if matches!(kind, LookAround::LookBehind | LookAround::LookBehindNeg) && max > 255 {
            return Err("Perl lookbehind requires a maximum width of at most 255 characters".to_owned());
        }
        let enter = self.emit(Instruction::EnterLook { kind, min, max, end: 0 });
        self.expr(child)?;
        if self.program[enter + 1..].iter().any(|instruction| matches!(instruction, Instruction::Keep)) {
            return Err("\\K is not permitted in Perl lookaround assertions".to_owned());
        }
        let mut cursor=enter+1;let mut accepts=false;
        while cursor<self.program.len() {
            match self.program[cursor] {
                Instruction::Verb(ControlVerb::Accept)=>{accepts=true;break;},
                Instruction::EnterAtomic {end}=>cursor=end+1,
                Instruction::EnterLook {end,..}=>cursor=end,
                _=>cursor+=1,
            }
        }
        let min=if accepts {0}else{min};
        self.emit(Instruction::LeaveLook);
        let end = self.program.len();
        self.program[enter] = Instruction::EnterLook { kind, min, max, end };
        Ok(())
    }

    fn literal_atom(&mut self, expr: &Expr) -> Option<(String, bool, bool, u8)> {
        if let Expr::Literal { val, casei } = expr {
            return Some((val.clone(), *casei, false, 0));
        }
        if let Expr::Group(_) = expr {
            match self.parsed.controls.get(&(self.physical + 1)) {
                Some(ControlVerb::AsciiInsensitiveLiteral(text)) => {
                    self.physical += 1;
                    return Some((text.clone(), true, true, 0));
                },
                Some(ControlVerb::LocaleUnicodeLiteral(text)) => {
                    self.physical+=1;
                    return Some((text.clone(),true,false,2));
                },
                Some(ControlVerb::LocaleInsensitiveLiteral(text)) => {
                    self.physical+=1;
                    return Some((text.clone(),true,false,1));
                },
                Some(ControlVerb::CharacterClass { ranges, negated: false, casei, ascii_restrict, locale, locale_pattern }) if ranges.len() == 1 && ranges[0].0 == ranges[0].1 => {
                    self.physical += 1;
                    let mode=if *casei&&*locale_pattern {if *locale {1}else{2}}else{0};
                    return Some((char::from_u32(ranges[0].0)?.to_string(), *casei, *ascii_restrict, mode));
                },
                _ => {},
            }
        }
        None
    }

    fn control(&mut self, verb: &ControlVerb) -> Result<(), String> {
        match verb {
            ControlVerb::CharacterClass { ranges, negated, casei, ascii_restrict, locale, .. } => {
                self.emit(Instruction::Class(CharacterClass::new(ranges, *negated, *casei, *ascii_restrict, *locale)));
            },
            ControlVerb::LocaleUnicodeLiteral(value)=>{self.emit(Instruction::Literal {value:value.clone(),insensitive:true,ascii_restrict:false});},
            ControlVerb::LocaleInsensitiveLiteral(value) => {self.emit(Instruction::LocaleLiteral(value.clone()));},
            ControlVerb::AsciiInsensitiveLiteral(value) => {
                self.emit(Instruction::Literal { value: value.clone(), insensitive: true, ascii_restrict: true });
            },
            ControlVerb::AsciiInsensitiveBackref(group) => {
                self.emit(Instruction::Backref { group: *group, insensitive: true, ascii_restrict: true });
            },
            _ => { self.emit(Instruction::Verb(verb.clone())); },
        }
        Ok(())
    }
}

fn folded_atom_width(expr:&Expr,parsed:&Parsed,physical:usize)->Option<(usize,u8)> {
    let folded=|text:&str,locale:bool|text.chars().map(|c|if locale&&c as u32<=255 {1}else{perl_unicode::fold_chars(c).len()}).sum();
    match expr {
        Expr::Literal {val,casei:true}=>Some((folded(val,false),0)),
        Expr::Group(_)=>match parsed.controls.get(&(physical+1)) {
            Some(ControlVerb::AsciiInsensitiveLiteral(text))=>Some((folded(text,false),1)),
            Some(ControlVerb::LocaleInsensitiveLiteral(text))=>Some((folded(text,true),2)),
            Some(ControlVerb::LocaleUnicodeLiteral(text))=>Some((folded(text,false),3)),
            Some(ControlVerb::CharacterClass {ranges,negated:false,casei:true,ascii_restrict,locale,locale_pattern}) if ranges.len()==1&&ranges[0].0==ranges[0].1=>{
                let c=char::from_u32(ranges[0].0)?;
                let width=if *locale&&c as u32<=255 {1}else{perl_unicode::fold_chars(c).len()};
                Some((width,if *ascii_restrict {1}else if *locale {2}else if *locale_pattern {3}else{0}))
            },
            _=>None,
        },
        _=>None,
    }
}

fn widths(expr: &Expr, parsed: &Parsed, physical: &mut usize) -> (usize, usize) {
    match expr {
        Expr::Literal { val, casei: false } => { let n = val.chars().count(); (n, n) },
        Expr::Literal { val, casei: true } => { let max = val.chars().map(|c| perl_unicode::fold_chars(c).len()).sum::<usize>(); (max.div_ceil(3), max) },
        Expr::Any { .. } | Expr::Delegate { .. } => (1, 1),
        Expr::GeneralNewline { .. } => (1, 2),
        Expr::Concat(children) => {
            let (mut min,mut max)=(0usize,0usize);let mut run:Option<(usize,u8)>=None;
            for child in children {
                let atom=folded_atom_width(child,parsed,*physical);
                let width=widths(child,parsed,physical);max=max.saturating_add(width.1);
                if let Some((length,mode))=atom {
                    if let Some((count,old_mode))=run {
                        if old_mode==mode {run=Some((count+length,mode));continue;}
                        min=min.saturating_add(count.div_ceil(3));
                    }
                    run=Some((length,mode));
                }else{
                    if let Some((count,_))=run.take() {min=min.saturating_add(count.div_ceil(3));}
                    min=min.saturating_add(width.0);
                }
            }
            if let Some((count,_))=run {min=min.saturating_add(count.div_ceil(3));}
            (min,max)
        },
        Expr::Alt(children) => children.iter().map(|child| widths(child, parsed, physical)).fold((usize::MAX, 0), |a, b| (a.0.min(b.0), a.1.max(b.1))),
        Expr::Group(child) => {
            *physical += 1;
            if let Some(control) = parsed.controls.get(physical) {
                match control {
                    ControlVerb::CharacterClass { ranges, negated, casei, .. } => {
                        let max = if *casei && !*negated { perl_unicode::nonidentity_folds().iter().filter(|&&(c, _)| contains(ranges, c)).map(|&(_, fold)| fold.len()).max().unwrap_or(1) } else { 1 };
                        (1, max)
                    },
                    ControlVerb::AsciiInsensitiveLiteral(text) | ControlVerb::LocaleUnicodeLiteral(text) => {let max=text.chars().map(|c|perl_unicode::fold_chars(c).len()).sum::<usize>();(max.div_ceil(3),max)},
                    ControlVerb::LocaleInsensitiveLiteral(text)=>{let max=text.chars().map(|c|if c as u32<=255 {1}else{perl_unicode::fold_chars(c).len()}).sum::<usize>();(max.div_ceil(3),max)},
                    ControlVerb::Grapheme | ControlVerb::AsciiInsensitiveBackref(_) | ControlVerb::AsciiInsensitiveBackrefs(_) | ControlVerb::LocaleInsensitiveBackrefs(_) => (0, usize::MAX),
                    _ => (0, 0),
                }
            } else { widths(child, parsed, physical) }
        },
        Expr::AtomicGroup(child) => widths(child, parsed, physical),
        Expr::Repeat { child, lo, hi, .. } => { let (min, max) = widths(child, parsed, physical); (min.saturating_mul(*lo), max.saturating_mul(*hi)) },
        Expr::Backref { .. } | Expr::SubroutineCall(_) => (0, usize::MAX),
        Expr::Conditional { condition, true_branch, false_branch } => {
            widths(condition, parsed, physical);
            let a = widths(true_branch, parsed, physical);
            let b = widths(false_branch, parsed, physical);
            (a.0.min(b.0), a.1.max(b.1))
        },
        Expr::LookAround(child, _) | Expr::DefineGroup { definitions: child } => { widths(child, parsed, physical); (0, 0) },
        _ => (0, 0),
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Match<'a> {
    value: &'a str,
    start: usize,
    end: usize,
}

impl<'a> Match<'a> {
    pub fn start(&self) -> usize { self.start }
    pub fn end(&self) -> usize { self.end }
    pub fn as_str(&self) -> &'a str { &self.value[self.start..self.end] }
}

#[derive(Debug)]
pub struct Captures<'a> {
    value: &'a str,
    slots: Vec<Option<(usize, usize)>>,
    last_closed: Option<(usize, usize)>,
}

impl<'a> Captures<'a> {
    pub fn len(&self) -> usize { self.slots.len() }
    pub fn get(&self, index: usize) -> Option<Match<'a>> {
        self.slots.get(index).copied().flatten().map(|(start, end)| Match { value: self.value, start, end })
    }
    pub fn last_closed(&self) -> Option<Match<'a>> {
        self.last_closed.map(|(start, end)| Match { value: self.value, start, end })
    }
}

fn end_anchored(expr: &Expr) -> bool {
    match expr {
        Expr::Assertion(Assertion::EndText | Assertion::EndTextIgnoreTrailingNewlines { .. } | Assertion::EndLine { .. }) => true,
        Expr::Concat(parts) => parts.last().is_some_and(end_anchored),
        Expr::Alt(parts) => parts.iter().all(end_anchored),
        Expr::Group(child) => end_anchored(child),
        Expr::AtomicGroup(child) | Expr::LookAround(child, LookAround::LookAhead) => end_anchored(child),
        Expr::Repeat { child, lo, .. } => *lo > 0 && end_anchored(child),
        Expr::Conditional { true_branch, false_branch, .. } => end_anchored(true_branch) && end_anchored(false_branch),
        _ => false,
    }
}

impl Regex {
    pub fn new(pattern: &str) -> Result<Self, String> {
        let parsed = perl_pattern::parse(pattern)?;
        let mut compiler = Compiler {
            parsed: &parsed,
            program: Vec::new(),
            groups: vec![None; parsed.capture_count + 1],
            physical: 0,
        };
        compiler.groups[0] = Some(0);
        compiler.expr(&parsed.expr)?;
        compiler.emit(Instruction::End);
        for instruction in &compiler.program {
            if let Instruction::Call(group) = instruction {
                if compiler.groups.get(*group).and_then(|entry| *entry).is_none() {
                    return Err(format!("Reference to nonexistent subroutine {group}"));
                }
            }
        }
        // Perl's BOUNDU start-class scanner retries the initial boundary
        // by advancing a character, and stops immediately if this reaches
        // the target end (regexec.c: do_boundu_utf8). End-anchored patterns
        // use their end-position search instead.
        let boundary_start_class = !end_anchored(&parsed.expr) && compiler.program.iter().find(|instruction| !matches!(instruction,
            Instruction::StartGroup { .. } | Instruction::EnterLook { kind: LookAround::LookAhead, .. }
        )).is_some_and(|instruction| matches!(instruction, Instruction::Verb(ControlVerb::ExtendedBoundary { negative: false, .. })));
        Ok(Self {
            program: compiler.program,
            groups: compiler.groups,
            capture_count: parsed.capture_count,
            boundary_start_class,
        })
    }

    pub fn is_match(&self, value: &str) -> Result<bool, String> {
        Ok(self.search(value, 0, 0, false)?.is_some())
    }

    pub fn captures_iter<'a>(&'a self, value: &'a str) -> CaptureIter<'a> {
        CaptureIter { regex: self, value, position: 0, previous_end: 0, retry_nonempty: false, done: false }
    }

    pub fn find_iter<'a>(&'a self, value: &'a str) -> impl Iterator<Item = Result<Match<'a>, String>> + 'a {
        self.captures_iter(value).map(|result| result.map(|captures| captures.get(0).expect("Successful regex has capture zero")))
    }

    fn search<'a>(&self, value: &'a str, from: usize, previous_end: usize, reject_empty: bool) -> Result<Option<Captures<'a>>, String> {
        let mut state = State::new(self, value, previous_end);
        let mut start = from;
        loop {
            state.reset(start);
            match state.run(reject_empty && start == from)? {
                Outcome::Match => return Ok(Some(Captures { value, slots: state.captures, last_closed: state.last_closed })),
                Outcome::Commit => return Ok(None),
                Outcome::Skip(position) if position > start => { start = position; },
                _ => {
                    if start == value.len() { return Ok(None); }
                    let initial = start == 0;
                    start += value[start..].chars().next().expect("Character boundary").len_utf8();
                    if initial && self.boundary_start_class && start >= value.len() { return Ok(None); }
                },
            }
            if start > value.len() { return Ok(None); }
        }
    }
}

pub struct CaptureIter<'a> {
    regex: &'a Regex,
    value: &'a str,
    position: usize,
    previous_end: usize,
    retry_nonempty: bool,
    done: bool,
}

impl<'a> Iterator for CaptureIter<'a> {
    type Item = Result<Captures<'a>, String>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done { return None; }
        match self.regex.search(self.value, self.position, self.previous_end, self.retry_nonempty) {
            Ok(Some(captures)) => {
                let matched = captures.get(0).expect("Successful regex has capture zero");
                self.position = matched.end;
                self.previous_end = matched.end;
                self.retry_nonempty = matched.start == matched.end;
                Some(Ok(captures))
            },
            Ok(None) => { self.done = true; None },
            Err(error) => { self.done = true; Some(Err(error)) },
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum FrameKind {
    Call { target: usize, physical: usize, position: usize, return_pc: usize, journal: usize },
    Group { physical: usize, logical: usize, start: usize },
    Alt { end: usize, trie: bool },
    Atomic { choices: usize, end: usize, mark: Option<usize> },
    Repeat { min: usize, max: usize, greedy: bool, count: usize, previous: usize, test: usize, end: usize },
    Look { kind: LookAround, original: usize, end: usize, choices: usize, journal: usize, mark: Option<usize>, script_start: usize },
    Condition { yes: usize, no: usize, original: usize, choices: usize },
}

#[derive(Copy, Clone, Debug)]
struct Frame { previous: Option<usize>, kind: FrameKind }

#[derive(Clone, Debug)]
enum ChoiceKind {
    Resume(usize),
    Branch(usize),
    TrieBranch(usize),
    LookFailed,
    ConditionFailed,
    Cut { pc: usize, skip: usize },
}

#[derive(Clone, Debug)]
struct Choice {
    kind: ChoiceKind,
    position: usize,
    journal: usize,
    frame: Option<usize>,
    match_start: usize,
    mark: Option<usize>,
    script_start: usize,
    alt: Option<usize>,
}

#[derive(Copy, Clone)]
enum Undo {
    Capture(usize, Option<(usize, usize)>),
    LastClosed(Option<(usize, usize)>),
}

struct Mark<'r> { name: &'r str, position: usize, previous: Option<usize> }

enum Outcome { Match, Fail, Skip(usize), Commit }

struct State<'r, 'a> {
    regex: &'r Regex,
    value: &'a str,
    pc: usize,
    position: usize,
    attempt_start: usize,
    match_start: usize,
    previous_end: usize,
    captures: Vec<Option<(usize, usize)>>,
    last_closed: Option<(usize, usize)>,
    journal: Vec<Undo>,
    choices: Vec<Choice>,
    frames: Vec<Frame>,
    frame: Option<usize>,
    marks: Vec<Mark<'r>>,
    mark: Option<usize>,
    script_start: usize,
    endpoints: Vec<usize>,
    restore_seen: Vec<usize>,
    restore_generation: usize,
}

impl<'r, 'a> State<'r, 'a> {
    fn new(regex: &'r Regex, value: &'a str, previous_end: usize) -> Self {
        Self {
            regex, value, pc: 0, position: 0, attempt_start: 0, match_start: 0, previous_end,
            captures: vec![None; regex.capture_count + 1], last_closed: None,
            journal: Vec::new(), choices: Vec::new(), frames: Vec::new(), frame: None,
            marks: Vec::new(), mark: None, script_start:0, endpoints: Vec::new(),
            restore_seen: vec![0; regex.capture_count + 1], restore_generation: 0,
        }
    }

    fn reset(&mut self, start: usize) {
        self.rollback(0);
        self.pc = 0;
        self.position = start;
        self.attempt_start = start;
        self.match_start = start;
        self.last_closed = None;
        self.choices.clear();
        self.frames.clear();
        self.frame = None;
        self.marks.clear();
        self.mark = None;
        self.script_start=0;
    }

    fn rollback(&mut self, checkpoint: usize) {
        while self.journal.len() > checkpoint {
            match self.journal.pop().expect("Journal entry") {
                Undo::Capture(slot, old) => self.captures[slot] = old,
                Undo::LastClosed(old) => self.last_closed = old,
            }
        }
    }

    fn capture(&mut self, slot: usize, value: Option<(usize, usize)>) {
        self.journal.push(Undo::Capture(slot, self.captures[slot]));
        self.captures[slot] = value;
    }

    fn close(&mut self, value: Option<(usize, usize)>) {
        self.journal.push(Undo::LastClosed(self.last_closed));
        self.last_closed = value;
    }

    fn restore_call(&mut self, checkpoint: usize) {
        // At most one restoring write per logical capture. Journal restoration
        // must not grow exponentially when returning through deep recursion.
        self.restore_generation = self.restore_generation.wrapping_add(1);
        if self.restore_generation == 0 {
            self.restore_seen.fill(0);
            self.restore_generation = 1;
        }
        let generation = self.restore_generation;
        let end = self.journal.len();
        let mut restored_last = false;
        for index in checkpoint..end {
            match self.journal[index] {
                Undo::Capture(slot, old) if self.restore_seen[slot] != generation => {
                    self.restore_seen[slot] = generation;
                    self.capture(slot, old);
                },
                Undo::LastClosed(old) if !restored_last => {
                    restored_last = true;
                    self.close(old);
                },
                _ => {},
            }
        }
    }

    fn push_frame(&mut self, kind: FrameKind) {
        let index = self.frames.len();
        self.frames.push(Frame { previous: self.frame, kind });
        self.frame = Some(index);
    }

    fn top(&self) -> Frame { self.frames[self.frame.expect("Bytecode continuation frame")] }

    fn enclosing_alt(&self) -> Option<usize> {
        let mut current = self.frame;
        while let Some(index) = current {
            match self.frames[index].kind {
                FrameKind::Alt { trie: false, .. } => return Some(index),
                FrameKind::Call { .. } | FrameKind::Look { .. } | FrameKind::Condition { .. } => return None,
                _ => current = self.frames[index].previous,
            }
        }
        None
    }

    fn choice(&mut self, kind: ChoiceKind) {
        self.choices.push(Choice {
            kind, position: self.position, journal: self.journal.len(), frame: self.frame,
            match_start: self.match_start, mark: self.mark, script_start:self.script_start, alt: self.enclosing_alt(),
        });
    }

    fn restore_choice(&mut self, choice: &Choice) {
        self.rollback(choice.journal);
        self.position = choice.position;
        self.frame = choice.frame;
        self.match_start = choice.match_start;
        self.mark = choice.mark;
        self.script_start=choice.script_start;
    }

    fn add_mark(&mut self, name: &'r str) {
        let index = self.marks.len();
        self.marks.push(Mark { name, position: self.position, previous: self.mark });
        self.mark = Some(index);
    }

    fn mark_position(&self, name: &str) -> Option<usize> {
        let mut current = self.mark;
        while let Some(index) = current {
            let mark = &self.marks[index];
            if mark.name == name { return Some(mark.position); }
            current = mark.previous;
        }
        None
    }

    fn finish_call(&mut self) {
        let frame = self.top();
        if let FrameKind::Call { return_pc, journal, .. } = frame.kind {
            self.restore_call(journal);
            self.frame = frame.previous;
            self.pc = return_pc;
        } else { unreachable!("Subroutine return frame") }
    }
    fn finish_look(&mut self, accepted: bool) -> bool {
        let frame = self.top();
        let FrameKind::Look { kind, original, end, choices, journal, mark, script_start } = frame.kind else { unreachable!("Lookaround frame") };
        if !accepted && matches!(kind, LookAround::LookBehind | LookAround::LookBehindNeg) && self.position != original { return false; }
        self.choices.truncate(choices);
        self.frame = frame.previous;
        self.position = original;
        self.mark = mark;
        self.script_start=script_start;
        if matches!(kind, LookAround::LookAheadNeg | LookAround::LookBehindNeg) {
            self.rollback(journal);
            return false;
        }
        self.pc = end;
        true
    }

    fn fail(&mut self) -> Option<Outcome> {
        while let Some(choice) = self.choices.pop() {
            self.restore_choice(&choice);
            match choice.kind {
                ChoiceKind::Resume(pc) | ChoiceKind::Branch(pc) | ChoiceKind::TrieBranch(pc) => { self.pc = pc; return None; },
                ChoiceKind::ConditionFailed => {
                    let frame = self.top();
                    if let FrameKind::Condition { no, original, .. } = frame.kind {
                        self.frame = frame.previous;
                        self.position = original;
                        self.pc = no;
                        return None;
                    }
                    unreachable!("Conditional failure frame")
                },
                ChoiceKind::LookFailed => {
                    let frame = self.top();
                    if let FrameKind::Look { kind, original, end, .. } = frame.kind {
                        self.frame = frame.previous;
                        self.position = original;
                        if matches!(kind, LookAround::LookAheadNeg | LookAround::LookBehindNeg) {
                            self.pc = end;
                            return None;
                        }
                    } else { unreachable!("Lookaround failure frame") }
                },
                ChoiceKind::Cut { pc, skip } => {
                    let Instruction::Verb(verb)=&self.regex.program[pc] else {unreachable!("Cut verb instruction")};
                    // In a negative assertion, a cut makes the assertion body
                    // fail (hence the assertion succeed), but still forbids
                    // retrying the containing pattern if its tail fails.
                    let mut local = self.frame;
                    while let Some(index) = local {
                        let frame = self.frames[index];
                        if let FrameKind::Look { kind, original, end, choices, journal, mark, script_start } = frame.kind {
                            if matches!(kind, LookAround::LookAheadNeg | LookAround::LookBehindNeg)
                                && !matches!(verb, ControlVerb::Then(_)) {
                                self.choices.truncate(choices);
                                self.rollback(journal);
                                self.position = original;
                                self.frame = frame.previous;
                                self.mark = mark;
                                self.script_start=script_start;
                                self.choice(ChoiceKind::Cut { pc, skip });
                                self.pc = end;
                                return None;
                            }
                            break;
                        }
                        if matches!(frame.kind, FrameKind::Call { .. }) { break; }
                        local = frame.previous;
                    }
                    match &verb {
                    ControlVerb::Commit(_) => return Some(Outcome::Commit),
                    ControlVerb::Prune(_) => return Some(Outcome::Fail),
                    ControlVerb::Skip(_) => return Some(Outcome::Skip(skip)),
                    ControlVerb::Then(_) => {
                        if let Some(alt) = choice.alt {
                            let end = match self.frames[alt].kind { FrameKind::Alt { end, .. } => end, _ => unreachable!() };
                            while self.choices.last().is_some_and(|previous| previous.frame.is_some_and(|mut index| {
                                loop { if index == alt { return true; } match self.frames[index].previous { Some(parent) => index = parent, None => return false } }
                            }) && (!matches!(previous.kind, ChoiceKind::Branch(pc) if pc <= end) || previous.alt != Some(alt))) { self.choices.pop(); }
                            if self.choices.last().is_some_and(|previous| previous.alt == Some(alt)) { continue; }
                            // Exhausting the enclosing alternation fails that
                            // group, not the entire outer pattern.
                            continue;
                        }
                        let mut local = self.frame;
                        while let Some(index) = local {
                            let frame = self.frames[index];
                            if matches!(frame.kind, FrameKind::Look { .. } | FrameKind::Call { .. }) { break; }
                            local = frame.previous;
                        }
                        if let Some(local) = local {
                            while self.choices.last().is_some_and(|previous| previous.frame.is_some_and(|mut index| loop {
                                if index == local { return true; }
                                match self.frames[index].previous { Some(parent) => index = parent, None => return false }
                            })) { self.choices.pop(); }
                            // THEN aborts a lookaround, irrespective of its
                            // polarity, or the local subroutine invocation.
                            continue;
                        }
                        return Some(Outcome::Fail);
                    },
                    _ => unreachable!("Cut control verb"),
                    }
                },
            }
        }
        Some(Outcome::Fail)
    }

    fn run(&mut self, reject_empty: bool) -> Result<Outcome, String> {
        loop {
            let instruction = &self.regex.program[self.pc];
            let mut failed = false;
            match instruction {
                Instruction::End => {
                    if self.frame.is_some_and(|index| matches!(self.frames[index].kind, FrameKind::Call { target: 0, .. })) {
                        self.finish_call();
                        continue;
                    }
                    if reject_empty && self.position == self.attempt_start { failed = true; }
                    else {
                        self.capture(0, Some((self.match_start, self.position)));
                        return Ok(Outcome::Match);
                    }
                },
                Instruction::Jump(pc) => { self.pc = *pc; continue; },
                Instruction::Split { second, trie } => self.choice(if *trie { ChoiceKind::TrieBranch(*second) } else { ChoiceKind::Branch(*second) }),
                Instruction::LocaleLiteral(value) => {
                    if let Some(end)=perl_unicode::match_folded_locale(value,self.value,self.position) {self.position=end;}else{failed=true;}
                },
                Instruction::Literal { value, insensitive, ascii_restrict } => {
                    if let Some(end) = match_literal(value, self.value, self.position, *insensitive, *ascii_restrict) { self.position = end; }
                    else { failed = true; }
                },
                Instruction::Class(class) => {
                    class.endpoints(self.value, self.position, &mut self.endpoints);
                    if let Some(&first) = self.endpoints.first() {
                        let position = self.position;
                        for index in (1..self.endpoints.len()).rev() {
                            self.position = self.endpoints[index];
                            self.choice(ChoiceKind::Resume(self.pc + 1));
                        }
                        self.position = first;
                        debug_assert!(first >= position);
                    } else { failed = true; }
                },
                Instruction::Delegate(regex) => {
                    if let Some(c) = self.value[self.position..].chars().next() {
                        let end = self.position + c.len_utf8();
                        if regex.is_match(&self.value[self.position..end]) { self.position = end; }
                        else { failed = true; }
                    } else { failed = true; }
                },
                Instruction::Any { newline, crlf } => {
                    if let Some(c) = self.value[self.position..].chars().next() {
                        if *newline || (c != '\n' && (!*crlf || c != '\r')) { self.position += c.len_utf8(); }
                        else { failed = true; }
                    } else { failed = true; }
                },
                Instruction::Newline { unicode } => {
                    let suffix = &self.value[self.position..];
                    if suffix.starts_with("\r\n") { self.position += 2; }
                    else if let Some(c) = suffix.chars().next() {
                        if matches!(c, '\n' | '\r' | '\u{b}' | '\u{c}') || (*unicode && matches!(c, '\u{85}' | '\u{2028}' | '\u{2029}')) { self.position += c.len_utf8(); }
                        else { failed = true; }
                    } else { failed = true; }
                },
                Instruction::Assert(assertion) => { failed = !assertion_matches(*assertion, self.value, self.position); },
                Instruction::PreviousEnd => { failed = self.position != self.previous_end; },
                Instruction::Keep => self.match_start = self.position,
                Instruction::StartGroup { physical, logical } => {
                    self.push_frame(FrameKind::Group { physical: *physical, logical: *logical, start: self.position });
                },
                Instruction::EndGroup { physical, logical } => {
                    let frame = self.top();
                    let FrameKind::Group { start, physical: open, .. } = frame.kind else { unreachable!("Open capture frame") };
                    debug_assert_eq!(open, *physical);
                    self.frame = frame.previous;
                    let value = Some((start, self.position));
                    self.capture(*logical, value);
                    self.close(value);
                    if self.frame.is_some_and(|index| matches!(self.frames[index].kind, FrameKind::Call { physical: target, .. } if target == *physical)) {
                        self.finish_call();
                        continue;
                    }
                },
                Instruction::Call(group) => {
                    let mut frame = self.frame;
                    while let Some(index) = frame {
                        let entry = self.frames[index];
                        if let FrameKind::Call { target, position, .. } = entry.kind {
                            if target == *group && position == self.position {
                                return Err("Infinite recursion in Perl regular expression".to_owned());
                            }
                        }
                        frame = entry.previous;
                    }
                    let target = self.regex.groups[*group].expect("Validated subroutine target");
                    let physical = match self.regex.program[target] {
                        Instruction::StartGroup { physical, .. } => physical,
                        _ => 0,
                    };
                    self.push_frame(FrameKind::Call { target: *group, physical, position: self.position, return_pc: self.pc + 1, journal: self.journal.len() });
                    self.pc = target;
                    continue;
                },
                Instruction::EnterAlt { end, trie } => self.push_frame(FrameKind::Alt { end: *end, trie: *trie }),
                Instruction::LeaveAlt => { let frame = self.top(); debug_assert!(matches!(frame.kind, FrameKind::Alt { .. })); self.frame = frame.previous; },
                Instruction::EnterAtomic {end} => self.push_frame(FrameKind::Atomic { choices: self.choices.len(), end:*end, mark:self.mark }),
                Instruction::LeaveAtomic => {
                    let frame = self.top();
                    if let FrameKind::Atomic { choices, mark, .. } = frame.kind {
                        self.choices.truncate(choices);
                        self.mark=mark;
                    }
                    else { unreachable!("Atomic frame") }
                    self.frame = frame.previous;
                },
                Instruction::EnterRepeat { min, max, greedy, test, end } => {
                    self.push_frame(FrameKind::Repeat { min: *min, max: *max, greedy: *greedy, count: 0, previous: self.position, test: *test, end: *end });
                },
                Instruction::RepeatTest => {
                    let frame = self.top();
                    if let FrameKind::Repeat { min, max, greedy, count, end, .. } = frame.kind {
                        if count >= max {
                            self.frame = frame.previous;
                            self.pc = end;
                            continue;
                        }
                        if count >= min {
                            if greedy {
                                let current = self.frame;
                                self.frame = frame.previous;
                                self.choice(ChoiceKind::Resume(end));
                                self.frame = current;
                            } else {
                                self.choice(ChoiceKind::Resume(self.pc + 1));
                                self.frame = frame.previous;
                                self.pc = end;
                                continue;
                            }
                        }
                    } else { unreachable!("Repeat frame") }
                },
                Instruction::RepeatEnd => {
                    let frame = self.top();
                    if let FrameKind::Repeat { min, max, greedy, count, previous, test, end } = frame.kind {
                        if self.position == previous && count + 1 >= min {
                            self.frame = frame.previous;
                            self.pc = end;
                        } else {
                            self.frame = frame.previous;
                            self.push_frame(FrameKind::Repeat { min, max, greedy, count: count + 1, previous: self.position, test, end });
                            self.pc = test;
                        }
                        continue;
                    } else { unreachable!("Repeat frame") }
                },
                Instruction::EnterLook { kind, min, max, end } => {
                    let original = self.position;
                    self.push_frame(FrameKind::Look { kind: *kind, original, end: *end, choices: self.choices.len(), journal: self.journal.len(), mark: self.mark, script_start:self.script_start });
                    self.choice(ChoiceKind::LookFailed);
                    if matches!(kind, LookAround::LookBehind | LookAround::LookBehindNeg) {
                        self.endpoints.clear();
                        let mut start = original;
                        let mut count = 0;
                        loop {
                            if count >= *min && count <= *max { self.endpoints.push(start); }
                            if start == 0 || count == *max { break; }
                            start = previous_char(self.value, start);
                            count += 1;
                        }
                        if self.endpoints.is_empty() { failed = true; }
                        else {
                            // Maximum width first, as Perl's variable lookbehind.
                            for index in 0..self.endpoints.len() - 1 {
                                self.position = self.endpoints[index];
                                self.choice(ChoiceKind::Resume(self.pc + 1));
                            }
                            self.position = *self.endpoints.last().expect("Lookbehind candidate");
                        }
                    }
                },
                Instruction::LeaveLook => {
                    if self.finish_look(false) { continue; }
                    failed = true;
                },
                Instruction::EnterCondition { yes, no } => {
                    self.push_frame(FrameKind::Condition { yes: *yes, no: *no, original: self.position, choices: self.choices.len() });
                    self.choice(ChoiceKind::ConditionFailed);
                },
                Instruction::LeaveCondition => {
                    let frame = self.top();
                    if let FrameKind::Condition { yes, original, choices, .. } = frame.kind {
                        self.choices.truncate(choices);
                        self.frame = frame.previous;
                        self.position = original;
                        self.pc = yes;
                        continue;
                    } else { unreachable!("Conditional frame") }
                },
                Instruction::CaptureExists(group) => { failed = self.captures.get(*group).copied().flatten().is_none(); },
                Instruction::Backref { group, insensitive, ascii_restrict } => {
                    if let Some((start, end)) = self.captures.get(*group).copied().flatten() {
                        if let Some(end) = match_literal(&self.value[start..end], self.value, self.position, *insensitive, *ascii_restrict) { self.position = end; }
                        else { failed = true; }
                    } else { failed = true; }
                },
                Instruction::Verb(verb) => match verb {
                    ControlVerb::Fail => failed = true,
                    ControlVerb::Accept => {
                        self.accept_open_groups();
                        if let Some(index) = self.frame {
                            match self.frames[index].kind {
                                FrameKind::Call { .. } => { self.finish_call(); continue; },
                                FrameKind::Atomic { end, .. }=>{self.pc=end;continue;},
                                FrameKind::Look { .. } => {
                                    if self.finish_look(true) { continue; }
                                    failed = true;
                                },
                                _ => unreachable!("ACCEPT local boundary"),
                            }
                        } else if reject_empty && self.position == self.attempt_start { failed = true; }
                        else {
                            self.capture(0, Some((self.match_start, self.position)));
                            return Ok(Outcome::Match);
                        }
                    },
                    ControlVerb::Mark(name) => self.add_mark(name),
                    ControlVerb::Prune(name) | ControlVerb::Then(name) | ControlVerb::Commit(name) => {
                        if let Some(name) = name { self.add_mark(name); }
                        self.choice(ChoiceKind::Cut { pc:self.pc, skip:self.position });
                    },
                    ControlVerb::Skip(name) => {
                        // A named SKIP without a corresponding MARK is ignored.
                        let skip=name.as_deref().map_or(Some(self.position),|name|self.mark_position(name));
                        if let Some(skip)=skip {
                            self.choice(ChoiceKind::Cut { pc:self.pc, skip });
                        }
                    },
                    ControlVerb::ConditionRecursion(group) => {
                        let mut frame = self.frame;
                        let mut matched = false;
                        while let Some(index) = frame {
                            let entry = self.frames[index];
                            if let FrameKind::Call { target, .. } = entry.kind {
                                matched = group.is_none_or(|group| group == target);
                                break;
                            }
                            frame = entry.previous;
                        }
                        failed = !matched;
                    },
                    ControlVerb::Grapheme => {
                        if let Some(end) = perl_unicode::grapheme_end(self.value, self.position) { self.position = end; }
                        else { failed = true; }
                    },
                    ControlVerb::WordBoundary { negative, ascii, locale } => {
                        let word = |c: char| if *ascii||(*locale&&c as u32<=255) { c.is_ascii_alphanumeric() || c == '_' } else { perl_unicode::is_word(c) };
                        let left = self.value[..self.position].chars().next_back().is_some_and(word);
                        let right = self.value[self.position..].chars().next().is_some_and(word);
                        failed = (left != right) == *negative;
                    },
                    ControlVerb::ExtendedBoundary { kind, negative } => {
                        failed = perl_unicode::is_boundary(kind, self.value, self.position)? == *negative;
                    },
                    ControlVerb::ScriptRunStart => self.script_start=self.position,
                    ControlVerb::ScriptRunEnd => {
                        failed = !perl_unicode::is_script_run(&self.value[self.script_start..self.position]);
                    },
                    ControlVerb::AsciiInsensitiveBackrefs(groups) => {
                        if let Some((start, end)) = groups.iter().find_map(|&group| self.captures[group]) {
                            if let Some(end) = match_literal(&self.value[start..end], self.value, self.position, true, true) { self.position = end; }
                            else { failed = true; }
                        } else { failed = true; }
                    },
                    ControlVerb::LocaleInsensitiveBackrefs(groups) => {
                        if let Some((start,end))=groups.iter().find_map(|&group|self.captures[group]) {
                            if let Some(end)=perl_unicode::match_folded_locale(&self.value[start..end],self.value,self.position) {self.position=end;}else{failed=true;}
                        }else{failed=true;}
                    },
                    _ => unreachable!("Parser metadata compiled as executable instruction"),
                },
            }
            if failed {
                if let Some(outcome) = self.fail() { return Ok(outcome); }
            } else { self.pc += 1; }
        }
    }

    fn accept_open_groups(&mut self) {
        while let Some(index) = self.frame {
            let frame = self.frames[index];
            match frame.kind {
                FrameKind::Group { logical, start, .. } => {
                    let value = Some((start, self.position));
                    self.capture(logical, value);
                    self.close(value);
                },
                FrameKind::Call { .. } | FrameKind::Look { .. } | FrameKind::Condition { .. } | FrameKind::Atomic { .. } => break,
                _ => {},
            }
            self.frame = frame.previous;
        }
    }
}

fn previous_char(value: &str, position: usize) -> usize {
    position - value[..position].chars().next_back().expect("Previous character").len_utf8()
}

fn match_literal(literal: &str, haystack: &str, start: usize, insensitive: bool, ascii_restrict: bool) -> Option<usize> {
    if !insensitive { return haystack.get(start..)?.starts_with(literal).then_some(start + literal.len()); }
    perl_unicode::match_folded_restricted(literal, haystack, start, ascii_restrict)
}

fn assertion_matches(assertion: Assertion, value: &str, position: usize) -> bool {
    let before = value[..position].chars().next_back();
    let after = value[position..].chars().next();
    let left_word = before.is_some_and(perl_unicode::is_word);
    let right_word = after.is_some_and(perl_unicode::is_word);
    match assertion {
        Assertion::StartText => position == 0,
        Assertion::EndText => position == value.len(),
        Assertion::EndTextIgnoreTrailingNewlines { .. } => position == value.len() || value[position..] == *"\n",
        Assertion::StartLine { crlf } | Assertion::StartLineOniguruma { crlf } => position == 0 || before == Some('\n') || (crlf && before == Some('\r') && after != Some('\n')),
        Assertion::EndLine { crlf } => position == value.len() || after == Some('\n') || (crlf && after == Some('\r')),
        Assertion::WordBoundary => left_word != right_word,
        Assertion::NotWordBoundary => left_word == right_word,
        Assertion::LeftWordBoundary => !left_word && right_word,
        Assertion::RightWordBoundary => left_word && !right_word,
        Assertion::LeftWordHalfBoundary => !left_word,
        Assertion::RightWordHalfBoundary => !right_word,
    }
}
