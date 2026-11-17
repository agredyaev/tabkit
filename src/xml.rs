use crate::{Error, Result, require};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{borrow::Cow, collections::HashMap, ops::Range};
use xmlparser::{ElementEnd, Token, Tokenizer};

const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    fn from_range(range: Range<usize>) -> Result<Self> {
        require(
            range.start <= range.end && range.end <= u32::MAX as usize,
            "LIMIT",
            "XML span exceeds 32-bit range",
        )?;
        Ok(Self {
            start: range.start as u32,
            end: range.end as u32,
        })
    }

    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }

    pub fn empty(at: usize) -> Result<Self> {
        Self::from_range(at..at)
    }
}

#[derive(Clone, Copy)]
struct Attr {
    name: u32,
    value: Span,
}

#[derive(Clone, Copy)]
struct Node {
    name: u32,
    parent: u32,
    first_child: u32,
    next_sibling: u32,
    first_attr: u32,
    attr_count: u32,
    span: Span,
    closing_tag_start: u32,
    subtree_end: u32,
    text: Option<Span>,
}

struct Frame {
    id: NodeId,
    last_child: Option<NodeId>,
}

pub struct Document {
    source: String,
    names: Vec<String>,
    nodes: Vec<Node>,
    attrs: Vec<Attr>,
    pub sha256: String,
}

