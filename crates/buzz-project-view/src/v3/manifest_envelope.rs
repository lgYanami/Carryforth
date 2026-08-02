//! Strict Human-readable JSON envelope for the binary v1 Resource manifest.
//!
//! The signed/hash contract remains the postcard encoding in `migration`.
//! This separate type keeps fixed bytes as canonical lower-hex and UUIDs as
//! canonical strings without changing those frozen binary serde semantics.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    CanonicalProjectResourceV3, CanonicalResourceCutoverV1, MigrationContractError,
    ResourceMappingManifestV1, ReviewSignature, ReviewedResourceMappingV1, MAX_MANIFEST_JSON_BYTES,
};

/// JSON form of a final locator-free v3 Resource body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalProjectResourceEnvelopeV1 {
    /// Exact accepted Resource name.
    pub name: String,
    /// Exact open kind token.
    pub resource_kind: String,
    /// Exact optional summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Canonical Guide Document UUID string.
    pub guide_document_id: Uuid,
}

/// JSON form of the signed final Resource payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalResourceCutoverEnvelopeV1 {
    /// Final locator-free Resource data.
    pub resource_data: CanonicalProjectResourceEnvelopeV1,
    /// Must remain empty for manifest schema v1.
    pub context_references: Vec<serde_json::Value>,
}

/// JSON form of one fully reviewed Resource mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedResourceMappingEnvelopeV1 {
    /// Canonical Resource UUID string.
    pub resource_id: Uuid,
    /// Exact legacy object revision.
    pub legacy_object_revision: u64,
    /// Lower-hex exact legacy projection event ID.
    pub legacy_projection_event_id: String,
    /// Lower-hex canonical legacy body digest.
    pub legacy_body_digest: String,
    /// Complete reviewed final v3 payload.
    pub reviewed_v3_payload: CanonicalResourceCutoverEnvelopeV1,
    /// Lower-hex final payload digest.
    pub v3_payload_digest: String,
    /// Exact Guide Document revision.
    pub guide_document_revision: u64,
    /// Lower-hex Guide head event ID.
    pub guide_head_event_id: String,
    /// Lower-hex immutable Guide revision event ID.
    pub guide_revision_event_id: String,
    /// Lower-hex canonical Guide snapshot digest.
    pub guide_content_digest: String,
    /// Lower-hex mapping-entry digest.
    pub mapping_entry_digest: String,
    /// Lower-hex Human reviewer x-only public key.
    pub reviewed_by_pubkey: String,
    /// Signed review timestamp in Unix microseconds.
    pub reviewed_at_unix_micros: i64,
    /// Lower-hex reviewer attestation digest.
    pub review_digest: String,
    /// Lower-hex 64-byte BIP-340 detached signature.
    pub review_signature: String,
}

/// Closed, bounded JSON envelope for a complete reviewed v1 manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceMappingManifestEnvelopeV1 {
    /// Must equal one.
    pub schema_version: u16,
    /// Canonical Community UUID string.
    pub community_id: Uuid,
    /// Lower-hex exact v2 metadata event ID.
    pub base_meta_event_id: String,
    /// Exact v2 base Project revision.
    pub base_project_revision: u64,
    /// Exact v2 base projection generation.
    pub base_projection_generation: u64,
    /// Reviewed entries in canonical Resource UUID byte order.
    pub entries: Vec<ReviewedResourceMappingEnvelopeV1>,
}

impl ResourceMappingManifestEnvelopeV1 {
    /// Parse a bounded closed JSON document and reconstruct the frozen binary
    /// value. Every byte field must already be exact lowercase hex.
    pub fn parse_json(bytes: &[u8]) -> Result<ResourceMappingManifestV1, MigrationContractError> {
        if bytes.len() > MAX_MANIFEST_JSON_BYTES {
            return invalid(format!(
                "manifest JSON exceeds {MAX_MANIFEST_JSON_BYTES} bytes"
            ));
        }
        let envelope: Self = serde_json::from_slice(bytes)
            .map_err(|error| MigrationContractError::InvalidManifest(error.to_string()))?;
        envelope.into_canonical()
    }

