//! Patch-field semantics for typed Project View updates.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A field in a typed update patch.
///
/// A missing JSON field is represented by [`Patch::Unchanged`], an explicit
/// JSON `null` by [`Patch::Clear`], and any non-null value by
/// [`Patch::Set`]. Callers should place `#[serde(default)]` and
/// `#[serde(skip_serializing_if = "Patch::is_unchanged")]` on fields using
/// this type.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Patch<T> {
    /// Leave the current value unchanged.
    #[default]
    Unchanged,
    /// Explicitly clear a nullable value.
    Clear,
    /// Replace the current value.
    Set(T),
}

impl<T> Patch<T> {
    /// Returns `true` when this patch does not mention the field.
    pub const fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }

    /// Returns `true` when this patch explicitly clears the field.
    pub const fn is_clear(&self) -> bool {
        matches!(self, Self::Clear)
    }

    /// Maps a set value while retaining unchanged and clear semantics.
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> Patch<U> {
        match self {
            Self::Unchanged => Patch::Unchanged,
            Self::Clear => Patch::Clear,
            Self::Set(value) => Patch::Set(map(value)),
        }
    }
}

impl<T> Serialize for Patch<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Unchanged | Self::Clear => serializer.serialize_none(),
            Self::Set(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T> Deserialize<'de> for Patch<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(|value| match value {
            Some(value) => Self::Set(value),
            None => Self::Clear,
        })
    }
}
