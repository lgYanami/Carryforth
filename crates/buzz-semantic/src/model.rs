use std::fmt;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

/// Frozen provider name for the first production-like semantic generation.
pub const DEFAULT_EMBEDDING_PROVIDER: &str = "volcengine_ark";
/// Frozen model version for the first production-like semantic generation.
pub const DEFAULT_EMBEDDING_MODEL: &str = "doubao-embedding-vision-251215";
/// Frozen vector width for the first production-like semantic generation.
pub const DEFAULT_EMBEDDING_DIMENSIONS: usize = 2048;

/// Errors produced by pure semantic validation and extraction.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SemanticError {
    /// A stable identity contains a nil UUID.
    #[error("semantic {field} must not be nil")]
    NilIdentity {
        /// Name of the invalid identity field.
        field: &'static str,
    },
    /// A required text field is blank.
    #[error("semantic {field} must not be blank")]
    BlankText {
        /// Name of the invalid field.
        field: &'static str,
    },
    /// Project data contains a NUL byte.
    #[error("semantic {field} must not contain NUL")]
    NulText {
        /// Name of the invalid field.
        field: &'static str,
    },
    /// A typed source identity and source basis describe different domains.
    #[error("semantic source identity and basis families do not match")]
    SourceBasisMismatch,
    /// An overview was requested for an ineligible source.
    #[error("semantic source is ineligible for overview extraction")]
    IneligibleSource,
    /// A source revision must be positive.
    #[error("semantic {field} must be positive")]
    InvalidRevision {
        /// Name of the invalid revision field.
        field: &'static str,
    },
    /// A model contract field is invalid.
    #[error("invalid semantic model contract: {reason}")]
    InvalidModelContract {
        /// Stable reason for rejection.
        reason: &'static str,
    },
    /// An embedding has the wrong number of values.
    #[error("embedding dimension mismatch: expected {expected}, observed {observed}")]
    EmbeddingDimensionMismatch {
        /// Dimension required by the model contract.
        expected: usize,
        /// Dimension returned by the encoder.
        observed: usize,
    },
    /// An embedding contains NaN or infinity.
    #[error("embedding contains a non-finite value at index {index}")]
    NonFiniteEmbedding {
        /// Index of the invalid value.
        index: usize,
    },
    /// A deterministic internal contract could not be serialized.
    #[error("semantic contract serialization failed")]
    Serialization,
    /// Encoder input text does not match its declared semantic digest.
    #[error("semantic encoder input digest mismatch")]
    EncoderInputDigestMismatch,
    /// An external provider was asked to encode a unit kind that has not been
    /// approved for data egress.
    #[error("semantic unit kind is not approved for the external provider boundary")]
    ExternalProviderBoundary,
    /// The approved provider could not be reached or timed out.
    #[error("semantic provider transport failed")]
    ProviderTransport,
    /// The provider asked the worker to retry later.
    #[error("semantic provider rate limited the request")]
    ProviderRateLimited {
        /// Optional server-provided retry delay.
        retry_after_seconds: Option<u64>,
    },
    /// The provider returned a retryable server status.
    #[error("semantic provider returned retryable status {status}")]
    ProviderRetryable {
        /// HTTP status without response body or source content.
        status: u16,
    },
    /// The provider rejected the request permanently.
    #[error("semantic provider rejected request with status {status}")]
    ProviderRejected {
        /// HTTP status without response body or source content.
        status: u16,
    },
    /// The provider returned malformed or contract-incompatible JSON.
    #[error("semantic provider response violated the generation contract")]
    ProviderResponse,
}

/// A domain-separated SHA-256 digest serialized as lowercase hex.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest32([u8; 32]);

impl Digest32 {
    /// Construct a digest from exactly 32 bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the digest bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Return lowercase hexadecimal encoding.
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    /// Parse a lowercase or uppercase 64-character hexadecimal digest.
    pub fn from_hex(value: &str) -> Result<Self, SemanticError> {
        let decoded = hex::decode(value).map_err(|_| SemanticError::InvalidModelContract {
            reason: "digest must be hexadecimal",
        })?;
        let bytes: [u8; 32] =
            decoded
                .try_into()
                .map_err(|_| SemanticError::InvalidModelContract {
                    reason: "digest must contain 32 bytes",
                })?;
        Ok(Self(bytes))
    }

    pub(crate) fn hash_domain(domain: &'static [u8], parts: &[&[u8]]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update((domain.len() as u64).to_be_bytes());
        hasher.update(domain);
        for part in parts {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part);
        }
        Self(hasher.finalize().into())
    }
}

impl fmt::Debug for Digest32 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Digest32")
            .field(&self.to_hex())
            .finish()
    }
}

