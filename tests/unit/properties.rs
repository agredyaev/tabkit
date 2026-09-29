//! Independent models, not assertions that just call the implementation twice.
use crate::{
    config::{Config, Limits, Policy},
    edit::{self, ChangeSet, Operation},
    fs,
    patch::{self, Patch},
    scalar::{Domain, Scalar},
    wire,
    workbook::{FieldId, FilterId, Workbook, state_hash},
    xml::{self, NodeId, Span},
};
use proptest::prelude::*;
use serde_json::{Value, json};

fn text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..64).prop_map(|v| v.into_iter().collect())
}
fn xml_text() -> impl Strategy<Value = String> {
    prop::collection::vec(
        any::<char>().prop_filter("XML 1.0", |c| {
            matches!(*c, '\n' | '\r' | '\t')
                || (*c as u32 >= 32 && !matches!(*c, '\u{fffe}' | '\u{ffff}'))
        }),
        0..64,
    )
    .prop_map(|v| v.into_iter().collect())
}
fn money(n: i64) -> String {
    let n = n as i128;
    let magnitude = n.abs();
    let sign = if n < 0 { "-" } else { "" };
    let whole = magnitude / 100;
    let fraction = magnitude % 100;
    if fraction == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction:02}")
            .trim_end_matches('0')
            .to_owned()
    }
}
fn config() -> Config {
    Config {
        workspace: std::path::PathBuf::new(),
        limits: Limits::default(),
        policy: Policy {
            allow_unverified_formula_edits: true,
            ..Default::default()
        },
        tableau: None,
        hyper: None,
    }
}
const GOOD: &str = include_str!("../../examples/synthetic.twb");

