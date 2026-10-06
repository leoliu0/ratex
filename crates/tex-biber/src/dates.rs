//! ISO 8601-1/2 date forms accepted by Biber 2.22's Date::Format.
use crate::model::{Control,Entry};
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

const DIVISIONS: [&str;21] = ["spring","summer","autumn","winter","springN","summerN","autumnN","winterN","springS","summerS","autumnS","winterS","Q1","Q2","Q3","Q4","QD1","QD2","QD3","S1","S2"];

pub fn parse(control: &Control, entry: &mut Entry, field: &str, value: &str) {
    let prefix=field.strip_suffix("date").unwrap_or(field);
    entry.fields.remove(field);
    let mut split=value.split('/');
    let start=split.next().unwrap_or("");let end=split.next();
    if split.next().is_some() || (!date_truthy(start) && !date_truthy(end.unwrap_or(""))) {warn_invalid(entry,field,value,false);return;}
    let expanded=expand_unspecified(start);
    let (start,end,unspecified)=if let Some((a,b,u))=&expanded {(a.as_str(),Some(b.as_str()),Some(*u))}
        else if start.contains('X'){("",None,None)}else{(start,end,None)};
    if let Some(u)=unspecified {derived_field(entry,format!("{prefix}dateunspecified"),u.to_owned());}
    if end.is_some_and(|v|!date_truthy(v)){entry.flags.insert(format!("{prefix}enddateunknown"));}
    if end.is_some() && !date_truthy(start){entry.flags.insert(format!("{prefix}dateunknown"));}
    let Some(first)=parse_point(control,start) else {warn_invalid(entry,field,value,false);return};
    let start_present=first.era.is_some();
    if prefix.is_empty() && start_present {
        for part in ["year","month"] {
            if let (Some(previous),Some(next))=(entry.fields.get(part),first.parts.get(part)) {
                let parsed=next.chars().map(|c|decimal(c).map_or(c,|d|char::from(b'0'+d))).collect::<String>().parse::<i64>().unwrap_or(0);
                let legacy=legacy_numeric(previous);
                if date_truthy(previous) && (part!="year" || parsed!=0) && parsed as f64!=legacy {
                    entry.warnings.push(format!("Overwriting field '{part}' with {part} value from field 'date' for entry '{}'",entry.key));
                }
            }
        }
    }
    apply(entry,prefix,"",first);
    if let Some(end)=end {
        if let Some(mut last)=parse_point(control,end){
            // Biber collects end-point metadata only inside its nonempty-start branch.
            if !start_present {last.uncertain=false;last.approximate=false;last.julian=false;}
            apply(entry,prefix,"end",last);
        }else{
            // Metadata is captured by Date::Format preprocessing even when the endpoint fails.
            if start_present {let (uncertain,approximate)=metadata(end);if uncertain{entry.flags.insert(format!("{prefix}enddateuncertain"));}if approximate{entry.flags.insert(format!("{prefix}enddatecirca"));}}
            warn_invalid(entry,field,value,true);
        }
    }
}

