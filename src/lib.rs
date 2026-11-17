#![deny(rustdoc::broken_intra_doc_links)]

//! Headless Tableau 2026.2.4 workbook runtime and MCP server.
//!
//! TapKit parses TWB/TWBX packages into a small typed semantic index, builds
//! lineage, applies supported semantic edits as source-preserving XML patches,
//! and delegates final workbook validation to Tableau REST 3.29.
//!
//! The crate does not implement a second Tableau XSD model or validator. The
//! supported compatibility boundary is defined by the version constants below.

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

/// Tableau product version targeted by this build.
pub const TABLEAU_PRODUCT_TARGET: &str = "2026.2.4";
/// Accepted TWB workbook version.
pub const TWB_VERSION: &str = "26.2";
/// Official Tableau TWB XSD version used as the external structural contract.
pub const TWB_XSD_VERSION: &str = "2026.2.0";
/// Tableau REST API version used by the client.
pub const REST_API_VERSION: &str = "3.29";

#[cfg(test)]
mod perf;

use std::fmt;

/// Result type used by TapKit public APIs.
pub type Result<T> = std::result::Result<T, Error>;

/// Stable error code plus a human-readable message.
#[derive(Debug)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}

impl Error {
    /// Creates an error with a stable machine-readable code.
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

/// Returns `Ok(())` when `ok` is true, otherwise a TapKit error.
pub fn require(ok: bool, code: &'static str, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::new(code, message))
    }
}
