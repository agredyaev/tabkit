use crate::error::{
    Error,
    Result,
    require
};
use chrono::{
    NaiveDate,
    NaiveDateTime
};
use rust_decimal::Decimal;
use schemars::JsonSchema;
use serde::{
    Deserialize,
    Serialize
};
use std::{
    cmp::Ordering,
    str::FromStr
};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case", deny_unknown_fields)]
pub enum Scalar {
    String(String),
    Integer(i64),
    Real(String),
    Boolean(bool),
    Date(String),
    DateTime(String)
}
impl Scalar {
    pub fn datatype(&self) -> &'static str {
        match self {
            Self::String(_) => "string",
            Self::Integer(_) => "integer",
            Self::Real(_) => "real",
            Self::Boolean(_) => "boolean",
            Self::Date(_) => "date",
            Self::DateTime(_) => "datetime"
        }
    }
    pub fn literal(&self, datatype: &str) -> Result<String> {
        require(self.datatype() == datatype, "TYPE_MISMATCH", format!("Expected {datatype}, received {}", self.datatype()))?;
        match self {
            Self::String(s) => {
                require(!s.contains(['"','\\','\r','\n','\0']), "UNSUPPORTED_LITERAL", "v1 string literals cannot contain double quotes, backslashes, newlines or NUL")?;
                Ok(format!("\"{s}\""))
            }
            Self::Integer(v) => Ok(v.to_string()),
            Self::Real(v) => {
                let n = Decimal::from_str(v).map_err(|_| Error::new("LITERAL", "Invalid fixed-point decimal (max 28 fractional digits)"))?;
                let canonical = n.normalize().to_string();
                require(*v == canonical, "NONCANONICAL_LITERAL", "Real values must be canonical decimal strings, e.g. 1.25 or 1 (not 1.0)")?;
                Ok(canonical)
            }
            Self::Boolean(v) => Ok(if *v {
                "true"
            } else {
                "false"
            }.into()),
            Self::Date(v) => {
                date(v)?;
                Ok(format!("#{v}#"))
            }
            Self::DateTime(v) => {
                datetime(v)?;
                Ok(format!("#{v}#"))
            }
        }
    }
    pub fn parse(datatype: &str, text: &str) -> Result<Self> {
        let t = text.trim();
        let value = match datatype {
            "string" => {
                let s = t.strip_prefix('"').and_then(|v| v.strip_suffix('"')).ok_or_else(|| Error::new("UNSUPPORTED_LITERAL", "Expected a double-quoted Tableau string literal"))?;
                Self::String(s.into())
            }
            "integer" => Self::Integer(t.parse().map_err(|_| Error::new("LITERAL", "Invalid integer"))?),
            "real" => Self::Real(Decimal::from_str(t).map_err(|_| Error::new("LITERAL", "Unsupported decimal"))?.normalize().to_string()),
            "boolean" => Self::Boolean(match t.to_ascii_lowercase().as_str() {
                "true" => true,
                "false" => false,
                _ => return Err(Error::new("LITERAL", "Invalid boolean"))
            }),
            "date" => Self::Date(t.trim_matches('#').into()),
            "datetime" => Self::DateTime(t.trim_matches('#').into()),
            _ => return Err(Error::new("UNSUPPORTED_DATATYPE", datatype)),
        };
        value.literal(datatype)?;
        Ok(value)
    }
    pub fn cmp_value(&self, other: &Self) -> Result<Ordering> {
        require(self.datatype() == other.datatype(), "TYPE_MISMATCH", "Incompatible comparison")?;
        Ok(match (self, other) {
            (Self::Integer(a), Self::Integer(b)) => a.cmp(b),
            (Self::Real(a), Self::Real(b)) => decimal(a)?.cmp(&decimal(b)?),
            (Self::Date(a), Self::Date(b)) => date(a)?.cmp(&date(b)?),
            (Self::DateTime(a), Self::DateTime(b)) => datetime(a)?.cmp(&datetime(b)?),
            (Self::String(a), Self::String(b)) => a.cmp(b),
            (Self::Boolean(a), Self::Boolean(b)) => a.cmp(b),
            _ => return Err(Error::new("TYPE_MISMATCH", "Incompatible scalar")),
        })
    }
}
fn decimal(s: &str) -> Result<Decimal> {
    Decimal::from_str(s).map_err(|_| Error::new("LITERAL", "Invalid decimal"))
}
fn date(s: &str) -> Result<NaiveDate> {
    require(s.len()==10,"LITERAL","Expected canonical YYYY-MM-DD")?;
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| Error::new("LITERAL", "Expected a real YYYY-MM-DD date"))
}
fn datetime(s: &str) -> Result<NaiveDateTime> {
    require(s.len()==19,"LITERAL","Expected canonical YYYY-MM-DD HH:MM:SS")?;
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").map_err(|_| Error::new("LITERAL", "Expected YYYY-MM-DD HH:MM:SS"))
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Domain {
    Any,
    List {
        values: Vec<Scalar>
    },
    Range {
        min: Scalar,
        max: Scalar,
        step: Option<Scalar>
    },
}
impl Domain {
    pub fn accepts(&self, value: &Scalar, datatype: &str) -> Result<()> {
        value.literal(datatype)?;
        match self {
            Self::Any => Ok(()),
            Self::List {
                values
            }
            => {
                require(!values.is_empty() && values.len() <= 10_000, "DOMAIN", "List domain must have 1..10000 values")?;
                let mut seen = std::collections::BTreeSet::new();
                for v in values {
                    require(seen.insert(v.literal(datatype)?), "DOMAIN", "Duplicate domain value")?;
                }
                require(values.iter().any(|v| v.cmp_value(value).ok() == Some(Ordering::Equal)), "DOMAIN", "Current parameter value is outside its domain")
            }
            Self::Range {
                min,
                max,
                step
            }
            => {
                min.literal(datatype)?;
                max.literal(datatype)?;
                require(!matches!(value, Scalar::String(_) | Scalar::Boolean(_)), "DOMAIN", "Range requires numeric or temporal values")?;
                require(min.cmp_value(max)? != Ordering::Greater && value.cmp_value(min)? != Ordering::Less && value.cmp_value(max)? != Ordering::Greater, "DOMAIN", "Value or range bounds are invalid")?;
                if let Some(s) = step {
                    s.literal(datatype)?;
                    match (value, min, s) {
                        (Scalar::Integer(v), Scalar::Integer(m), Scalar::Integer(s)) => {
                            require(*s > 0 && ((*v as i128 - *m as i128) % *s as i128 == 0), "DOMAIN", "Integer value violates range granularity")?;
                        }
                        (Scalar::Real(v), Scalar::Real(m), Scalar::Real(s)) => {
                            let s = decimal(s)?;
                            require(s > Decimal::ZERO, "DOMAIN", "Step must be positive")?;
                            let diff = decimal(v)?.checked_sub(decimal(m)?).ok_or_else(|| Error::new("DOMAIN", "Decimal range overflow"))?;
                            require(diff.checked_rem(s) == Some(Decimal::ZERO), "DOMAIN", "Value violates decimal granularity")?;
                        }
                        _ => return Err(Error::new("UNSUPPORTED_SHAPE", "Temporal granularity changes require a verified version profile")),
                    }
                }
                Ok(())
            }
        }
    }
}
