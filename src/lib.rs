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

pub const TABLEAU_PRODUCT_TARGET: &str = "2026.2.4";
pub const TWB_VERSION: &str = "26.2";
pub const TWB_XSD_VERSION: &str = "2026.2.0";
pub const REST_API_VERSION: &str = "3.29";

#[cfg(test)]
mod perf;

use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}

impl Error {
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
            Self::new("HTTP_TIMEOUT", "Tableau request timed out")
        } else {
            Self::new("HTTP", "Tableau request failed")
        }
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(value: zip::result::ZipError) -> Self {
        Self::new("ZIP", value.to_string())
    }
}

pub fn require(ok: bool, code: &'static str, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::new(code, message))
    }
}