impl Serialize for Digest32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Digest32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(de::Error::custom)
    }
}

/// Closed Project View source subtypes eligible for semantic observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectViewSemanticType {
    /// Project profile.
    ProjectProfile,
    /// Project goal.
    Goal,
    /// Project role.
    Role,
    /// Project plan.
    Plan,
    /// Project stage.
    Stage,
    /// Requirement.
    Requirement,
    /// Issue.
    Issue,
    /// Work item.
    Work,
    /// Resource.
    Resource,
}

impl ProjectViewSemanticType {
    /// Stable human-readable label encoded into an overview unit.
    pub const fn type_label(self) -> &'static str {
        match self {
            Self::ProjectProfile => "Project View Profile",
            Self::Goal => "Project View Goal",
            Self::Role => "Project View Role",
            Self::Plan => "Project View Plan",
            Self::Stage => "Project View Stage",
            Self::Requirement => "Project View Requirement",
            Self::Issue => "Project View Issue",
            Self::Work => "Project View Work",
            Self::Resource => "Project View Resource",
        }
    }
}

/// Closed canonical source kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "family", content = "subtype", rename_all = "snake_case")]
pub enum SemanticSourceKind {
    /// A Project View object.
    ProjectView(ProjectViewSemanticType),
    /// A Project Document.
    ProjectDocument,
    /// A Meeting.
    Meeting,
}

impl SemanticSourceKind {
    /// Stable label included in overview encoder input.
    pub const fn type_label(self) -> &'static str {
        match self {
            Self::ProjectView(subtype) => subtype.type_label(),
            Self::ProjectDocument => "Project Document",
            Self::Meeting => "Meeting",
        }
    }
}

/// Stable tenant-scoped canonical source identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticSourceIdentity {
    /// Community tenant boundary.
    pub community_id: Uuid,
    /// Canonical source kind and subtype.
    pub kind: SemanticSourceKind,
    /// Stable source UUID inside the Community.
    pub source_id: Uuid,
}

impl SemanticSourceIdentity {
    /// Validate non-nil tenant and source identities.
    pub fn validate(&self) -> Result<(), SemanticError> {
        if self.community_id.is_nil() {
            return Err(SemanticError::NilIdentity {
                field: "community_id",
            });
        }
        if self.source_id.is_nil() {
            return Err(SemanticError::NilIdentity { field: "source_id" });
        }
        Ok(())
    }
}

/// Project View source-currentness evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectViewSourceBasis {
    /// Canonical Project View schema version.
    pub schema_version: u16,
    /// Object-local revision.
    pub object_revision: u64,
    /// Canonical change/event identity that produced the object head.
    pub source_change_id: Digest32,
}

/// Project Document source-currentness evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDocumentSourceBasis {
    /// Document-local revision.
    pub document_revision: u64,
    /// Canonical change/event identity that produced the current revision.
    pub source_change_id: Digest32,
}

/// Meeting source-currentness evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingSourceBasis {
    /// Immutable Meeting Create event identity.
    pub create_event_id: Digest32,
    /// Terminal End event identity when the Meeting has ended.
    pub end_event_id: Option<Digest32>,
}

/// Closed typed source-currentness bases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "family", content = "basis", rename_all = "snake_case")]
pub enum SemanticSourceBasis {
    /// Project View evidence.
    ProjectView(ProjectViewSourceBasis),
    /// Project Document evidence.
    ProjectDocument(ProjectDocumentSourceBasis),
    /// Meeting evidence.
    Meeting(MeetingSourceBasis),
}

impl SemanticSourceBasis {
    fn validate_for(&self, kind: SemanticSourceKind) -> Result<(), SemanticError> {
        match (self, kind) {
            (Self::ProjectView(basis), SemanticSourceKind::ProjectView(_)) => {
                if basis.schema_version == 0 {
                    return Err(SemanticError::InvalidRevision {
                        field: "schema_version",
                    });
                }
                if basis.object_revision == 0 {
                    return Err(SemanticError::InvalidRevision {
                        field: "object_revision",
                    });
                }
                Ok(())
            }
            (Self::ProjectDocument(basis), SemanticSourceKind::ProjectDocument) => {
                if basis.document_revision == 0 {
                    return Err(SemanticError::InvalidRevision {
                        field: "document_revision",
                    });
                }
                Ok(())
            }
            (Self::Meeting(_), SemanticSourceKind::Meeting) => Ok(()),
            _ => Err(SemanticError::SourceBasisMismatch),
        }
    }
}

/// Cross-family lifecycle class used only as query filter metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticLifecycleClass {
    /// Active mutable source.
    Active,
    /// Active Meeting action-finalization state.
    Finalizing,
    /// Business-complete source that still owns readable canonical content.
    Terminal,
    /// Source-native bodyless tombstone.
    Tombstone,
    /// Hard-deleted or erased source.
    Deleted,
}

