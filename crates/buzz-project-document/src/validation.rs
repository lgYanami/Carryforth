//! Project Document v1 limits and canonical-value validation.

use serde::Deserialize;
use serde_json::Value;
use uuid::{Uuid, Variant};

use crate::{DocumentError, DocumentResult};

/// Maximum UTF-8 byte length of one command event's JSON content.
pub const MAX_COMMAND_CONTENT_BYTES: usize = 65_536;
/// Maximum nesting depth of one command JSON value.
pub const MAX_COMMAND_JSON_DEPTH: usize = 16;
/// Maximum UTF-8 byte length of a title.
pub const MAX_TITLE_BYTES: usize = 256;
/// Maximum UTF-8 byte length of a summary.
pub const MAX_SUMMARY_BYTES: usize = 4_096;
/// Maximum UTF-8 byte length of one Markdown snapshot.
pub const MAX_CONTENT_MARKDOWN_BYTES: usize = 49_152;
/// Largest revision or generation exactly representable by supported clients.
pub const MAX_SAFE_REVISION: u64 = 9_007_199_254_740_991;

pub(crate) fn deserialize_optional_non_null<'de, D, T>(
    deserializer: D,
) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

pub(crate) fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

pub(crate) fn validate_document_id(document_id: Uuid) -> DocumentResult<()> {
    if document_id.is_nil()
        || document_id.get_version_num() != 4
        || document_id.get_variant() != Variant::RFC4122
    {
        return Err(DocumentError::InvalidDocumentId { document_id });
    }
    Ok(())
}

pub(crate) fn validate_positive_revision(value: u64, field: &str) -> DocumentResult<()> {
    if !(1..=MAX_SAFE_REVISION).contains(&value) {
        return Err(DocumentError::InvalidRevisionTarget {
            reason: format!("{field} must be in 1..={MAX_SAFE_REVISION}"),
        });
    }
    Ok(())
}

pub(crate) fn validate_nonnegative_revision(value: u64, field: &str) -> DocumentResult<()> {
    if value > MAX_SAFE_REVISION {
        return Err(DocumentError::InvalidRevisionTarget {
            reason: format!("{field} must be in 0..={MAX_SAFE_REVISION}"),
        });
    }
    Ok(())
}

pub(crate) fn validate_snapshot(
    title: &str,
    summary: Option<&str>,
    content_markdown: &str,
) -> DocumentResult<()> {
    validate_no_nul("title", title)?;
    if title.is_empty() || title.trim() != title {
        return Err(DocumentError::InvalidSnapshot {
            reason: "title must be non-empty and have no leading or trailing whitespace".to_owned(),
        });
    }
    validate_len("title", title, MAX_TITLE_BYTES)?;

    if let Some(summary) = summary {
        validate_no_nul("summary", summary)?;
        if summary.is_empty() {
            return Err(DocumentError::InvalidSnapshot {
                reason: "an empty summary is non-canonical; omit the field".to_owned(),
            });
        }
        validate_len("summary", summary, MAX_SUMMARY_BYTES)?;
    }

    validate_no_nul("content_markdown", content_markdown)?;
    validate_len(
        "content_markdown",
        content_markdown,
        MAX_CONTENT_MARKDOWN_BYTES,
    )
}

fn validate_no_nul(field: &str, value: &str) -> DocumentResult<()> {
    if value.contains('\0') {
        return Err(DocumentError::InvalidSnapshot {
            reason: format!("{field} must not contain NUL"),
        });
    }
    Ok(())
}

fn validate_len(field: &str, value: &str, max: usize) -> DocumentResult<()> {
    if value.len() > max {
        return Err(DocumentError::InvalidSnapshot {
            reason: format!("{field} exceeds {max} UTF-8 bytes (got {})", value.len()),
        });
    }
    Ok(())
}
