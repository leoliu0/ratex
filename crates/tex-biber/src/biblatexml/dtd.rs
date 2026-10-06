//! Internal DTD parameter entities and default-attribute access.
use std::borrow::Cow;
use std::collections::BTreeMap;

pub type Defaults = BTreeMap<String,BTreeMap<String,String>>;

pub fn doctype(input:&str)->Option<&str> {
    let start=input.find("<!DOCTYPE")?;
    let mut quote=0u8;let mut depth=0usize;
    for (i,b) in input.as_bytes()[start..].iter().copied().enumerate() {
        if quote!=0 {if b==quote{quote=0;}continue;}
        match b {b'\''|b'"'=>quote=b,b'['=>depth+=1,b']'=>depth=depth.saturating_sub(1),b'>' if depth==0=>return Some(&input[start..start+i+1]),_=>{}}
    }
    None
}

fn declaration_end(input:&str,start:usize)->Option<usize> {
    let mut quote=0u8;
    for (i,b) in input.as_bytes()[start..].iter().copied().enumerate() {
        if quote!=0 {if b==quote{quote=0;}}
        else if matches!(b,b'\''|b'"'){quote=b;}
        else if b==b'>'{return Some(start+i+1);}
    }
    None
}

fn token<'a>(value:&mut &'a str)->Option<&'a str> {
    *value=value.trim_start();
    let first=value.chars().next()?;
    let end=if matches!(first,'\''|'"') {value[1..].find(first)?+2}
        else if first=='(' {value.find(')')?+1}
        else {value.find(char::is_whitespace).unwrap_or(value.len())};
    let result=&value[..end];*value=&value[end..];Some(result)
}
fn unquote(value:&str)->Option<&str> {
    let quote=value.chars().next()?;
    if matches!(quote,'\''|'"')&&value.ends_with(quote)&&value.len()>1{Some(&value[1..value.len()-1])}else{None}
}

/// Expand parameter declarations only in the internal subset. External DTDs
/// and external parameter entities are not loaded by XML::LibXML defaults.
pub fn prepare(input:&str)->Result<Cow<'_,str>,String> {
    let Some(dt)=doctype(input)else{return Ok(Cow::Borrowed(input))};
    let Some(begin)=dt.find('[')else{return Ok(Cow::Borrowed(input))};
    let Some(end)=dt.rfind(']')else{return Ok(Cow::Borrowed(input))};
    let subset=&dt[begin+1..end];
    if !subset.contains("<!ENTITY %")&&!subset.contains("<!ENTITY\t%")&&!subset.contains("<!ENTITY\n%") {return Ok(Cow::Borrowed(input));}
    let mut entities=BTreeMap::<String,String>::new();
    let expanded=expand(subset,&mut entities,0)?;
    let offset=dt.as_ptr() as usize-input.as_ptr() as usize;
    let mut result=String::with_capacity(input.len()+expanded.len());
    result.push_str(&input[..offset+begin+1]);result.push_str(&expanded);result.push_str(&input[offset+end..]);
    Ok(Cow::Owned(result))
}
fn expand(input:&str,entities:&mut BTreeMap<String,String>,depth:usize)->Result<String,String> {
    if depth>40{return Err("XML parameter entity expansion loop or depth limit".into());}
    let mut result=String::with_capacity(input.len());let mut pos=0;
    while pos<input.len() {
        if input[pos..].starts_with("<!--") {
            let end=input[pos+4..].find("-->").ok_or("Unclosed DTD comment")?+pos+7;
            result.push_str(&input[pos..end]);pos=end;continue;
        }
        if input[pos..].starts_with("<!") {
            let end=declaration_end(input,pos).ok_or("Unclosed DTD declaration")?;
            let declaration=&input[pos..end];
            if let Some(body)=declaration.strip_prefix("<!ENTITY") {
                let mut body=body[..body.len()-1].trim_start();
                if body.starts_with('%') {
                    body=&body[1..];
                    let name=token(&mut body).ok_or("Invalid parameter entity name")?;
                    let value=token(&mut body).ok_or("Invalid parameter entity declaration")?;
                    let value=if matches!(value,"SYSTEM"|"PUBLIC"){String::new()}else{unquote(value).ok_or("Invalid parameter entity value")?.to_owned()};
                    entities.entry(name.to_owned()).or_insert(value);
                    result.push_str("<!-- parameter entity declaration -->");
                }else{result.push_str(declaration);}
            }else{result.push_str(declaration);}
            pos=end;continue;
        }
        if input.as_bytes()[pos]==b'%' {
            let end=input[pos+1..].find(';').ok_or("Unclosed parameter entity reference")?+pos+1;
            let name=&input[pos+1..end];
            let value=entities.get(name).cloned().ok_or_else(||format!("Undefined XML parameter entity '{name}'"))?;
            result.push_str(&expand(&value,entities,depth+1)?);
            pos=end+1;
        }else{
            let c=input[pos..].chars().next().unwrap();result.push(c);pos+=c.len_utf8();
        }
        if result.len()>10_000_000{return Err("XML parameter entity expansion size limit".into());}
    }
    Ok(result)
}

pub fn defaults(input:&str)->Result<Defaults,String> {
    let mut result=Defaults::new();
    let Some(dt)=doctype(input)else{return Ok(result)};
    let mut pos=0;
    while pos<dt.len() {
        if dt[pos..].starts_with("<!--") {if let Some(end)=dt[pos+4..].find("-->"){pos+=end+7;continue;}else{break;}}
        if dt[pos..].starts_with("<!ATTLIST") {
            let Some(end)=declaration_end(dt,pos)else{break};
            let mut body=&dt[pos+9..end-1];
            let Some(element)=token(&mut body)else{break};
            while let Some(name)=token(&mut body) {
                let Some(kind)=token(&mut body)else{break};
                if kind=="NOTATION" {if token(&mut body).is_none(){break;}}
                let Some(mut default)=token(&mut body)else{break};
                if default=="#FIXED" {let Some(value)=token(&mut body)else{break};default=value;}
                if let Some(value)=unquote(default) {
                    let xml = format!("{dt}<defaults value=\"{}\"/>", value.replace('"', "&quot;"));
                    let document = roxmltree::Document::parse_with_options(&xml, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() })
                        .map_err(|error| format!("Invalid XML DTD default for '{element}/@{name}': {error}"))?;
                    let value = document.root_element().attribute("value").unwrap_or("");
                    let value=if kind=="CDATA"{value.to_owned()}else{value.split_whitespace().collect::<Vec<_>>().join(" ")};
                    result.entry(element.to_owned()).or_default().entry(name.to_owned()).or_insert(value);
                }
            }
            pos=end;continue;
        }
        if dt[pos..].starts_with("<!ENTITY") {if let Some(end)=declaration_end(dt,pos){pos=end;continue;}else{break;}}
        pos+=dt[pos..].chars().next().unwrap().len_utf8();
    }
    Ok(result)
}
