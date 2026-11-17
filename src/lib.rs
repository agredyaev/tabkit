#![deny(rustdoc::broken_intra_doc_links)]

//! Headless Tableau 2026.2.4 workbook runtime and MCP server.
//!
//! # Compatibility contract
//!
//! TapKit is pinned to TWB 26.2 and Tableau REST 3.29. It intentionally does not
//! maintain a second Tableau XSD model or validator: the original TWB XML remains
//! authoritative locally, and Tableau `validateWorkbook` is the final XSD and
//! semantic validation boundary.
//!
//! # Mutation invariant
//!
//! Supported edits are expressed semantically, lowered to source-checked XML
//! patches, and reparsed before a candidate is returned. Callers never need to
//! mutate the XML index directly, and unrelated source regions are preserved.
//!
//! # Error contract
//!
//! Public fallible APIs return [`Error`]. Branch on [`Error::code`], not the
//! diagnostic message; messages are intended to explain the concrete failed
//! operation without exposing credentials or raw upstream payloads.

pub mod edit;
pub mod formula;
pub mod lineage;
pub mod mcp;
pub mod package;
pub mod patch;
pub mod rest;
mod text;
pub mod workbook;
pub mod xml;

/// Tableau Server release against which version-dependent behavior is pinned.
pub const TABLEAU_PRODUCT_TARGET: &str = "2026.2.4";
/// Only TWB `workbook/@version` value admitted by the semantic index.
pub const TWB_VERSION: &str = "26.2";
/// Official external XSD contract corresponding to [`TWB_VERSION`].
pub const TWB_XSD_VERSION: &str = "2026.2.0";
/// Only Tableau REST API version accepted by [`rest::Config::validate`].
pub const REST_API_VERSION: &str = "3.29";

#[cfg(test)]
mod perf;

use std::fmt;

/// Result type used by TapKit public APIs.
pub type Result<T> = std::result::Result<T, Error>;

/// TapKit failure with a stable classification and a concrete diagnostic.
///
/// The symbolic code is the API contract. The message may gain detail over time
/// and must not be parsed by clients.
#[derive(Debug)]
pub struct Error {
    /// Stable symbolic classification used by Rust and MCP clients.
    pub code: &'static str,
    /// User-facing diagnostic for the failed operation; never a secret carrier.
    pub message: String,
}

impl Error {
    /// Constructs an error without transforming either part of the public contract.
    ///
    /// Call sites choose a stable symbolic `code` and a concrete, secret-free
    /// diagnostic. Transport adapters may classify the code further, but must
    /// preserve it.
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
            Self::new("HTTP", "Could not connect to the configured Tableau server")
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
/// On failure the supplied code and message are preserved exactly. Use this for
/// expected admission/precondition failures rather than hidden runtime invariants.
pub fn require(ok: bool, code: &'static str, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::new(code, message))
    }
}