    /// Convert the Human envelope into the frozen postcard value.
    pub fn into_canonical(self) -> Result<ResourceMappingManifestV1, MigrationContractError> {
        let manifest = ResourceMappingManifestV1 {
            schema_version: self.schema_version,
            community_id: *self.community_id.as_bytes(),
            base_meta_event_id: decode_hex::<32>(&self.base_meta_event_id, "base_meta_event_id")?,
            base_project_revision: self.base_project_revision,
            base_projection_generation: self.base_projection_generation,
            entries: self
                .entries
                .into_iter()
                .map(ReviewedResourceMappingEnvelopeV1::into_canonical)
                .collect::<Result<Vec<_>, _>>()?,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Build the stable Human envelope from one already canonical manifest.
    pub fn from_canonical(
        manifest: &ResourceMappingManifestV1,
    ) -> Result<Self, MigrationContractError> {
        manifest.validate()?;
        Ok(Self {
            schema_version: manifest.schema_version,
            community_id: Uuid::from_bytes(manifest.community_id),
            base_meta_event_id: hex::encode(manifest.base_meta_event_id),
            base_project_revision: manifest.base_project_revision,
            base_projection_generation: manifest.base_projection_generation,
            entries: manifest
                .entries
                .iter()
                .map(ReviewedResourceMappingEnvelopeV1::from_canonical)
                .collect(),
        })
    }

    /// Serialize a deterministic pretty JSON representation for review files.
    pub fn to_pretty_json(
        manifest: &ResourceMappingManifestV1,
    ) -> Result<Vec<u8>, MigrationContractError> {
        let envelope = Self::from_canonical(manifest)?;
        serde_json::to_vec_pretty(&envelope)
            .map_err(|error| MigrationContractError::Serialization(error.to_string()))
    }
}

impl ReviewedResourceMappingEnvelopeV1 {
    fn into_canonical(self) -> Result<ReviewedResourceMappingV1, MigrationContractError> {
        if !self.reviewed_v3_payload.context_references.is_empty() {
            return invalid("v1 cutover Context Reference set must be empty");
        }
        Ok(ReviewedResourceMappingV1 {
            resource_id: *self.resource_id.as_bytes(),
            legacy_object_revision: self.legacy_object_revision,
            legacy_projection_event_id: decode_hex::<32>(
                &self.legacy_projection_event_id,
                "legacy_projection_event_id",
            )?,
            legacy_body_digest: decode_hex::<32>(&self.legacy_body_digest, "legacy_body_digest")?,
            reviewed_v3_payload: CanonicalResourceCutoverV1 {
                resource_data: CanonicalProjectResourceV3 {
                    name: self.reviewed_v3_payload.resource_data.name,
                    resource_kind: self.reviewed_v3_payload.resource_data.resource_kind,
                    summary: self.reviewed_v3_payload.resource_data.summary,
                    guide_document_id: *self
                        .reviewed_v3_payload
                        .resource_data
                        .guide_document_id
                        .as_bytes(),
                },
                context_references: Vec::new(),
            },
            v3_payload_digest: decode_hex::<32>(&self.v3_payload_digest, "v3_payload_digest")?,
            guide_document_revision: self.guide_document_revision,
            guide_head_event_id: decode_hex::<32>(
                &self.guide_head_event_id,
                "guide_head_event_id",
            )?,
            guide_revision_event_id: decode_hex::<32>(
                &self.guide_revision_event_id,
                "guide_revision_event_id",
            )?,
            guide_content_digest: decode_hex::<32>(
                &self.guide_content_digest,
                "guide_content_digest",
            )?,
            mapping_entry_digest: decode_hex::<32>(
                &self.mapping_entry_digest,
                "mapping_entry_digest",
            )?,
            reviewed_by_pubkey: decode_hex::<32>(&self.reviewed_by_pubkey, "reviewed_by_pubkey")?,
            reviewed_at_unix_micros: self.reviewed_at_unix_micros,
            review_digest: decode_hex::<32>(&self.review_digest, "review_digest")?,
            review_signature: ReviewSignature::from_bytes(decode_hex::<64>(
                &self.review_signature,
                "review_signature",
            )?),
        })
    }

    fn from_canonical(entry: &ReviewedResourceMappingV1) -> Self {
        Self {
            resource_id: Uuid::from_bytes(entry.resource_id),
            legacy_object_revision: entry.legacy_object_revision,
            legacy_projection_event_id: hex::encode(entry.legacy_projection_event_id),
            legacy_body_digest: hex::encode(entry.legacy_body_digest),
            reviewed_v3_payload: CanonicalResourceCutoverEnvelopeV1 {
                resource_data: CanonicalProjectResourceEnvelopeV1 {
                    name: entry.reviewed_v3_payload.resource_data.name.clone(),
                    resource_kind: entry
                        .reviewed_v3_payload
                        .resource_data
                        .resource_kind
                        .clone(),
                    summary: entry.reviewed_v3_payload.resource_data.summary.clone(),
                    guide_document_id: Uuid::from_bytes(
                        entry.reviewed_v3_payload.resource_data.guide_document_id,
                    ),
                },
                context_references: Vec::new(),
            },
            v3_payload_digest: hex::encode(entry.v3_payload_digest),
            guide_document_revision: entry.guide_document_revision,
            guide_head_event_id: hex::encode(entry.guide_head_event_id),
            guide_revision_event_id: hex::encode(entry.guide_revision_event_id),
            guide_content_digest: hex::encode(entry.guide_content_digest),
            mapping_entry_digest: hex::encode(entry.mapping_entry_digest),
            reviewed_by_pubkey: hex::encode(entry.reviewed_by_pubkey),
            reviewed_at_unix_micros: entry.reviewed_at_unix_micros,
            review_digest: hex::encode(entry.review_digest),
            review_signature: hex::encode(entry.review_signature.as_bytes()),
        }
    }
}

fn decode_hex<const N: usize>(value: &str, field: &str) -> Result<[u8; N], MigrationContractError> {
    if value.len() != N * 2
        || value
            .bytes()
            .any(|byte| !matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return invalid(format!(
            "{field} must be exactly {} lowercase hexadecimal characters",
            N * 2
        ));
    }
    let decoded = hex::decode(value)
        .map_err(|error| MigrationContractError::InvalidManifest(error.to_string()))?;
    decoded.try_into().map_err(|decoded: Vec<u8>| {
        MigrationContractError::InvalidManifest(format!(
            "{field} decoded to {} bytes instead of {N}",
            decoded.len()
        ))
    })
}

fn invalid<T>(reason: impl Into<String>) -> Result<T, MigrationContractError> {
    Err(MigrationContractError::InvalidManifest(reason.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v3::{
        manifest_digest, mapping_entry_digest, resource_cutover_payload_digest, review_digest,
        CanonicalResourceMappingEntryV1, CanonicalResourceReviewV1,
    };

    fn manifest() -> ResourceMappingManifestV1 {
        let community_id = *Uuid::parse_str("018f6f4f-1e10-7c0b-9b37-2e4094c9a111")
            .expect("community UUID")
            .as_bytes();
        let resource_id = *Uuid::parse_str("0f85e5f0-c7d5-4c30-a0f2-c18478d21001")
            .expect("resource UUID")
            .as_bytes();
        let guide_document_id = *Uuid::parse_str("a91cc436-f558-4b36-9d33-e970419c2211")
            .expect("Guide UUID")
            .as_bytes();
        let payload = CanonicalResourceCutoverV1 {
            resource_data: CanonicalProjectResourceV3 {
                name: "Repository".to_owned(),
                resource_kind: "repository".to_owned(),
                summary: None,
                guide_document_id,
            },
            context_references: Vec::new(),
        };
        let payload_digest = resource_cutover_payload_digest(&payload).expect("payload digest");
        let mapping = CanonicalResourceMappingEntryV1 {
            community_id,
            base_meta_event_id: [1; 32],
            base_project_revision: 7,
            base_projection_generation: 2,
            resource_id,
            legacy_object_revision: 3,
            legacy_projection_event_id: [2; 32],
            legacy_body_digest: [3; 32],
            v3_payload_digest: payload_digest,
            guide_document_id,
            guide_document_revision: 4,
            guide_head_event_id: [4; 32],
            guide_revision_event_id: [5; 32],
            guide_content_digest: [6; 32],
        };
        let mapping_digest = mapping_entry_digest(&mapping).expect("mapping digest");
        let review = CanonicalResourceReviewV1 {
            mapping_entry_digest: mapping_digest,
            reviewed_by_pubkey: [7; 32],
            reviewed_at_unix_micros: 1_725_000_000_000_000,
        };
        ResourceMappingManifestV1 {
            schema_version: 1,
            community_id,
            base_meta_event_id: [1; 32],
            base_project_revision: 7,
            base_projection_generation: 2,
            entries: vec![ReviewedResourceMappingV1 {
                resource_id,
                legacy_object_revision: 3,
                legacy_projection_event_id: [2; 32],
                legacy_body_digest: [3; 32],
                reviewed_v3_payload: payload,
                v3_payload_digest: payload_digest,
                guide_document_revision: 4,
                guide_head_event_id: [4; 32],
                guide_revision_event_id: [5; 32],
                guide_content_digest: [6; 32],
                mapping_entry_digest: mapping_digest,
                reviewed_by_pubkey: [7; 32],
                reviewed_at_unix_micros: review.reviewed_at_unix_micros,
                review_digest: review_digest(&review).expect("review digest"),
                review_signature: ReviewSignature::from_bytes([8; 64]),
            }],
        }
    }

    #[test]
    fn human_envelope_roundtrips_without_changing_manifest_digest() {
        let manifest = manifest();
        let expected = manifest_digest(&manifest).expect("manifest digest");
        let bytes = ResourceMappingManifestEnvelopeV1::to_pretty_json(&manifest)
            .expect("serialize envelope");
        let parsed = ResourceMappingManifestEnvelopeV1::parse_json(&bytes).expect("parse envelope");
        assert_eq!(manifest, parsed);
        assert_eq!(manifest_digest(&parsed).expect("parsed digest"), expected);
    }

    #[test]
    fn human_envelope_rejects_uppercase_hex_and_nonempty_context() {
        let manifest = manifest();
        let mut value = serde_json::to_value(
            ResourceMappingManifestEnvelopeV1::from_canonical(&manifest).expect("envelope"),
        )
        .expect("value");
        value["base_meta_event_id"] = serde_json::Value::String("AA".repeat(32));
        let bytes = serde_json::to_vec(&value).expect("json");
        assert!(ResourceMappingManifestEnvelopeV1::parse_json(&bytes).is_err());

        let mut value = serde_json::to_value(
            ResourceMappingManifestEnvelopeV1::from_canonical(&manifest).expect("envelope"),
        )
        .expect("value");
        value["entries"][0]["reviewed_v3_payload"]["context_references"] =
            serde_json::json!([{"type":"resource"}]);
        let bytes = serde_json::to_vec(&value).expect("json");
        assert!(ResourceMappingManifestEnvelopeV1::parse_json(&bytes).is_err());
    }
}
