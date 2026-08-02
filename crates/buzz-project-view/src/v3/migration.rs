//! Canonical Project View v2-to-v3 Resource cutover digest contracts.

use postcard::to_stdvec;
use serde::de::{SeqAccess, Visitor};
use serde::ser::SerializeTuple;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fmt;

use crate::{ProjectResource, ProjectViewRelations, MAX_SAFE_REVISION};

/// Maximum reviewed Resource mappings in one v1 manifest.
pub const MAX_MANIFEST_ENTRIES: usize = 4_096;
/// Maximum JSON envelope accepted before allocating a manifest body.
pub const MAX_MANIFEST_JSON_BYTES: usize = 256 * 1024 * 1024;

/// Domain separator for a canonical legacy Resource body digest.
pub const LEGACY_RESOURCE_DIGEST_DOMAIN: &[u8] = b"buzz-pv3-legacy-resource-v1\0";
/// Domain separator for the reviewed final v3 Resource payload digest.
pub const RESOURCE_CUTOVER_PAYLOAD_DIGEST_DOMAIN: &[u8] = b"buzz-pv3-resource-cutover-payload-v1\0";
/// Domain separator for an exact active Guide snapshot digest.
pub const GUIDE_SNAPSHOT_DIGEST_DOMAIN: &[u8] = b"buzz-pv3-guide-snapshot-v1\0";
/// Domain separator for one Resource-to-Guide mapping entry digest.
pub const MAPPING_ENTRY_DIGEST_DOMAIN: &[u8] = b"buzz-pv3-resource-mapping-v1\0";
/// Domain separator for the Human reviewer attestation digest.
pub const REVIEW_DIGEST_DOMAIN: &[u8] = b"buzz-pv3-resource-review-v1\0";
/// Domain separator for the complete sorted manifest digest.
pub const MANIFEST_DIGEST_DOMAIN: &[u8] = b"buzz-pv3-resource-manifest-v1\0";

/// Canonical cutover serialization or shape failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MigrationContractError {
    /// A value could not be serialized with the frozen postcard contract.
    #[error("cannot encode canonical cutover value: {0}")]
    Serialization(String),
    /// A manifest or digest field violated the v1 contract.
    #[error("invalid Resource cutover manifest: {0}")]
    InvalidManifest(String),
}

/// Canonical lifecycle of a legacy Project View Resource body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalLegacyObjectStateV1 {
    /// The Resource has a complete active body.
    Active,
    /// The Resource is a bodyless tombstone.
    Deleted,
}

/// Exact legacy Resource object boundary hashed at the cutover base.
///
/// Field order is a binary protocol: changing it requires a new schema and
/// digest domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalLegacyResourceV1 {
    /// Legacy Project View schema version, one or two.
    pub schema_version: u16,
    /// Stable Resource UUID bytes.
    pub resource_id: [u8; 16],
    /// Canonical object-local revision.
    pub object_revision: u64,
    /// Canonical Project View revision.
    pub project_revision: u64,
    /// Active/tombstone lifecycle at the pinned base.
    pub state: CanonicalLegacyObjectStateV1,
    /// Complete legacy Resource business body when active.
    pub resource_data: Option<ProjectResource>,
    /// Complete structural relations at the pinned base.
    pub relations: ProjectViewRelations,
}

/// Project Resource v3 body represented only by binary primitives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalProjectResourceV3 {
    /// Exact accepted UTF-8 name.
    pub name: String,
    /// Exact accepted open Resource kind token.
    pub resource_kind: String,
    /// Exact optional UTF-8 summary.
    pub summary: Option<String>,
    /// Required Guide Project Document UUID bytes.
    pub guide_document_id: [u8; 16],
}

/// Binary canonical Document reference mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanonicalDocumentReferenceModeV1 {
    /// Resolve the current active head.
    Live,
    /// Resolve one exact active-content revision.
    Pinned,
}

/// Binary canonical Context coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanonicalContextReferenceV1 {
    /// Stable Resource UUID bytes.
    Resource([u8; 16]),
    /// Stable Document UUID bytes, mode, and optional pinned revision.
    Document {
        /// Stable Document UUID bytes.
        document_id: [u8; 16],
        /// Live or pinned resolution mode.
        mode: CanonicalDocumentReferenceModeV1,
        /// Absent for live and positive for pinned.
        document_revision: Option<u64>,
    },
}

