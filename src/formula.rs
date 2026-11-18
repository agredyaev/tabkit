//! Bounded lexical analysis for Tableau formulas.
//!
//! The analyzer validates editor preconditions and discovers bracketed field
//! references. It does not parse or evaluate Tableau formula semantics; Tableau
//! remains the semantic validation authority.
//!
//! The editing boundary rejects external SQL/script/model-extension functions
//! instead of passing them through as opaque executable content.
use crate::{Error, Result, require};
use serde::Serialize;

/// Bracketed field reference discovered outside strings, comments, and date literals.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Reference {
    pub datasource: Option<String>,
    pub field: String,
}

/// Deduplicated references plus non-fatal runtime-state warnings.
#[derive(Debug)]
pub struct Analysis {
    pub references: Vec<Reference>,
    pub warnings: Vec<String>,
}

/// Performs the lexical checks required before TabKit authors a calculation.
///
/// Input is limited to 128 KiB. The scan rejects malformed lexical structure,
/// statement separators, NUL, and external execution functions. Successful
/// analysis does **not** mean Tableau will accept or evaluate the formula; it
/// establishes only the local editing preconditions and extracts deduplicated
/// field references.
pub fn analyze(source: &str) -> Result<Analysis> {
    require(
        !source.trim().is_empty() && source.len() <= 128 * 1024,
        "FORMULA",
        "Formula must contain 1..131072 UTF-8 bytes",
    )?;

    let bytes = source.as_bytes();
    let mut i = 0;
    let mut groups = Vec::new();
    let mut blocks = Vec::new();
    let mut references = Vec::new();
    let mut warnings = Vec::new();

    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let end = source[i + 2..]
                    .find("*/")
                    .ok_or_else(|| Error::new("FORMULA", "Unclosed comment"))?;
                i += end + 4;
            }
            b'\'' | b'"' => {
                let quote = bytes[i];
                i += 1;
                let mut closed = false;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i = i.saturating_add(2);
                        continue;
                    }
                    if bytes[i] == quote {
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                require(closed && i <= bytes.len(), "FORMULA", "Unclosed string")?;
            }
            b'#' => {
                let end = source[i + 1..]
                    .find('#')
                    .ok_or_else(|| Error::new("FORMULA", "Unclosed date literal"))?;
                i += end + 2;
            }
            b'[' => {
                let (first, end) = bracket(source, i)?;
                i = end;
                let mut p = i;
                while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                    p += 1;
                }
                if bytes.get(p) == Some(&b'.') {
                    p += 1;
                    while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                        p += 1;
                    }
                    require(
                        bytes.get(p) == Some(&b'['),
                        "FORMULA",
                        "Malformed qualified field reference",
                    )?;
                    let (field, end) = bracket(source, p)?;
                    i = end;
                    references.push(Reference {
                        datasource: Some(first),
                        field: bracketed(&field),
                    });
                } else {
                    references.push(Reference {
                        datasource: None,
                        field: bracketed(&first),
                    });
                }
            }
            b'(' | b'{' => {
                groups.push(bytes[i]);
                i += 1;
            }
            b')' | b'}' => {
                let expected = if bytes[i] == b')' { b'(' } else { b'{' };
                require(
                    groups.pop() == Some(expected),
                    "FORMULA",
                    "Unbalanced delimiters",
                )?;
                i += 1;
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let word = source[start..i].to_ascii_uppercase();
                require(
                    !word.starts_with("RAWSQL")
                        && !word.starts_with("SCRIPT")
                        && !word.starts_with("MODEL_EXTENSION"),
                    "UNSAFE_FORMULA",
                    "External SQL/scripts/model extensions are not accepted by this editor",
                )?;
                if matches!(word.as_str(), "IF" | "CASE") {
                    blocks.push(word.clone());
                }
                if word == "END" {
                    require(blocks.pop().is_some(), "FORMULA", "END without IF/CASE")?;
                }
                let mut p = i;
                while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                    p += 1;
                }
                if bytes.get(p) == Some(&b'(')
                    && matches!(word.as_str(), "NOW" | "TODAY" | "RANDOM")
                {
                    warnings.push(format!("{word} depends on runtime state"));
                }
            }
            b';' | 0 => {
                return Err(Error::new(
                    "FORMULA",
                    "Statement separators and NUL are forbidden",
                ));
            }
            _ => i += 1,
        }
    }

    require(
        groups.is_empty() && blocks.is_empty(),
        "FORMULA",
        "Unclosed delimiter or IF/CASE block",
    )?;
    references.sort();
    references.dedup();
    warnings.sort();
    warnings.dedup();

    Ok(Analysis {
        references,
        warnings,
    })
}

