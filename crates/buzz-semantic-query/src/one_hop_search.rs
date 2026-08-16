//! Closed contracts for structure-scoped one-hop semantic selection.
//!
//! The request names exactly one Coordinate or Edge scope. The result returns
//! ranked candidates from that scope plus canonical lightweight observations;
//! it never returns a path or the next structural layer.

use std::{cmp::Ordering, collections::BTreeSet};

use buzz_project_context::{EdgeKey, ProjectContextCoordinate, MAX_SAFE_REVISION};
use buzz_semantic::{Digest32, SemanticLifecycleClass, SemanticSourceBasis};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    build_problem_query_encoder_input, query_contract_digest, ProjectContextCoordinateTypeFilter,
    QueryContractResult, Score, SemanticQueryEncoderInput, MAX_PROBLEM_BYTES,
};

/// Default number of one-hop candidates returned to the caller.
pub const DEFAULT_ONE_HOP_SEMANTIC_LIMIT: u8 = 8;
/// Hard cap on one-hop candidates returned to the caller.
pub const MAX_ONE_HOP_SEMANTIC_LIMIT: u8 = 32;
/// Maximum raw closed inner request bytes.
pub const MAX_ONE_HOP_SEMANTIC_REQUEST_BYTES: usize = 64 * 1024;
/// Maximum exact authenticated `/query` filter-array bytes.
pub const MAX_ONE_HOP_SEMANTIC_EXACT_HTTP_BODY_BYTES: usize = 64 * 1024;
/// Maximum exact Provider input bytes inherited from the semantic Q0 contract.
pub const MAX_ONE_HOP_SEMANTIC_PROVIDER_INPUT_BYTES: usize = crate::MAX_PROVIDER_QUERY_INPUT_BYTES;
/// Maximum final Relay-signed Event-array bytes.
pub const MAX_ONE_HOP_SEMANTIC_RESPONSE_BYTES: usize = 512 * 1024;
/// Absolute server wall-time budget in milliseconds.
pub const MAX_ONE_HOP_SEMANTIC_WALL_TIME_MS: u32 = 45_000;
/// Maximum incident Edge identities materialized before ranking.
pub const MAX_ONE_HOP_INCIDENT_EDGES: u32 = 1_024;
/// Maximum relation Document bindings materialized before ranking.
pub const MAX_ONE_HOP_RELATION_BINDINGS: u32 = 2_048;
/// Maximum complete Edge members materialized before ranking.
pub const MAX_ONE_HOP_EDGE_COORDINATES: u32 = 4_096;
/// Maximum ranked Document candidates retained per Edge.
pub const MAX_ONE_HOP_DOCUMENTS_PER_EDGE: usize = 3;
/// Maximum canonical Hyperedge identity bytes accepted by the scoped read.
pub const MAX_ONE_HOP_HYPEREDGE_IDENTITY_BYTES: usize = 64 * 1024;

const INCIDENT_EDGE_RANKING_DESCRIPTOR: &str = concat!(
    "contract=carryforth.project-context-one-hop-incident-edge-ranking.v1\n",
    "query=semantic-graph-query.problem\n",
    "document-score=direct-current-head-cosine\n",
    "edge-score=max-document-score\n",
    "edge-order=score-desc-edge-key-asc\n",
    "document-order=score-desc-document-id-asc\n",
    "documents-per-edge=3\n",
    "floor=none\n",
    "coherence=none\n",
    "max-edges=32\n",
    "max-materialized-edges=1024\n",
    "max-materialized-bindings=2048"
);

const EDGE_COORDINATE_RANKING_DESCRIPTOR: &str = concat!(
    "contract=carryforth.project-context-one-hop-edge-coordinate-ranking.v1\n",
    "query=semantic-graph-query.problem\n",
    "coordinate-score=direct-current-head-cosine\n",
    "order=score-desc-coordinate-ord-asc\n",
    "lifecycle=all-current\n",
    "floor=none\n",
    "coherence=none\n",
    "max-coordinates=32\n",
    "max-materialized-coordinates=4096"
);

const EDGE_COORDINATE_FILTERED_RANKING_DESCRIPTOR: &str = concat!(
    "contract=carryforth.project-context-one-hop-edge-coordinate-ranking.v2\n",
    "query=semantic-graph-query.problem\n",
    "scope=complete-edge-members-filtered-by-closed-coordinate-type-before-scoring\n",
    "coordinate-score=direct-current-head-cosine\n",
    "order=score-desc-coordinate-ord-asc\n",
    "lifecycle=all-current\n",
    "floor=none\n",
    "coherence=none\n",
    "max-coordinates=32\n",
    "max-materialized-coordinates=4096"
);

const ONE_HOP_HTTP_REQUEST_BINDING_DOMAIN: &[u8] =
    b"carryforth.project-context-one-hop-semantic-search-http-request\0";
const ONE_HOP_V2_HTTP_REQUEST_BINDING_DOMAIN: &[u8] =
    b"carryforth.project-context-one-hop-semantic-search-v2-http-request\0";

/// Result alias for the pure one-hop semantic contract.
pub type OneHopSemanticResult<T> = Result<T, OneHopSemanticError>;

/// Closed pure-contract failures for one-hop semantic selection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OneHopSemanticError {
    /// Raw inner request exceeded its resource boundary.
    #[error("one-hop semantic request is {observed} bytes; maximum is {maximum}")]
    RequestTooLarge {
        /// Observed byte count.
        observed: usize,
        /// Maximum accepted byte count.
        maximum: usize,
    },
    /// JSON was malformed or violated the closed schema.
    #[error("invalid one-hop semantic JSON: {0}")]
    InvalidJson(String),
    /// A required UUID was nil or not UUIDv4.
    #[error("one-hop semantic {field} must be UUIDv4")]
    InvalidUuid {
        /// Rejected field.
        field: &'static str,
    },
    /// The trimmed natural-language query was empty.
    #[error("one-hop semantic query must not be blank")]
    BlankQuery,
    /// Query or returned text contained a forbidden NUL byte.
    #[error("one-hop semantic {field} must not contain NUL")]
    NulText {
        /// Rejected field.
        field: &'static str,
    },
    /// Query text exceeded its UTF-8 resource boundary.
    #[error("one-hop semantic query is {observed} bytes; maximum is {maximum}")]
    QueryTooLarge {
        /// Observed byte count.
        observed: usize,
        /// Maximum accepted byte count.
        maximum: usize,
    },
    /// Requested result limit was outside the closed range.
    #[error("one-hop semantic limit {observed} is outside 1..={maximum}")]
    InvalidLimit {
        /// Rejected limit.
        observed: u8,
        /// Maximum accepted limit.
        maximum: u8,
    },
    /// A Coordinate was malformed or crossed the host Project boundary.
    #[error("invalid one-hop semantic Coordinate: {0}")]
    InvalidCoordinate(String),
    /// A present Edge-member Coordinate type filter was empty or malformed.
    #[error("invalid one-hop Coordinate type filter: {0}")]
    InvalidCoordinateTypes(String),
    /// A completed result violated a closed invariant.
    #[error("invalid one-hop semantic state: {0}")]
    InvalidState(String),
    /// Deterministic serialization failed.
    #[error("one-hop semantic serialization failed")]
    Serialization,
}

