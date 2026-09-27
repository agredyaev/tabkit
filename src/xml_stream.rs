//! Checked XML events to source-relative tables; never constructs a parser DOM.
use super::*;
use xmlparser::{ElementEnd, Token, Tokenizer};
const XML_URI: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_URI: &str = "http://www.w3.org/2000/xmlns/";
#[derive(Clone, Copy)]
struct TempAttribute<'a> { prefix: &'a str, local: &'a str }
enum NamespaceUndo<'a> {
    Prefix(&'a str, Option<u32>),
    Default(u32),
}
struct Frame<'a> {
    id: NodeId, prefix: &'a str, local: &'a str, ns_mark: usize,
    text_begin: usize, leaf: bool, last_text: bool, last_element: Option<NodeId>,
    sheet: Option<NodeId>, dependency: Option<NodeId>, has_column_attr: bool,
}
struct Builder<'a> {
    source: &'a str, limits: &'a Limits, result: Xml,
    bindings: HashMap<&'a str, u32>, default_ns: u32, undo: Vec<NamespaceUndo<'a>>,
    uri_ids: HashMap<String, u32>, uris: Vec<String>,
    tag_ids: HashMap<(u32, &'a str), TextId>, last_tag: Option<(u32, &'a str, TextId)>,
    attrs: Vec<TempAttribute<'a>>, expanded: Vec<(u32, &'a str)>, common_attrs:u32,
    decoded_cache: HashMap<&'a str,Span>,
    stack: Vec<Frame<'a>>, pending: Option<Frame<'a>>,
    seen_root: bool, node_count: u32, attribute_count: usize,
}
pub(super) fn parse(bytes:Vec<u8>,limits:&Limits)->Result<Xml>{
    require(bytes.len() as u64<=limits.xml_bytes&&bytes.len()<=u32::MAX as usize,
        "LIMIT","TWB is too large")?;
    let text=String::from_utf8(bytes).map_err(|_|Error::new("ENCODING","Only UTF-8 TWB is supported; no implicit transcoding"))?;
    if text.len()<256*1024 {
        let sha256=crate::fs::sha256(text.as_bytes());
        return parse_text(text,limits,sha256);
    }
    let (mut result,sha256)=std::thread::scope(|scope|{
        let source=text.as_str();
        let digest=scope.spawn(move ||{
            fast::validate_characters(source.as_bytes())?;
            Ok::<_,Error>(crate::fs::sha256(source.as_bytes()))
        });
        // Character admission is performed by the parallel worker for this large-input path.
        let parsed=parse_text_without_hash(source,limits,true);
        let sha256=digest.join().map_err(|_|Error::new("INTERNAL","Hash worker failed"))??;
        Ok::<_,Error>((parsed?,sha256))
    })?;
    result.sha256=sha256;result.text=text;Ok(result)
}
pub(super) fn parse_with_sha256(bytes:Vec<u8>,limits:&Limits,sha256:String)->Result<Xml>{
    require(bytes.len() as u64<=limits.xml_bytes&&bytes.len()<=u32::MAX as usize,
        "LIMIT","TWB is too large")?;
    let text=String::from_utf8(bytes).map_err(|_|Error::new("ENCODING","Only UTF-8 TWB is supported; no implicit transcoding"))?;
    parse_text(text,limits,sha256)
}
fn parse_text(text:String,limits:&Limits,sha256:String)->Result<Xml>{
    let mut result=parse_text_without_hash(&text,limits,false)?;
    result.sha256=sha256;result.text=text;Ok(result)
}
fn parse_text_without_hash(text:&str,limits:&Limits,characters_checked:bool)->Result<Xml>{
    check_encoding_declaration(text)?;
    let fast=if characters_checked {fast::parse_prechecked(text,limits)} else {fast::parse(text,limits)}?;
    if let Some(result)=fast { return Ok(result); }
    Builder::new(text,limits).run()
}
impl<'a> Builder<'a> {
    fn new(source: &'a str, limits: &'a Limits) -> Self {
        Self { source, limits, result: Xml { text: String::new(), nodes: Vec::new(),
            normalized:Normalized::default(), names:Names::default(), leaf_texts:Vec::new(),
            leaf_text:String::new(), semantic:SemanticNodes::default(), sha256:String::new() },
            bindings: HashMap::from([("xml", 1)]), default_ns: 0, undo: Vec::new(),
            uri_ids: HashMap::from([(String::new(), 0), (XML_URI.to_owned(), 1)]),
            uris: vec![String::new(), XML_URI.to_owned()], tag_ids: HashMap::new(), last_tag: None,
            attrs: Vec::new(), expanded: Vec::new(), common_attrs:0, decoded_cache: HashMap::new(),
            stack: Vec::new(), pending: None,
            seen_root: false, node_count: 1, attribute_count: 0 }
    }
    fn count_node(&mut self) -> Result<()> {
        self.node_count = self.node_count.checked_add(1).ok_or_else(|| Error::new("XML", "XML node count overflow"))?;
        require(self.node_count <= self.limits.xml_nodes, "XML", "XML node limit exceeded")
    }
    fn run(mut self) -> Result<Xml> {
        require(self.node_count <= self.limits.xml_nodes, "XML", "XML node limit exceeded")?;
        for token in Tokenizer::from(self.source) {
            match token.map_err(|e| Error::new("XML", e.to_string()))? {
                Token::ElementStart { prefix, local, span } => self.start(prefix.as_str(), local.as_str(), span.range())?,
                Token::Attribute { prefix, local, value, .. } => self.attribute(prefix.as_str(), local.as_str(), value.range())?,
                Token::ElementEnd { end, span } => self.end(end, span.range())?,
                Token::Text { text } => self.text(text.as_str(), false)?,
                Token::Cdata { text, .. } => self.text(text.as_str(), true)?,
                Token::Comment { .. } | Token::ProcessingInstruction { .. } => {
                    self.count_node()?;
                    if let Some(f) = self.stack.last_mut() { f.last_text = false; }
                }
                Token::Declaration { version, encoding, .. } => {
                    require(version.as_str() == "1.0", "XML", "Only XML 1.0 is supported")?;
                    require(encoding.is_none_or(|e| e.as_str().eq_ignore_ascii_case("utf-8")), "ENCODING", "Only UTF-8 is supported")?;
                }
                Token::DtdStart { .. } | Token::EmptyDtd { .. } | Token::EntityDeclaration { .. } | Token::DtdEnd { .. } =>
                    return Err(Error::new("XML", "DTD declarations are disabled")),
            }
        }
        self.finish_run()
    }
    fn finish_run(self)->Result<Xml> {
        require(self.seen_root && self.stack.is_empty() && self.pending.is_none(), "XML", "Incomplete XML document")?;
        Ok(self.result)
    }
    fn start(&mut self, prefix: &'a str, local: &'a str, span: Range<usize>) -> Result<()> {
        require(self.pending.is_none(), "XML", "Unfinished start tag")?;
        if self.stack.is_empty() {
            require(!self.seen_root, "XML", "Multiple XML root elements")?; self.seen_root = true;
        }
        self.count_node()?;
        let id = NodeId(self.result.nodes.len() as u32);
        let parent = self.stack.last().map(|f| f.id);
        let sheet = self.stack.last().and_then(|f| f.sheet);
        let dependency = self.stack.last().and_then(|f| f.dependency);
        if let Some(f) = self.stack.last_mut() {
            if f.leaf { self.result.leaf_text.truncate(f.text_begin); f.leaf = false; }
            f.last_text = false;
            if let Some(last) = f.last_element { self.result.nodes[last.0 as usize].next_sibling=id.0; }
            else { self.result.nodes[f.id.0 as usize].first_child=id.0; }
            f.last_element = Some(id);
        }
        let normalized = self.result.normalized.source_starts.len();
        self.result.nodes.push(Node { parent:Node::link(parent), first_child:NONE_ID, next_sibling:NONE_ID,
            name:TextId(0), span:Span::new(span.clone())?,
            attributes:IndexRange::empty(self.attribute_count), leaf_text:NONE_ID,
            worksheet_owner:Node::link(sheet), dependency_owner:Node::link(dependency),
            raw_attributes:Span::new(span.end..span.end)?, normalized:IndexRange::empty(normalized) });
        self.attrs.clear(); self.expanded.clear(); self.common_attrs=0;
        self.pending = Some(Frame { id, prefix, local, ns_mark: self.undo.len(),
            text_begin: self.result.leaf_text.len(), leaf: true, last_text: false,
            last_element: None, sheet, dependency, has_column_attr: false });
        Ok(())
    }
    #[inline]
    fn common_attr_bit(local:&str)->Option<u32> {
        Some(match local {
            "name"=>1<<0, "type"=>1<<1, "caption"=>1<<2, "datatype"=>1<<3,
            "role"=>1<<4, "class"=>1<<5, "formula"=>1<<6, "datasource"=>1<<7,
            "column"=>1<<8, "derivation"=>1<<9, "key"=>1<<10, "source-build"=>1<<11,
            "id"=>1<<12, "x"=>1<<13, "y"=>1<<14, "w"=>1<<15, "h"=>1<<16,
            _=>return None,
        })
    }
    fn attribute(&mut self, prefix: &'a str, local: &'a str, value: Range<usize>) -> Result<()> {
        let raw: &'a str = &self.source[value.clone()];
        self.attribute_known(prefix,local,value,attribute_special(raw.as_bytes()).is_some())
    }
    fn attribute_known(&mut self, prefix: &'a str, local: &'a str, value: Range<usize>, special:bool) -> Result<()> {
        require(self.pending.is_some(), "XML", "Attribute outside a start tag")?;
        let raw: &'a str = &self.source[value.clone()];
        let decoded = if special {
            let span=if let Some(span)=self.decoded_cache.get(raw) {*span} else {
                let pool=&mut self.result.normalized;
                let begin=pool.text.len(); decode_attribute(raw,&mut pool.text)?;
                let span=Span::new(begin..pool.text.len())?;
                self.decoded_cache.insert(raw,span); span
            };
            let pool=&mut self.result.normalized;
            pool.source_starts.push(value.start as u32);pool.spans.push(span);Some(span)
        } else { None };
        self.attribute_count += 1;
        let declaration=prefix=="xmlns" || (prefix.is_empty()&&local=="xmlns");
        if declaration {
            let declaration_name=if prefix.is_empty() {"xmlns"} else {local};
            let expanded=(u32::MAX,declaration_name);
            require(!self.expanded.contains(&expanded),"XML","Duplicate expanded attribute name")?;
            self.expanded.push(expanded);
            let uri = match decoded { Some(s) => &self.result.normalized.text[s.range()], None => raw };
            let p = if prefix.is_empty() { "" } else { local };
            require(p != "xmlns" && uri != XMLNS_URI, "XML", "Reserved xmlns namespace")?;
            require((p == "xml") == (uri == XML_URI), "XML", "Reserved xml namespace binding")?;
            require(p.is_empty() || !uri.is_empty(), "XML", "A prefix cannot bind an empty namespace")?;
            let ns = match self.uri_ids.get(uri) {
                Some(id) => *id,
                None => { let id = self.uris.len() as u32;
                    self.uris.push(uri.to_owned()); self.uri_ids.insert(uri.to_owned(), id); id }
            };
            if p.is_empty() {
                self.undo.push(NamespaceUndo::Default(self.default_ns));
                self.default_ns = ns;
            } else {
                let old=self.bindings.insert(p,ns);
                self.undo.push(NamespaceUndo::Prefix(p,old));
            }
        } else if prefix.is_empty() {
            if let Some(bit)=Self::common_attr_bit(local) {
                require(self.common_attrs&bit==0,"XML","Duplicate expanded attribute name")?;
                self.common_attrs|=bit;
            } else {
                let expanded=(0,local);
                require(!self.expanded.contains(&expanded),"XML","Duplicate expanded attribute name")?;
                self.expanded.push(expanded);
            }
            if local=="column" {
                if let Some(f)=self.pending.as_mut() { f.has_column_attr=true; }
            }
        } else {
            // Prefix bindings apply to the whole start tag, including declarations that follow.
            self.attrs.push(TempAttribute{prefix,local});
        }
        Ok(())
    }
    fn namespace(&self, prefix: &str, default: bool) -> Result<u32> {
        if prefix.is_empty() { return Ok(if default { self.default_ns } else { 0 }); }
        self.bindings.get(prefix).copied().ok_or_else(|| Error::new("XML", format!("Undeclared namespace prefix {prefix}")))
    }
    fn rollback_namespaces(&mut self, mark: usize) {
        while self.undo.len() > mark {
            match self.undo.pop() {
                Some(NamespaceUndo::Default(old)) => self.default_ns=old,
                Some(NamespaceUndo::Prefix(prefix,old)) => match old {
                    Some(ns)=>{self.bindings.insert(prefix,ns);},
                    None=>{self.bindings.remove(prefix);},
                },
                None=>break,
            }
        }
    }
    fn intern_tag(&mut self, key:(u32,&'a str))->TextId {
        if let Some(id)=self.tag_ids.get(&key) { return *id; }
        let (ns,local)=key;
        let id=TextId(self.result.names.strings.len() as u32);
        self.result.names.strings.push(if ns==0 {local.to_owned()} else {format!("{{{}}}{}",self.uris[ns as usize],local)});
        self.tag_ids.insert(key,id); id
    }
    fn end(&mut self, end: ElementEnd<'a>, span: Range<usize>) -> Result<()> {
        if let ElementEnd::Close(prefix, local) = end {
            require(self.pending.is_none(), "XML", "Unexpected end tag")?;
            let f = self.stack.pop().ok_or_else(|| Error::new("XML", "End tag without start"))?;
            require((f.prefix, f.local) == (prefix.as_str(), local.as_str()), "XML", "Start and end tags differ")?;
            self.finish(f, span.end)?; return Ok(());
        }
        let mut f = self.pending.take().ok_or_else(|| Error::new("XML", "Missing start tag"))?;
        // Unprefixed and namespace-declaration duplicates were checked as attributes arrived.
        // Only prefixed attributes need deferred expanded-name resolution after all declarations.
        for a in &self.attrs {
            let expanded=(self.namespace(a.prefix,false)?,a.local);
            require(!self.expanded.contains(&expanded),"XML","Duplicate expanded attribute name")?;
            self.expanded.push(expanded);
        }
        let ns = self.namespace(f.prefix, true)?;
        require(f.id != NodeId(0) || ns == 0, "UNSUPPORTED_SHAPE", "Namespaced workbook roots are unsupported")?;
        let key=(ns,f.local);
        let name=if let Some((cached_ns,cached_local,id))=self.last_tag {
            if cached_ns==ns && cached_local==f.local { id } else {
                let id=self.intern_tag(key);
                self.last_tag=Some((ns,f.local,id)); id
            }
        } else {
            let id=self.intern_tag(key);
            self.last_tag=Some((ns,f.local,id)); id
        };
        if ns==0 {
            let semantic=&mut self.result.semantic;
            match f.local {
                "datasource-dependencies"=>semantic.datasource_dependencies.push(f.id),
                "column-instance"=>semantic.column_instances.push(f.id),
                "rows"|"cols"=>semantic.shelves.push(f.id),
                "worksheet"=>semantic.worksheets.push(f.id),
                "dashboard"=>semantic.dashboards.push(f.id),
                "filter"=>semantic.filters.push(f.id),
                "metadata-record"=>semantic.metadata_records.push(f.id),
                _=>{}
            }
            if f.local!="column-instance" && f.has_column_attr {
                semantic.column_bindings.push(f.id);
            }
        }
        let n = &mut self.result.nodes[f.id.0 as usize];
        n.name = name; n.attributes.end=self.attribute_count as u32;
        n.raw_attributes.end = span.start as u32;
        n.normalized.end=self.result.normalized.source_starts.len() as u32;
        if ns == 0 && f.local == "worksheet" { f.sheet = Some(f.id); }
        if ns == 0 && f.local == "datasource-dependencies" { f.dependency = Some(f.id); }
        if end == ElementEnd::Empty { self.finish(f, span.end)?; }
        else { self.stack.push(f); }
        Ok(())
    }
    fn finish(&mut self, f: Frame<'a>, end: usize) -> Result<()> {
        let n = &mut self.result.nodes[f.id.0 as usize]; n.span.end = end as u32;
        if f.leaf && self.result.leaf_text.len() > f.text_begin {
            n.leaf_text=self.result.leaf_texts.len() as u32;
            self.result.leaf_texts.push(Span::new(f.text_begin..self.result.leaf_text.len())?);
        }
        self.rollback_namespaces(f.ns_mark);
        Ok(())
    }
    fn text(&mut self, raw: &str, cdata: bool) -> Result<()> {
        if self.stack.is_empty() {
            require(!cdata && raw.bytes().all(whitespace), "XML", "Text outside the document element")?;
            return Ok(());
        }
        if raw.is_empty() && !cdata { return Ok(()); }
        if self.stack.last().is_some_and(|f| !f.last_text) { self.count_node()?; }
        let f = self.stack.last_mut().ok_or_else(|| Error::new("INTERNAL", "Missing text owner"))?;
        f.last_text = true;
        normalize_text(raw, cdata, if f.leaf { Some(&mut self.result.leaf_text) } else { None })
    }
}
/// Entity expansion is bounded: DTDs are rejected, only predefined/numeric references exist.
pub(super) fn reference(entity: &str) -> Result<char> {
    let value = match entity {
        "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'),
        "quot" => Some('"'), "apos" => Some('\''),
        _ => {
            let n = if let Some(s) = entity.strip_prefix("#x") {
                if s.is_empty() || !s.bytes().all(|b| b.is_ascii_hexdigit()) { None } else { u32::from_str_radix(s, 16).ok() }
            } else if let Some(s) = entity.strip_prefix('#') {
                if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) { None } else { s.parse::<u32>().ok() }
            } else { None };
            n.and_then(char::from_u32)
        }
    };
    value.filter(|c| valid_xml_char(*c)).ok_or_else(|| Error::new("XML", "Invalid or undeclared character/entity reference"))
}
fn normalize_text(mut raw: &str, cdata: bool, mut out: Option<&mut String>) -> Result<()> {
    loop {
        let at=if cdata {memchr::memchr(b'\r',raw.as_bytes())} else {memchr::memchr2(b'&',b'\r',raw.as_bytes())};
        let Some(at) = at else { if let Some(out) = out { out.push_str(raw); } return Ok(()); };
        if let Some(out) = out.as_deref_mut() { out.push_str(&raw[..at]); }
        let kind = raw.as_bytes()[at]; raw = &raw[at+1..];
        let value = if kind == b'\r' {
            if raw.starts_with('\n') { raw = &raw[1..]; }
            '\n'
        } else {
            let end = raw.find(';').ok_or_else(|| Error::new("XML", "Unterminated character reference"))?;
            let value = reference(&raw[..end])?; raw = &raw[end+1..]; value
        };
        if let Some(out) = out.as_deref_mut() { out.push(value); }
    }
}

#[cfg(test)]
pub(super) fn fast_for_test(bytes:Vec<u8>,limits:&Limits)->Result<Option<Xml>> {
    let text=String::from_utf8(bytes).map_err(|_|Error::new("ENCODING","Only UTF-8 TWB is supported; no implicit transcoding"))?;
    check_encoding_declaration(&text)?;
    let Some(mut result)=fast::parse(&text,limits)? else { return Ok(None); };
    result.sha256=crate::fs::sha256(text.as_bytes());result.text=text;Ok(Some(result))
}
#[cfg(test)]
pub(super) fn fallback_for_test(bytes:Vec<u8>,limits:&Limits)->Result<Xml> {
    let text=String::from_utf8(bytes).map_err(|_|Error::new("ENCODING","Only UTF-8 TWB is supported; no implicit transcoding"))?;
    check_encoding_declaration(&text)?;
    let mut result=Builder::new(&text,limits).run()?;
    result.sha256=crate::fs::sha256(text.as_bytes());result.text=text;Ok(result)
}
#[path = "xml_fast.rs"]
mod fast;
