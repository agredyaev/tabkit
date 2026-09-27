//! Wire admission regressions use an independent parser, not just the index itself.
use super::*;
fn parsed(s: &str) -> Xml { Xml::parse(s.as_bytes().to_vec(), &Limits::default()).unwrap() }
#[test]
fn expanded_names_and_namespace_shadowing_are_checked() {
    for s in [
        "<a xmlns:p='urn:u' xmlns:q='urn:u' p:x='1' q:x='2'/>",
        "<a xmlns:p='urn:a' xmlns:p='urn:b'/>", "<a p:x='1'/>", "<a><p:x/></a>",
        "<a xmlns:p=''/>", "<a xmlns:xml='urn:wrong'/>",
        "<a xmlns:p='http://www.w3.org/XML/1998/namespace'/>",
        "<a xmlns='http://www.w3.org/2000/xmlns/'/>", "<a xmlns:xmlns='urn:x'/>",
        "<a><p:x xmlns:p='urn:x'/><p:x/></a>",
    ] { assert!(Xml::parse(s.as_bytes().to_vec(), &Limits::default()).is_err(), "{s}"); }
    let x = parsed("<a><p:x p:v='1' q:v='2' xmlns:p='urn:a' xmlns:q='urn:b'/><b xmlns='urn:d'><c xmlns=''/></b></a>");
    assert_eq!(x.tag(NodeId(1)), "{urn:a}x");
    assert_eq!(x.tag(NodeId(2)), "{urn:d}b"); assert_eq!(x.tag(NodeId(3)), "c");
    assert_eq!(x.value(NodeId(1), "p:v"), Some("1"));
    assert_eq!(x.value(NodeId(1), "q:v"), Some("2"));
}
#[test]
fn invalid_references_and_truncated_documents_never_produce_an_index() {
    for s in ["<a>&missing;</a>", "<a x='&#0;'/>", "<a x='&#xD800;'/>",
        "<a x='&#x110000;'/>", "<a x='&amp'/>", "<a>&#x+41;</a>",
        "<a x='&#-1;'/>", "<a x='&#x;'/>", "<a x='a<b'/>",
        "<a><b></a></b>", "<a/><!--ok--><b/>", "<a>", "", "<a/>text"] {
        // XML 1.0 rejection contract; the oracle substitutes some invalid numeric references.
        assert!(Xml::parse(s.as_bytes().to_vec(), &Limits::default()).is_err(), "{s}");
    }
}
#[test]
fn text_and_cdata_keep_newline_and_reference_rules_separate() {
    let s = "<a>pre&#13;\r\n<![CDATA[&amp;\r\n]]><!--x-->post&#x1f980;</a>";
    let x = parsed(s); let oracle = roxmltree::Document::parse(s).unwrap();
    let value: String = oracle.root_element().children().filter(|n| n.is_text()).filter_map(|n| n.text()).collect();
    assert_eq!(x.text_content(NodeId(0)).unwrap(), value);
    assert_eq!(value, "pre\r\n&amp;\npost🦀");
    assert_eq!(parsed("<a>discard<b>keep</b>discard</a>").text_content(NodeId(1)).unwrap(), "keep");
}
#[test]
fn iterator_supports_interleaved_front_and_back_without_allocation_or_copy() {
    let x = parsed("<a zero = '' one=\"a&amp;b\" two='two' three = \"三\"/>");
    let mut it = x.attributes(NodeId(0));
    assert_eq!(it.len(), 4); assert_eq!(it.next().unwrap().name, "zero");
    assert_eq!(it.next_back().unwrap().value, "三");
    assert_eq!(it.next().unwrap().value, "a&b"); assert_eq!(it.len(), 1);
    assert_eq!(it.next_back().unwrap().value, "two");
    assert!(it.next().is_none()); assert!(it.next_back().is_none());
}
#[test]
fn streamed_context_matches_generic_ancestor_walk() {
    let x = parsed("<a><worksheet><table><datasource-dependencies><column/><x/></datasource-dependencies></table></worksheet><n:worksheet xmlns:n='urn:x'><column/></n:worksheet></a>");
    for i in 0..x.nodes.len() { for tag in ["worksheet", "datasource-dependencies"] {
        let mut parent = x.node(NodeId(i as u32)).parent(); let mut expected = None;
        while let Some(n) = parent { if x.tag(n) == tag { expected = Some(n); break; } parent = x.node(n).parent(); }
        assert_eq!(x.ancestor(NodeId(i as u32), tag), expected);
    }}
}
#[test]
fn node_budget_includes_non_element_nodes_and_document_root() {
    for s in ["<a/>", "<!--p--><a> x <b/> y </a><?z t?>", "<a>x<![CDATA[y]]>z</a>",
        "<a><![CDATA[]]></a>", "<a>x<!--c-->y</a>"] {
        for budget in 1..20 {
            let limits = Limits { xml_nodes: budget, ..Limits::default() };
            let oracle = roxmltree::Document::parse_with_options(s,
                roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: budget });
            let actual = Xml::parse(s.as_bytes().to_vec(), &limits);
            assert_eq!(actual.is_ok(), oracle.is_ok(), "budget={budget}, {s}");
        }
    }
}
#[test]
fn unsupported_vendor_bytes_are_checked_even_when_not_in_semantic_projection() {
    assert!(Xml::parse(b"<a><vendor x='&#0;'/></a>".to_vec(), &Limits::default()).is_err());
    assert!(Xml::parse(b"<a><!-- bad \x01 --></a>".to_vec(), &Limits::default()).is_err());
    assert!(Xml::parse(b"<a><vendor>&undeclared;</vendor></a>".to_vec(), &Limits::default()).is_err());
    assert!(Xml::parse(b"<!DOCTYPE a><a/>".to_vec(), &Limits::default()).is_err());
}