/// Exact structure scope searched by a one-hop semantic request.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OneHopSemanticScope {
    /// Rank incident Edges through their bound relation Documents.
    IncidentEdges {
        /// Existing current Coordinate whose incident Edges form the scope.
        coordinate: ProjectContextCoordinate,
    },
    /// Rank current member Coordinates inside one complete active Edge.
    EdgeCoordinates {
        /// Exact current Edge identity whose complete members form the scope.
        edge_key: EdgeKey,
        /// Optional closed Coordinate types; omitted preserves the v1 scope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        coordinate_types: Option<ProjectContextCoordinateTypeFilter>,
    },
}

impl std::fmt::Debug for OneHopSemanticScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::IncidentEdges { .. } => "IncidentEdges(<redacted-coordinate>)",
            Self::EdgeCoordinates { .. } => "EdgeCoordinates(<redacted-edge>)",
        })
    }
}

impl OneHopSemanticScope {
    /// Ranking contract required by this exact scope variant.
    #[must_use]
    pub fn ranking_contract_digest(&self) -> Digest32 {
        match self {
            Self::IncidentEdges { .. } => incident_edge_ranking_contract_digest(),
            Self::EdgeCoordinates {
                coordinate_types: Some(_),
                ..
            } => edge_coordinate_filtered_ranking_contract_digest(),
            Self::EdgeCoordinates { .. } => edge_coordinate_ranking_contract_digest(),
        }
    }
}

/// Closed natural-language request for one structure-scoped semantic choice.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextOneHopSemanticQuery {
    /// Caller-generated request correlation UUIDv4.
    pub request_id: Uuid,
    /// Host-derived Project UUIDv4 echoed into the authenticated request.
    pub project_id: Uuid,
    /// Natural-language selection query.
    pub query: String,
    /// Maximum returned Edge or Coordinate candidates in `1..=32`.
    pub limit: u8,
    /// Exact structure scope and result variant.
    pub scope: OneHopSemanticScope,
}

impl std::fmt::Debug for ProjectContextOneHopSemanticQuery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectContextOneHopSemanticQuery")
            .field("request_id", &self.request_id)
            .field("project_id", &self.project_id)
            .field("query", &"<redacted>")
            .field("query_bytes", &self.query.len())
            .field("limit", &self.limit)
            .field("scope", &self.scope)
            .finish()
    }
}

impl ProjectContextOneHopSemanticQuery {
    /// Parse a bounded closed JSON request and canonicalize its query text.
    pub fn parse_json(bytes: &[u8]) -> OneHopSemanticResult<Self> {
        if bytes.len() > MAX_ONE_HOP_SEMANTIC_REQUEST_BYTES {
            return Err(OneHopSemanticError::RequestTooLarge {
                observed: bytes.len(),
                maximum: MAX_ONE_HOP_SEMANTIC_REQUEST_BYTES,
            });
        }
        let request = serde_json::from_slice(bytes)
            .map_err(|error| OneHopSemanticError::InvalidJson(error.to_string()))?;
        Self::validate_and_canonicalize(request)
    }

    /// Validate a trusted in-memory request and trim its query.
    pub fn validate_and_canonicalize(mut self) -> OneHopSemanticResult<Self> {
        validate_uuid_v4(self.request_id, "request_id")?;
        validate_uuid_v4(self.project_id, "project_id")?;
        let query = self.query.trim();
        if query.is_empty() {
            return Err(OneHopSemanticError::BlankQuery);
        }
        if query.as_bytes().contains(&0) {
            return Err(OneHopSemanticError::NulText { field: "query" });
        }
        if query.len() > MAX_PROBLEM_BYTES {
            return Err(OneHopSemanticError::QueryTooLarge {
                observed: query.len(),
                maximum: MAX_PROBLEM_BYTES,
            });
        }
        if self.limit == 0 || self.limit > MAX_ONE_HOP_SEMANTIC_LIMIT {
            return Err(OneHopSemanticError::InvalidLimit {
                observed: self.limit,
                maximum: MAX_ONE_HOP_SEMANTIC_LIMIT,
            });
        }
        if let OneHopSemanticScope::IncidentEdges { coordinate } = &self.scope {
            coordinate
                .validate_for_project(self.project_id)
                .map_err(|error| OneHopSemanticError::InvalidCoordinate(error.to_string()))?;
        }
        if let OneHopSemanticScope::EdgeCoordinates {
            coordinate_types, ..
        } = &mut self.scope
        {
            *coordinate_types = coordinate_types
                .as_ref()
                .map(ProjectContextCoordinateTypeFilter::canonicalized)
                .transpose()
                .map_err(|error| OneHopSemanticError::InvalidCoordinateTypes(error.to_string()))?;
        }
        self.query = query.to_owned();
        let canonical =
            serde_json::to_vec(&self).map_err(|_| OneHopSemanticError::Serialization)?;
        if canonical.len() > MAX_ONE_HOP_SEMANTIC_REQUEST_BYTES {
            return Err(OneHopSemanticError::RequestTooLarge {
                observed: canonical.len(),
                maximum: MAX_ONE_HOP_SEMANTIC_REQUEST_BYTES,
            });
        }
        Ok(self)
    }
}

/// Build the exact semantic-graph Q0 input for one scoped request.
pub fn build_one_hop_semantic_query_encoder_input(
    request: &ProjectContextOneHopSemanticQuery,
) -> QueryContractResult<SemanticQueryEncoderInput> {
    build_problem_query_encoder_input(request.request_id, &request.query)
}

/// Return the frozen Coordinate-to-Edge ranking contract digest.
#[must_use]
pub fn incident_edge_ranking_contract_digest() -> Digest32 {
    hash_domain(
        b"carryforth.project-context-one-hop-ranking-contract",
        &[INCIDENT_EDGE_RANKING_DESCRIPTOR.as_bytes()],
    )
}

/// Return the frozen Edge-to-Coordinate ranking contract digest.
#[must_use]
pub fn edge_coordinate_ranking_contract_digest() -> Digest32 {
    hash_domain(
        b"carryforth.project-context-one-hop-ranking-contract",
        &[EDGE_COORDINATE_RANKING_DESCRIPTOR.as_bytes()],
    )
}

/// Return the filtered Edge-member Coordinate v2 ranking contract digest.
#[must_use]
pub fn edge_coordinate_filtered_ranking_contract_digest() -> Digest32 {
    hash_domain(
        b"carryforth.project-context-one-hop-ranking-contract",
        &[EDGE_COORDINATE_FILTERED_RANKING_DESCRIPTOR.as_bytes()],
    )
}

/// Derive the domain-separated binding for one authenticated exact HTTP request.
///
/// The exact body contains the request UUID, scope, query, and exclusive Relay
/// author filter. The explicit identities prevent the same bytes from being
/// replayed across a host-derived Project, caller, Relay, or NIP-98 Event.
pub fn derive_one_hop_semantic_http_request_binding(
    host_project_id: Uuid,
    authenticated_caller_pubkey: &[u8; 32],
    expected_relay_pubkey: &[u8; 32],
    nip98_auth_event_id: Digest32,
    exact_authenticated_body: &[u8],
) -> OneHopSemanticResult<Digest32> {
    validate_uuid_v4(host_project_id, "host_project_id")?;
    let body_digest: [u8; 32] = Sha256::digest(exact_authenticated_body).into();
    Ok(hash_domain(
        ONE_HOP_HTTP_REQUEST_BINDING_DOMAIN,
        &[
            host_project_id.as_bytes(),
            authenticated_caller_pubkey,
            expected_relay_pubkey,
            nip98_auth_event_id.as_bytes(),
            &body_digest,
        ],
    ))
}