impl Document {
    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        let source =
            String::from_utf8(bytes).map_err(|_| Error::new("ENCODING", "TWB must be UTF-8"))?;
        Self::parse_string(source)
    }

    pub fn parse_string(source: String) -> Result<Self> {
        let mut names = Vec::<String>::new();
        let mut name_ids = HashMap::<String, u32>::new();
        let mut nodes = Vec::<Node>::new();
        let mut attrs = Vec::<Attr>::new();
        let mut stack = Vec::<Frame>::new();
        let mut pending: Option<NodeId> = None;
        let mut root_seen = false;

        fn intern(value: &str, names: &mut Vec<String>, ids: &mut HashMap<String, u32>) -> u32 {
            if let Some(id) = ids.get(value) {
                return *id;
            }
            let id = names.len() as u32;
            names.push(value.to_owned());
            ids.insert(value.to_owned(), id);
            id
        }

        for token in Tokenizer::from(source.as_str()) {
            match token.map_err(|e| Error::new("XML", e.to_string()))? {
                Token::ElementStart {
                    prefix,
                    local,
                    span,
                } => {
                    require(
                        pending.is_none(),
                        "XML",
                        "Nested start tag before previous start tag closed",
                    )?;
                    if stack.is_empty() {
                        require(!root_seen, "XML", "Multiple XML document roots")?;
                        root_seen = true;
                    }

                    let qname = if prefix.as_str().is_empty() {
                        local.as_str().to_owned()
                    } else {
                        format!("{}:{}", prefix.as_str(), local.as_str())
                    };
                    let name = intern(&qname, &mut names, &mut name_ids);
                    let id = NodeId(nodes.len() as u32);
                    let parent = stack.last().map(|frame| frame.id.0).unwrap_or(NONE);

                    nodes.push(Node {
                        name,
                        parent,
                        first_child: NONE,
                        next_sibling: NONE,
                        first_attr: attrs.len() as u32,
                        attr_count: 0,
                        span: Span::from_range(span.range())?,
                        closing_tag_start: NONE,
                        subtree_end: NONE,
                        text: None,
                    });

                    if let Some(frame) = stack.last_mut() {
                        if let Some(previous) = frame.last_child {
                            nodes[previous.0 as usize].next_sibling = id.0;
                        } else {
                            nodes[frame.id.0 as usize].first_child = id.0;
                        }
                        frame.last_child = Some(id);
                    }
                    pending = Some(id);
                }
                Token::Attribute {
                    prefix,
                    local,
                    value,
                    ..
                } => {
                    let id = pending
                        .ok_or_else(|| Error::new("XML", "Attribute outside a start tag"))?;
                    let qname = if prefix.as_str().is_empty() {
                        local.as_str().to_owned()
                    } else {
                        format!("{}:{}", prefix.as_str(), local.as_str())
                    };
                    let name = intern(&qname, &mut names, &mut name_ids);
                    attrs.push(Attr {
                        name,
                        value: Span::from_range(value.range())?,
                    });
                    nodes[id.0 as usize].attr_count += 1;
                }
                Token::ElementEnd { end, span } => match end {
                    ElementEnd::Open => {
                        let id = pending
                            .take()
                            .ok_or_else(|| Error::new("XML", "Missing start tag"))?;
                        stack.push(Frame {
                            id,
                            last_child: None,
                        });
                    }
                    ElementEnd::Empty => {
                        let id = pending
                            .take()
                            .ok_or_else(|| Error::new("XML", "Missing start tag"))?;
                        nodes[id.0 as usize].span.end = span.end() as u32;
                        nodes[id.0 as usize].subtree_end =
                            id.0.checked_add(1)
                                .ok_or_else(|| Error::new("LIMIT", "XML node id overflow"))?;
                    }
                    ElementEnd::Close(prefix, local) => {
                        require(pending.is_none(), "XML", "Unexpected closing tag")?;
                        let frame = stack
                            .pop()
                            .ok_or_else(|| Error::new("XML", "Closing tag without opener"))?;
                        let qname = if prefix.as_str().is_empty() {
                            local.as_str().to_owned()
                        } else {
                            format!("{}:{}", prefix.as_str(), local.as_str())
                        };
                        require(
                            names[nodes[frame.id.0 as usize].name as usize] == qname,
                            "XML",
                            "Mismatched closing tag",
                        )?;
                        let closing = Span::from_range(span.range())?;
                        nodes[frame.id.0 as usize].closing_tag_start = closing.start;
                        nodes[frame.id.0 as usize].span.end = closing.end;
                        nodes[frame.id.0 as usize].subtree_end = u32::try_from(nodes.len())
                            .map_err(|_| {
                                Error::new("LIMIT", "XML node count exceeds 32-bit range")
                            })?;
                    }
                },
                Token::Text { text } => {
                    if let Some(frame) = stack.last()
                        && !text.as_str().trim().is_empty()
                        && nodes[frame.id.0 as usize].text.is_none()
                    {
                        nodes[frame.id.0 as usize].text = Some(Span::from_range(text.range())?);
                    } else if stack.is_empty() && !text.as_str().trim().is_empty() {
                        return Err(Error::new("XML", "Text outside document root"));
                    }
                }
                Token::Cdata { text, .. } => {
                    if let Some(frame) = stack.last()
                        && nodes[frame.id.0 as usize].text.is_none()
                    {
                        nodes[frame.id.0 as usize].text = Some(Span::from_range(text.range())?);
                    }
                }
                Token::DtdStart { .. }
                | Token::EmptyDtd { .. }
                | Token::EntityDeclaration { .. }
                | Token::DtdEnd { .. } => {
                    return Err(Error::new("XML", "DTD declarations are disabled"));
                }
                Token::Declaration { .. }
                | Token::Comment { .. }
                | Token::ProcessingInstruction { .. } => {}
            }
        }

        require(
            root_seen && stack.is_empty() && pending.is_none(),
            "XML",
            "Incomplete XML document",
        )?;

        let sha256 = format!("{:x}", Sha256::digest(source.as_bytes()));
        Ok(Self {
            source,
            names,
            nodes,
            attrs,
            sha256,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    pub fn tag(&self, id: NodeId) -> &str {
        &self.names[self.nodes[id.0 as usize].name as usize]
    }

    pub fn span(&self, id: NodeId) -> Span {
        self.nodes[id.0 as usize].span
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        link(self.nodes[id.0 as usize].parent)
    }

    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            document: self,
            next: link(self.nodes[id.0 as usize].first_child),
        }
    }

    pub fn direct_child(&self, id: NodeId, tag: &str) -> Option<NodeId> {
        self.children(id).find(|child| self.tag(*child) == tag)
    }

    pub fn descendants(&self, root: NodeId) -> Descendants<'_> {
        let node = self.nodes[root.0 as usize];
        Descendants {
            document: self,
            next: root.0.saturating_add(1),
            end: node.subtree_end,
        }
    }

    pub fn subtree_end(&self, id: NodeId) -> u32 {
        self.nodes[id.0 as usize].subtree_end
    }

    pub fn ancestor(&self, mut id: NodeId, tag: &str) -> Option<NodeId> {
        while let Some(parent) = self.parent(id) {
            if self.tag(parent) == tag {
                return Some(parent);
            }
            id = parent;
        }
        None
    }

    pub fn attr_raw(&self, id: NodeId, wanted: &str) -> Option<&str> {
        let node = self.nodes[id.0 as usize];
        let start = node.first_attr as usize;
        let end = start + node.attr_count as usize;
        self.attrs[start..end].iter().find_map(|attr| {
            (self.names[attr.name as usize] == wanted).then(|| &self.source[attr.value.range()])
        })
    }

    pub fn attr(&self, id: NodeId, wanted: &str) -> Result<Option<Cow<'_, str>>> {
        self.attr_raw(id, wanted).map(decode_attribute).transpose()
    }

    pub fn text(&self, id: NodeId) -> Option<&str> {
        self.nodes[id.0 as usize]
            .text
            .map(|span| &self.source[span.range()])
    }

    pub fn closing_tag_start(&self, id: NodeId) -> Result<usize> {
        let at = self.nodes[id.0 as usize].closing_tag_start;
        if at == NONE {
            return Err(Error::new(
                "XML_SHAPE",
                format!("<{}> is self-closing", self.tag(id)),
            ));
        }
        Ok(at as usize)
    }
}