proptest! {
    #[test]
    fn patch_matches_segment_oracle(parts in prop::collection::vec((text(),text(),text()),1..12),tail in text()) {
        let mut input=String::new();let mut expected=String::new();let mut patches=Vec::new();
        for (keep,old,new) in parts {
            input.push('|');input.push_str(&keep);expected.push('|');expected.push_str(&keep);
            let start=input.len();input.push_str(&old);let end=input.len();expected.push_str(&new);
            patches.push(Patch{span:Span::new(start..end).unwrap(),expected:old,replacement:new,reason:"property".into()});
        }
        input.push_str(&tail);expected.push_str(&tail);
        // Input order must not affect a non-overlapping patch set.
        patches.reverse();
        let result=patch::apply(&input,&fs::sha256(input.as_bytes()),&patches,65536).unwrap();
        prop_assert_eq!(&result,&expected);
        prop_assert!(patch::verify_preservation(&input,&result,&patches).is_ok());
        let tampered=format!("!{result}");
        prop_assert!(patch::verify_preservation(&input,&tampered,&patches).is_err());
        prop_assert!(patch::apply(&input,"stale",&patches,65536).is_err());
        if !result.is_empty() {prop_assert!(patch::apply(&input,&fs::sha256(input.as_bytes()),&patches,(result.len()-1) as u64).is_err());}
    }

    #[test]
    fn arbitrary_patch_ranges_never_panic(input in text(), replacement in text(),start in 0u32..256,end in 0u32..256) {
        let range=start as usize..end as usize;
        let expected=input.get(range.clone()).unwrap_or("").to_owned();
        let p=Patch{span:Span{start,end},expected:expected.clone(),replacement:replacement.clone(),reason:String::new()};
        let valid=range.start<=range.end && input.get(range.clone()).is_some();
        let result=patch::apply(&input,&fs::sha256(input.as_bytes()),&[p.clone()],65536);
        prop_assert_eq!(result.is_ok(),valid);
        if let Ok(out)=result {
            let mut independent=input.as_bytes().to_vec();
            independent.splice(range,replacement.bytes());
            prop_assert_eq!(out.as_bytes(),independent);
            prop_assert!(patch::verify_preservation(&input,&out,&[p]).is_ok());
        }
    }

    #[test]
    fn escaped_attributes_roundtrip_exactly(value in xml_text(),single in any::<bool>()) {
        let quote=if single {b'\''}else{b'"'};
        let escaped=xml::escape_attribute(&value,quote).unwrap();
        let q=quote as char;
        let source=format!("<workbook a={q}{escaped}{q}><leaf>text</leaf></workbook>");
        // Independent XML parser verifies our scanner and encoder, not our decoder.
        let oracle=roxmltree::Document::parse(&source).unwrap();
        prop_assert_eq!(oracle.root_element().attribute("a"),Some(value.as_str()));
        let indexed=xml::Xml::parse(source.as_bytes().to_vec(),&Limits::default()).unwrap();
        prop_assert_eq!(indexed.value(NodeId(0),"a"),Some(value.as_str()));
        let a=indexed.attr(NodeId(0),"a").unwrap();
        prop_assert_eq!(&indexed.text[a.span.range()],escaped.as_str());
    }

    #[test]
    fn wire_roundtrip_preserves_values_and_enforces_exact_budget(s in text(),n in any::<i64>(),flag in any::<bool>()) {
        let v=json!({"s":s,"n":n,"flag":flag,"nested":[null,{"value":"line\nnext"}]});
        let bytes=serde_json::to_vec(&v).unwrap();
        prop_assert_eq!(wire::decode::<Value>(&bytes,bytes.len() as u64).unwrap(),v);
        prop_assert!(wire::validate(&bytes,(bytes.len()-1) as u64).is_err());
        let mut extra=bytes.clone();extra.extend_from_slice(b" null");
        prop_assert!(wire::validate(&extra,65536).is_err());
        let key=serde_json::to_string(&s).unwrap();
        let duplicate=format!("{{{key}:1,{key}:2}}");
        prop_assert!(wire::decode::<Value>(duplicate.as_bytes(),65536).is_err());
    }

    #[test]
    fn integer_domain_matches_i128_oracle(min in any::<i64>(),max in any::<i64>(),v in any::<i64>(),step in 1i64..i64::MAX) {
        let domain=Domain::Range{min:Scalar::Integer(min),max:Scalar::Integer(max),step:Some(Scalar::Integer(step))};
        let expected=min<=max && min<=v && v<=max && (v as i128-min as i128)%(step as i128)==0;
        prop_assert_eq!(domain.accepts(&Scalar::Integer(v),"integer").is_ok(),expected);
    }

    #[test]
    fn decimal_domain_matches_integer_cents_oracle(lo in -1000000i64..1000000,span in 0i64..10000,offset in -1000i64..11000,step in 1i64..1000) {
        let hi=lo+span;let v=lo+offset;
        let d=Domain::Range{min:Scalar::Real(money(lo)),max:Scalar::Real(money(hi)),step:Some(Scalar::Real(money(step)))};
        prop_assert_eq!(d.accepts(&Scalar::Real(money(v)),"real").is_ok(),offset>=0 && offset<=span && offset%step==0);
    }

    #[test]
    fn date_admission_matches_gregorian_calendar(year in 1u32..10000,month in 0u32..14,day in 0u32..34) {
        let leap=year%4==0&&(year%100!=0||year%400==0);
        let days=match month {1|3|5|7|8|10|12=>31,4|6|9|11=>30,2=>if leap {29}else{28},_=>0};
        let s=format!("{year:04}-{month:02}-{day:02}");
        prop_assert_eq!(Scalar::Date(s).literal("date").is_ok(),day>=1 && day<=days);
    }

    #[test]
    fn scalar_literals_roundtrip(n in any::<i64>(),cents in -1000000000i64..1000000000,b in any::<bool>()) {
        for v in [Scalar::Integer(n),Scalar::Real(money(cents)),Scalar::Boolean(b)] {
            let literal=v.literal(v.datatype()).unwrap();
            prop_assert_eq!(Scalar::parse(v.datatype(),&literal).unwrap(),v);
        }
    }

    #[test]
    fn batch_edits_are_idempotent_and_do_not_touch_layout(n in 1i64..101,k in 1i64..10000,lo in -10000i64..0,hi in 1i64..10000) {
        let cfg=config();let book=Workbook::parse(GOOD.as_bytes().to_vec(),&cfg.limits).unwrap();
        let(c,d)=book.parameter_state(FieldId(4)).unwrap();
        let mut ops=vec![
            Operation::SetCalculation{field_id:FieldId(3),expected_formula:"[Profit] / [Sales]".into(),formula:format!("[Profit] / ([Sales] + {k})")},
            Operation::SetParameter{field_id:FieldId(4),expected_state_hash:state_hash(&json!({"current":c,"domain":d})),current:Some(Scalar::Integer(n)),domain:None},
            Operation::SetFilterValues{filter_id:FilterId(0),expected_state_hash:state_hash(&book.filter_state(FilterId(0)).unwrap()),values:vec!["East".into(),"West".into()]},
            Operation::SetFilterRange{filter_id:FilterId(1),expected_state_hash:state_hash(&book.filter_state(FilterId(1)).unwrap()),min:Scalar::Real(lo.to_string()),max:Scalar::Real(hi.to_string())},
        ];
        let hash=fs::sha256(GOOD.as_bytes());
        let(p,after)=edit::plan("in.twb",&hash,&book,ChangeSet{schema_version:1,input_sha256:hash.clone(),operations:ops.clone()},&cfg).unwrap();
        let doc=roxmltree::Document::parse(&after.xml.text).unwrap();
        let params:Vec<_>=doc.descendants().filter(|x|x.is_element()&&x.attribute("name")==Some("[Parameter 1]")).collect();
        prop_assert_eq!(params.len(),2);
        let number=n.to_string();
        for param in params {prop_assert_eq!(param.attribute("value"),Some(number.as_str()));}
        prop_assert!(after.xml.text.contains("x='0' y='0' w='100' h='100'"));
        prop_assert!(after.xml.text.contains("<color column='[ds_orders].[none:Region:nk]'/>") );
        prop_assert!(crate::validation::local(&after).passed);
        patch::verify_preservation(GOOD,&after.xml.text,&p.patches).unwrap();
        for op in &mut ops {match op {
            Operation::SetCalculation{expected_formula,formula,..}=>*expected_formula=formula.clone(),
            Operation::SetParameter{expected_state_hash,..}=>{let(c,d)=after.parameter_state(FieldId(4)).unwrap();*expected_state_hash=state_hash(&json!({"current":c,"domain":d}));},
            Operation::SetFilterValues{filter_id,expected_state_hash,..}|Operation::SetFilterRange{filter_id,expected_state_hash,..}=>*expected_state_hash=state_hash(&after.filter_state(*filter_id).unwrap()),
        }}
        let(p2,second)=edit::plan("in.twb",&after.xml.sha256,&after,ChangeSet{schema_version:1,input_sha256:after.xml.sha256.clone(),operations:ops},&cfg).unwrap();
        prop_assert!(p2.patches.is_empty());prop_assert!(p2.delta.is_empty());
        prop_assert_eq!(&after.xml.text,&second.xml.text);
    }
}
