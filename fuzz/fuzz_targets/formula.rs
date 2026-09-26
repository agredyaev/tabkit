#![no_main]
use libfuzzer_sys::fuzz_target;
use tabkit_fuzz::formula;
fuzz_target!(|data:&[u8]|{
    if data.len()>32768{return;}
    if let Ok(text)=std::str::from_utf8(data){
        let _=formula::qualified(text);
        if let Ok(a)=formula::analyze(text){
            assert!(a.references.windows(2).all(|p|p[0]<p[1]));
            assert!(a.warnings.windows(2).all(|p|p[0]<p[1]));
            for r in a.references {
                assert!(r.field.starts_with('[')&&r.field.ends_with(']'));
                if let Some(ds)=r.datasource {
                    let text=format!("[{}].{}",ds.replace(']',"]]"),r.field);
                    let got=formula::qualified(&text).unwrap();assert_eq!(got,(ds,r.field));
                }
            }
        }
    }
});