#[test]
fn tokenizer_rejects_forbidden_xml_characters_without_a_prescan() {
    for c in ['\u{1}','\u{b}','\u{c}','\u{fffe}','\u{ffff}'] {
        for source in [
            format!("<workbook>{c}</workbook>"),
            format!("<workbook a='{c}'/>"),
            format!("<workbook><![CDATA[{c}]]></workbook>"),
            format!("<workbook><!--{c}--></workbook>"),
            format!("<workbook><?p {c}?></workbook>"),
        ] {
            assert!(Xml::parse(source.into_bytes(),&Limits::default()).is_err(),
                "accepted U+{:04X}",c as u32);
        }
    }
}

#[test]
fn tokenizer_rejects_literal_lt_in_attribute_without_a_second_scan() {
    for source in [r#"<workbook a='bad<value'/>"#,r#"<workbook a="bad<value"/>"#] {
        assert!(Xml::parse(source.as_bytes().to_vec(),&Limits::default()).is_err(),"{source}");
    }
}

#[test]
fn semantic_side_tables_match_full_node_scans() {
    let source="<workbook><datasources><datasource name='d'><metadata-record class='column'/></datasource></datasources><worksheets><worksheet name='s'><table><view><datasource-dependencies datasource='d'><column name='[x]'/><column-instance name='[i]' column='[x]'/><filter class='categorical' column='[d].[i]'/></datasource-dependencies><rows>[d].[i]</rows></view></table></worksheet></worksheets><dashboards><dashboard name='d'/></dashboards></workbook>";
    let x=Xml::parse(source.as_bytes().to_vec(),&Limits::default()).unwrap();
    let by=|tag:&str|x.nodes.iter().enumerate().filter_map(|(i,_)|(x.tag(NodeId(i as u32))==tag).then_some(NodeId(i as u32))).collect::<Vec<_>>();
    assert_eq!(x.semantic.datasource_dependencies,by("datasource-dependencies"));
    assert_eq!(x.semantic.column_instances,by("column-instance"));
    assert_eq!(x.semantic.shelves,[by("rows"),by("cols")].concat());
    assert_eq!(x.semantic.worksheets,by("worksheet"));
    assert_eq!(x.semantic.dashboards,by("dashboard"));
    assert_eq!(x.semantic.filters,by("filter"));
    assert_eq!(x.semantic.metadata_records,by("metadata-record"));
    let expected:Vec<_>=x.nodes.iter().enumerate().filter_map(|(i,_)|{let id=NodeId(i as u32);(x.tag(id)!="column-instance"&&x.value(id,"column").is_some()).then_some(id)}).collect();
    assert_eq!(x.semantic.column_bindings,expected);
}
