#![no_main]
use libfuzzer_sys::fuzz_target;
use tabkit_fuzz::wire;
fuzz_target!(|data:&[u8]|{
    if data.len()>32768{return;}
    if wire::validate(data,32768).is_ok(){
        let v:serde_json::Value=serde_json::from_slice(data).expect("admitted JSON must decode");
        let encoded=wire::encode(&v,256*1024,false).unwrap();
        assert_eq!(wire::decode::<serde_json::Value>(&encoded,256*1024).unwrap(),v);
        if !data.is_empty(){assert!(wire::validate(data,(data.len()-1) as u64).is_err());}
    }
});
