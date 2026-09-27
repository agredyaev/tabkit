//! Storage/lifetime checks; observable XML behavior is also checked against roxmltree.
use super::*;
use crate::config::Limits;
fn parse(source: &str) -> Xml {
    Xml::parse(source.as_bytes().to_vec(), &Limits::default()).unwrap()
}
fn check_columns(xml: &Xml) {
    assert_eq!(xml.normalized.source_starts.len(), xml.normalized.spans.len());
    assert!(xml.normalized.source_starts.windows(2).all(|p| p[0] < p[1]));
    for i in 0..xml.nodes.len() {
        let node = xml.node(NodeId(i as u32));
        for a in xml.attributes(NodeId(i as u32)) {
            let begin = a.name.as_ptr() as usize - xml.text.as_ptr() as usize;
            assert_eq!(&xml.text[begin..begin+a.name.len()], a.name);
            let expected = xml.attribute_value(a.span, node.normalized);
            assert_eq!(a.value.as_ptr(), expected.as_ptr());
            assert_eq!(xml.text.as_bytes()[a.span.start as usize-1], a.quote);
            assert_eq!(xml.text.as_bytes()[a.span.end as usize], a.quote);
        }
    }
}
#[test]
fn plain_values_borrow_source_and_only_transformed_values_use_the_pool() {
    let x = parse("<workbook plain='España 🦀' empty='' a='A&amp;B&#10;C' b='x\r\ny\t' />");
    check_columns(&x);
    assert_eq!(x.attributes(NodeId(0)).count(), 4);
    assert_eq!(x.normalized.spans, vec![Span { start: 0, end: 5 }, Span { start: 5, end: 9 }]);
    assert_eq!(x.normalized.text, "A&B\nCx y ");
    assert_eq!(x.value(NodeId(0), "plain"), Some("España 🦀"));
    assert_eq!(x.value(NodeId(0), "empty"), Some(""));
    assert!(x.attr(NodeId(0), "missing").is_none());
}
#[test]
fn moving_a_grown_document_preserves_all_owner_relative_spans() {
    let mut source = String::from("<workbook>");
    for i in 0..2048 {
        source.push_str(&format!("<n a='{i}' b='left&amp;🦀&#xA;{i}'/>"));
    }
    source.push_str("</workbook>");
    let parsed = parse(&source);
    let mut documents = Vec::new();
    documents.push(parsed);
    // Move the owner; the table contains offsets, never pointers into itself.
    let moved = documents.pop().unwrap();
    check_columns(&moved);
    assert_eq!(moved.text, source);
    for i in 0..2048 {
        let id = NodeId(i + 1);
        assert_eq!(moved.value(id, "a"), Some(i.to_string().as_str()));
        assert_eq!(moved.value(id, "b"), Some(format!("left&🦀\n{i}").as_str()));
    }
}
#[test]
fn namespace_declarations_and_prefixed_names_remain_in_original_order() {
    let x = parse("<workbook xmlns:u='urn:one' u:a='A&amp;B' a='plain'><n xmlns:u='urn:two' u:a='other'/></workbook>");
    check_columns(&x);
    let root: Vec<_> = x.attributes(NodeId(0)).map(|a| (a.name, a.value)).collect();
    assert_eq!(root, vec![("xmlns:u", "urn:one"), ("u:a", "A&B"), ("a", "plain")]);
    assert_eq!(x.value(NodeId(1), "xmlns:u"), Some("urn:two"));
    assert_eq!(x.value(NodeId(1), "u:a"), Some("other"));
    assert!(x.value(NodeId(1), "a").is_none());
    let reversed: Vec<_> = x.attributes(NodeId(0)).rev().map(|a| a.name).collect();
    assert_eq!(reversed, vec!["a", "u:a", "xmlns:u"]);
}
#[test]
fn separate_documents_never_share_mutable_decoded_storage() {
    let first = parse("<workbook x='first&amp;' plain='one'/>");
    let second = parse("<workbook x='other&amp;' plain='two'/>");
    check_columns(&first);
    check_columns(&second);
    let a = first.attr(NodeId(0), "x").unwrap();
    let b = second.attr(NodeId(0), "x").unwrap();
    assert_eq!(a.span, b.span);
    assert_eq!(a.value, "first&");
    assert_eq!(b.value, "other&");
    assert_ne!(a.value.as_ptr(), b.value.as_ptr());
    drop(second);
    assert_eq!(first.value(NodeId(0), "x"), Some("first&"));
}
#[test]
fn empty_tables_and_unicode_names_keep_exact_source_spans() {
    let empty = parse("<workbook><leaf/></workbook>");
    check_columns(&empty);
    assert!(empty.normalized.text.is_empty());
    assert_eq!(empty.attributes(NodeId(0)).len(), 0);
    assert_eq!(empty.attributes(NodeId(1)).len(), 0);
    let source = "<workbook café='é' 名='&#x1F980;' a='&amp;amp;'/>";
    let x = parse(source);
    check_columns(&x);
    let oracle = roxmltree::Document::parse(source).unwrap();
    for original in oracle.root_element().attributes() {
        let actual = x.attr(NodeId(0), original.name()).unwrap();
        assert_eq!(actual.value, original.value());
    }
    assert_eq!(x.value(NodeId(0), "a"), Some("&amp;"));
    assert_eq!(x.value(NodeId(0), "名"), Some("🦀"));
}