/// Final reviewed v3 Resource body and its outer Context set.
///
/// Stage 0 cutover requires `context_references` to be the canonical empty
/// vector. Keeping it in the signed value prevents a migration from silently
/// seeding Context before the sub-capability is ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalResourceCutoverV1 {
    /// Final Resource business body.
    pub resource_data: CanonicalProjectResourceV3,
    /// Must be empty for the v1 cutover.
    pub context_references: Vec<CanonicalContextReferenceV1>,
}

/// Exact active Project Document business snapshot hashed for a Guide pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalGuideSnapshotV1 {
    /// Stable Document UUID bytes.
    pub document_id: [u8; 16],
    /// Positive Document-local revision.
    pub document_revision: u64,
    /// Exact canonical title.
    pub title: String,
    /// Exact optional summary.
    pub summary: Option<String>,
    /// Exact Markdown bytes.
    pub content_markdown: String,
}

/// Canonical value bound by one mapping-entry digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalResourceMappingEntryV1 {
    /// Host-derived Community UUID bytes.
    pub community_id: [u8; 16],
    /// Exact v2 base metadata event.
    pub base_meta_event_id: [u8; 32],
    /// Exact v2 base Project revision.
    pub base_project_revision: u64,
    /// Exact v2 base projection generation.
    pub base_projection_generation: u64,
    /// Stable Resource UUID bytes.
    pub resource_id: [u8; 16],
    /// Exact legacy object revision.
    pub legacy_object_revision: u64,
    /// Exact legacy projection event.
    pub legacy_projection_event_id: [u8; 32],
    /// Canonical legacy business-body digest.
    pub legacy_body_digest: [u8; 32],
    /// Digest of the complete reviewed v3 cutover payload.
    pub v3_payload_digest: [u8; 32],
    /// Guide Document UUID bytes duplicated as an explicit mapping pin.
    pub guide_document_id: [u8; 16],
    /// Exact active Guide revision.
    pub guide_document_revision: u64,
    /// Exact Guide head event.
    pub guide_head_event_id: [u8; 32],
    /// Exact Guide immutable revision event.
    pub guide_revision_event_id: [u8; 32],
    /// Canonical Guide business-snapshot digest.
    pub guide_content_digest: [u8; 32],
}

/// Canonical reviewer attestation input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalResourceReviewV1 {
    /// Digest of the exact Resource mapping entry.
    pub mapping_entry_digest: [u8; 32],
    /// Current direct Human member reviewer x-only pubkey bytes.
    pub reviewed_by_pubkey: [u8; 32],
    /// Signed canonical review time in Unix microseconds.
    pub reviewed_at_unix_micros: i64,
}

/// Exact 64-byte detached BIP-340 Schnorr signature.
///
/// Serde does not implement arrays above 32 elements, so this transparent
/// protocol type supplies fixed-length tuple encoding explicitly. Postcard
/// therefore receives exactly 64 bytes with no variable-length vector prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewSignature([u8; 64]);

impl ReviewSignature {
    /// Construct a signature from its exact BIP-340 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    /// Borrow the exact BIP-340 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }

    /// Consume the wrapper and return the exact BIP-340 bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 64] {
        self.0
    }
}

impl Serialize for ReviewSignature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut tuple = serializer.serialize_tuple(64)?;
        for byte in self.0 {
            tuple.serialize_element(&byte)?;
        }
        tuple.end()
    }
}

impl<'de> Deserialize<'de> for ReviewSignature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SignatureVisitor;

        impl<'de> Visitor<'de> for SignatureVisitor {
            type Value = ReviewSignature;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("exactly 64 signature bytes")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut bytes = [0_u8; 64];
                for (index, byte) in bytes.iter_mut().enumerate() {
                    *byte = sequence
                        .next_element()?
                        .ok_or_else(|| serde::de::Error::invalid_length(index, &self))?;
                }
                if sequence.next_element::<u8>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(65, &self));
                }
                Ok(ReviewSignature(bytes))
            }
        }

        deserializer.deserialize_tuple(64, SignatureVisitor)
    }
}

