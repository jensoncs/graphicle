//! Immutable repository objects: text for retrieval, raw bytes for compiler inputs.
//! Callers authorize the repository before loading. Hashes always cover raw bytes.
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{Connection, OptionalExtension, params, types::Value as SqlValue};
use serde_json::{Value, json};

#[derive(Clone)]
pub(crate) enum Content {
    Text(String),
    Binary(Vec<u8>),
}
impl Content {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Text(v) => v.as_bytes(),
            Self::Binary(v) => v,
        }
    }
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(v) => Some(v),
            Self::Binary(_) => None,
        }
    }
    pub fn sql(&self) -> SqlValue {
        match self {
            Self::Text(v) => SqlValue::Text(v.clone()),
            Self::Binary(v) => SqlValue::Blob(v.clone()),
        }
    }
    pub fn wire(&self) -> Value {
        match self {
            Self::Text(v) => json!({"content":v}),
            Self::Binary(v) => json!({"content_base64":STANDARD.encode(v)}),
        }
    }
    pub fn decode(value: &Value, limit: usize) -> Result<Self> {
        let bytes = match (value.get("content"), value.get("content_base64")) {
            (Some(Value::String(text)), None) if text.len() <= limit && !text.contains('\0') => {
                return Ok(Self::Text(text.clone()));
            }
            (None, Some(Value::String(encoded)))
                if encoded.len() <= limit.saturating_mul(4) / 3 + 4 =>
            {
                STANDARD
                    .decode(encoded)
                    .map_err(|_| Error::bad("Invalid source base64"))?
            }
            _ => {
                return Err(Error::bad(
                    "Supply exactly one bounded content or content_base64 value",
                ));
            }
        };
        if bytes.len() > limit {
            return Err(Error::bad("Source exceeds file size limit"));
        }
        // One canonical storage type per byte sequence, regardless of transport.
        if !bytes.contains(&0)
            && let Ok(text) = std::str::from_utf8(&bytes)
        {
            return Ok(Self::Text(text.to_owned()));
        }
        Ok(Self::Binary(bytes))
    }
}

pub(crate) fn load(db: &Connection, repository: &str, sha: &str) -> Result<Option<Content>> {
    let value: Option<SqlValue> = db
        .query_row(
            "SELECT content FROM repo_objects WHERE repository_id=? AND sha256=?",
            params![repository, sha],
            |r| r.get(0),
        )
        .optional()?;
    let content = match value {
        None => return Ok(None),
        Some(SqlValue::Text(v)) => Content::Text(v),
        Some(SqlValue::Blob(v)) => Content::Binary(v),
        _ => return Err(Error::conflict("Invalid stored source object")),
    };
    if graphicle_memory::text::sha256(content.bytes()) != sha {
        return Err(Error::conflict(
            "Source object does not match its content digest",
        ));
    }
    Ok(Some(content))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_encoding_cannot_bypass_size_or_text_validation() {
        for invalid in [
            json!({"content":"ok","content_base64":"AA=="}),
            json!({"content":"\0"}),
            json!({"content_base64":"%%%"}),
            json!({"content_base64":STANDARD.encode([0,1,2,3,4])}),
        ] {
            assert!(Content::decode(&invalid, 4).is_err());
        }
        assert!(matches!(
            Content::decode(&json!({"content_base64":STANDARD.encode("text")}), 4).unwrap(),
            Content::Text(_)
        ));
        assert!(matches!(
            Content::decode(&json!({"content_base64":"AP8="}), 4).unwrap(),
            Content::Binary(_)
        ));
    }
}
