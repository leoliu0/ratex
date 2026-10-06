//! Biber's integer-field Roman and Unicode::UCD::num conversion.
use super::normalization;
include!("collation_numeric_data.rs");
fn numeric(c:char)->Option<f64>{let cp=c as u32;let i=NUMERIC.partition_point(|&(_,hi,_,_)|hi<cp);NUMERIC.get(i).filter(|&&(lo,_,_,_)|lo<=cp).map(|&(lo,_,n,step)|n+if step{(cp-lo)as f64}else{0.0})}
fn decimal(c:char)->bool{let cp=c as u32;let i=DECIMAL.partition_point(|&(_,hi)|hi<cp);DECIMAL.get(i).is_some_and(|&(lo,_)|lo<=cp)}
fn roman(s:&str)->Option<u64>{
    let normalized=normalization::normalize(s,"NFKD");let mut upper=String::with_capacity(normalized.len());for c in normalized.chars(){crate::perl_unicode::append_upper(&mut upper,c);}
    let bytes=upper.as_bytes();if bytes.is_empty()||bytes.iter().any(|b|!b"IVXLCDM".contains(b)){return None;}
    let mut previous=0;let mut count=0;for &b in bytes{if b==previous{count+=1;}else{count=1;previous=b;}if count>=if b"IXCM".contains(&b){4}else{2}{return None;}}
    if ["IXI","XCX","CMC","IVI","VIV","VXV","XVX","XLX","LXL","LCL","CLC","CDC","DCD","DMD","MDM"].iter().any(|bad|upper.contains(bad)){return None;}
    let mut value=0;let mut i=0;while i<bytes.len(){let mut pair=false;for (text,n) in [(b"IV",4),(b"IX",9),(b"XL",40),(b"XC",90),(b"CD",400),(b"CM",900)]{if bytes[i..].starts_with(text){value+=n;i+=2;pair=true;break;}}if !pair{value+=match bytes[i]{b'I'=>1,b'V'=>5,b'X'=>10,b'L'=>50,b'C'=>100,b'D'=>500,b'M'=>1000,_=>unreachable!()};i+=1;}}
    Some(value)
}
// Perl's default NV string precision is 15 significant decimal digits.
fn scalar(n:f64)->String {
    if n==0.0{return "0".into();}
    let sci=format!("{n:.14e}");let (mantissa,exp)=sci.split_once('e').unwrap();let exp:i32=exp.parse().unwrap();
    if !(-4..15).contains(&exp){let mantissa=mantissa.trim_end_matches('0').trim_end_matches('.');return format!("{mantissa}e{}{abs:02}",if exp<0{'-'}else{'+'},abs=exp.abs());}
    let decimals=(14-exp).max(0) as usize;let fixed=format!("{n:.decimals$}");if decimals==0{fixed}else{fixed.trim_end_matches('0').trim_end_matches('.').into()}
}
pub(crate) fn integer_sort_value(s:&str,noroman:bool)->String {
    if !noroman{if let Some(value)=roman(s){return value.to_string();}}
    let mut chars=s.chars();let Some(first)=chars.next()else{return String::new();};let Some(value)=numeric(first)else{return s.into();};
    if chars.clone().next().is_none(){return if value.fract()==0.0{format!("{value:.0}")}else{scalar(value)};}
    if !decimal(first){return s.into();}
    let zero=first as u32-value as u32;let mut exact=Some(value as u64);let mut float=value;
    for c in chars{
        let Some(digit)=(c as u32).checked_sub(zero).filter(|&n|n<=9)else{return s.into();};
        if let Some(n)=exact{exact=n.checked_mul(10).and_then(|n|n.checked_add(digit as u64));if exact.is_none(){float=n as f64*10.0+digit as f64;}}
        else{float=float*10.0+digit as f64;}
    }
    exact.map(|n|n.to_string()).unwrap_or_else(||scalar(float))
}
