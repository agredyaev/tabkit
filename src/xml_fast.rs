//! Fast ASCII-QName scanner for ordinary Tableau XML.
//! Anything lexically unusual falls back to xmlparser before admission.
use super::*;

enum FastError { Unsupported, Product(Error) }
impl From<Error> for FastError { fn from(e:Error)->Self { Self::Product(e) } }
type FastResult<T>=std::result::Result<T,FastError>;

fn ws(b:u8)->bool { matches!(b,b' '|b'\t'|b'\n'|b'\r') }
#[inline]
fn has_forbidden_ascii_control(bytes:&[u8])->bool {
    const HIGHS:u64=0x8080808080808080;
    const LIMIT:u64=0x2020202020202020;
    let (chunks,tail)=bytes.as_chunks::<8>();
    for &chunk in chunks {
        let word=u64::from_ne_bytes(chunk);
        if word.wrapping_sub(LIMIT)&!word&HIGHS!=0
            && chunk.iter().any(|b|*b<32&&!matches!(*b,9|10|13)) { return true; }
    }
    tail.iter().any(|b|*b<32&&!matches!(*b,9|10|13))
}
#[inline]
fn has_forbidden_noncharacter(bytes:&[u8])->bool {
    let mut offset=0usize;
    while let Some(found)=memchr::memchr(0xef,&bytes[offset..]) {
        let p=offset+found;
        if bytes.get(p+1)==Some(&0xbf) && matches!(bytes.get(p+2),Some(0xbe|0xbf)) { return true; }
        offset=p+1;
    }
    false
}
const fn name_classes()->[u8;256] {
    let mut table=[0u8;256]; let mut i=0usize;
    while i<256 { let b=i as u8;
        table[i]=if (b>=b'A'&&b<=b'Z')||(b>=b'a'&&b<=b'z')||b==b'_' {3}
            else if (b>=b'0'&&b<=b'9')||b==b'-'||b==b'.' {2} else {0};
        i+=1;
    }
    table
}
const NAME_CLASS:[u8;256]=name_classes();
fn name_start(b:u8)->bool { NAME_CLASS[b as usize]&1!=0 }
fn name_rest(b:u8)->bool { NAME_CLASS[b as usize]&2!=0 }

fn qname<'a>(source:&'a str, mut p:usize)->FastResult<(&'a str,&'a str,usize)> {
    let bytes=source.as_bytes(); let start=p;
    if p>=bytes.len() || !name_start(bytes[p]) { return Err(FastError::Unsupported); }
    p+=1;
    while p<bytes.len() && name_rest(bytes[p]) { p+=1; }
    if p<bytes.len() && bytes[p]==b':' {
        let colon=p; p+=1;
        if p>=bytes.len() || !name_start(bytes[p]) { return Err(FastError::Unsupported); }
        p+=1; while p<bytes.len() && name_rest(bytes[p]) { p+=1; }
        if p<bytes.len() && bytes[p]==b':' { return Err(FastError::Unsupported); }
        Ok((&source[start..colon],&source[colon+1..p],p))
    } else {
        Ok(("",&source[start..p],p))
    }
}

fn skip_ws(bytes:&[u8],p:&mut usize) { while *p<bytes.len() && ws(bytes[*p]) {*p+=1;} }

pub(super) fn parse(source:&str,limits:&Limits)->Result<Option<Xml>> {
    match parse_inner(source,limits) {
        Ok(v)=>Ok(Some(v)),
        Err(FastError::Unsupported)=>Ok(None),
        Err(FastError::Product(e))=>Err(e),
    }
}