/// BiblateXML stores start/end independently and never expands unspecified dates into an end.
pub fn parse_xml(control:&Control,entry:&mut Entry,field:&str,start:&str,end:Option<&str>)->Result<(),String>{
    let prefix=field.strip_suffix("date").unwrap_or(field);
    entry.fields.remove(field);
    entry.computed.insert(format!("{prefix}datesplit"),"1".to_owned());
    let mut split=start.split('/');let first=split.next().unwrap_or("");let last=split.next();
    let valid_range=date_truthy(first)||last.is_some_and(date_truthy);
    let expanded=expand_unspecified(first);
    let (first,last,unspecified)=if let Some((a,b,u))=&expanded{(a.as_str(),Some(b.as_str()),Some(*u))}else if first.contains('X'){("",None,None)}else{(first,last,None)};
    if valid_range&&last.is_some_and(|v|!date_truthy(v)){entry.flags.insert(format!("{prefix}enddateunknown"));}
    if valid_range&&last.is_some()&&!date_truthy(first){entry.flags.insert(format!("{prefix}dateunknown"));}
    let parsed=if valid_range&&split.next().is_none(){parse_point(control,first)}else{None};
    // Perl's list assignment is truthy even on a failed parse: the accessor raises a fatal error.
    let p=parsed.ok_or_else(||"Can't call method \"year\" on an undefined value".to_owned())?;
    if p.era.is_none(){return Err("Can't locate object method \"year\" via package \"0\"".to_owned());}
    if let Some(u)=unspecified{derived_field(entry,format!("{prefix}dateunspecified"),u.to_owned());}
    apply_xml(entry,prefix,"",p);
    if let Some(end)=end{
        if let Some(p)=parse_point(control,end){apply_xml(entry,prefix,"end",p);}
        else{entry.warnings.push(format!("{} entry '{}' ({}): Invalid format '{}' of date field 'bltx:date' range end - ignoring",entry.kind,entry.key,entry.source,end));}
    }
    Ok(())
}
fn apply_xml(entry:&mut Entry,prefix:&str,end:&str,mut p:Point){
    // XML dates use DateTime accessors directly rather than restoring Unicode digit scripts.
    for (part,value) in &mut p.parts{
        if *part=="yeardivision"||*part=="timezone"||value.is_empty(){continue;}
        let normalized:String=value.chars().map(|c|decimal(c).map_or(c,|d|char::from(b'0'+d))).collect();
        *value=normalized.parse::<i64>().map(|v|v.to_string()).unwrap_or(normalized);
    }
    apply(entry,prefix,end,p);
}
fn date_truthy(raw:&str)->bool{!raw.is_empty()&&raw!="0"}

#[derive(Default)]
struct Point {parts:BTreeMap<&'static str,String>,era:Option<&'static str>,uncertain:bool,approximate:bool,julian:bool,dayofyear:Option<u16>}
fn apply(entry:&mut Entry,prefix:&str,end:&str,p:Point){
    for (part,value) in p.parts {
        let name=format!("{prefix}{end}{part}");
        if part=="yeardivision"{derived_field(entry,name,value);}
        else{entry.fields.insert(name,value);}
    }
    if let Some(era)=p.era {
        entry.computed.insert(format!("{prefix}datesplit"),"1".to_owned());
        entry.computed.insert(format!("{prefix}{end}era"),era.to_owned());
    }
    if let Some(day)=p.dayofyear {entry.computed.insert(format!("{prefix}{end}dayofyear"),day.to_string());}
    if p.uncertain {entry.flags.insert(format!("{prefix}{end}dateuncertain"));}
    if p.approximate {entry.flags.insert(format!("{prefix}{end}datecirca"));}
    if p.julian {entry.flags.insert(format!("{prefix}{end}datejulian"));}
}

/// Authored datafields shadow generated divisions without deleting the derived value.
pub(crate) fn authored_field(entry:&mut Entry,field:&str){
    if !field.ends_with("yeardivision")&&!field.ends_with("dateunspecified"){return;}
    if entry.computed.remove(&format!("derivedfield:{field}")).is_some(){
        if let Some(value)=entry.fields.remove(field){entry.computed.insert(format!("shadowderivedfield:{field}"),value);}
    }
}