/// One fully reviewed and signed Resource mapping.
///
/// Field order is the frozen `ReviewedResourceMappingV1` postcard order from
/// the Stage 0 protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedResourceMappingV1 {
    /// Stable Resource UUID bytes.
    pub resource_id: [u8; 16],
    /// Exact legacy object revision.
    pub legacy_object_revision: u64,
    /// Exact legacy projection event.
    pub legacy_projection_event_id: [u8; 32],
    /// Canonical legacy business-body digest.
    pub legacy_body_digest: [u8; 32],
    /// Complete final v3 Resource and empty Context payload.
    pub reviewed_v3_payload: CanonicalResourceCutoverV1,
    /// Domain-separated digest of `reviewed_v3_payload`.
    pub v3_payload_digest: [u8; 32],
    /// Exact active Guide revision.
    pub guide_document_revision: u64,
    /// Exact Guide head event.
    pub guide_head_event_id: [u8; 32],
    /// Exact Guide immutable revision event.
    pub guide_revision_event_id: [u8; 32],
    /// Canonical Guide business-snapshot digest.
    pub guide_content_digest: [u8; 32],
    /// Domain-separated digest of the mapping pins.
    pub mapping_entry_digest: [u8; 32],
    /// Current direct Human member reviewer x-only pubkey bytes.
    pub reviewed_by_pubkey: [u8; 32],
    /// Signed canonical review time in Unix microseconds.
    pub reviewed_at_unix_micros: i64,
    /// Domain-separated reviewer attestation digest.
    pub review_digest: [u8; 32],
    /// Detached BIP-340 Schnorr signature over `review_digest`.
    pub review_signature: ReviewSignature,
}

/// Complete sorted v1 Resource mapping manifest.
///
/// UUID, event ID, pubkey, digest, and signature values use binary primitives
/// in the canonical postcard value. Human JSON tooling is responsible for its
/// separately validated lower-hex envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceMappingManifestV1 {
    /// Must equal one.
    pub schema_version: u16,
    /// Host-derived Community UUID bytes.
    pub community_id: [u8; 16],
    /// Exact v2 base metadata event.
    pub base_meta_event_id: [u8; 32],
    /// Exact v2 base Project revision.
    pub base_project_revision: u64,
    /// Exact v2 base projection generation.
    pub base_projection_generation: u64,
    /// Reviewed entries sorted by Resource UUID bytes.
    pub entries: Vec<ReviewedResourceMappingV1>,
}

impl ResourceMappingManifestV1 {
    /// Validate bounds, canonical order, digest parity, and signature shape.
    ///
    /// Eligibility and cryptographic BIP-340 verification require the pinned
    /// membership snapshot and are deliberately performed by the coordinator.
    pub fn validate(&self) -> Result<(), MigrationContractError> {
        if self.schema_version != 1 {
            return invalid("schema_version must be one");
        }
        require_positive_safe(self.base_project_revision, "base_project_revision")?;
        require_positive_safe(
            self.base_projection_generation,
            "base_projection_generation",
        )?;
        if self.entries.len() > MAX_MANIFEST_ENTRIES {
            return invalid(format!(
                "manifest contains more than {MAX_MANIFEST_ENTRIES} entries"
            ));
        }
        if self
            .entries
            .windows(2)
            .any(|pair| pair[0].resource_id >= pair[1].resource_id)
        {
            return invalid("entries must be unique and sorted by Resource UUID bytes");
        }
        for entry in &self.entries {
            validate_entry(self, entry)?;
        }
        Ok(())
    }
}

/// Compute the canonical legacy Resource body digest.
pub fn legacy_resource_digest(
    value: &CanonicalLegacyResourceV1,
) -> Result<[u8; 32], MigrationContractError> {
    digest(LEGACY_RESOURCE_DIGEST_DOMAIN, value)
}

/// Compute the canonical final v3 Resource payload digest.
pub fn resource_cutover_payload_digest(
    value: &CanonicalResourceCutoverV1,
) -> Result<[u8; 32], MigrationContractError> {
    digest(RESOURCE_CUTOVER_PAYLOAD_DIGEST_DOMAIN, value)
}

/// Compute the canonical exact Guide snapshot digest.
pub fn guide_snapshot_digest(
    value: &CanonicalGuideSnapshotV1,
) -> Result<[u8; 32], MigrationContractError> {
    digest(GUIDE_SNAPSHOT_DIGEST_DOMAIN, value)
}

/// Compute the canonical Resource mapping-entry digest.
pub fn mapping_entry_digest(
    value: &CanonicalResourceMappingEntryV1,
) -> Result<[u8; 32], MigrationContractError> {
    digest(MAPPING_ENTRY_DIGEST_DOMAIN, value)
}