/// Verify a returned binding against the exact authenticated HTTP transcript.
pub fn verify_one_hop_semantic_http_request_binding(
    observed: Digest32,
    host_project_id: Uuid,
    authenticated_caller_pubkey: &[u8; 32],
    expected_relay_pubkey: &[u8; 32],
    nip98_auth_event_id: Digest32,
    exact_authenticated_body: &[u8],
) -> OneHopSemanticResult<()> {
    let expected = derive_one_hop_semantic_http_request_binding(
        host_project_id,
        authenticated_caller_pubkey,
        expected_relay_pubkey,
        nip98_auth_event_id,
        exact_authenticated_body,
    )?;
    if observed != expected {
        return invalid("one-hop semantic HTTP request binding digest mismatch");
    }
    Ok(())
}

/// Derive the independently versioned filtered one-hop request binding.
pub fn derive_one_hop_semantic_v2_http_request_binding(
    host_project_id: Uuid,
    authenticated_caller_pubkey: &[u8; 32],
    expected_relay_pubkey: &[u8; 32],
    nip98_auth_event_id: Digest32,
    exact_authenticated_body: &[u8],
) -> OneHopSemanticResult<Digest32> {
    validate_uuid_v4(host_project_id, "host_project_id")?;
    let body_digest: [u8; 32] = Sha256::digest(exact_authenticated_body).into();
    Ok(hash_domain(
        ONE_HOP_V2_HTTP_REQUEST_BINDING_DOMAIN,
        &[
            host_project_id.as_bytes(),
            authenticated_caller_pubkey,
            expected_relay_pubkey,
            nip98_auth_event_id.as_bytes(),
            &body_digest,
        ],
    ))
}

/// Verify a filtered one-hop binding against exact authenticated bytes.
pub fn verify_one_hop_semantic_v2_http_request_binding(
    observed: Digest32,
    host_project_id: Uuid,
    authenticated_caller_pubkey: &[u8; 32],
    expected_relay_pubkey: &[u8; 32],
    nip98_auth_event_id: Digest32,
    exact_authenticated_body: &[u8],
) -> OneHopSemanticResult<()> {
    let expected = derive_one_hop_semantic_v2_http_request_binding(
        host_project_id,
        authenticated_caller_pubkey,
        expected_relay_pubkey,
        nip98_auth_event_id,
        exact_authenticated_body,
    )?;
    if observed != expected {
        return invalid("filtered one-hop HTTP request binding digest mismatch");
    }
    Ok(())
}

/// Exact generation and Project Context snapshot observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OneHopSemanticObservations {
    /// Active semantic generation observed in the read snapshot.
    pub semantic_generation_id: Uuid,
    /// Complete source-generation contract digest.
    pub source_generation_contract_digest: Digest32,
    /// Comparable model-space fence.
    pub embedding_space_fence: Digest32,
    /// Reused semantic-graph Q0 query contract digest.
    pub query_contract_digest: Digest32,
    /// Scope-specific direct-ranking contract digest.
    pub ranking_contract_digest: Digest32,
    /// Active Project Context projection generation.
    pub projection_generation: u64,
    /// Project Context catalog revision in the read snapshot.
    pub project_context_revision: u64,
    /// Writer-DB transaction observation time.
    pub snapshot_observed_at: DateTime<Utc>,
}

/// Source-owned content returned with every ranked candidate.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OneHopCandidatePreview {
    /// Current canonical title or name.
    pub title: String,
    /// Current canonical description when the source family owns one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub description: Option<String>,
    /// Complete source-owned summary when one exists.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
}

impl std::fmt::Debug for OneHopCandidatePreview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OneHopCandidatePreview")
            .field("title", &"<redacted>")
            .field("title_bytes", &self.title.len())
            .field(
                "description",
                &self.description.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "description_bytes",
                &self.description.as_ref().map(String::len),
            )
            .field("summary", &self.summary.as_ref().map(|_| "<redacted>"))
            .field("summary_bytes", &self.summary.as_ref().map(String::len))
            .finish()
    }
}

/// Typed, deterministic read command for a retained candidate.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "read_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OneHopCanonicalRead {
    /// Read one Project View object and compare its object revision.
    ProjectView {
        /// Existing Carryforth command.
        command: String,
        /// Object revision represented by this semantic observation.
        expected_object_revision: u64,
    },
    /// Read one Project Document at its exact represented revision.
    Document {
        /// Existing revision-pinned Carryforth command.
        fetch_command: String,
        /// Document revision represented by this semantic observation.
        expected_document_revision: u64,
    },
    /// Read current Meeting metadata, board, or speech through existing commands.
    Meeting {
        /// Existing Meeting metadata command.
        metadata: String,
        /// Existing Meeting Board command.
        board: String,
        /// Existing bounded Meeting speech/history command.
        speech: String,
        /// Create Event represented by this observation.
        expected_create_event_id: Digest32,
        /// Terminal End Event represented by this observation when present.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_optional_non_null"
        )]
        expected_end_event_id: Option<Digest32>,
    },
}

impl std::fmt::Debug for OneHopCanonicalRead {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ProjectView { .. } => "ProjectView(<redacted-command>)",
            Self::Document { .. } => "Document(<redacted-command>)",
            Self::Meeting { .. } => "Meeting(<redacted-commands>)",
        })
    }
}

/// Canonical lightweight observation attached to one ranked candidate.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OneHopCanonicalCandidateObservation {
    /// Typed canonical source basis.
    pub source_basis: SemanticSourceBasis,
    /// Current source invalidation epoch.
    pub source_invalidation_epoch: u64,
    /// Exact semantic source snapshot digest.
    pub source_snapshot_digest: Digest32,
    /// Cross-family current lifecycle.
    pub lifecycle: SemanticLifecycleClass,
    /// Optional current source-native status.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub source_status: Option<String>,
    /// Canonical candidate content available for immediate context filtering.
    pub preview: OneHopCandidatePreview,
    /// Deterministic on-demand full-source read entry.
    pub canonical_read: OneHopCanonicalRead,
}

impl std::fmt::Debug for OneHopCanonicalCandidateObservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OneHopCanonicalCandidateObservation")
            .field("source_basis", &self.source_basis)
            .field("source_invalidation_epoch", &self.source_invalidation_epoch)
            .field("source_snapshot_digest", &self.source_snapshot_digest)
            .field("lifecycle", &self.lifecycle)
            .field(
                "source_status",
                &self.source_status.as_ref().map(|_| "<redacted>"),
            )
            .field("preview", &self.preview)
            .field("canonical_read", &self.canonical_read)
            .finish()
    }
}

impl OneHopCanonicalCandidateObservation {
    fn validate_common(&self) -> OneHopSemanticResult<()> {
        if self.source_invalidation_epoch == 0 {
            return invalid("candidate source invalidation epoch must be positive");
        }
        if matches!(
            self.lifecycle,
            SemanticLifecycleClass::Tombstone | SemanticLifecycleClass::Deleted
        ) {
            return invalid("ranked candidate lifecycle must remain readable");
        }
        validate_required_text("candidate.preview.title", &self.preview.title)?;
        validate_optional_text(
            "candidate.preview.description",
            self.preview.description.as_deref(),
        )?;
        validate_optional_text("candidate.preview.summary", self.preview.summary.as_deref())?;
        validate_optional_text("candidate.source_status", self.source_status.as_deref())
    }

