//! Validated XML plus an offset index. The serializer is deliberately absent.
use crate::{
    config::Limits,
    error::{
        Error,
        Result,
        require
    }
};
use schemars::JsonSchema;
use serde::{
    Deserialize,
    Serialize
};
use std::ops::Range;
use ahash::AHashMap as HashMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub end: u32
}
impl Span {
    pub fn new(r: Range<usize>) -> Result<Self> {
        require(r.start <= r.end && r.end <= u32::MAX as usize, "LIMIT", "XML span overflow")?;
        Ok(Self {
            start: r.start as u32,
            end: r.end as u32
        })
    }
    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct NodeId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextId(pub u32);
/// Only the frozen string table survives XML admission.
#[derive(Default)]
pub struct Names { strings: Vec<String> }
impl Names {
    fn get(&self, id: TextId) -> &str { &self.strings[id.0 as usize] }
}
const NONE_ID:u32=u32::MAX;
#[derive(Clone, Copy)]
struct LeafTextId(u32);
#[derive(Clone, Copy)]
struct IndexRange { start:u32, end:u32 }
impl IndexRange {
    fn empty(at:usize)->Self { let at=at as u32; Self{start:at,end:at} }
    fn len(self)->usize { (self.end-self.start) as usize }
    fn is_empty(self)->bool { self.start==self.end }
    fn range(self)->Range<usize> { self.start as usize..self.end as usize }
}
pub struct Node {
    parent:u32,
    first_child:u32,
    next_sibling:u32,
    pub name:TextId,
    pub span:Span,
    attributes:IndexRange,
    leaf_text:u32,
    worksheet_owner:u32,
    dependency_owner:u32,
    raw_attributes:Span,
    normalized:IndexRange,
}
impl Node {
    fn link(id:Option<NodeId>)->u32 { id.map(|v|v.0).unwrap_or(NONE_ID) }
    fn unlink(id:u32)->Option<NodeId> { (id!=NONE_ID).then_some(NodeId(id)) }
    pub fn parent(&self)->Option<NodeId> { Self::unlink(self.parent) }
    pub fn first_child(&self)->Option<NodeId> { Self::unlink(self.first_child) }
    pub fn next_sibling(&self)->Option<NodeId> { Self::unlink(self.next_sibling) }
    pub fn attributes_empty(&self)->bool { self.attributes.is_empty() }
    #[allow(dead_code)]
    pub fn attribute_index_range(&self)->Range<usize> { self.attributes.range() }
}
/// A borrowed view of one row; it owns no text and is never stored per attribute.
#[derive(Clone, Copy)]
pub struct Attribute<'a> {
    pub name: &'a str,
    pub value: &'a str,
    pub span: Span,
    pub quote: u8,
}
/// Sparse normalization columns; ordinary attributes stay only in source bytes.
#[derive(Default)]
struct Normalized {
    source_starts: Vec<u32>,
    spans: Vec<Span>,
    text: String,
}
#[derive(Default)]
pub(crate) struct SemanticNodes {
    pub datasource_dependencies: Vec<NodeId>,
    pub column_instances: Vec<NodeId>,
    pub column_bindings: Vec<NodeId>,
    pub shelves: Vec<NodeId>,
    pub worksheets: Vec<NodeId>,
    pub dashboards: Vec<NodeId>,
    pub filters: Vec<NodeId>,
    pub metadata_records: Vec<NodeId>,
}
pub struct Xml {
    pub text: String,
    pub nodes: Vec<Node>,
    normalized: Normalized,
    names: Names,
    leaf_texts: Vec<Span>,
    leaf_text: String,
    pub(crate) semantic: SemanticNodes,
    pub sha256: String,
}
impl Xml {
    fn leaf_id(id:u32)->Option<LeafTextId> { (id!=NONE_ID).then_some(LeafTextId(id)) }
    pub fn into_text(self) -> String { self.text }
    pub fn parse(bytes: Vec<u8>, limits: &Limits) -> Result<Self> {
        stream::parse(bytes, limits)
    }
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn parse_with_sha256(bytes: Vec<u8>, limits: &Limits, sha256: String) -> Result<Self> {
        stream::parse_with_sha256(bytes, limits, sha256)
    }
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }
    pub fn tag(&self, id: NodeId) -> &str {
        self.names.get(self.node(id).name)
    }
    fn attribute_value(&self, value: Span, normalized: IndexRange) -> &str {
        let starts = &self.normalized.source_starts[normalized.range()];
        match starts.binary_search(&value.start) {
            Ok(i) => &self.normalized.text[self.normalized.spans[normalized.start as usize+i].range()],
            Err(_) => &self.text[value.range()],
        }
    }
    pub fn attributes(&self, id: NodeId) -> attrs::Attributes<'_> {
        let node = self.node(id);
        attrs::Attributes::new(self, node.raw_attributes, node.attributes.len(), node.normalized)
    }
    pub fn attr(&self, id: NodeId, name: &str) -> Option<Attribute<'_>> {
        // Scan only this admitted start tag; decode metadata only for the match.
        let mut it = self.attributes(id);
        while let Some((key, value, quote)) = it.next_spans() {
            if self.text[key.range()] == *name {
                return Some(Attribute { name: &self.text[key.range()],
                    value: self.attribute_value(value, self.node(id).normalized), span: value, quote });
            }
        }
        None
    }
    pub fn value(&self, id: NodeId, name: &str) -> Option<&str> {
        self.attr(id, name).map(|a| a.value)
    }
    pub(crate) fn name_value(&self, id: NodeId) -> Option<&str> {
        let node=self.node(id);
        let bytes=self.text.as_bytes();
        let mut p=node.raw_attributes.start as usize;
        let end=node.raw_attributes.end as usize;
        while p<end && whitespace(bytes[p]) { p+=1; }
        if bytes.get(p..p+4)!=Some(b"name") { return self.value(id,"name"); }
        p+=4;
        if p<end && !whitespace(bytes[p]) && bytes[p]!=b'=' { return self.value(id,"name"); }
        while p<end && whitespace(bytes[p]) { p+=1; }
        if bytes.get(p)!=Some(&b'=') { return self.value(id,"name"); }
        p+=1;
        while p<end && whitespace(bytes[p]) { p+=1; }
        let quote=*bytes.get(p)?;
        if !matches!(quote,b'\''|b'"') { return self.value(id,"name"); }
        p+=1;
        let begin=p;
        while p<end && bytes[p]!=quote { p+=1; }
        if p>=end { return self.value(id,"name"); }
        let span=Span { start:begin as u32, end:p as u32 };
        Some(self.attribute_value(span,node.normalized))
    }
    pub fn required(&self, id: NodeId, name: &str) -> Result<&str> {
        self.value(id, name).ok_or_else(|| Error::new("UNSUPPORTED_SHAPE", format!("{} lacks {name}", self.tag(id))))
    }
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            xml: self,
            next: self.node(id).first_child()
        }
    }
    pub fn named_children<'a>(&'a self, id: NodeId, tag: &'a str) -> impl Iterator<Item=NodeId> + 'a {
        self.children(id).filter(move |n| self.tag(*n) == tag)
    }
    pub fn one_child(&self, id: NodeId, tag: &str) -> Result<NodeId> {
        let mut it = self.named_children(id, tag);
        let first = it.next().ok_or_else(|| Error::new("UNSUPPORTED_SHAPE", format!("Missing {tag}")))?;
        if it.next().is_some() { return Err(Error::new("AMBIGUOUS_TARGET", format!("Multiple {tag} elements"))); }
        Ok(first)
    }
    pub fn ancestor(&self, id: NodeId, tag: &str) -> Option<NodeId> {
        if tag=="worksheet" { return Node::unlink(self.node(id).worksheet_owner); }
        if tag=="datasource-dependencies" { return Node::unlink(self.node(id).dependency_owner); }
        let mut p = self.node(id).parent();
        while let Some(n) = p {
            if self.tag(n) == tag {
                return Some(n);
            }
            p = self.node(n).parent();
        }
        None
    }
    pub fn text_content(&self, id: NodeId) -> Result<&str> {
        require(self.node(id).first_child().is_none(), "UNSUPPORTED_SHAPE",
            "Expected a leaf element, not mixed or nested content")?;
        Ok(Self::leaf_id(self.node(id).leaf_text).map(|i| &self.leaf_text[self.leaf_texts[i.0 as usize].range()]).unwrap_or(""))
    }

}
pub struct Children<'a> {
    xml: &'a Xml,
    next: Option<NodeId>
}
impl Iterator for Children<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let id = self.next?;
        self.next = self.xml.node(id).next_sibling();
        Some(id)
    }
}
fn whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}
fn attribute_special(bytes:&[u8])->Option<usize> {
    match (memchr::memchr3(b'&',b'\r',b'\n',bytes), memchr::memchr(b'\t',bytes)) {
        (Some(a),Some(b))=>Some(a.min(b)),
        (Some(a),None)=>Some(a),
        (None,Some(b))=>Some(b),
        (None,None)=>None,
    }
}
fn decode_attribute(raw: &str, out: &mut String) -> Result<()> {
    // Literal XML whitespace is normalized; whitespace from references is not.
    // Append only transformed values; no per-attribute allocation or self-borrow.
    out.try_reserve(raw.len()).map_err(|_| Error::new("LIMIT", "Cannot reserve decoded attribute storage"))?;
    let mut rest=raw;
    while let Some(at)=attribute_special(rest.as_bytes()) {
        out.push_str(&rest[..at]);
        let kind=rest.as_bytes()[at];
        rest=&rest[at+1..];
        if kind!=b'&' {
            out.push(' ');
            if kind==b'\r' && rest.starts_with('\n') { rest=&rest[1..]; }
            continue;
        }
        let end=rest.find(';').ok_or_else(|| Error::new("XML", "Unterminated entity"))?;
        let entity=&rest[..end];
        let ch = stream::reference(entity)?;
        out.push(ch);
        rest=&rest[end+1..];
    }
    out.push_str(rest);
    Ok(())
}
pub fn escape_attribute(value: &str, quote: u8) -> Result<String> {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quote == b'"' => out.push_str("&quot;"),
            '\'' if quote == b'\'' => out.push_str("&apos;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c if (c as u32) < 0x20 || matches!(c, '\u{FFFE}' | '\u{FFFF}') => return Err(Error::new("XML_CHARACTER", "XML 1.0 forbids this character")),
            c => out.push(c),
        }
    }
    Ok(out)
}
pub fn escape_text(value: &str) -> Result<String> {
    escape_attribute(value, b'"')
}
/// Called after XML parsing; this checks the encoding contract, not XML grammar.
fn check_encoding_declaration(text:&str)->Result<()> {
    let text=text.trim_start_matches('\u{FEFF}');
    if text.starts_with("<?xml ") || text.starts_with("<?xml\t") || text.starts_with("<?xml\n") || text.starts_with("<?xml\r") {
        let end=text.find("?>").ok_or_else(||Error::new("XML","Unclosed XML declaration"))?;
        let declaration=&text[..end];
        if let Some(at)=declaration.find("encoding") {
            let tail=declaration[at+8..].trim_start().strip_prefix('=').ok_or_else(||Error::new("XML","Invalid encoding declaration"))?.trim_start();
            let quote=tail.chars().next().ok_or_else(||Error::new("XML","Missing encoding quote"))?;
            require(matches!(quote,'\''|'"'),"XML","Invalid encoding quote")?;
            let tail=&tail[quote.len_utf8()..];
            let end=tail.find(quote).ok_or_else(||Error::new("XML","Unclosed encoding value"))?;
            require(tail[..end].eq_ignore_ascii_case("utf-8"),"ENCODING","Only an explicit UTF-8 or default encoding is supported")?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/xml_soa_tests.rs"]
mod soa_tests;

#[path = "xml_stream.rs"]
mod stream;
#[path = "xml_attrs.rs"]
mod attrs;
fn valid_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\r' | '\n') || (c >= ' ' && !matches!(c, '\u{fffe}' | '\u{ffff}'))
}

#[cfg(test)]
#[path = "../../tests/unit/xml_stream_tests.rs"]
mod stream_tests;