/// Compute the canonical Human reviewer attestation digest.
pub fn review_digest(
    value: &CanonicalResourceReviewV1,
) -> Result<[u8; 32], MigrationContractError> {
    digest(REVIEW_DIGEST_DOMAIN, value)
}

/// Compute the canonical complete manifest digest, including signatures.
pub fn manifest_digest(
    value: &ResourceMappingManifestV1,
) -> Result<[u8; 32], MigrationContractError> {
    digest(MANIFEST_DIGEST_DOMAIN, value)
}

fn validate_entry(
    manifest: &ResourceMappingManifestV1,
    entry: &ReviewedResourceMappingV1,
) -> Result<(), MigrationContractError> {
    require_positive_safe(entry.legacy_object_revision, "legacy_object_revision")?;
    require_positive_safe(entry.guide_document_revision, "guide_document_revision")?;
    if !entry.reviewed_v3_payload.context_references.is_empty() {
        return invalid("v1 cutover Context Reference set must be empty");
    }
    validate_canonical_resource(&entry.reviewed_v3_payload.resource_data)?;
    let expected_payload = resource_cutover_payload_digest(&entry.reviewed_v3_payload)?;
    if entry.v3_payload_digest != expected_payload {
        return invalid("v3_payload_digest does not match reviewed_v3_payload");
    }
    let mapping = CanonicalResourceMappingEntryV1 {
        community_id: manifest.community_id,
        base_meta_event_id: manifest.base_meta_event_id,
        base_project_revision: manifest.base_project_revision,
        base_projection_generation: manifest.base_projection_generation,
        resource_id: entry.resource_id,
        legacy_object_revision: entry.legacy_object_revision,
        legacy_projection_event_id: entry.legacy_projection_event_id,
        legacy_body_digest: entry.legacy_body_digest,
        v3_payload_digest: entry.v3_payload_digest,
        guide_document_id: entry.reviewed_v3_payload.resource_data.guide_document_id,
        guide_document_revision: entry.guide_document_revision,
        guide_head_event_id: entry.guide_head_event_id,
        guide_revision_event_id: entry.guide_revision_event_id,
        guide_content_digest: entry.guide_content_digest,
    };
    if entry.mapping_entry_digest != mapping_entry_digest(&mapping)? {
        return invalid("mapping_entry_digest does not match canonical mapping pins");
    }
    let review = CanonicalResourceReviewV1 {
        mapping_entry_digest: entry.mapping_entry_digest,
        reviewed_by_pubkey: entry.reviewed_by_pubkey,
        reviewed_at_unix_micros: entry.reviewed_at_unix_micros,
    };
    if entry.review_digest != review_digest(&review)? {
        return invalid("review_digest does not match canonical reviewer attestation");
    }
    Ok(())
}

fn validate_canonical_resource(
    resource: &CanonicalProjectResourceV3,
) -> Result<(), MigrationContractError> {
    if resource.name.is_empty()
        || resource.name.trim() != resource.name
        || resource.name.contains('\0')
        || resource.name.len() > 256
    {
        return invalid("Resource name is not canonical");
    }
    let kind = resource.resource_kind.as_bytes();
    if kind.is_empty()
        || kind.len() > 64
        || !kind.iter().enumerate().all(|(index, byte)| {
            matches!(byte, b'a'..=b'z' | b'0'..=b'9')
                || (index > 0 && matches!(byte, b'.' | b'_' | b'-'))
        })
    {
        return invalid("Resource kind is not canonical");
    }
    if resource.summary.as_ref().is_some_and(|summary| {
        summary.is_empty() || summary.contains('\0') || summary.len() > 4_096
    }) {
        return invalid("Resource summary is not canonical");
    }
    Ok(())
}

fn require_positive_safe(value: u64, field: &str) -> Result<(), MigrationContractError> {
    if !(1..=MAX_SAFE_REVISION).contains(&value) {
        return invalid(format!("{field} must be JavaScript-safe and positive"));
    }
    Ok(())
}

fn digest<T: Serialize>(domain: &[u8], value: &T) -> Result<[u8; 32], MigrationContractError> {
    let canonical = to_stdvec(value)
        .map_err(|error| MigrationContractError::Serialization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(canonical);
    Ok(hasher.finalize().into())
}

fn invalid<T>(reason: impl Into<String>) -> Result<T, MigrationContractError> {
    Err(MigrationContractError::InvalidManifest(reason.into()))
}
