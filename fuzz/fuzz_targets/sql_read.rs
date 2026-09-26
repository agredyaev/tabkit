#![no_main]
use libfuzzer_sys::fuzz_target;
use tabkit_fuzz::sql;
fuzz_target!(|data:&[u8]|{
    if data.len()>32768{return;}
    if let Ok(s)=std::str::from_utf8(data){
        if let Ok(q)=sql::admit(s,100){
            assert!(!q.tables.is_empty());assert!(q.sql.ends_with("LIMIT 101"));
            let q2=sql::admit(s,100).unwrap();assert_eq!(q.sql,q2.sql);assert_eq!(q.tables,q2.tables);
            assert!(q.tables.windows(2).all(|t|t[0]<t[1]));
            assert!(sql::admit(&format!("{s}\n; DELETE FROM \"Extract\".\"Extract\""),100).is_err());
        }
        let _=sql::identifier(s);
    }
});
