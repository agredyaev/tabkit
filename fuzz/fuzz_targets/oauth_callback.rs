#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data:&[u8]|{
    if data.len()>16384{return;}
    if let Ok(text)=std::str::from_utf8(data){
        if let Ok(code)=tabkit_fuzz::callback(text){assert!(!code.is_empty());}
        // Generated valid request plus adversarial/duplicate query parameters.
        if !text.chars().any(|c|c.is_control()||c.is_whitespace()){
            let req=format!("GET /callback?state=fuzz-state&code=ok&{text} HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n");
            let _=tabkit_fuzz::callback(&req);
            let bad=format!("GET /callback?state=wrong&code=ok&{text} HTTP/1.1\r\n\r\n");
            assert!(tabkit_fuzz::callback(&bad).is_err());
        }
    }
});
