//! Project Context Edge v1 limits and canonical-value validation.

use serde::Deserialize;
use serde_json::Value;
use uuid::{Uuid, Variant};

use crate::{ProjectContextError, ProjectContextResult};

/// Minimum number of distinct coordinates that constitute an edge.
pub const MIN_EDGE_COORDINATES: usize = 2;
/// Maximum UTF-8 byte length of one command event's JSON content.
pub const MAX_COMMAND_CONTENT_BYTES: usize = 65_536;
/// Maximum UTF-8 byte length of one relay projection JSON content value.
pub const MAX_PROJECTION_CONTENT_BYTES: usize = 65_536;
/// Maximum nesting depth of one command JSON value.
pub const MAX_COMMAND_JSON_DEPTH: usize = 16;
/// Largest revision, count, or generation exactly representable by supported clients.
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

pub(crate) fn validate_uuid_v4(id: Uuid, field: &str) -> ProjectContextResult<()> {
    if id.is_nil() || id.get_version_num() != 4 || id.get_variant() != Variant::RFC4122 {
        return Err(ProjectContextError::InvalidCoordinate {
            reason: format!("{field} must be an RFC 4122 UUID v4"),
        });
    }
    Ok(())
}

pub(crate) fn validate_document_id(document_id: Uuid) -> ProjectContextResult<()> {
    if document_id.is_nil()
        || document_id.get_version_num() != 4
        || document_id.get_variant() != Variant::RFC4122
    {
        return Err(ProjectContextError::InvalidDocumentId { document_id });
    }
    Ok(())
}

pub(crate) fn validate_nonnegative(value: u64, field: &str) -> ProjectContextResult<()> {
    if value > MAX_SAFE_REVISION {
        return Err(ProjectContextError::InvalidRevision {
            reason: format!("{field} must be in 0..={MAX_SAFE_REVISION}"),
        });
    }
    Ok(())
}

pub(crate) fn validate_positive(value: u64, field: &str) -> ProjectContextResult<()> {
    if !(1..=MAX_SAFE_REVISION).contains(&value) {
        return Err(ProjectContextError::InvalidRevision {
            reason: format!("{field} must be in 1..={MAX_SAFE_REVISION}"),
        });
    }
    Ok(())
}

pub(crate) fn validate_projection_size<T: serde::Serialize>(value: &T) -> ProjectContextResult<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| ProjectContextError::InvalidProjection {
            reason: error.to_string(),
        })?;
    if bytes.len() > MAX_PROJECTION_CONTENT_BYTES {
        return Err(ProjectContextError::ProjectionTooLarge {
            max: MAX_PROJECTION_CONTENT_BYTES,
            actual: bytes.len(),
        });
    }
    Ok(())
}
