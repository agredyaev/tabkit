#![deny(rustdoc::broken_intra_doc_links)]

//! Headless Tableau 2026.2.4 workbook runtime and MCP server.
//!
//! # Compatibility contract
//!
//! TabKit is pinned to TWB 26.2 and Tableau REST 3.29. It does not maintain a
//! second Tableau XSD model or validator: the original TWB XML remains
//! authoritative locally, and Tableau `validateWorkbook` is the final XSD and
//! semantic validation boundary.
//!
//! # Mutation invariant
//!
//! TabKit lowers supported semantic edits to source-checked XML patches, applies
//! them, and reparses the candidate before returning it. The public edit API
//! exposes no raw XML mutation. Patch verification preserves unrelated regions.
//!
//! # Error contract
//!
//! Public fallible APIs return [`Error`]. Branch on [`Error::code`], not the
//! diagnostic message; messages explain the concrete failed
//! operation without exposing credentials or raw upstream payloads.

pub mod edit;
pub mod extension_security;
pub mod formula;
pub mod lineage;
pub mod mcp;
pub mod package;
pub mod patch;
pub mod rest;
mod text;
pub mod workbook;
pub mod xml;

/// Tableau Server release for version-dependent behavior.
pub const TABLEAU_PRODUCT_TARGET: &str = "2026.2.4";
/// Only TWB `workbook/@version` value the semantic index accepts.
pub const TWB_VERSION: &str = "26.2";
/// Official external XSD contract corresponding to [`TWB_VERSION`].
pub const TWB_XSD_VERSION: &str = "2026.2.0";
/// Only Tableau REST API version [`rest::Config::validate`] accepts.
pub const REST_API_VERSION: &str = "3.29";

#[cfg(test)]
mod perf;

use std::fmt;

/// Result type for TabKit public APIs.
pub type Result<T> = std::result::Result<T, Error>;

/// TabKit failure with a contract code and diagnostic text.
///
/// The symbolic code is the API contract. Message text is diagnostic and is not
/// a compatibility contract.
#[derive(Debug)]
pub struct Error {
    /// Symbolic classification for Rust and MCP clients.
    pub code: &'static str,
    /// Diagnostic for the failed operation; must not contain secrets.
    pub message: String,
}

impl Error {
    /// Constructs an error without transforming either part of the public contract.
    ///
    /// Call sites choose the symbolic `code` and a secret-free diagnostic.
    /// Transport adapters classify the code further without changing it.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::new("IO", value.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::new("JSON", value.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(value: reqwest::Error) -> Self {
        if value.is_timeout() {
            Self::new(
                "HTTP_TIMEOUT",
                "Tableau HTTP request timed out before completion",
            )
        } else if value.is_connect() {
            Self::new("HTTP", "Connection to the configured Tableau server failed")
        } else {
            Self::new(
                "HTTP",
                "Tableau HTTP request failed before a valid response was received",
            )
        }
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(value: zip::result::ZipError) -> Self {
        Self::new("ZIP", value.to_string())
    }
}

/// Enforces a caller-visible precondition without changing its error contract.
///
/// On failure, `require` preserves the supplied code and message. Use it for
/// expected admission/precondition failures rather than runtime invariants.
pub fn require(ok: bool, code: &'static str, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::new(code, message))
    }
}
