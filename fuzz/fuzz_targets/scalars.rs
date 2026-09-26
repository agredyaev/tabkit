#![no_main]
use libfuzzer_sys::fuzz_target;
use tabkit_fuzz::scalar::{Scalar,Domain};
fuzz_target!(|data:&[u8]|{
    if data.is_empty()||data.len()>4096{return;}
    if let Ok(text)=std::str::from_utf8(&data[1..]){
        let dtype=["integer","real","date","datetime","boolean","string"][data[0] as usize%6];
        if let Ok(v)=Scalar::parse(dtype,text){
            let literal=v.literal(dtype).unwrap();
            assert_eq!(Scalar::parse(dtype,&literal).unwrap(),v);
            assert!(Domain::List{values:vec![v.clone()]}.accepts(&v,dtype).is_ok());
            assert!(Domain::List{values:vec![v.clone(),v.clone()]}.accepts(&v,dtype).is_err());
        }
    }
});