    fn validate_for_document(&self, document_id: Uuid, revision: u64) -> OneHopSemanticResult<()> {
        self.validate_common()?;
        if revision == 0 || revision > MAX_SAFE_REVISION {
            return invalid("candidate Document revision is out of range");
        }
        let SemanticSourceBasis::ProjectDocument(basis) = &self.source_basis else {
            return invalid("Document candidate must carry Project Document source basis");
        };
        if self.preview.description.is_some() {
            return invalid("Project Document candidate must not invent a description");
        }
        if basis.document_revision != revision {
            return invalid("Document candidate revision and source basis disagree");
        }
        let OneHopCanonicalRead::Document {
            fetch_command,
            expected_document_revision,
        } = &self.canonical_read
        else {
            return invalid("Document candidate must carry a Document read descriptor");
        };
        let expected =
            format!("cf documents get {document_id} --revision {revision} --content-only");
        if *expected_document_revision != revision || fetch_command != &expected {
            return invalid("Document read descriptor does not match candidate basis");
        }
        Ok(())
    }

    fn validate_for_coordinate(
        &self,
        coordinate: &ProjectContextCoordinate,
    ) -> OneHopSemanticResult<()> {
        self.validate_common()?;
        match (coordinate, &self.source_basis, &self.canonical_read) {
            (
                ProjectContextCoordinate::ProjectViewObject {
                    object_type,
                    object_id,
                },
                SemanticSourceBasis::ProjectView(basis),
                OneHopCanonicalRead::ProjectView {
                    command,
                    expected_object_revision,
                },
            ) if basis.object_revision == *expected_object_revision
                && basis.schema_version > 0
                && basis.object_revision > 0
                && basis.object_revision <= MAX_SAFE_REVISION
                && command
                    == &format!(
                        "cf project-view get-object {} {object_id}",
                        object_type.as_str()
                    ) =>
            {
                Ok(())
            }
            (
                ProjectContextCoordinate::Document { document_id },
                SemanticSourceBasis::ProjectDocument(basis),
                OneHopCanonicalRead::Document {
                    fetch_command,
                    expected_document_revision,
                },
            ) if basis.document_revision == *expected_document_revision
                && basis.document_revision > 0
                && basis.document_revision <= MAX_SAFE_REVISION
                && fetch_command
                    == &format!(
                        "cf documents get {document_id} --revision {} --content-only",
                        basis.document_revision
                    ) =>
            {
                Ok(())
            }
            (
                ProjectContextCoordinate::Meeting { meeting_id },
                SemanticSourceBasis::Meeting(basis),
                OneHopCanonicalRead::Meeting {
                    metadata,
                    board,
                    speech,
                    expected_create_event_id,
                    expected_end_event_id,
                },
            ) if expected_create_event_id == &basis.create_event_id
                && expected_end_event_id == &basis.end_event_id
                && metadata == &format!("cf meetings show --meeting {meeting_id}")
                && board == &format!("cf meetings board get --meeting {meeting_id}")
                && speech
                    == &format!(
                        "cf --format compact meetings history --meeting {meeting_id} --limit 200"
                    ) =>
            {
                Ok(())
            }
            _ => invalid("candidate source basis/read descriptor does not match Coordinate"),
        }
    }
}

/// Mutually exclusive reasons an in-scope semantic candidate was unscorable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OneHopOmittedCandidateCounts {
    /// Canonical source was not found.
    pub source_not_found: u32,
    /// Canonical source was tombstoned or deleted.
    pub source_tombstoned_or_deleted: u32,
    /// Canonical source was ineligible or unreadable.
    pub source_ineligible_or_unreadable: u32,
    /// No current semantic head exists.
    pub semantic_head_missing: u32,
    /// The current semantic head is still building.
    pub semantic_head_building: u32,
    /// The current semantic head failed or is unsupported.
    pub semantic_head_failed_or_unsupported: u32,
    /// The source produced no queryable non-zero vector.
    pub non_queryable_zero_vector: u32,
}

impl OneHopOmittedCandidateCounts {
    fn total(self) -> OneHopSemanticResult<u32> {
        [
            self.source_not_found,
            self.source_tombstoned_or_deleted,
            self.source_ineligible_or_unreadable,
            self.semantic_head_missing,
            self.semantic_head_building,
            self.semantic_head_failed_or_unsupported,
            self.non_queryable_zero_vector,
        ]
        .into_iter()
        .try_fold(0_u32, |total, value| total.checked_add(value))
        .ok_or_else(|| OneHopSemanticError::InvalidState("coverage count overflow".to_owned()))
    }
}

/// Coordinate-to-Edge candidate-pool coverage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncidentEdgeCoverage {
    /// Current active Edges incident to the input Coordinate.
    pub active_incident_edges: u32,
    /// Unique active `(Edge, Document)` relation bindings.
    pub active_relation_bindings: u32,
    /// Relation bindings with a current scorable semantic head.
    pub scorable_relation_bindings: u32,
    /// Unique Edges with at least one scorable relation binding.
    pub scorable_edges: u32,
    /// Scorable bindings whose current overview contains title only.
    pub title_only_scorable_bindings: u32,
    /// Closed mutually exclusive omission counts.
    pub omitted_relation_bindings: OneHopOmittedCandidateCounts,
}

/// Edge-to-Coordinate candidate-pool coverage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeCoordinateCoverage {
    /// Complete member count of the current active Edge.
    pub edge_coordinate_count: u32,
    /// Complete members matching the applied v2 type filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_matched_coordinate_count: Option<u32>,
    /// Complete members excluded only because their Coordinate type differed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_filtered_out_coordinates: Option<u32>,
    /// Members in the active result pool with a current scorable semantic head.
    ///
    /// The pool is the complete Edge for v1 and the type-matched partition for v2.
    pub scorable_coordinates: u32,
    /// Scorable members in that same pool whose current overview contains title only.
    pub title_only_scorable_coordinates: u32,
    /// Closed mutually exclusive omission counts.
    pub omitted_coordinates: OneHopOmittedCandidateCounts,
}

/// One relation Document retained inside a ranked incident Edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OneHopRankedDocument {
    /// One-based rank inside its Edge.
    pub rank: u8,
    /// Stable Project Document identity.
    pub document_id: Uuid,
    /// Exact represented Document revision.
    pub document_revision: u64,
    /// Direct fixed-point Q0 cosine score.
    pub score: Score,
    /// Current canonical preview, provenance, and read descriptor.
    pub canonical_observation: OneHopCanonicalCandidateObservation,
}

/// One incident Edge ranked through its best relation Document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OneHopRankedEdge {
    /// One-based rank in the returned Edge list.
    pub rank: u8,
    /// Exact current Edge identity.
    pub edge_key: EdgeKey,
    /// Score of the first ranked relation Document.
    pub score: Score,
    /// At most three highest-ranked relation Documents.
    pub ranked_documents: Vec<OneHopRankedDocument>,
    /// All current active Document bindings on this Edge.
    pub binding_document_count: u32,
    /// Current bindings with a scorable semantic head.
    pub scorable_document_count: u32,
    /// Whether more than three scorable Documents exist on this Edge.
    pub documents_truncated: bool,
}