/// Parses exactly one Tableau `[datasource].[field]` reference.
///
/// Both components must be bracketed. Outside those components, the input accepts
/// surrounding whitespace only. The function normalizes the returned field to
/// Tableau bracket syntax, including escaped closing brackets.
pub fn qualified(source: &str) -> Result<(String, String)> {
    let source = source.trim();
    require(
        source.starts_with('['),
        "REFERENCE",
        "Expected a bracketed datasource qualifier",
    )?;
    let (datasource, end) = bracket(source, 0)?;
    let tail = source[end..].trim_start();
    let tail = tail
        .strip_prefix('.')
        .ok_or_else(|| Error::new("REFERENCE", "Expected datasource.field"))?
        .trim_start();
    require(
        tail.starts_with('['),
        "REFERENCE",
        "Expected a bracketed field",
    )?;
    let (field, end) = bracket(tail, 0)?;
    require(
        tail[end..].trim().is_empty(),
        "REFERENCE",
        "Reference contains trailing expressions",
    )?;
    Ok((datasource, bracketed(&field)))
}

fn bracket(source: &str, start: usize) -> Result<(String, usize)> {
    let bytes = source.as_bytes();
    let mut i = start + 1;
    let mut value = Vec::new();

    while i < bytes.len() {
        if bytes[i] == b']' {
            if bytes.get(i + 1) == Some(&b']') {
                value.push(b']');
                i += 2;
                continue;
            }
            let value = String::from_utf8(value)
                .map_err(|_| Error::new("FORMULA", "Invalid field encoding"))?;
            require(!value.is_empty(), "FORMULA", "Empty field reference")?;
            return Ok((value, i + 1));
        }
        value.push(bytes[i]);
        i += 1;
    }

    Err(Error::new("FORMULA", "Unclosed field reference"))
}

fn bracketed(value: &str) -> String {
    format!("[{}]", value.replace(']', "]]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_qualified_and_unqualified_references() -> Result<()> {
        let analysis = analyze("[Sales] / [Orders].[Profit]")?;
        assert_eq!(
            analysis.references,
            vec![
                Reference {
                    datasource: None,
                    field: "[Sales]".into(),
                },
                Reference {
                    datasource: Some("Orders".into()),
                    field: "[Profit]".into(),
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn ignores_non_code_references_and_deduplicates_runtime_warnings() -> Result<()> {
        let analysis = analyze(
            "'[StringField]' + \"[DoubleQuoted]\" + #2026-11-18# + [Sales] // [LineComment]\n\
             /* [BlockComment] */ + [Sales] + NOW() + NOW()",
        )?;
        assert_eq!(
            analysis.references,
            vec![Reference {
                datasource: None,
                field: "[Sales]".into(),
            }]
        );
        assert_eq!(analysis.warnings, vec!["NOW depends on runtime state"]);
        Ok(())
    }

    #[test]
    fn rejects_external_execution_and_malformed_formula_structure() {
        for source in [
            "RAWSQL_STR('select 1')",
            "SCRIPT_REAL('return 1', [Sales])",
            "MODEL_EXTENSION_BOOL('model', [Sales])",
        ] {
            let error = match analyze(source) {
                Ok(_) => panic!("external execution was accepted"),
                Err(error) => error,
            };
            assert_eq!(error.code, "UNSAFE_FORMULA", "{source}");
        }

        for source in [
            "",
            "'unterminated",
            "/* unterminated",
            "#2026-11-18",
            "[unterminated",
            "(1}",
            "IF [Sales] THEN 1",
            "END",
            "1; 2",
            "\0",
        ] {
            let error = match analyze(source) {
                Ok(_) => panic!("malformed formula was accepted"),
                Err(error) => error,
            };
            assert_eq!(error.code, "FORMULA", "{source:?}");
        }
    }

    #[test]
    fn qualified_reference_requires_exact_bracketed_shape() -> Result<()> {
        assert_eq!(
            qualified(" [Orders] . [Profit]] Ratio] ")?,
            ("Orders".into(), "[Profit]] Ratio]".into())
        );

        for source in ["[Orders]", "Orders.[Profit]", "[Orders].[Profit] + 1"] {
            let error = match qualified(source) {
                Ok(_) => panic!("invalid qualified reference was accepted"),
                Err(error) => error,
            };
            assert_eq!(error.code, "REFERENCE", "{source}");
        }
        Ok(())
    }
}