/// Source-native lifecycle and status metadata excluded from encoder input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticFilterMetadata {
    /// Cross-family lifecycle class.
    pub lifecycle: SemanticLifecycleClass,
    /// Optional source-native status such as `completed` or `closed`.
    pub source_status: Option<String>,
}

/// Stable reasons a canonical source cannot currently produce semantic units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IneligibilityReason {
    /// Source-native bodyless tombstone.
    Tombstone,
    /// Source was hard-deleted or erased.
    Deleted,
    /// Canonical state failed typed verification.
    InvalidCanonicalState,
    /// Required source capability is not ready.
    SourceCapabilityUnavailable,
}

/// Whether a canonical observation may have current semantic units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum SemanticEligibility {
    /// The source may be indexed.
    Eligible,
    /// The source must not have a current semantic head.
    Ineligible(IneligibilityReason),
}

/// Verified canonical source observation consumed by the pure extractor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalSemanticSourceObservation {
    /// Stable canonical source identity.
    pub identity: SemanticSourceIdentity,
    /// Typed currentness evidence.
    pub basis: SemanticSourceBasis,
    /// Whether the source may currently be indexed.
    pub eligibility: SemanticEligibility,
    /// Lifecycle/status metadata excluded from semantic text.
    pub filter: SemanticFilterMetadata,
    /// Current source-owned title or name.
    pub title: String,
    /// Current source-owned optional summary.
    pub summary: Option<String>,
    /// Digest of the complete verified observation used for CAS currentness.
    pub snapshot_digest: Digest32,
}

impl CanonicalSemanticSourceObservation {
    /// Validate fields and construct an observation with a deterministic,
    /// domain-separated snapshot digest.
    pub fn new(
        identity: SemanticSourceIdentity,
        basis: SemanticSourceBasis,
        eligibility: SemanticEligibility,
        filter: SemanticFilterMetadata,
        title: String,
        summary: Option<String>,
    ) -> Result<Self, SemanticError> {
        identity.validate()?;
        basis.validate_for(identity.kind)?;
        validate_text(
            "title",
            &title,
            matches!(eligibility, SemanticEligibility::Eligible),
        )?;
        if let Some(summary) = summary.as_deref() {
            validate_text("summary", summary, true)?;
        }
        if matches!(eligibility, SemanticEligibility::Ineligible(_)) && summary.is_some() {
            return Err(SemanticError::InvalidModelContract {
                reason: "ineligible observations must not expose a summary",
            });
        }
        if let Some(status) = filter.source_status.as_deref() {
            validate_text("source_status", status, true)?;
        }
        let canonical =
            postcard::to_stdvec(&(&identity, &basis, eligibility, &filter, &title, &summary))
                .map_err(|_| SemanticError::Serialization)?;
        let snapshot_digest =
            Digest32::hash_domain(b"buzz.semantic.source-snapshot.v1", &[canonical.as_slice()]);
        Ok(Self {
            identity,
            basis,
            eligibility,
            filter,
            title,
            summary,
            snapshot_digest,
        })
    }
}

/// Semantic unit types reserved by the foundation schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticUnitKind {
    /// Source title/summary overview.
    Overview,
    /// Reserved for a future full-content slicing design.
    ContentChunk,
}

/// Stable identity of one extracted semantic unit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticUnitIdentity {
    /// Canonical source identity.
    pub source: SemanticSourceIdentity,
    /// Unit kind.
    pub kind: SemanticUnitKind,
    /// Stable source-local key (`overview` in the first release).
    pub key: String,
    /// Stable source-local ordinal.
    pub ordinal: u32,
    /// Optional source-native path reserved for future chunks.
    pub path: Option<String>,
    /// Canonical source snapshot used by this unit set.
    pub source_snapshot_digest: Digest32,
    /// Versioned extractor contract.
    pub extractor_version: String,
}

/// Coverage of canonical source metadata in an overview unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticCoverage {
    /// The source had only title/name metadata.
    TitleOnly,
    /// The source had title/name and a source-owned summary.
    TitleAndSummary,
}

/// Complete extracted semantic unit before model encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticUnit {
    /// Stable unit identity and extraction provenance.
    pub identity: SemanticUnitIdentity,
    /// Deterministic visible text sent to an approved encoder.
    pub text: String,
    /// Domain-separated digest of `text`.
    pub semantic_text_digest: Digest32,
    /// Metadata coverage represented by `text`.
    pub coverage: SemanticCoverage,
}