/// One current Edge member ranked directly against Q0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OneHopRankedCoordinate {
    /// One-based rank in the returned Coordinate list.
    pub rank: u8,
    /// Current complete Edge member identity.
    pub coordinate: ProjectContextCoordinate,
    /// Direct fixed-point Q0 cosine score.
    pub score: Score,
    /// Current canonical preview, provenance, and read descriptor.
    pub canonical_observation: OneHopCanonicalCandidateObservation,
}

/// Closed result union matching the exact request scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "selection_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OneHopSemanticSelection {
    /// Ranked incident Edges; never contains Edge member Coordinates.
    IncidentEdges {
        /// Input Coordinate echoed from the request.
        coordinate: ProjectContextCoordinate,
        /// Ranked Edge selections.
        edges: Vec<OneHopRankedEdge>,
        /// Complete candidate-pool coverage.
        coverage: IncidentEdgeCoverage,
        /// Whether at least one more scorable Edge existed.
        truncated: bool,
    },
    /// Ranked members of one Edge; never contains relation Documents.
    EdgeCoordinates {
        /// Input Edge identity echoed from the request.
        edge_key: EdgeKey,
        /// Applied v2 Coordinate type scope; omitted only for v1 results.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        coordinate_types: Option<ProjectContextCoordinateTypeFilter>,
        /// Ranked current member selections.
        ranked_coordinates: Vec<OneHopRankedCoordinate>,
        /// Complete candidate-pool coverage.
        coverage: EdgeCoordinateCoverage,
        /// Whether at least one more scorable member existed.
        truncated: bool,
    },
}

/// Complete Relay-signed one-hop semantic result content.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextOneHopSemanticQueryResult {
    /// Request correlation identity.
    pub request_id: Uuid,
    /// Host-derived Project identity.
    pub project_id: Uuid,
    /// Authenticated exact-request binding digest.
    pub request_binding_digest: Digest32,
    /// Generation and Project Context snapshot observations.
    pub observations: OneHopSemanticObservations,
    /// Exact scope-specific result.
    pub selection: OneHopSemanticSelection,
}

impl std::fmt::Debug for ProjectContextOneHopSemanticQueryResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (selection_type, candidate_count) = match &self.selection {
            OneHopSemanticSelection::IncidentEdges { edges, .. } => ("incident_edges", edges.len()),
            OneHopSemanticSelection::EdgeCoordinates {
                ranked_coordinates, ..
            } => ("edge_coordinates", ranked_coordinates.len()),
        };
        formatter
            .debug_struct("ProjectContextOneHopSemanticQueryResult")
            .field("request_id", &self.request_id)
            .field("selection_type", &selection_type)
            .field("candidate_count", &candidate_count)
            .finish_non_exhaustive()
    }
}

impl ProjectContextOneHopSemanticQueryResult {
    /// Validate identities, observations, ordering, coverage, and closed reads.
    pub fn validate(&self) -> OneHopSemanticResult<()> {
        validate_uuid_v4(self.request_id, "result.request_id")?;
        validate_uuid_v4(self.project_id, "result.project_id")?;
        validate_uuid_v4(
            self.observations.semantic_generation_id,
            "result.observations.semantic_generation_id",
        )?;
        if self.observations.query_contract_digest != query_contract_digest() {
            return invalid("result Q0 query contract digest mismatch");
        }
        if self.observations.projection_generation == 0
            || self.observations.projection_generation > MAX_SAFE_REVISION
            || self.observations.project_context_revision == 0
            || self.observations.project_context_revision > MAX_SAFE_REVISION
        {
            return invalid("result graph observations are out of range");
        }
        let expected_ranking = match &self.selection {
            OneHopSemanticSelection::IncidentEdges { .. } => {
                self.validate_incident_edges()?;
                incident_edge_ranking_contract_digest()
            }
            OneHopSemanticSelection::EdgeCoordinates {
                coordinate_types, ..
            } => {
                self.validate_edge_coordinates()?;
                if coordinate_types.is_some() {
                    edge_coordinate_filtered_ranking_contract_digest()
                } else {
                    edge_coordinate_ranking_contract_digest()
                }
            }
        };
        if self.observations.ranking_contract_digest != expected_ranking {
            return invalid("result ranking contract digest mismatch");
        }
        Ok(())
    }

    /// Validate this result against the exact canonical request.
    pub fn validate_for_request(
        &self,
        request: &ProjectContextOneHopSemanticQuery,
    ) -> OneHopSemanticResult<()> {
        let request = request.clone().validate_and_canonicalize()?;
        self.validate()?;
        if self.request_id != request.request_id || self.project_id != request.project_id {
            return invalid("result does not belong to the request");
        }
        match (&request.scope, &self.selection) {
            (
                OneHopSemanticScope::IncidentEdges {
                    coordinate: expected,
                },
                OneHopSemanticSelection::IncidentEdges {
                    coordinate,
                    edges,
                    truncated,
                    ..
                },
            ) if coordinate == expected => {
                validate_limit(edges.len(), *truncated, request.limit)?;
            }
            (
                OneHopSemanticScope::EdgeCoordinates {
                    edge_key: expected,
                    coordinate_types: expected_types,
                },
                OneHopSemanticSelection::EdgeCoordinates {
                    edge_key,
                    coordinate_types,
                    ranked_coordinates,
                    truncated,
                    ..
                },
            ) if edge_key == expected && coordinate_types == expected_types => {
                validate_limit(ranked_coordinates.len(), *truncated, request.limit)?;
            }
            _ => return invalid("result scope variant or identity does not match request"),
        }
        Ok(())
    }