fn parse_inner(source:&str,limits:&Limits)->FastResult<Xml> {
    let bytes=source.as_bytes();
    if has_forbidden_ascii_control(bytes) || has_forbidden_noncharacter(bytes) {
        return Err(FastError::Product(Error::new("XML","XML 1.0 forbids this character")));
    }
    let mut b=Builder::new(source,limits); let mut p=0usize;
    if source.starts_with('\u{feff}') { p='\u{feff}'.len_utf8(); }
    // Keep declaration parsing on the established tokenizer unless it is the common explicit 1.0 form.
    if bytes.get(p..p+5)==Some(b"<?xml") {
        let rest=&source[p+5..];
        if rest.as_bytes().first().is_none_or(|b|!ws(*b)) { return Err(FastError::Unsupported); }
        let Some(off)=rest.find("?>") else { return Err(FastError::Unsupported); };
        let body=rest[..off].trim();
        let common=matches!(body,
            "version='1.0'" | "version=\"1.0\"" |
            "version='1.0' encoding='utf-8'" | "version=\"1.0\" encoding=\"utf-8\"");
        if !common { return Err(FastError::Unsupported); }
        p+=5+off+2;
    }
    while p<bytes.len() {
        if bytes[p]!=b'<' {
            let start=p;
            match memchr::memchr(b'<',&bytes[p..]) { Some(i)=>p+=i, None=>p=bytes.len() }
            if source[start..p].contains("]]>") { return Err(FastError::Unsupported); }
            b.text(&source[start..p],false)?;
            continue;
        }
        if bytes.get(p..p+4)==Some(b"<!--") {
            let start=p+4; let rest=&source[start..]; let Some(off)=rest.find("-->") else { return Err(FastError::Unsupported); };
            if rest[..off].contains("--") { return Err(FastError::Unsupported); }
            b.count_node()?; if let Some(f)=b.stack.last_mut(){f.last_text=false;}
            p=start+off+3; continue;
        }
        if bytes.get(p..p+9)==Some(b"<![CDATA[") {
            let start=p+9; let rest=&source[start..]; let Some(off)=rest.find("]]>") else { return Err(FastError::Unsupported); };
            b.text(&source[start..start+off],true)?; p=start+off+3; continue;
        }
        if bytes.get(p..p+2)==Some(b"<?") || bytes.get(p..p+2)==Some(b"<!") {
            return Err(FastError::Unsupported);
        }
        if bytes.get(p..p+2)==Some(b"</") {
            let start=p; p+=2;
            let (prefix,local,next)=qname(source,p)?; p=next; skip_ws(bytes,&mut p);
            if bytes.get(p)!=Some(&b'>') { return Err(FastError::Unsupported); }
            let end=p+1;
            b.end(ElementEnd::Close(prefix.into(),local.into()),start..end)?;
            p=end; continue;
        }
        let start=p; p+=1;
        let (prefix,local,next)=qname(source,p)?; p=next;
        b.start(prefix,local,start..p)?;
        loop {
            let before_ws=p; skip_ws(bytes,&mut p); let separated=p>before_ws;
            if bytes.get(p)==Some(&b'>') {
                b.end(ElementEnd::Open,p..p+1)?; p+=1; break;
            }
            if bytes.get(p..p+2)==Some(b"/>") {
                b.end(ElementEnd::Empty,p..p+2)?; p+=2; break;
            }
            if !separated { return Err(FastError::Unsupported); }
            let (aprefix,alocal,next)=qname(source,p)?; p=next; skip_ws(bytes,&mut p);
            if bytes.get(p)!=Some(&b'=') { return Err(FastError::Unsupported); }
            p+=1; skip_ws(bytes,&mut p);
            let quote=*bytes.get(p).ok_or(FastError::Unsupported)?;
            if !matches!(quote,b'\''|b'"') { return Err(FastError::Unsupported); }
            p+=1; let value_start=p; let mut special=false;
            loop {
                let Some(&byte)=bytes.get(p) else { return Err(FastError::Unsupported); };
                if byte==quote { break; }
                if byte==b'<' { return Err(FastError::Product(Error::new("XML","Literal '<' in an attribute"))); }
                special|=matches!(byte,b'&'|b'\r'|b'\n'|b'\t');
                p+=1;
            }
            b.attribute_known(aprefix,alocal,value_start..p,special)?;
            p+=1;
        }
    }
    b.finish_run().map_err(Into::into)
}
