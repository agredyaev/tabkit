#![no_main]
use libfuzzer_sys::fuzz_target;
use tabkit_fuzz::{config::Limits,xml::{Xml,NodeId},workbook::Workbook,validation};
fuzz_target!(|data:&[u8]|{
    if data.len()>65536{return;}
    let limits=Limits{xml_bytes:65536,xml_nodes:10000,..Default::default()};
    if let Ok(xml)=Xml::parse(data.to_vec(),&limits){
        assert_eq!(xml.text.as_bytes(),data);
        for(i,node)in xml.nodes.iter().enumerate(){
            let id=NodeId(i as u32);
            assert!(xml.text.get(node.span.range()).is_some());
            for a in xml.attributes(id){
                assert!(xml.text.get(a.span.range()).is_some());
                assert!(a.span.start>=node.span.start&&a.span.end<=node.span.end);
            }
            for c in xml.children(id){assert_eq!(xml.node(c).parent,Some(id));}
            let _=xml.text_content(id);
        }
        if let Ok(b)=Workbook::from_xml(xml){
            let _=b.snapshot();let _=b.overview();let _=b.require_acyclic();let _=validation::local(&b);
            for i in 0..b.fields.len(){let _=b.field_report(i);}
            for i in 0..b.filters.len(){let _=b.filter_report(i);}
            for i in 0..b.dependency_scopes.len(){let _=b.scope_report(i);}
        }
    }
});