pub struct Children<'a> {
    document: &'a Document,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.next?;
        self.next = link(self.document.nodes[id.0 as usize].next_sibling);
        Some(id)
    }
}

pub struct Descendants<'a> {
    document: &'a Document,
    next: u32,
    end: u32,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        let id = NodeId(self.next);
        debug_assert!((id.0 as usize) < self.document.nodes.len());
        self.next += 1;
        Some(id)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.end - self.next) as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Descendants<'_> {}

fn link(value: u32) -> Option<NodeId> {
    (value != NONE).then_some(NodeId(value))
}

fn decode_attribute(value: &str) -> Result<Cow<'_, str>> {
    if !value.contains('&') {
        return Ok(Cow::Borrowed(value));
    }

    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        output.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let end = tail
            .find(';')
            .ok_or_else(|| Error::new("XML", "Unterminated XML entity"))?;
        let entity = &tail[..end];
        let decoded = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ if entity.starts_with("#x") => {
                let value = u32::from_str_radix(&entity[2..], 16)
                    .map_err(|_| Error::new("XML", "Invalid hex character reference"))?;
                char::from_u32(value)
                    .ok_or_else(|| Error::new("XML", "Invalid XML character reference"))?
            }
            _ if entity.starts_with('#') => {
                let value = entity[1..]
                    .parse::<u32>()
                    .map_err(|_| Error::new("XML", "Invalid character reference"))?;
                char::from_u32(value)
                    .ok_or_else(|| Error::new("XML", "Invalid XML character reference"))?
            }
            _ => return Err(Error::new("XML", "Unsupported XML entity")),
        };
        output.push(decoded);
        rest = &tail[end + 1..];
    }
    output.push_str(rest);
    Ok(Cow::Owned(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendants_use_preorder_subtree_ranges() -> Result<()> {
        let document = Document::parse(b"<root><a><b/></a><c/></root>".to_vec())?;
        let root = document.root();
        let descendants = document.descendants(root).collect::<Vec<_>>();
        let tags = descendants
            .iter()
            .map(|node| document.tag(*node))
            .collect::<Vec<_>>();
        assert_eq!(tags, vec!["a", "b", "c"]);

        let a = descendants[0];
        let b = descendants[1];
        let c = descendants[2];
        assert_eq!(
            document
                .descendants(a)
                .map(|node| document.tag(node))
                .collect::<Vec<_>>(),
            vec!["b"]
        );
        assert_eq!(document.subtree_end(a), c.0);
        assert!(b.0 > a.0 && b.0 < document.subtree_end(a));
        assert!(!(c.0 > a.0 && c.0 < document.subtree_end(a)));
        Ok(())
    }

    #[test]
    fn closing_tag_offsets_are_indexed_during_parse() -> Result<()> {
        let source = "<root><child>value</child><empty/></root>";
        let document = Document::parse(source.as_bytes().to_vec())?;
        let root = document.root();
        let child = document
            .direct_child(root, "child")
            .ok_or_else(|| Error::new("TEST", "child missing"))?;
        let empty = document
            .direct_child(root, "empty")
            .ok_or_else(|| Error::new("TEST", "empty missing"))?;

        assert_eq!(
            document.closing_tag_start(root)?,
            source
                .find("</root>")
                .ok_or_else(|| Error::new("TEST", "root close missing"))?
        );
        assert_eq!(
            document.closing_tag_start(child)?,
            source
                .find("</child>")
                .ok_or_else(|| Error::new("TEST", "child close missing"))?
        );
        let error = match document.closing_tag_start(empty) {
            Ok(_) => {
                return Err(Error::new(
                    "TEST",
                    "self-closing node exposed a closing tag",
                ));
            }
            Err(error) => error,
        };
        assert_eq!(error.code, "XML_SHAPE");
        Ok(())
    }
}
