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
use std::{
    collections::BTreeMap,
    ops::Range
};
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
#[derive(Clone, Copy)]
struct LeafTextId(u32);
pub struct Node {
    pub parent: Option<NodeId>,
    pub first_child: Option<NodeId>,
    pub next_sibling: Option<NodeId>,
    pub name: TextId,
    pub span: Span,
    pub attributes: Range<usize>,
    leaf_text: Option<LeafTextId>,
}
#[derive(Clone)]
pub struct Attribute {
    pub name: String,
    pub value: String,
    pub span: Span,
    pub quote: u8,
}
pub struct Xml {
    pub text: String,
    pub nodes: Vec<Node>,
    pub attributes: Vec<Attribute>,
    names: Names,
    leaf_texts: Vec<String>,
    pub sha256: String,
}
impl Xml {
    pub fn parse(bytes: Vec<u8>, limits: &Limits) -> Result<Self> {
        require(bytes.len() as u64 <= limits.xml_bytes, "LIMIT", "TWB is too large")?;
        let text = String::from_utf8(bytes).map_err(|_| Error::new("ENCODING", "Only UTF-8 TWB is supported; no implicit transcoding"))?;
        let options = roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: limits.xml_nodes
        };
        let doc = roxmltree::Document::parse_with_options(&text, options)
        .map_err(|e| Error::new("XML", e.to_string()))?;
        check_encoding_declaration(&text)?;
        require(doc.root_element().tag_name().namespace().is_none(), "UNSUPPORTED_SHAPE", "Namespaced workbook roots are unsupported")?;
        let mut nodes: Vec<Node> = Vec::new();
        let mut attributes = Vec::new();
        let mut names = Names::default();
        let mut name_lookup = BTreeMap::new();
        let mut leaf_texts = Vec::new();
        let mut ids = std::collections::HashMap::new();
        let mut last_child: Vec<Option<NodeId>> = Vec::new();
        for n in doc.descendants().filter(|n| n.is_element()) {
            let id = NodeId(nodes.len() as u32);
            let parent = n.parent().and_then(|p| ids.get(&p.id()).copied());
            let begin = attributes.len();
            attributes.extend(scan_attributes(&text, n.range().start)?);
            let end = attributes.len();
            // Include namespace identity in tag names. Unknown vendor nodes stay opaque.
            let name = match n.tag_name().namespace() {
                None => n.tag_name().name().to_string(),
                Some(ns) => format!("{{{ns}}}{}", n.tag_name().name()),
            };
            let name_id = match name_lookup.get(&name) {
                Some(id) => *id,
                None => {
                    let id = TextId(names.strings.len() as u32);
                    names.strings.push(name.clone());
                    name_lookup.insert(name, id);
                    id
                }
            };
            // Read character data in its original namespace context. No fragment parser.
            let leaf_text = if !n.children().any(|c| c.is_element()) {
                let text: String = n.children().filter(|c| c.is_text())
                    .filter_map(|c| c.text()).collect();
                if text.is_empty() { None } else {
                    let id = LeafTextId(leaf_texts.len() as u32);
                    leaf_texts.push(text);
                    Some(id)
                }
            } else { None };
            nodes.push(Node {
                parent,
                first_child: None,
                next_sibling: None,
                name: name_id,
                span: Span::new(n.range())?,
                attributes: begin..end,
                leaf_text
            });
            last_child.push(None);
            if let Some(p) = parent {
                if let Some(last) = last_child[p.0 as usize] {
                    nodes[last.0 as usize].next_sibling = Some(id);
                }
                else {
                    nodes[p.0 as usize].first_child = Some(id);
                }
                last_child[p.0 as usize] = Some(id);
            }
            ids.insert(n.id(), id);
        }
        drop(doc);
        require(!nodes.is_empty(), "XML", "Empty XML document")?;
        let sha256 = crate::fs::sha256(text.as_bytes());
        Ok(Self {
            text,
            nodes,
            attributes,
            names,
            leaf_texts,
            sha256
        })
    }
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }
    pub fn tag(&self, id: NodeId) -> &str {
        self.names.get(self.node(id).name)
    }
    pub fn attr(&self, id: NodeId, name: &str) -> Option<&Attribute> {
        self.attributes[self.node(id).attributes.clone()].iter().find(|a| a.name == name)
    }
    pub fn value(&self, id: NodeId, name: &str) -> Option<&str> {
        self.attr(id, name).map(|a| a.value.as_str())
    }
    pub fn required(&self, id: NodeId, name: &str) -> Result<&str> {
        self.value(id, name).ok_or_else(|| Error::new("UNSUPPORTED_SHAPE", format!("{} lacks {name}", self.tag(id))))
    }
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            xml: self,
            next: self.node(id).first_child
        }
    }
    pub fn named_children<'a>(&'a self, id: NodeId, tag: &'a str) -> impl Iterator<Item=NodeId> + 'a {
        self.children(id).filter(move |n| self.tag(*n) == tag)
    }
    pub fn one_child(&self, id: NodeId, tag: &str) -> Result<NodeId> {
        let mut it = self.named_children(id, tag);
        let first = it.next().ok_or_else(|| Error::new("UNSUPPORTED_SHAPE", format!("Missing {tag}")))?;
        require(it.next().is_none(), "AMBIGUOUS_TARGET", format!("Multiple {tag} elements"))?;
        Ok(first)
    }
    pub fn ancestor(&self, id: NodeId, tag: &str) -> Option<NodeId> {
        let mut p = self.node(id).parent;
        while let Some(n) = p {
            if self.tag(n) == tag {
                return Some(n);
            }
            p = self.node(n).parent;
        }
        None
    }
    pub fn text_content(&self, id: NodeId) -> Result<&str> {
        require(self.node(id).first_child.is_none(), "UNSUPPORTED_SHAPE",
            "Expected a leaf element, not mixed or nested content")?;
        Ok(self.node(id).leaf_text.map(|i| self.leaf_texts[i.0 as usize].as_str()).unwrap_or(""))
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
        self.next = self.xml.node(id).next_sibling;
        Some(id)
    }
}
fn whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}
/// This locates ranges only AFTER roxmltree has validated the document.
fn scan_attributes(text: &str, start: usize) -> Result<Vec<Attribute>> {
    let bytes = text.as_bytes();
    let mut p = start + 1;
    while p < bytes.len() && !whitespace(bytes[p]) && !matches!(bytes[p], b'/' | b'>') {
        p += 1;
    }
    let mut out = Vec::new();
    loop {
        while p < bytes.len() && whitespace(bytes[p]) {
            p += 1;
        }
        require(p < bytes.len(), "XML", "Unterminated tag")?;
        if matches!(bytes[p], b'/' | b'>') {
            break;
        }
        let ns = p;
        while p < bytes.len() && !whitespace(bytes[p]) && bytes[p] != b'=' {
            p += 1;
        }
        let name = text.get(ns..p).ok_or_else(|| Error::new("XML", "Invalid attribute range"))?.to_string();
        while p < bytes.len() && whitespace(bytes[p]) {
            p += 1;
        }
        require(bytes.get(p) == Some(&b'='), "XML", "Missing attribute equals")?;
        p += 1;
        while p < bytes.len() && whitespace(bytes[p]) {
            p += 1;
        }
        let quote = *bytes.get(p).ok_or_else(|| Error::new("XML", "Missing quote"))?;
        require(quote == b'\'' || quote == b'"', "XML", "Invalid quote")?;
        p += 1;
        let begin = p;
        while p < bytes.len() && bytes[p] != quote {
            p += 1;
        }
        require(p < bytes.len(), "XML", "Unterminated attribute")?;
        let value = decode_attribute(&text[begin..p])?;
        out.push(Attribute {
            name,
            value,
            span: Span::new(begin..p)?,
            quote
        });
        p += 1;
    }
    Ok(out)
}
fn decode_attribute(raw: &str) -> Result<String> {
    let normalized = raw.replace("\r\n", " ").replace(['\r','\n','\t'], " ");
    let mut out = String::with_capacity(raw.len());
    let mut rest = normalized.as_str();
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at+1..];
        let end = rest.find(';').ok_or_else(|| Error::new("XML", "Unterminated entity"))?;
        let entity = &rest[..end];
        let ch = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let n = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                }
                else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse::<u32>().ok()
                } else {
                    None
                };
                n.and_then(char::from_u32).ok_or_else(|| Error::new("XML", "Unsupported entity"))?
            }
        };
        out.push(ch);
        rest = &rest[end+1..];
    }
    out.push_str(rest);
    Ok(out)
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
