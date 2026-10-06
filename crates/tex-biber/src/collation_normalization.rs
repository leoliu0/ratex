//! Unicode::Normalize's bundled Unicode 15 data, independent of host Unicode versions.
use std::borrow::Cow;
include!("collation_normalization_data.rs");
pub(crate) fn canonical_combining_class(c: char) -> u8 {
    let cp=c as u32;
    let i=CLASSES.partition_point(|&(_,hi,_)|hi<cp);
    CLASSES.get(i).filter(|&&(lo,_,_)|lo<=cp).map(|&(_,_,class)|class).unwrap_or(0)
}
fn class(cp:u32)->u8 {canonical_combining_class(char::from_u32(cp).expect("Unicode scalar table"))}
fn decompose(cp:u32,compat:bool,out:&mut Vec<u32>) {
    if (0xac00..=0xd7a3).contains(&cp) {
        let i=cp-0xac00;out.extend([0x1100+i/588,0x1161+(i%588)/28]);
        if i%28!=0 {out.push(0x11a7+i%28);}return;
    }
    if let Ok(i)=DECOMPOSITIONS.binary_search_by_key(&cp,|x|x.0) {
        let (_,is_compat,off,n)=DECOMPOSITIONS[i];
        if compat||!is_compat {for &x in &DECOMPOSED[off..off+n]{decompose(x,compat,out);}return;}
    }
    out.push(cp);
}
fn composite(a:u32,b:u32)->Option<u32> {
    if (0x1100..=0x1112).contains(&a)&&(0x1161..=0x1175).contains(&b) {return Some(0xac00+(a-0x1100)*588+(b-0x1161)*28);}
    if (0xac00..=0xd7a3).contains(&a)&&(a-0xac00)%28==0&&(0x11a8..=0x11c2).contains(&b){return Some(a+b-0x11a7);}
    COMPOSITIONS.binary_search_by_key(&(a,b),|x|(x.0,x.1)).ok().map(|i|COMPOSITIONS[i].2)
}
pub(crate) fn normalize<'a>(s:&'a str,form:&str)->Cow<'a,str> {
    if !matches!(form,"NFD"|"D"|"NFC"|"C"|"NFKD"|"KD"|"NFKC"|"KC"|"FCD"|"FCC"){return Cow::Borrowed(s);}
    let compat=matches!(form,"NFKD"|"KD"|"NFKC"|"KC");
    let mut cps=Vec::with_capacity(s.chars().count());
    for c in s.chars(){decompose(c as u32,compat,&mut cps);}
    // Stable canonical ordering; starters divide independent combining sequences.
    let mut start=0;
    for i in 0..cps.len(){let cc=class(cps[i]);if cc==0{start=i+1;continue;}let mut j=i;while j>start&&class(cps[j-1])>cc{cps.swap(j-1,j);j-=1;}}
    if form=="FCD" {
        let mut previous=0;let mut ordered=true;
        for c in s.chars(){let mut d=Vec::new();decompose(c as u32,false,&mut d);let lead=class(d[0]);if lead!=0&&previous>lead{ordered=false;break;}previous=class(*d.last().unwrap());}
        if ordered{return Cow::Borrowed(s);}
    } else if matches!(form,"NFC"|"C"|"NFKC"|"KC"|"FCC")&&!cps.is_empty() {
        let mut starter=0;let mut last_class=class(cps[0]);let mut write=1;
        for i in 1..cps.len(){let cp=cps[i];let cc=class(cp);
            let combined=if last_class==0||last_class<cc{composite(cps[starter],cp)}else{None};
            if let Some(c)=combined{cps[starter]=c;}else{if cc==0{starter=write;}else if form=="FCC"{starter=write;}cps[write]=cp;write+=1;last_class=cc;}
        }
        cps.truncate(write);
    }
    Cow::Owned(cps.into_iter().map(|cp|char::from_u32(cp).expect("Unicode scalar table")).collect())
}
