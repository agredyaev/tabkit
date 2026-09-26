//! Original-byte admission. A typed object/map cannot reveal duplicate wire keys.
//! The external-byte pass is deliberately separate from SDK/DTO decoding.
use crate::error::{Error, Result, require};
use serde::{Deserialize, Serialize, de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor}};
use std::{collections::BTreeSet, fmt, io::{self,Write}, pin::Pin, task::{Context,Poll}};
use tokio::io::{AsyncRead, AsyncBufRead, BufReader, ReadBuf};

struct Checked;
impl<'de> Deserialize<'de> for Checked {
    fn deserialize<D:de::Deserializer<'de>>(d:D)->std::result::Result<Self,D::Error> {
        d.deserialize_any(Unique)
    }
}
struct Unique;
impl<'de> Visitor<'de> for Unique {
    type Value=Checked;
    fn expecting(&self,f:&mut fmt::Formatter<'_>)->fmt::Result { f.write_str("unambiguous JSON") }
    fn visit_bool<E:de::Error>(self,_:bool)->std::result::Result<Checked,E>{Ok(Checked)}
    fn visit_i64<E:de::Error>(self,_:i64)->std::result::Result<Checked,E>{Ok(Checked)}
    fn visit_u64<E:de::Error>(self,_:u64)->std::result::Result<Checked,E>{Ok(Checked)}
    fn visit_f64<E:de::Error>(self,_:f64)->std::result::Result<Checked,E>{Ok(Checked)}
    fn visit_str<E:de::Error>(self,_:&str)->std::result::Result<Checked,E>{Ok(Checked)}
    fn visit_string<E:de::Error>(self,_:String)->std::result::Result<Checked,E>{Ok(Checked)}
    fn visit_unit<E:de::Error>(self)->std::result::Result<Checked,E>{Ok(Checked)}
    fn visit_seq<A:SeqAccess<'de>>(self,mut a:A)->std::result::Result<Checked,A::Error>{
        while a.next_element::<Checked>()?.is_some() {} Ok(Checked)
    }
    fn visit_map<A:MapAccess<'de>>(self,mut a:A)->std::result::Result<Checked,A::Error>{
        let mut keys=BTreeSet::new();
        while let Some(key)=a.next_key::<String>()? {
            // Do not echo arbitrary key/value content into transport diagnostics.
            if !keys.insert(key){return Err(de::Error::custom("duplicate JSON object key"));}
            a.next_value::<Checked>()?;
        }
        Ok(Checked)
    }
}
pub fn validate(bytes:&[u8],max:u64)->Result<()> {
    require(bytes.len() as u64<=max,"INPUT_LIMIT","JSON exceeds byte budget")?;
    let mut d=serde_json::Deserializer::from_slice(bytes);
    Checked::deserialize(&mut d).map_err(|e|Error::new("JSON_WIRE",format!("Invalid or ambiguous JSON at line {} column {}",e.line(),e.column())))?;
    d.end().map_err(|_|Error::new("JSON_WIRE","Trailing data after JSON value"))
}
pub fn decode<T:DeserializeOwned>(bytes:&[u8],max:u64)->Result<T> {
    validate(bytes,max)?;
    Ok(serde_json::from_slice(bytes)?)
}
struct Counter { n:usize,max:usize }
impl Write for Counter {
    fn write(&mut self,b:&[u8])->io::Result<usize>{
        if b.len()>self.max.saturating_sub(self.n){return Err(io::Error::other("JSON output limit"));}
        self.n+=b.len(); Ok(b.len())
    }
    fn flush(&mut self)->io::Result<()>{Ok(())}
}
/// Counts encoded bytes without allocating an additional serialized copy.
pub fn encoded_len<T:Serialize>(v:&T,max:usize)->Result<usize>{
    let mut w=Counter{n:0,max};
    serde_json::to_writer(&mut w,v).map_err(|_|Error::new("RESULT_LIMIT","Encoded JSON exceeds output budget"))?;
    Ok(w.n)
}

/// One bounded newline-delimited frame, checked before any bytes reach rmcp.
/// The SDK still owns protocol parsing, negotiation and cancellation.
pub struct JsonLines<R> {
    reader:BufReader<R>,frame:Vec<u8>,sent:usize,ready:bool,max:usize,failed:bool,
}
impl<R:AsyncRead> JsonLines<R> {
    pub fn new(reader:R,max:usize)->Self {
        Self{reader:BufReader::with_capacity(8192,reader),frame:Vec::new(),sent:0,ready:false,max,failed:false}
    }
}
impl<R:AsyncRead+Unpin> AsyncRead for JsonLines<R> {
    fn poll_read(self:Pin<&mut Self>,cx:&mut Context<'_>,out:&mut ReadBuf<'_>)->Poll<io::Result<()>> {
        let me=self.get_mut();
        if out.remaining()==0{return Poll::Ready(Ok(()));}
        if me.failed{return Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData,"MCP input is closed after invalid frame")));}
        loop {
            if me.ready {
                let n=out.remaining().min(me.frame.len()-me.sent);
                out.put_slice(&me.frame[me.sent..me.sent+n]); me.sent+=n;
                if me.sent==me.frame.len(){me.frame.clear();me.sent=0;me.ready=false;}
                return Poll::Ready(Ok(()));
            }
            let available=match Pin::new(&mut me.reader).poll_fill_buf(cx) {
                Poll::Pending=>return Poll::Pending,
                Poll::Ready(Err(e))=>return Poll::Ready(Err(e)),
                Poll::Ready(Ok(b))=>b,
            };
            if available.is_empty(){
                if me.frame.is_empty(){return Poll::Ready(Ok(()));}
                me.failed=true;
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::UnexpectedEof,"Truncated MCP JSON line")));
            }
            let take=available.iter().position(|&b|b==b'\n').map(|i|i+1).unwrap_or(available.len());
            if take>me.max.saturating_sub(me.frame.len()){
                me.failed=true;
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData,"MCP frame exceeds configured byte budget")));
            }
            let complete=available[take-1]==b'\n';
            me.frame.extend_from_slice(&available[..take]);
            Pin::new(&mut me.reader).consume(take);
            if complete {
                if validate(&me.frame,me.max as u64).is_err(){
                    me.failed=true;
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData,"MCP frame is invalid or contains duplicate keys")));
                }
                me.ready=true;
            }
        }
    }
}
struct Output { bytes:Vec<u8>,max:usize }
impl Write for Output {
    fn write(&mut self,b:&[u8])->io::Result<usize>{
        if b.len()>self.max.saturating_sub(self.bytes.len()){return Err(io::Error::other("JSON output limit"));}
        self.bytes.try_reserve(b.len()).map_err(|_|io::Error::other("JSON allocation failed"))?;
        self.bytes.extend_from_slice(b); Ok(b.len())
    }
    fn flush(&mut self)->io::Result<()>{Ok(())}
}
pub fn encode<T:Serialize>(v:&T,max:usize,pretty:bool)->Result<Vec<u8>>{
    let mut out=Output{bytes:Vec::new(),max};
    let result=if pretty {serde_json::to_writer_pretty(&mut out,v)}else{serde_json::to_writer(&mut out,v)};
    result.map_err(|_|Error::new("JSON_OUTPUT_LIMIT","JSON artifact exceeds budget or cannot be allocated"))?;
    Ok(out.bytes)
}
