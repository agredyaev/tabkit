//! Lexical/reference checks, NOT a Tableau expression evaluator or full compiler.
use crate::error::{
    Error,
    Result,
    require
};
use serde::Serialize;
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Reference {
    pub datasource: Option<String>,
    pub field: String
}
#[derive(Debug)]
pub struct Analysis {
    pub references: Vec<Reference>,
    pub warnings: Vec<String>
}
pub fn analyze(s: &str) -> Result<Analysis> {
    require(!s.trim().is_empty() && s.len() <= 128 * 1024, "FORMULA", "Formula must contain 1..131072 UTF-8 bytes")?;
    let b = s.as_bytes();
    let mut i = 0;
    let mut groups = Vec::new();
    let mut blocks = Vec::new();
    let mut refs = Vec::new();
    let mut warnings = Vec::new();
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'\n' | b'\r' => {
                i += 1;
            }
            b'/' if b.get(i+1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i+1) == Some(&b'*') => {
                let end = s[i+2..].find("*/").ok_or_else(|| Error::new("FORMULA", "Unclosed comment"))?;
                i += 2 + end + 2;
            }
            b'\'' | b'"' => {
                let q = b[i];
                i += 1;
                let mut closed = false;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == q {
                        if b.get(i+1) == Some(&q) {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                require(closed && i <= b.len(), "FORMULA", "Unclosed string")?;
            }
            b'#' => {
                let end = s[i+1..].find('#').ok_or_else(|| Error::new("FORMULA", "Unclosed date literal"))?;
                i += end + 2;
            }
            b'[' => {
                let (first, end) = bracket(s, i)?;
                i = end;
                let mut p = i;
                while p < b.len() && b[p].is_ascii_whitespace() {
                    p += 1;
                }
                if b.get(p) == Some(&b'.') {
                    p += 1;
                    while p < b.len() && b[p].is_ascii_whitespace() {
                        p += 1;
                    }
                    require(b.get(p) == Some(&b'['), "FORMULA", "Malformed qualified field reference")?;
                    let (field, end) = bracket(s, p)?;
                    i = end;
                    refs.push(Reference {
                        datasource: Some(first),
                        field: format!("[{}]", field.replace(']', "]]"))
                    });
                } else {
                    refs.push(Reference {
                        datasource: None,
                        field: format!("[{}]", first.replace(']', "]]"))
                    });
                }
            }
            b'(' | b'{' => {
                groups.push(b[i]);
                i += 1;
            }
            b')' | b'}' => {
                let expected = if b[i] == b')' {
                    b'('
                } else {
                    b'{'
                };
                require(groups.pop() == Some(expected), "FORMULA", "Unbalanced delimiters")?;
                i += 1;
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                i += 1;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                let word = s[start..i].to_ascii_uppercase();
                // Reject the identifier itself, even when comments separate it from '('.
                require(!word.starts_with("RAWSQL") && !word.starts_with("SCRIPT") && !word.starts_with("MODEL_EXTENSION"), "UNSAFE_FORMULA", "External SQL/scripts/model extensions are not accepted by this editor")?;
                if word == "IF" || word == "CASE" {
                    blocks.push(word.clone());
                }
                if word == "END" {
                    require(blocks.pop().is_some(), "FORMULA", "END without IF/CASE")?;
                }
                let mut p = i;
                while p < b.len() && b[p].is_ascii_whitespace() {
                    p += 1;
                }
                if b.get(p) == Some(&b'(') {
                    if matches!(word.as_str(), "NOW" | "TODAY" | "RANDOM") {
                        warnings.push(format!("{word} depends on runtime state"));
                    }
                }
            }
            b';' | 0 => return Err(Error::new("FORMULA", "Statement separators and NUL are forbidden")),
            _ => {
                i += 1;
            }
        }
    }
    require(groups.is_empty() && blocks.is_empty(), "FORMULA", "Unclosed delimiter or IF/CASE block")?;
    refs.sort();
    refs.dedup();
    warnings.sort();
    warnings.dedup();
    Ok(Analysis {
        references: refs,
        warnings
    })
}
fn bracket(s: &str, start: usize) -> Result<(String, usize)> {
    let b = s.as_bytes();
    let mut i = start+1;
    let mut value = Vec::new();
    while i < b.len() {
        if b[i] == b']' {
            if b.get(i+1) == Some(&b']') {
                value.push(b']');
                i += 2;
                continue;
            }
            let value = String::from_utf8(value).map_err(|_| Error::new("FORMULA", "Invalid field encoding"))?;
            require(!value.is_empty(), "FORMULA", "Empty field reference")?;
            return Ok((value, i+1));
        }
        value.push(b[i]);
        i += 1;
    }
    Err(Error::new("FORMULA", "Unclosed field reference"))
}
/// A qualified reference used by worksheet column instances.
pub fn qualified(s: &str) -> Result<(String, String)> {
    let s=s.trim();
    require(s.starts_with('['),"REFERENCE","Expected a bracketed datasource qualifier")?;
    let(ds,end)=bracket(s,0)?;
    let tail=s[end..].trim_start();
    let tail=tail.strip_prefix('.').ok_or_else(||Error::new("REFERENCE","Expected datasource.field"))?.trim_start();
    require(tail.starts_with('['),"REFERENCE","Expected a bracketed field")?;
    let(field,end)=bracket(tail,0)?;
    require(tail[end..].trim().is_empty(),"REFERENCE","Reference contains trailing expressions")?;
    Ok((ds,format!("[{}]",field.replace(']',"]]"))))
}
