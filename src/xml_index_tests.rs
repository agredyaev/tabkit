//! Differential checks for the XML index/decoder, using the parser as the oracle.
use crate::{config::Limits, xml::{Xml, NodeId}};
fn compare(source:&str) {
    let oracle=roxmltree::Document::parse(source).unwrap();
    let elements:Vec<_>=oracle.descendants().filter(|n|n.is_element()).collect();
    let indexed=Xml::parse(source.as_bytes().to_vec(),&Limits::default()).unwrap();
    assert_eq!(indexed.text,source); assert_eq!(indexed.nodes.len(),elements.len());
    for (i,n) in elements.iter().enumerate() {
        let id=NodeId(i as u32);let actual=indexed.node(id);
        let position=|node:roxmltree::Node<'_, '_>|elements.iter().position(|e|*e==node).map(|v|NodeId(v as u32));
        assert_eq!(actual.span.range(),n.range());
        assert_eq!(actual.parent,n.parent().and_then(position));
        assert_eq!(indexed.children(id).collect::<Vec<_>>(),n.children().filter(|c|c.is_element()).map(|c|position(c).unwrap()).collect::<Vec<_>>());
        let tag=n.tag_name();let expected=tag.namespace().map(|ns|format!("{{{ns}}}{}",tag.name())).unwrap_or_else(||tag.name().into());
        assert_eq!(indexed.tag(id),expected);
        for a in n.attributes().filter(|a|a.namespace().is_none()) {
            assert_eq!(indexed.value(id,a.name()),Some(a.value()));
            let stored=indexed.attr(id,a.name()).unwrap();
            assert_eq!(source.as_bytes()[stored.span.start as usize-1],stored.quote);
            assert_eq!(source.as_bytes()[stored.span.end as usize],stored.quote);
        }
        if !n.children().any(|c|c.is_element()) {
            let expected:String=n.children().filter(|c|c.is_text()).filter_map(|c|c.text()).collect();
            assert_eq!(indexed.text_content(id).unwrap(),expected);
        } else { assert!(indexed.text_content(id).is_err()); }
    }
}
#[test]
fn decoded_attributes_preserve_xml_whitespace_and_reference_semantics() {
    let values=["", "plain", "España 🦀", "a\tb\r\nc\rd\ne", "&#9;&#10;&#13;",
        "&amp;lt; &amp;amp; &lt; &gt; &quot; &apos;", "a\r\n&#10;\t&#xD;z", "&#x1F980;&#241;"];
    for q in ['\'', '"'] { for value in values {
        compare(&format!("<workbook a={q}{value}{q}><leaf a={q}{value}{q}>x<![CDATA[y]]>&amp;z</leaf></workbook>"));
    }}
    let x=Xml::parse(b"<workbook a='&#10;\n&#13;\r\n&#9;\t'/>".to_vec(),&Limits::default()).unwrap();
    assert_eq!(x.value(NodeId(0),"a"),Some("\n \r \t "));
}
#[test]
fn preorder_links_and_namespace_identity_survive_comments_and_empty_nodes() {
    for depth in [0,1,2,8,32] {
        let nesting="<branch><empty/><!-- between -->".repeat(depth);
        let closing="<tail/></branch>".repeat(depth);
        let source=format!("<workbook xmlns:u='urn:one'><u:a/><a/>{nesting}<u:a xmlns:u='urn:two'><u:b/></u:a>{closing}<?end ok?><last>text</last></workbook>");
        compare(&source);
        compare(&format!("\u{feff}<?xml version='1.0' encoding='UTF-8'?>{source}"));
    }
}
#[test]
fn parser_rejections_are_not_relaxed_by_index_optimization() {
    for source in ["", "<workbook a='1' a='2'/>", "<workbook a='&unknown;'/>",
        "<workbook a='&#0;'/>", "<workbook><child></workbook>",
        "<!DOCTYPE workbook [<!ENTITY x 'v'>]><workbook a='&x;'/>"] {
        assert!(Xml::parse(source.as_bytes().to_vec(),&Limits::default()).is_err(),"{source}");
    }
}
use proptest::prelude::*;
proptest! {
    #[test]
    fn generated_attribute_and_tree_shapes_match_parser(
        controls in prop::collection::vec(any::<u8>(),0..32),
        single in any::<bool>(), crlf in any::<bool>()) {
        let fragments=["a","é","🦀","&amp;","&#xA;","&#13;","&#9;","\t","\n","\r\n","&amp;lt;","&quot;","&apos;"];
        let q=if single {'\''} else {'"'};
        let mut source="<workbook xmlns:u='urn:one'>".to_owned();
        for (i,b) in controls.iter().enumerate() {
            let v=fragments[usize::from(*b)%fragments.len()];
            source.push_str(&format!("<branch id='{i}' a={q}{v}{q}><u:n/><empty/><!--keep--><leaf>{v}</leaf></branch>"));
        }
        source.push_str("</workbook>");
        if crlf { source=source.replace('\n',"\r\n"); }
        compare(&source);
    }
}
