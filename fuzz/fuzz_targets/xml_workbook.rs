#![no_main]
use libfuzzer_sys::fuzz_target;
use tabkit_fuzz::{config::Limits,xml::{Xml,NodeId},workbook::Workbook,validation};
fuzz_target!(|data:&[u8]|{
    if data.len()>65536{return;}
    let limits=Limits{xml_bytes:65536,xml_nodes:10000,..Default::default()};
    if let Ok(xml)=Xml::parse(data.to_vec(),&limits){
        assert_eq!(xml.text.as_bytes(),data);
        let oracle=roxmltree::Document::parse(&xml.text).expect("accepted XML must satisfy independent parser");
        let mut elements=oracle.descendants().filter(|n|n.is_element());
        for(i,node)in xml.nodes.iter().enumerate(){
            let n=elements.next().unwrap();let id=NodeId(i as u32);
            assert_eq!(node.span.range(),n.range());
            let tag=n.tag_name();let expected=tag.namespace().map(|uri|format!("{{{uri}}}{}",tag.name())).unwrap_or_else(||tag.name().into());
            assert_eq!(xml.tag(id),expected);
            for a in xml.attributes(id){
                assert!(xml.text.get(a.span.range()).is_some());
                assert!(a.span.start>=node.span.start&&a.span.end<=node.span.end);
                if a.name=="xmlns"||a.name.starts_with("xmlns:"){continue;}
                let (prefix,local)=a.name.split_once(':').unwrap_or(("",a.name));
                let ns=if prefix.is_empty(){None}else{n.lookup_namespace_uri(Some(prefix))};
                let value=n.attributes().find(|b|b.name()==local&&b.namespace()==ns).unwrap().value();
                assert_eq!(a.value,value);
            }
            for c in xml.children(id){assert_eq!(xml.node(c).parent(),Some(id));}
            if node.first_child().is_none(){let value:String=n.children().filter(|c|c.is_text()).filter_map(|c|c.text()).collect();assert_eq!(xml.text_content(id).unwrap(),value);}
        }
        assert!(elements.next().is_none());
        drop(oracle);
        if let Ok(b)=Workbook::from_xml(xml){
            let _=b.snapshot();let _=b.overview();let _=b.require_acyclic();let _=validation::local(&b);
            for i in 0..b.fields.len(){let _=b.field_report(i);}
            for i in 0..b.filters.len(){let _=b.filter_report(i);}
            for i in 0..b.dependency_scopes.len(){let _=b.scope_report(i);}
        }
    }
});
