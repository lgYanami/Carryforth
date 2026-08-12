//! Relay information document fields used by Meeting capability discovery.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct Nip11Document {
    #[serde(default)]
    pub(super) supported_extensions: Vec<String>,
    #[serde(default)]
    pub(super) buzz_supported_extensions_status: Option<SupportedExtensionsObservationStatus>,
    #[serde(rename = "self")]
    pub(super) relay_self: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SupportedExtensionsObservationStatus {
    Observed,
    TemporarilyUnavailable,
}
