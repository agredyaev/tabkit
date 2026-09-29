//! Fuzzes the exact production source files; no parser/editor copies.
#![allow(dead_code)]
#[cfg(feature="hyper")]
compile_error!("Fuzz harness does not execute the proprietary Hyper runtime");
#[path="../../src/error.rs"]
pub mod error;
#[path="../../src/config.rs"]
pub mod config;
#[path="../../src/fs.rs"]
pub mod fs;
#[path="../../src/wire.rs"]
pub mod wire;
#[path="../../src/workbook/xml.rs"]
pub mod xml;
#[path="../../src/workbook/scalar.rs"]
pub mod scalar;
#[path="../../src/workbook/formula.rs"]
pub mod formula;
#[path="../../src/workbook/patch.rs"]
pub mod patch;
#[path="../../src/workbook/workbook.rs"]
pub mod workbook;
#[path="../../src/workbook/validation.rs"]
pub mod validation;
#[path="../../src/workbook/edit.rs"]
pub mod edit;
#[path="../../src/workbook/package.rs"]
pub mod package;
#[path="../../src/integrations/sql.rs"]
pub mod sql;
#[path="../../src/integrations/oauth.rs"]
pub mod oauth;
#[path="../../src/integrations/rest.rs"]
pub mod rest;

pub fn callback(request: &str) -> error::Result<zeroize::Zeroizing<String>> {
    let redirect=reqwest::Url::parse("http://127.0.0.1:8765/callback").unwrap();
    oauth::callback_code(request,&redirect,"fuzz-state")
}