/// Distance function used by one model generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticDistanceMetric {
    /// Cosine distance (`1 - cosine similarity`).
    Cosine,
}

/// Client-side vector normalization contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticNormalization {
    /// Preserve finite provider values without client-side normalization.
    None,
}

/// Where canonical overview text is encoded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "provider", rename_all = "snake_case")]
pub enum SemanticProviderBoundary {
    /// Deterministic offline encoder used only by tests.
    DeterministicFake,
    /// Approved external provider for explicitly enabled Communities.
    External(String),
}

/// Closed model contract persisted by each semantic generation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticModelContract {
    /// Stable provider identifier.
    pub provider: String,
    /// Exact model version, never a mutable alias.
    pub model: String,
    /// Required vector width.
    pub dimensions: usize,
    /// Distance metric.
    pub distance_metric: SemanticDistanceMetric,
    /// Client normalization behavior.
    pub normalization: SemanticNormalization,
    /// Versioned encoder-input contract.
    pub input_contract_version: String,
    /// Data execution boundary.
    pub provider_boundary: SemanticProviderBoundary,
}

impl SemanticModelContract {
    /// Frozen Volcengine overview contract selected for the first generation.
    pub fn volcengine_overview_v1() -> Self {
        Self {
            provider: DEFAULT_EMBEDDING_PROVIDER.to_string(),
            model: DEFAULT_EMBEDDING_MODEL.to_string(),
            dimensions: DEFAULT_EMBEDDING_DIMENSIONS,
            distance_metric: SemanticDistanceMetric::Cosine,
            normalization: SemanticNormalization::None,
            input_contract_version: "overview-v1".to_string(),
            provider_boundary: SemanticProviderBoundary::External(
                DEFAULT_EMBEDDING_PROVIDER.to_string(),
            ),
        }
    }

    /// Validate the closed generation contract.
    pub fn validate(&self) -> Result<(), SemanticError> {
        for (field, value) in [
            ("provider", self.provider.as_str()),
            ("model", self.model.as_str()),
            (
                "input_contract_version",
                self.input_contract_version.as_str(),
            ),
        ] {
            validate_text(field, value, true).map_err(|_| SemanticError::InvalidModelContract {
                reason: "text fields must be non-blank and contain no NUL",
            })?;
        }
        if self.dimensions == 0 || self.dimensions > 16_000 {
            return Err(SemanticError::InvalidModelContract {
                reason: "dimensions must be between 1 and 16000",
            });
        }
        match &self.provider_boundary {
            SemanticProviderBoundary::External(provider) if provider != &self.provider => {
                Err(SemanticError::InvalidModelContract {
                    reason: "external provider boundary must match provider",
                })
            }
            SemanticProviderBoundary::External(provider) => {
                validate_text("provider_boundary", provider, true).map_err(|_| {
                    SemanticError::InvalidModelContract {
                        reason: "provider boundary must be non-blank and contain no NUL",
                    }
                })
            }
            SemanticProviderBoundary::DeterministicFake => Ok(()),
        }
    }

    /// Domain-separated digest used to bind embeddings to this exact contract.
    pub fn digest(&self) -> Result<Digest32, SemanticError> {
        self.validate()?;
        let canonical = postcard::to_stdvec(self).map_err(|_| SemanticError::Serialization)?;
        Ok(Digest32::hash_domain(
            b"buzz.semantic.model-contract.v1",
            &[canonical.as_slice()],
        ))
    }
}

/// Validated finite embedding values.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingVector {
    values: Vec<f32>,
}

impl EmbeddingVector {
    /// Validate provider output against a generation contract.
    pub fn new(values: Vec<f32>, contract: &SemanticModelContract) -> Result<Self, SemanticError> {
        contract.validate()?;
        if values.len() != contract.dimensions {
            return Err(SemanticError::EmbeddingDimensionMismatch {
                expected: contract.dimensions,
                observed: values.len(),
            });
        }
        if let Some((index, _)) = values
            .iter()
            .enumerate()
            .find(|(_, value)| !value.is_finite())
        {
            return Err(SemanticError::NonFiniteEmbedding { index });
        }
        Ok(Self { values })
    }

    /// Borrow validated values for database binding.
    pub fn as_slice(&self) -> &[f32] {
        &self.values
    }

    /// Consume the wrapper and return validated values.
    pub fn into_values(self) -> Vec<f32> {
        self.values
    }
}

fn validate_text(
    field: &'static str,
    value: &str,
    require_non_blank: bool,
) -> Result<(), SemanticError> {
    if value.contains('\0') {
        return Err(SemanticError::NulText { field });
    }
    if require_non_blank && value.trim().is_empty() {
        return Err(SemanticError::BlankText { field });
    }
    Ok(())
}
