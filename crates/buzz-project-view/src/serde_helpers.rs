//! Shared serde helpers for canonical Project View wire shapes.

use serde::Deserialize;

/// Deserialize an optional field only when a non-null value is present.
///
/// Pair this helper with `#[serde(default)]`: an omitted field becomes `None`,
/// while an explicit JSON `null` is rejected because `T` must deserialize.
pub(crate) fn deserialize_optional_non_null<'de, D, T>(
    deserializer: D,
) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
