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
    if split.next().is_some() || (start.is_empty() && end.unwrap_or("").is_empty()) {warn_invalid(entry,field,value,false);return;}
    let expanded=expand_unspecified(start);
    let (start,end,unspecified)=if let Some((a,b,u))=&expanded {(a.as_str(),Some(b.as_str()),Some(*u))}
        else if start.contains('X'){("",None,None)}else{(start,end,None)};
    if end.is_some_and(str::is_empty){entry.flags.insert(format!("{prefix}enddateunknown"));}
    if end.is_some() && start.is_empty(){entry.flags.insert(format!("{prefix}dateunknown"));}
    let Some(first)=parse_point(control,start) else {warn_invalid(entry,field,value,false);return};
    if let Some(u)=unspecified {entry.fields.insert(format!("{prefix}dateunspecified"),u.to_owned());}
    let start_present=first.era.is_some();
    if prefix.is_empty() && start_present {
        for part in ["year","month"] {
            if let (Some(previous),Some(next))=(entry.fields.get(part),first.parts.get(part)) {
                let parsed=next.chars().map(|c|decimal(c).map_or(c,|d|char::from(b'0'+d))).collect::<String>().parse::<i64>().unwrap_or(0);
                let legacy=previous.parse::<i64>().unwrap_or(0);
                if !previous.is_empty() && previous!="0" && (part!="year" || parsed!=0) && parsed!=legacy {
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
        }else{warn_invalid(entry,field,value,true);}
    }
}

#[derive(Default)]
struct Point {parts:BTreeMap<&'static str,String>,era:Option<&'static str>,uncertain:bool,approximate:bool,julian:bool,dayofyear:Option<u16>}
fn apply(entry:&mut Entry,prefix:&str,end:&str,p:Point){
    for (part,value) in p.parts {entry.fields.insert(format!("{prefix}{end}{part}"),value);}
    if let Some(era)=p.era {
        entry.computed.insert(format!("{prefix}datesplit"),"1".to_owned());
        entry.computed.insert(format!("{prefix}{end}era"),era.to_owned());
    }
    if let Some(day)=p.dayofyear {entry.computed.insert(format!("{prefix}{end}dayofyear"),day.to_string());}
    if p.uncertain {entry.flags.insert(format!("{prefix}{end}dateuncertain"));}
    if p.approximate {entry.flags.insert(format!("{prefix}{end}datecirca"));}
    if p.julian {entry.flags.insert(format!("{prefix}{end}datejulian"));}
}
fn leap(year:i64)->bool{year%4==0 && (year%100!=0 || year%400==0)}
fn month_days(year:i64,month:u8)->u8{match month{1|3|5|7|8|10|12=>31,4|6|9|11=>30,2=>if leap(year){29}else{28},_=>0}}
fn expand_unspecified(s:&str)->Option<(String,String,&'static str)>{
    if !s.is_ascii(){
        let normalized:String=s.chars().map(|c|if matches!(c,'\u{2010}'..='\u{2015}'|'\u{2e17}'|'\u{2e1a}'|'\u{2e3a}'|'\u{2e3b}'|'\u{301c}'|'\u{3030}'|'\u{fe31}'|'\u{fe32}'|'\u{fe58}'|'\u{fe63}'|'\u{ff0d}'){'-'}else{c}).collect();
        return if normalized.is_ascii(){expand_unspecified(&normalized)}else{None};
    }
    if s.len()==4 && s[..3].bytes().all(|c|c.is_ascii_digit()) && s.ends_with('X'){return Some((s.replace('X',"0"),s.replace('X',"9"),"yearindecade"));}
    if s.len()==4 && s[..2].bytes().all(|c|c.is_ascii_digit()) && s.ends_with("XX"){return Some((s.replace('X',"0"),s.replace('X',"9"),"yearincentury"));}
    let parts:Vec<_>=s.split('-').collect();
    if parts.first()?.len()!=4 || !parts[0].bytes().all(|c|c.is_ascii_digit()){return None;}
    match parts.as_slice(){
        [year,"XX"]=>Some((format!("{year}-01"),format!("{year}-12"),"monthinyear")),
        [year,"XX","XX"]=>Some((format!("{year}-01-01"),format!("{year}-12-31"),"dayinyear")),
        [year,month,"XX"] if month.len()==2=>{let y=year.parse().ok()?;let m=month.parse().ok()?;let days=month_days(y,m);if days==0{return None;}Some((format!("{year}-{month}-01"),format!("{year}-{month}-{days}"),"dayinmonth"))},
        _=>None,
    }
}
static DATE_RE:LazyLock<Regex>=LazyLock::new(||Regex::new(r"^(?:(-?[0-9]{4})(?:-([0-9]{2})(?:-([0-9]{2})(?:T([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\.[0-9]{3})?)?)?)?|Y(-?[0-9]{5,}))$").unwrap());
fn parse_point(control:&Control,raw:&str)->Option<Point>{
    if raw.is_empty() || raw==".." {let mut p=Point::default();p.parts.insert("year",String::new());return Some(p);}
    let mut p=Point::default();let mut raw=raw.to_owned();
    // Biber removes these in this order, permitting '~?' as well as '%'.
    if raw.trim_end().ends_with('?'){p.uncertain=true;raw=raw.trim().strip_suffix('?')?.trim().to_owned();}
    if raw.trim_end().ends_with('~'){p.approximate=true;raw=raw.trim().strip_suffix('~')?.trim().to_owned();}
    if raw.trim_end().ends_with('%'){p.uncertain=true;p.approximate=true;raw=raw.trim().strip_suffix('%')?.trim().to_owned();}
    let mut scripts=BTreeMap::new();
    let mut ascii=String::with_capacity(raw.len());let mut run=String::new();let mut arabic=String::new();
    for c in raw.chars().chain(std::iter::once('\0')) {
        if let Some(d)=decimal(c){run.push(c);arabic.push(char::from(b'0'+d));}
        else{
            if !run.is_empty(){if run!=arabic {scripts.insert(arabic.clone(),run.clone());if let Ok(n)=arabic.parse::<u64>(){scripts.insert(n.to_string(),run.clone());}}ascii.push_str(&arabic);run.clear();arabic.clear();}
            if c!='\0'{ascii.push(c);}
        }
    }
    if !ascii.is_ascii(){return None;}
    let mut zone=None;
    if ascii.ends_with('Z'){ascii.pop();zone=Some("Z".to_owned());}
    else if ascii.len()>=6 {
        let z=&ascii[ascii.len()-6..];
        if matches!(z.as_bytes()[0],b'+'|b'-') && z.as_bytes()[3]==b':' && z[1..3].bytes().all(|c|c.is_ascii_digit()) && z[4..].bytes().all(|c|c.is_ascii_digit()) {
            let h:u8=z[1..3].parse().ok()?;let m:u8=z[4..].parse().ok()?;
            if h>23||m>59{return None;}
            zone=Some(format!("{}\\bibtzminsep {}",&z[..3],&z[4..]));ascii.truncate(ascii.len()-6);
        }
    }
    let caps=DATE_RE.captures(&ascii)?;
    let year:i64=caps.get(1).or_else(||caps.get(7))?.as_str().parse().ok()?;
    // DateTime accepts only a finite signed machine integer range.
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
    if let Some(zone)=zone {p.parts.insert("timezone",zone);}
    if control.option("","julian")=="true" {
        if let (Some(m),Some(d))=(month,day) {
            let boundary=control.option("","gregorianstart");
            let mut parts=boundary.split('-').filter_map(|v|v.parse::<i64>().ok());
            let cutoff=(parts.next().unwrap_or(1582),parts.next().unwrap_or(10),parts.next().unwrap_or(15));
            if (year,m as i64,d as i64)<cutoff {
                let (jy,jm,jd)=gregorian_to_julian(year,m,d);
                p.julian=true;p.era=Some(if jy<=0{"bce"}else{"ce"});
                p.parts.insert("year",jy.to_string());p.parts.insert("month",jm.to_string());p.parts.insert("day",jd.to_string());
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
fn warn_invalid(entry:&mut Entry,field:&str,value:&str,end:bool){
    entry.warnings.push(format!("{} entry '{}' ({}): Invalid format '{}' of {}date field '{}' - ignoring",entry.kind,entry.key,entry.source,value,if end{"end "}else{""},field));
}