    fn validate_incident_edges(&self) -> OneHopSemanticResult<()> {
        let OneHopSemanticSelection::IncidentEdges {
            coordinate,
            edges,
            coverage,
            truncated,
        } = &self.selection
        else {
            return invalid("expected incident Edge selection");
        };
        coordinate
            .validate_for_project(self.project_id)
            .map_err(|error| OneHopSemanticError::InvalidCoordinate(error.to_string()))?;
        if coverage.active_incident_edges > MAX_ONE_HOP_INCIDENT_EDGES
            || coverage.active_relation_bindings > MAX_ONE_HOP_RELATION_BINDINGS
            || coverage.scorable_relation_bindings > coverage.active_relation_bindings
            || coverage.scorable_edges > coverage.active_incident_edges
            || coverage.title_only_scorable_bindings > coverage.scorable_relation_bindings
            || coverage
                .scorable_relation_bindings
                .checked_add(coverage.omitted_relation_bindings.total()?)
                != Some(coverage.active_relation_bindings)
        {
            return invalid("incident Edge coverage is inconsistent");
        }
        validate_count_against_pool(edges.len(), *truncated, coverage.scorable_edges)?;
        let mut seen_edges = BTreeSet::new();
        for (index, edge) in edges.iter().enumerate() {
            validate_rank(index, edge.rank, "Edge")?;
            if !seen_edges.insert(edge.edge_key) {
                return invalid("incident Edge candidates must be unique");
            }
            if edge.ranked_documents.is_empty()
                || edge.ranked_documents.len() > MAX_ONE_HOP_DOCUMENTS_PER_EDGE
                || edge.binding_document_count < edge.scorable_document_count
                || edge.scorable_document_count
                    < u32::try_from(edge.ranked_documents.len())
                        .map_err(|_| OneHopSemanticError::Serialization)?
                || edge.documents_truncated
                    != (edge.scorable_document_count
                        > u32::try_from(MAX_ONE_HOP_DOCUMENTS_PER_EDGE)
                            .map_err(|_| OneHopSemanticError::Serialization)?)
                || edge.ranked_documents.len()
                    != usize::try_from(edge.scorable_document_count)
                        .unwrap_or(usize::MAX)
                        .min(MAX_ONE_HOP_DOCUMENTS_PER_EDGE)
            {
                return invalid("ranked Edge Document counts are inconsistent");
            }
            let mut seen_documents = BTreeSet::new();
            for (document_index, document) in edge.ranked_documents.iter().enumerate() {
                validate_rank(document_index, document.rank, "Document")?;
                validate_uuid_v4(document.document_id, "result.document_id")?;
                if !seen_documents.insert(document.document_id) {
                    return invalid("ranked Documents must be unique within an Edge");
                }
                document
                    .canonical_observation
                    .validate_for_document(document.document_id, document.document_revision)?;
            }
            if edge.score != edge.ranked_documents[0].score
                || edge
                    .ranked_documents
                    .windows(2)
                    .any(|pair| document_order(&pair[0], &pair[1]) == Ordering::Greater)
            {
                return invalid("ranked Edge Documents are not canonically score-sorted");
            }
        }
        if edges
            .windows(2)
            .any(|pair| edge_order(&pair[0], &pair[1]) == Ordering::Greater)
        {
            return invalid("incident Edges are not canonically score-sorted");
        }
        Ok(())
    }

    fn validate_edge_coordinates(&self) -> OneHopSemanticResult<()> {
        let OneHopSemanticSelection::EdgeCoordinates {
            coordinate_types,
            ranked_coordinates,
            coverage,
            truncated,
            ..
        } = &self.selection
        else {
            return invalid("expected Edge Coordinate selection");
        };
        if coverage.edge_coordinate_count > MAX_ONE_HOP_EDGE_COORDINATES
            || coverage.title_only_scorable_coordinates > coverage.scorable_coordinates
        {
            return invalid("Edge Coordinate coverage is inconsistent");
        }
        match coordinate_types {
            None => {
                if coverage.type_matched_coordinate_count.is_some()
                    || coverage.type_filtered_out_coordinates.is_some()
                    || coverage.scorable_coordinates > coverage.edge_coordinate_count
                    || coverage
                        .scorable_coordinates
                        .checked_add(coverage.omitted_coordinates.total()?)
                        != Some(coverage.edge_coordinate_count)
                {
                    return invalid("v1 Edge Coordinate coverage is inconsistent");
                }
            }
            Some(filter) => {
                if !filter.is_canonical() {
                    return invalid("filtered Edge Coordinate types are not canonical");
                }
                let (Some(type_matched), Some(type_filtered_out)) = (
                    coverage.type_matched_coordinate_count,
                    coverage.type_filtered_out_coordinates,
                ) else {
                    return invalid("filtered Edge Coordinate coverage is incomplete");
                };
                if type_matched.checked_add(type_filtered_out)
                    != Some(coverage.edge_coordinate_count)
                    || coverage.scorable_coordinates > type_matched
                    || coverage
                        .scorable_coordinates
                        .checked_add(coverage.omitted_coordinates.total()?)
                        != Some(type_matched)
                {
                    return invalid("filtered Edge Coordinate coverage is inconsistent");
                }
            }
        }
        validate_count_against_pool(
            ranked_coordinates.len(),
            *truncated,
            coverage.scorable_coordinates,
        )?;
        let mut seen_coordinates = BTreeSet::new();
        for (index, candidate) in ranked_coordinates.iter().enumerate() {
            validate_rank(index, candidate.rank, "Coordinate")?;
            candidate
                .coordinate
                .validate_for_project(self.project_id)
                .map_err(|error| OneHopSemanticError::InvalidCoordinate(error.to_string()))?;
            if !seen_coordinates.insert(candidate.coordinate.clone()) {
                return invalid("ranked Coordinates must be unique");
            }
            if coordinate_types
                .as_ref()
                .is_some_and(|filter| !filter.matches(&candidate.coordinate))
            {
                return invalid("ranked Coordinate violates the applied type filter");
            }
            candidate
                .canonical_observation
                .validate_for_coordinate(&candidate.coordinate)?;
        }
        if ranked_coordinates
            .windows(2)
            .any(|pair| coordinate_order(&pair[0], &pair[1]) == Ordering::Greater)
        {
            return invalid("ranked Coordinates are not canonically score-sorted");
        }
        Ok(())
    }
}

fn validate_limit(count: usize, truncated: bool, limit: u8) -> OneHopSemanticResult<()> {
    if count > usize::from(limit) || (truncated && count != usize::from(limit)) {
        return invalid("result candidate count exceeds the request limit");
    }
    Ok(())
}

fn validate_count_against_pool(
    count: usize,
    truncated: bool,
    scorable: u32,
) -> OneHopSemanticResult<()> {
    let count = u32::try_from(count).map_err(|_| OneHopSemanticError::Serialization)?;
    if count > u32::from(MAX_ONE_HOP_SEMANTIC_LIMIT)
        || count > scorable
        || truncated != (scorable > count)
    {
        return invalid("result truncation does not match the scorable candidate pool");
    }
    Ok(())
}

fn validate_rank(index: usize, rank: u8, kind: &str) -> OneHopSemanticResult<()> {
    let expected = u8::try_from(index + 1).map_err(|_| OneHopSemanticError::Serialization)?;
    if rank != expected {
        return invalid(format!("{kind} ranks must be consecutive and one-based"));
    }
    Ok(())
}

fn validate_required_text(field: &'static str, value: &str) -> OneHopSemanticResult<()> {
    if value.as_bytes().contains(&0) {
        return Err(OneHopSemanticError::NulText { field });
    }
    if value.trim().is_empty() {
        return invalid(format!("{field} must not be blank"));
    }
    Ok(())
}

fn validate_optional_text(field: &'static str, value: Option<&str>) -> OneHopSemanticResult<()> {
    if let Some(value) = value {
        validate_required_text(field, value)?;
    }
    Ok(())
}

fn deserialize_optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn validate_uuid_v4(value: Uuid, field: &'static str) -> OneHopSemanticResult<()> {
    if value.is_nil() || value.get_version_num() != 4 {
        return Err(OneHopSemanticError::InvalidUuid { field });
    }
    Ok(())
}

fn edge_order(left: &OneHopRankedEdge, right: &OneHopRankedEdge) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| left.edge_key.cmp(&right.edge_key))
}

fn document_order(left: &OneHopRankedDocument, right: &OneHopRankedDocument) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| left.document_id.cmp(&right.document_id))
}

fn coordinate_order(left: &OneHopRankedCoordinate, right: &OneHopRankedCoordinate) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| left.coordinate.cmp(&right.coordinate))
}

fn invalid<T>(reason: impl Into<String>) -> OneHopSemanticResult<T> {
    Err(OneHopSemanticError::InvalidState(reason.into()))
}