/// A datafield takes precedence over a derived field even when the date node comes later.
pub(crate) fn derived_field(entry:&mut Entry,field:String,value:String){
    let marker=format!("derivedfield:{field}");
    if entry.fields.contains_key(&field)&&!entry.computed.contains_key(&marker){
        entry.computed.insert(format!("shadowderivedfield:{field}"),value);
    }else{
        entry.fields.insert(field,value);entry.computed.insert(marker,"1".to_owned());
    }
}
fn leap(year:i64)->bool{year%4==0 && (year%100!=0 || year%400==0)}
fn month_days(year:i64,month:u8)->u8{match month{1|3|5|7|8|10|12=>31,4|6|9|11=>30,2=>if leap(year){29}else{28},_=>0}}
fn expand_unspecified(s:&str)->Option<(String,String,&'static str)>{
    if !s.contains('X'){return None;}
    let normalized:String=s.chars().map(|c|if matches!(c,'\u{2010}'..='\u{2015}'|'\u{2e17}'|'\u{2e1a}'|'\u{2e3a}'|'\u{2e3b}'|'\u{301c}'|'\u{3030}'|'\u{fe31}'|'\u{fe32}'|'\u{fe58}'|'\u{fe63}'|'\u{ff0d}'|'\u{2212}'|'\u{05be}'|'\u{1400}'|'\u{1806}'|'\u{10ead}'){'-'}else{c}).collect();
    let s=normalized.as_str();
    let chars:Vec<char>=s.chars().collect();
    if chars.len()==4 && chars[..3].iter().all(|c|decimal(*c).is_some()) && chars[3]=='X'{return Some((s.replace('X',"0"),s.replace('X',"9"),"yearindecade"));}
    if chars.len()==4 && chars[..2].iter().all(|c|decimal(*c).is_some()) && chars[2..]==['X','X']{return Some((s.replace('X',"0"),s.replace('X',"9"),"yearincentury"));}
    let parts:Vec<_>=s.split('-').collect();
    if parts.first()?.chars().count()!=4 || !parts[0].chars().all(|c|decimal(c).is_some()){return None;}
    match parts.as_slice(){
        [year,"XX"]=>Some((format!("{year}-01"),format!("{year}-12"),"monthinyear")),
        [year,"XX","XX"]=>Some((format!("{year}-01-01"),format!("{year}-12-31"),"dayinyear")),
        [year,month,"XX"] if month.chars().count()==2 && month.chars().all(|c|decimal(c).is_some())=>{
            let y=year.parse().unwrap_or(0);let days=month.parse().ok().map(|m|month_days(y,m)).unwrap_or(0);
            Some((format!("{year}-{month}-01"),if days==0{format!("{year}-{month}-")}else{format!("{year}-{month}-{days}")},"dayinmonth"))
        },
        _=>None,
    }
}
static DATE_RE:LazyLock<Regex>=LazyLock::new(||Regex::new(r"^(?:(-?[0-9]{4})(?:-([0-9]{2})(?:-([0-9]{2})(?:T([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\.[0-9]{3})?)?)?)?|Y(-?[0-9]{5,}))$").unwrap());
fn parse_point(control:&Control,raw:&str)->Option<Point>{
    if !date_truthy(raw) || raw==".." {let mut p=Point::default();p.parts.insert("year",String::new());return Some(p);}
    let mut p=Point::default();let mut raw=raw;
    // Biber removes these in this order, permitting '~?' as well as '%'.
    if let Some(v)=strip_marker(raw,'?'){p.uncertain=true;raw=v;}
    if let Some(v)=strip_marker(raw,'~'){p.approximate=true;raw=v;}
    if let Some(v)=strip_marker(raw,'%'){p.uncertain=true;p.approximate=true;raw=v;}
    let mut scripts=BTreeMap::new();let mut conversion=BTreeMap::new();
    let mut ascii=String::with_capacity(raw.len());
    if raw.is_ascii(){ascii.push_str(raw);}else{
    let mut run=String::new();let mut arabic=String::new();
    for c in raw.chars().chain(std::iter::once('\0')) {
        if let Some(d)=decimal(c){run.push(c);arabic.push(char::from(b'0'+d));}
        else{
            if !run.is_empty(){
                // Unicode::UCD::num rejects mixed digit scripts; sprintf then treats undef as zero.
                let mut bases=run.chars().map(|c|c as u32-decimal(c).unwrap()as u32);let base=bases.next().unwrap();
                let mixed=bases.any(|b|b!=base);
                if mixed{arabic="0".repeat(run.chars().count());}
                if run!=arabic {
                    scripts.insert(arabic.clone(),run.clone());
                    if !mixed {if let Ok(n)=arabic.parse::<u64>(){scripts.insert(n.to_string(),run.clone());}}
                    conversion.insert(run.clone(),arabic.clone());
                }
                run.clear();arabic.clear();
            }
        }
    }
    for c in raw.chars().chain(std::iter::once('\0')){
        if decimal(c).is_some(){run.push(c);}else{
            if !run.is_empty(){if conversion.is_empty(){ascii.push_str(&run);}else if let Some(v)=conversion.get(&run){ascii.push_str(v);}run.clear();}
            if c!='\0'{ascii.push(c);}
        }
    }
    }
    if !ascii.is_ascii(){return None;}
    let mut zone=None;
    if ascii.ends_with('Z'){ascii.pop();zone=Some("Z".to_owned());}
    else if ascii.len()>=6 {
        let z=&ascii[ascii.len()-6..];
        if matches!(z.as_bytes()[0],b'+'|b'-') && z.as_bytes()[3]==b':' && z[1..3].bytes().all(|c|c.is_ascii_digit()) && z[4..].bytes().all(|c|c.is_ascii_digit()) {
            let h:u8=z[1..3].parse().ok()?;let m:u8=z[4..].parse().ok()?;
            if m>59{return None;}
            zone=Some(if h==0&&m==0{"Z".to_owned()}else{format!("{}\\bibtzminsep {}",&z[..3],&z[4..])});ascii.truncate(ascii.len()-6);
        }
    }
    let caps=DATE_RE.captures(&ascii)?;
    let year=perl_year(caps.get(1).or_else(||caps.get(7))?.as_str())?;
    // DateTime's native calendar roundtrip uses signed 64-bit wrapping arithmetic.
    let year=if caps.get(7).is_some(){datetime_year(year)}else{year};
    p.era=Some(if year<=0{"bce"}else{"ce"});
    let year_text=year.to_string();p.parts.insert("year",scripts.get(&year_text).cloned().unwrap_or(year_text));
    let mut month=None;let mut day=None;
    if let Some(m)=caps.get(2){let value:u8=m.as_str().parse().ok()?;
        if (20..=41).contains(&value) && caps.get(3).is_none(){if value>=21{p.parts.insert("yeardivision",DIVISIONS[(value-21)as usize].to_owned());}}
        else{if !(1..=12).contains(&value){return None;}month=Some(value);let v=value.to_string();p.parts.insert("month",scripts.get(&v).cloned().unwrap_or(v));}
    }
    if let Some(d)=caps.get(3){let value:u8=d.as_str().parse().ok()?;if value==0 || value>month_days(year,month?){return None;}day=Some(value);let v=value.to_string();p.parts.insert("day",scripts.get(&v).cloned().unwrap_or(v));}
    if let (Some(m),Some(d))=(month,day){p.dayofyear=Some((1..m).map(|m|month_days(year,m)as u16).sum::<u16>()+d as u16);}
    if let Some(h)=caps.get(4){
        let hour:u8=h.as_str().parse().ok()?;let minute:u8=caps.get(5)?.as_str().parse().ok()?;let second:u8=caps.get(6)?.as_str().parse().ok()?;
        if hour>23||minute>59||second>60{return None;}
        if second==60 && !valid_leap_second(year,month?,day?,hour,minute,zone.as_deref()){return None;}
        for (name,value) in [("hour",hour),("minute",minute),("second",second)]{let v=value.to_string();p.parts.insert(name,scripts.get(&v).cloned().unwrap_or(v));}
    }
    if caps.get(4).is_some(){if let Some(zone)=zone {p.parts.insert("timezone",zone);}}
    if matches!(control.option("","julian"),"true"|"1") {
        if let (Some(m),Some(d))=(month,day) {
            let boundary=control.option("","gregorianstart");
            let mut parts=boundary.split('-').filter_map(|v|v.parse::<i64>().ok());
            let cutoff=(parts.next().unwrap_or(1582),parts.next().unwrap_or(10),parts.next().unwrap_or(15));
            if (year,m as i64,d as i64)<cutoff {
                let (jy,jm,jd)=gregorian_to_julian(year,m,d);
                p.julian=true;p.era=Some(if jy<=0{"bce"}else{"ce"});
                for (part,value) in [("year",jy.to_string()),("month",jm.to_string()),("day",jd.to_string())]{p.parts.insert(part,scripts.get(&value).cloned().unwrap_or(value));}
                p.dayofyear=Some((1..jm).map(|m|julian_month_days(jy,m)as u16).sum::<u16>()+jd as u16);
            }
        }
    }
    Some(p)
}
fn decimal(c:char)->Option<u8>{
    let code=c as u32;
    const STARTS:&[u32]=&[0x30,0x660,0x6f0,0x7c0,0x966,0x9e6,0xa66,0xae6,0xb66,0xbe6,0xc66,0xce6,0xd66,0xde6,0xe50,0xed0,0xf20,0x1040,0x1090,0x17e0,0x1810,0x1946,0x19d0,0x1a80,0x1a90,0x1b50,0x1bb0,0x1c40,0x1c50,0xa620,0xa8d0,0xa900,0xa9d0,0xa9f0,0xaa50,0xabf0,0xff10,0x104a0,0x10d30,0x11066,0x110f0,0x11136,0x111d0,0x112f0,0x11450,0x114d0,0x11650,0x116c0,0x11730,0x118e0,0x11950,0x11c50,0x11d50,0x11da0,0x16a60,0x16ac0,0x16b50,0x1d7ce,0x1d7d8,0x1d7e2,0x1d7ec,0x1d7f6,0x1e140,0x1e2f0,0x1e950];
    STARTS.iter().find_map(|start|if code>=*start&&code<*start+10{Some((code-start)as u8)}else{None})
}
fn julian_month_days(year:i64,month:u8)->u8{if month==2{if year%4==0{29}else{28}}else{month_days(year,month)}}
fn gregorian_to_julian(year:i64,month:u8,day:u8)->(i64,u8,u8){
    let a=(14-month as i128).div_euclid(12);let y=year as i128+4800-a;let m=month as i128+12*a-3;
    let jdn=day as i128+(153*m+2).div_euclid(5)+365*y+y.div_euclid(4)-y.div_euclid(100)+y.div_euclid(400)-32045;
    let c=jdn+32082;let d=(4*c+3).div_euclid(1461);let e=c-(1461*d).div_euclid(4);let m=(5*e+2).div_euclid(153);
    let day=e-(153*m+2).div_euclid(5)+1;let month=m+3-12*m.div_euclid(10);let year=d-4800+m.div_euclid(10);
    (year as i64,month as u8,day as u8)
}
fn day_number(year:i64,month:u8,day:u8)->i128{
    let a=(14-month as i128).div_euclid(12);let y=year as i128+4800-a;let m=month as i128+12*a-3;
    day as i128+(153*m+2).div_euclid(5)+365*y+y.div_euclid(4)-y.div_euclid(100)+y.div_euclid(400)-32045
}
fn valid_leap_second(year:i64,month:u8,day:u8,hour:u8,minute:u8,zone:Option<&str>)->bool{
    let offset=match zone {
        Some("Z")=>0,
        Some(z)=>{let h:i32=z[1..3].parse().unwrap_or(0);let m:i32=z.rsplit(' ').next().unwrap_or("0").parse().unwrap_or(0);(h*60+m)*if z.starts_with('-'){-1}else{1}},
        None=>return false,
    };
    let time=hour as i32*60+minute as i32-offset;
    if time.rem_euclid(1440)!=1439{return false;}
    let target=day_number(year,month,day)+time.div_euclid(1440)as i128;
    const LEAPS:&[(i64,u8,u8)]=&[(1972,6,30),(1972,12,31),(1973,12,31),(1974,12,31),(1975,12,31),(1976,12,31),(1977,12,31),(1978,12,31),(1979,12,31),(1981,6,30),(1982,6,30),(1983,6,30),(1985,6,30),(1987,12,31),(1989,12,31),(1990,12,31),(1992,6,30),(1993,6,30),(1994,6,30),(1995,12,31),(1997,6,30),(1998,12,31),(2005,12,31),(2008,12,31),(2012,6,30),(2015,6,30),(2016,12,31)];
    LEAPS.iter().any(|&(y,m,d)|day_number(y,m,d)==target)
}
fn metadata(raw:&str)->(bool,bool){
    let mut raw=raw;let mut uncertain=false;let mut approximate=false;
    if let Some(v)=strip_marker(raw,'?'){uncertain=true;raw=v;}
    if let Some(v)=strip_marker(raw,'~'){approximate=true;raw=v;}
    if strip_marker(raw,'%').is_some(){uncertain=true;approximate=true;}
    (uncertain,approximate)
}
fn strip_marker(raw:&str,marker:char)->Option<&str>{
    let prefix=raw.trim().strip_suffix(marker)?.trim();
    (!prefix.is_empty()).then_some(prefix)
}
fn legacy_numeric(raw:&str)->f64{
    // Perl's numeric comparison accepts an initial numeric prefix, including exponent notation.
    static NUMBER:LazyLock<Regex>=LazyLock::new(||Regex::new(r"^\s*([+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?)").unwrap());
    NUMBER.captures(raw).and_then(|c|c.get(1)?.as_str().parse().ok()).unwrap_or(0.0)
}
fn perl_year(raw:&str)->Option<i64>{
    if let Ok(v)=raw.parse::<i64>(){return Some(v);}
    if !raw.starts_with('-'){return Some(raw.parse::<u64>().map(|v|v as i64).unwrap_or(-1));}
    None
}
fn datetime_year(mut y:i64)->i64{
    // DateTime.xs _ymd2rd(year, 1, 1), then _rd2ymd; preserve overflow, not idealised ISO arithmetic.
    y=y.wrapping_sub(1);let mut d=1_i64;
    if y<0{let adj=399_i64.wrapping_sub(y)/400;d=d.wrapping_sub(146097_i64.wrapping_mul(adj));y=y.wrapping_add(400_i64.wrapping_mul(adj));}
    d=d.wrapping_add((13*367-1094)/12).wrapping_add((y%100).wrapping_mul(1461)/4).wrapping_add((y/100).wrapping_mul(36524).wrapping_add(y/400)).wrapping_sub(306);
    d=d.wrapping_add(306);let mut adjustment=0_i64;
    if d<=0{adjustment=(d.wrapping_neg()/146097).wrapping_add(1).wrapping_neg();d=d.wrapping_sub(adjustment.wrapping_mul(146097));}
    let c=d.wrapping_mul(4).wrapping_sub(1)/146097;d=d.wrapping_sub(c.wrapping_mul(146097)/4);
    let mut y=d.wrapping_mul(4).wrapping_sub(1)/1461;d=d.wrapping_sub(y.wrapping_mul(1461)/4);
    let m=d.wrapping_mul(12).wrapping_add(1093)/367;y=y.wrapping_add(c.wrapping_mul(100)).wrapping_add(adjustment.wrapping_mul(400));
    if m>12{y=y.wrapping_add(1);}y
}
fn warn_invalid(entry:&mut Entry,field:&str,value:&str,end:bool){
    entry.warnings.push(format!("{} entry '{}' ({}): Invalid format '{}' of {}date field '{}' - ignoring",entry.kind,entry.key,entry.source,value,if end{"end "}else{""},field));
}