fn hash_domain(domain: &[u8], parts: &[&[u8]]) -> Digest32 {
    let mut hasher = Sha256::new();
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain);
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    Digest32::from_bytes(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use buzz_project_view::ProjectViewObjectType;
    use buzz_semantic::{ProjectDocumentSourceBasis, ProjectViewSourceBasis};

    use crate::ProjectContextCoordinateType;

    use super::*;

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_0000_0000 | value)
    }

    fn digest(value: u8) -> Digest32 {
        Digest32::from_bytes([value; 32])
    }

    fn work(value: u128) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: uuid(value),
        }
    }

    fn edge(project_id: Uuid, left: u128, right: u128) -> EdgeKey {
        let mut coordinates = vec![work(left), work(right)];
        coordinates.sort();
        EdgeKey::derive(project_id, &coordinates).expect("edge")
    }

    fn request(scope: OneHopSemanticScope) -> ProjectContextOneHopSemanticQuery {
        ProjectContextOneHopSemanticQuery {
            request_id: uuid(1),
            project_id: uuid(2),
            query: " client authorization context ".to_owned(),
            limit: DEFAULT_ONE_HOP_SEMANTIC_LIMIT,
            scope,
        }
    }

    fn observations(ranking_contract_digest: Digest32) -> OneHopSemanticObservations {
        OneHopSemanticObservations {
            semantic_generation_id: uuid(10),
            source_generation_contract_digest: digest(11),
            embedding_space_fence: digest(12),
            query_contract_digest: query_contract_digest(),
            ranking_contract_digest,
            projection_generation: 3,
            project_context_revision: 4,
            snapshot_observed_at: Utc::now(),
        }
    }

    fn document_observation(
        document_id: Uuid,
        revision: u64,
    ) -> OneHopCanonicalCandidateObservation {
        OneHopCanonicalCandidateObservation {
            source_basis: SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                document_revision: revision,
                source_change_id: digest(20),
            }),
            source_invalidation_epoch: 2,
            source_snapshot_digest: digest(21),
            lifecycle: SemanticLifecycleClass::Active,
            source_status: Some("active".to_owned()),
            preview: OneHopCandidatePreview {
                title: "Authorization relation".to_owned(),
                description: None,
                summary: Some("Client-side authorization evidence".to_owned()),
            },
            canonical_read: OneHopCanonicalRead::Document {
                fetch_command: format!(
                    "cf documents get {document_id} --revision {revision} --content-only"
                ),
                expected_document_revision: revision,
            },
        }
    }

    fn work_observation(
        coordinate: &ProjectContextCoordinate,
    ) -> OneHopCanonicalCandidateObservation {
        let ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        } = coordinate
        else {
            panic!("work Coordinate")
        };
        OneHopCanonicalCandidateObservation {
            source_basis: SemanticSourceBasis::ProjectView(ProjectViewSourceBasis {
                schema_version: 3,
                object_revision: 7,
                source_change_id: digest(30),
            }),
            source_invalidation_epoch: 3,
            source_snapshot_digest: digest(31),
            lifecycle: SemanticLifecycleClass::Active,
            source_status: Some("active".to_owned()),
            preview: OneHopCandidatePreview {
                title: "Authorization UI".to_owned(),
                description: Some("Client implementation".to_owned()),
                summary: Some("Disclosure-safe errors".to_owned()),
            },
            canonical_read: OneHopCanonicalRead::ProjectView {
                command: format!(
                    "cf project-view get-object {} {object_id}",
                    object_type.as_str()
                ),
                expected_object_revision: 7,
            },
        }
    }

    #[test]
    fn request_is_closed_trimmed_redacted_and_q0_compatible() {
        let unfiltered_request = request(OneHopSemanticScope::IncidentEdges {
            coordinate: work(3),
        });
        let canonical = unfiltered_request
            .clone()
            .validate_and_canonicalize()
            .expect("request");
        assert_eq!(canonical.query, "client authorization context");
        assert!(!format!("{canonical:?}").contains("authorization"));
        let input = build_one_hop_semantic_query_encoder_input(&canonical).expect("Q0");
        assert_eq!(input.query_contract_digest(), query_contract_digest());
        assert_eq!(
            input.channel_kind(),
            &crate::SemanticQueryChannelKind::Problem
        );

        let filtered = request(OneHopSemanticScope::EdgeCoordinates {
            edge_key: edge(canonical.project_id, 3, 4),
            coordinate_types: Some(
                ProjectContextCoordinateTypeFilter::new(vec![ProjectContextCoordinateType::Work])
                    .expect("filter"),
            ),
        })
        .validate_and_canonicalize()
        .expect("filtered request");
        let filtered_input =
            build_one_hop_semantic_query_encoder_input(&filtered).expect("filtered Q0");
        assert_eq!(filtered_input.text(), input.text());
        assert_eq!(filtered_input.text_digest(), input.text_digest());

        let mut value = serde_json::to_value(&canonical).expect("json");
        value["unexpected"] = serde_json::json!(true);
        assert!(matches!(
            ProjectContextOneHopSemanticQuery::parse_json(
                &serde_json::to_vec(&value).expect("bytes")
            ),
            Err(OneHopSemanticError::InvalidJson(_))
        ));
    }

    #[test]
    fn request_rejects_query_and_limit_boundaries() {
        let scope = OneHopSemanticScope::IncidentEdges {
            coordinate: work(3),
        };
        let mut blank = request(scope.clone());
        blank.query = "  ".to_owned();
        assert_eq!(
            blank.validate_and_canonicalize(),
            Err(OneHopSemanticError::BlankQuery)
        );
        let mut nul = request(scope.clone());
        nul.query = "a\0b".to_owned();
        assert_eq!(
            nul.validate_and_canonicalize(),
            Err(OneHopSemanticError::NulText { field: "query" })
        );
        let mut too_many = request(scope);
        too_many.limit = MAX_ONE_HOP_SEMANTIC_LIMIT + 1;
        assert_eq!(
            too_many.validate_and_canonicalize(),
            Err(OneHopSemanticError::InvalidLimit {
                observed: MAX_ONE_HOP_SEMANTIC_LIMIT + 1,
                maximum: MAX_ONE_HOP_SEMANTIC_LIMIT,
            })
        );
    }

    #[test]
    fn incident_result_validates_preview_order_and_scope_without_coordinates() {
        let request = request(OneHopSemanticScope::IncidentEdges {
            coordinate: work(3),
        })
        .validate_and_canonicalize()
        .expect("request");
        let document_id = uuid(40);
        let ranked_document = OneHopRankedDocument {
            rank: 1,
            document_id,
            document_revision: 7,
            score: Score::new(863_300).expect("score"),
            canonical_observation: document_observation(document_id, 7),
        };
        let result = ProjectContextOneHopSemanticQueryResult {
            request_id: request.request_id,
            project_id: request.project_id,
            request_binding_digest: digest(41),
            observations: observations(incident_edge_ranking_contract_digest()),
            selection: OneHopSemanticSelection::IncidentEdges {
                coordinate: work(3),
                edges: vec![OneHopRankedEdge {
                    rank: 1,
                    edge_key: edge(request.project_id, 3, 4),
                    score: ranked_document.score,
                    ranked_documents: vec![ranked_document],
                    binding_document_count: 1,
                    scorable_document_count: 1,
                    documents_truncated: false,
                }],
                coverage: IncidentEdgeCoverage {
                    active_incident_edges: 1,
                    active_relation_bindings: 1,
                    scorable_relation_bindings: 1,
                    scorable_edges: 1,
                    title_only_scorable_bindings: 0,
                    omitted_relation_bindings: OneHopOmittedCandidateCounts::default(),
                },
                truncated: false,
            },
        };
        result.validate_for_request(&request).expect("result");
        let json = serde_json::to_value(&result).expect("json");
        assert_eq!(
            json["selection"]["edges"][0]["ranked_documents"][0]["canonical_observation"]
                ["preview"]["title"],
            "Authorization relation"
        );
        assert!(
            json["selection"]["edges"][0]["ranked_documents"][0]["canonical_observation"]
                ["preview"]
                .get("description")
                .is_none()
        );
        assert!(json["selection"]["edges"][0].get("coordinates").is_none());

        let mut forged_null = json;
        forged_null["selection"]["edges"][0]["ranked_documents"][0]["canonical_observation"]
            ["preview"]["description"] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<ProjectContextOneHopSemanticQueryResult>(forged_null).is_err()
        );
    }

    #[test]
    fn edge_coordinate_result_validates_preview_and_read_descriptor() {
        let project_id = uuid(2);
        let edge_key = edge(project_id, 3, 4);
        let unfiltered_request = request(OneHopSemanticScope::EdgeCoordinates {
            edge_key,
            coordinate_types: None,
        })
        .validate_and_canonicalize()
        .expect("request");
        let coordinate = work(3);
        let result = ProjectContextOneHopSemanticQueryResult {
            request_id: unfiltered_request.request_id,
            project_id,
            request_binding_digest: digest(41),
            observations: observations(edge_coordinate_ranking_contract_digest()),
            selection: OneHopSemanticSelection::EdgeCoordinates {
                edge_key,
                coordinate_types: None,
                ranked_coordinates: vec![OneHopRankedCoordinate {
                    rank: 1,
                    coordinate: coordinate.clone(),
                    score: Score::new(841_230).expect("score"),
                    canonical_observation: work_observation(&coordinate),
                }],
                coverage: EdgeCoordinateCoverage {
                    edge_coordinate_count: 1,
                    type_matched_coordinate_count: None,
                    type_filtered_out_coordinates: None,
                    scorable_coordinates: 1,
                    title_only_scorable_coordinates: 0,
                    omitted_coordinates: OneHopOmittedCandidateCounts::default(),
                },
                truncated: false,
            },
        };
        result
            .validate_for_request(&unfiltered_request)
            .expect("result");
        let json = serde_json::to_value(&result).expect("json");
        assert_eq!(
            json["selection"]["ranked_coordinates"][0]["canonical_observation"]["preview"]
                ["description"],
            "Client implementation"
        );
        assert!(json["selection"].get("ranked_documents").is_none());

        let filter =
            ProjectContextCoordinateTypeFilter::new(vec![ProjectContextCoordinateType::Work])
                .expect("filter");
        let filtered_request = request(OneHopSemanticScope::EdgeCoordinates {
            edge_key,
            coordinate_types: Some(filter.clone()),
        })
        .validate_and_canonicalize()
        .expect("filtered request");
        let mut filtered_result = result;
        filtered_result.request_id = filtered_request.request_id;
        filtered_result.observations.ranking_contract_digest =
            edge_coordinate_filtered_ranking_contract_digest();
        let OneHopSemanticSelection::EdgeCoordinates {
            coordinate_types,
            coverage,
            ..
        } = &mut filtered_result.selection
        else {
            panic!("Edge Coordinate selection")
        };
        *coordinate_types = Some(filter);
        coverage.type_matched_coordinate_count = Some(1);
        coverage.type_filtered_out_coordinates = Some(0);
        filtered_result
            .validate_for_request(&filtered_request)
            .expect("filtered result");
        let OneHopSemanticSelection::EdgeCoordinates { coverage, .. } =
            &mut filtered_result.selection
        else {
            panic!("Edge Coordinate selection")
        };
        coverage.type_filtered_out_coordinates = Some(1);
        assert!(filtered_result
            .validate_for_request(&filtered_request)
            .is_err());
    }

    #[test]
    fn result_rejects_identity_only_and_forged_read_descriptor() {
        let coordinate = work(3);
        let mut observation = work_observation(&coordinate);
        let OneHopCanonicalRead::ProjectView { command, .. } = &mut observation.canonical_read
        else {
            panic!("Project View read")
        };
        *command =
            "cf project-view get-object work 00000000-0000-0000-0000-000000000000".to_owned();
        assert!(observation.validate_for_coordinate(&coordinate).is_err());

        let mut empty = work_observation(&coordinate);
        empty.preview.title.clear();
        assert!(empty.validate_for_coordinate(&coordinate).is_err());
    }

    #[test]
    fn ranking_contracts_are_distinct_and_stable() {
        assert_ne!(
            incident_edge_ranking_contract_digest(),
            edge_coordinate_ranking_contract_digest()
        );
        assert_ne!(
            edge_coordinate_ranking_contract_digest(),
            edge_coordinate_filtered_ranking_contract_digest()
        );
        assert_eq!(
            incident_edge_ranking_contract_digest(),
            incident_edge_ranking_contract_digest()
        );
    }

    #[test]
    fn http_binding_covers_project_caller_relay_auth_and_every_body_byte() {
        let body = br#"[{"kinds":[40914]}]"#;
        let binding = derive_one_hop_semantic_http_request_binding(
            uuid(2),
            &[3; 32],
            &[4; 32],
            digest(5),
            body,
        )
        .expect("binding");
        verify_one_hop_semantic_http_request_binding(
            binding,
            uuid(2),
            &[3; 32],
            &[4; 32],
            digest(5),
            body,
        )
        .expect("binding verifies");
        let v2 = derive_one_hop_semantic_v2_http_request_binding(
            uuid(2),
            &[3; 32],
            &[4; 32],
            digest(5),
            body,
        )
        .expect("v2 binding");
        assert_ne!(binding, v2);
        verify_one_hop_semantic_v2_http_request_binding(
            v2,
            uuid(2),
            &[3; 32],
            &[4; 32],
            digest(5),
            body,
        )
        .expect("v2 binding verifies");

        for changed in [
            derive_one_hop_semantic_http_request_binding(
                uuid(6),
                &[3; 32],
                &[4; 32],
                digest(5),
                body,
            ),
            derive_one_hop_semantic_http_request_binding(
                uuid(2),
                &[6; 32],
                &[4; 32],
                digest(5),
                body,
            ),
            derive_one_hop_semantic_http_request_binding(
                uuid(2),
                &[3; 32],
                &[6; 32],
                digest(5),
                body,
            ),
            derive_one_hop_semantic_http_request_binding(
                uuid(2),
                &[3; 32],
                &[4; 32],
                digest(6),
                body,
            ),
            derive_one_hop_semantic_http_request_binding(
                uuid(2),
                &[3; 32],
                &[4; 32],
                digest(5),
                br#"[{"kinds":[40914]} ]"#,
            ),
        ] {
            assert_ne!(binding, changed.expect("changed binding"));
        }
    }
}
