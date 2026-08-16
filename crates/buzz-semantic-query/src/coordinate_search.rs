//! Closed contracts for natural-language Project Context Coordinate search.
//!
//! This module deliberately stops before graph root selection or traversal.
//! It maps one natural-language query to a bounded list of current indexed
//! Coordinate candidates and contains no database, network, or authorization
//! work.

use std::{cmp::Ordering, collections::BTreeSet};

use buzz_project_context::{ProjectContextCoordinate, MAX_SAFE_REVISION};
use buzz_semantic::{Digest32, EmbeddingVector, SemanticModelContract};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    ProjectContextCoordinateTypeFilter, ProviderEncodedSemanticInput, Score, SemanticQueryInput,
    SemanticQueryInputBundle, SemanticQueryInputKind,
};

/// Default number of starting Coordinate candidates.
pub const DEFAULT_COORDINATE_SEARCH_LIMIT: u8 = 8;
/// Hard cap on returned starting Coordinate candidates.
pub const MAX_COORDINATE_SEARCH_LIMIT: u8 = 32;
/// Maximum bytes in the raw closed request object.
pub const MAX_COORDINATE_SEARCH_REQUEST_BYTES: usize = 64 * 1024;
/// Maximum UTF-8 bytes in the trimmed natural-language query.
pub const MAX_COORDINATE_SEARCH_QUERY_BYTES: usize = 16 * 1024;
/// Maximum bytes in one exact Provider input.
pub const MAX_COORDINATE_SEARCH_PROVIDER_INPUT_BYTES: usize = 64 * 1024;
/// Maximum serialized virtual Event-array bytes returned to a caller.
pub const MAX_COORDINATE_SEARCH_RESPONSE_BYTES: usize = 64 * 1024;
/// Absolute server wall-time budget in milliseconds.
pub const MAX_COORDINATE_SEARCH_WALL_TIME_MS: u32 = 45_000;
/// Independently versioned Provider query-text contract.
pub const COORDINATE_SEARCH_QUERY_CONTRACT: &str =
    "carryforth.project-context-coordinate-search.query";

const QUERY_CONTRACT_DESCRIPTOR: &str = concat!(
    "contract=carryforth.project-context-coordinate-search.query\n",
    "field-order=contract,query\n",
    "escape=quote-backslash-short-c0-other-c0-lower-hex\n",
    "unicode=raw-utf8-no-normalization\n",
    "ranking=one-query-vector-direct-cosine-no-floor\n",
    "default-limit=8\n",
    "max-limit=32\n",
    "max-query-bytes=16384\n",
    "max-provider-input-bytes=65536"
);

const REQUEST_BINDING_DOMAIN: &[u8] =
    b"carryforth.project-context-coordinate-search-http-request\0";
const FILTERED_REQUEST_BINDING_DOMAIN: &[u8] =
    b"carryforth.project-context-coordinate-search-v2-http-request\0";

/// Result alias for the closed Coordinate-search contract.
pub type CoordinateSearchResult<T> = Result<T, CoordinateSearchError>;

/// Errors returned by pure Coordinate-search parsing and validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoordinateSearchError {
    /// Raw request object exceeds its public resource boundary.
    #[error("Coordinate search request is {observed} bytes; maximum is {maximum}")]
    RequestTooLarge {
        /// Observed byte count.
        observed: usize,
        /// Maximum accepted byte count.
        maximum: usize,
    },
    /// JSON is malformed or violates the closed schema.
    #[error("invalid Coordinate search JSON: {0}")]
    InvalidJson(String),
    /// A required UUID is nil or not UUIDv4.
    #[error("Coordinate search {field} must be UUIDv4")]
    InvalidUuid {
        /// Rejected field.
        field: &'static str,
    },
    /// The trimmed natural-language query is empty.
    #[error("Coordinate search query must not be blank")]
    BlankQuery,
    /// Query text contains a forbidden NUL byte.
    #[error("Coordinate search query must not contain NUL")]
    NulQuery,
    /// Query text exceeds the public UTF-8 resource boundary.
    #[error("Coordinate search query is {observed} bytes; maximum is {maximum}")]
    QueryTooLarge {
        /// Observed byte count.
        observed: usize,
        /// Maximum accepted byte count.
        maximum: usize,
    },
    /// Requested result limit is outside the closed range.
    #[error("Coordinate search limit {observed} is outside 1..={maximum}")]
    InvalidLimit {
        /// Rejected limit.
        observed: u8,
        /// Maximum accepted limit.
        maximum: u8,
    },
    /// Canonical Provider input exceeds its independent boundary.
    #[error("Coordinate search Provider input is {observed} bytes; maximum is {maximum}")]
    ProviderInputTooLarge {
        /// Observed byte count.
        observed: usize,
        /// Maximum accepted byte count.
        maximum: usize,
    },
    /// A returned Coordinate is invalid for the host-derived Project.
    #[error("invalid Coordinate search candidate: {0}")]
    InvalidCoordinate(String),
    /// A present Coordinate type filter was empty or malformed.
    #[error("invalid Coordinate search type filter: {0}")]
    InvalidCoordinateTypes(String),
    /// A completed result violates a closed invariant.
    #[error("invalid Coordinate search state: {0}")]
    InvalidState(String),
    /// Deterministic serialization failed.
    #[error("Coordinate search serialization failed")]
    Serialization,
}

/// Closed natural-language request for starting Coordinate candidates.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextCoordinateSearchQuery {
    /// Caller-generated request correlation UUIDv4.
    pub request_id: Uuid,
    /// Host-derived Project/Community UUIDv4.
    pub project_id: Uuid,
    /// Natural-language candidate discovery query.
    pub query: String,
    /// Optional closed Coordinate types; omitted preserves the v1 all-type scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinate_types: Option<ProjectContextCoordinateTypeFilter>,
    /// Maximum returned candidates in `1..=32`.
    pub limit: u8,
}

impl std::fmt::Debug for ProjectContextCoordinateSearchQuery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectContextCoordinateSearchQuery")
            .field("request_id", &self.request_id)
            .field("project_id", &self.project_id)
            .field("query", &"<redacted>")
            .field("query_bytes", &self.query.len())
            .field("coordinate_types", &self.coordinate_types)
            .field("limit", &self.limit)
            .finish()
    }
}

impl ProjectContextCoordinateSearchQuery {
    /// Parse a bounded closed JSON request and return its canonical form.
    pub fn parse_json(bytes: &[u8]) -> CoordinateSearchResult<Self> {
        if bytes.len() > MAX_COORDINATE_SEARCH_REQUEST_BYTES {
            return Err(CoordinateSearchError::RequestTooLarge {
                observed: bytes.len(),
                maximum: MAX_COORDINATE_SEARCH_REQUEST_BYTES,
            });
        }
        let request = serde_json::from_slice(bytes)
            .map_err(|error| CoordinateSearchError::InvalidJson(error.to_string()))?;
        Self::validate_and_canonicalize(request)
    }

    /// Validate a trusted in-memory request and trim its natural-language query.
    pub fn validate_and_canonicalize(mut self) -> CoordinateSearchResult<Self> {
        validate_uuid_v4(self.request_id, "request_id")?;
        validate_uuid_v4(self.project_id, "project_id")?;
        let query = self.query.trim();
        if query.is_empty() {
            return Err(CoordinateSearchError::BlankQuery);
        }
        if query.as_bytes().contains(&0) {
            return Err(CoordinateSearchError::NulQuery);
        }
        if query.len() > MAX_COORDINATE_SEARCH_QUERY_BYTES {
            return Err(CoordinateSearchError::QueryTooLarge {
                observed: query.len(),
                maximum: MAX_COORDINATE_SEARCH_QUERY_BYTES,
            });
        }
        if self.limit == 0 || self.limit > MAX_COORDINATE_SEARCH_LIMIT {
            return Err(CoordinateSearchError::InvalidLimit {
                observed: self.limit,
                maximum: MAX_COORDINATE_SEARCH_LIMIT,
            });
        }
        self.coordinate_types = self
            .coordinate_types
            .as_ref()
            .map(ProjectContextCoordinateTypeFilter::canonicalized)
            .transpose()
            .map_err(|error| CoordinateSearchError::InvalidCoordinateTypes(error.to_string()))?;
        self.query = query.to_owned();
        let canonical =
            serde_json::to_vec(&self).map_err(|_| CoordinateSearchError::Serialization)?;
        if canonical.len() > MAX_COORDINATE_SEARCH_REQUEST_BYTES {
            return Err(CoordinateSearchError::RequestTooLarge {
                observed: canonical.len(),
                maximum: MAX_COORDINATE_SEARCH_REQUEST_BYTES,
            });
        }
        Ok(self)
    }
}

/// One immutable, digest-bound Coordinate-search Provider input.
#[derive(Clone, PartialEq, Eq)]
pub struct CoordinateSearchEncoderInput {
    semantic_input: SemanticQueryInput,
}

impl std::fmt::Debug for CoordinateSearchEncoderInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CoordinateSearchEncoderInput")
            .field("request_id", &self.semantic_input.request_id())
            .field("text", &"<redacted>")
            .field("text_bytes", &self.text().len())
            .finish_non_exhaustive()
    }
}

impl CoordinateSearchEncoderInput {
    /// Request identity owning this one input.
    pub const fn request_id(&self) -> Uuid {
        self.semantic_input.request_id()
    }

    /// Digest of the independently versioned query template.
    pub const fn query_contract_digest(&self) -> Digest32 {
        self.semantic_input.encoding_contract_digest()
    }

    /// Digest of the exact Provider input bytes.
    pub const fn text_digest(&self) -> Digest32 {
        self.semantic_input.input_digest()
    }

    /// Exact canonical UTF-8 Provider input.
    pub fn text(&self) -> &str {
        self.semantic_input.exact_utf8_text()
    }

    /// Common closed input shared by every semantic Provider adapter.
    pub const fn semantic_input(&self) -> &SemanticQueryInput {
        &self.semantic_input
    }

    /// Build the required single-input common bundle.
    pub fn semantic_input_bundle(&self) -> CoordinateSearchResult<SemanticQueryInputBundle> {
        SemanticQueryInputBundle::from_closed_inputs(vec![self.semantic_input.clone()])
            .map_err(|error| CoordinateSearchError::InvalidState(error.to_string()))
    }

    /// Revalidate immutable digests before crossing the Provider boundary.
    pub fn validate(&self) -> CoordinateSearchResult<()> {
        if !matches!(
            self.semantic_input.channel_kind(),
            SemanticQueryInputKind::CoordinateSearch
        ) || self.query_contract_digest() != coordinate_search_query_contract_digest()
        {
            return Err(CoordinateSearchError::InvalidState(
                "query contract digest mismatch".to_owned(),
            ));
        }
        if self.text_digest() != coordinate_search_query_text_digest(self.text().as_bytes()) {
            return Err(CoordinateSearchError::InvalidState(
                "query text digest mismatch".to_owned(),
            ));
        }
        self.semantic_input
            .validate()
            .map_err(|error| CoordinateSearchError::InvalidState(error.to_string()))?;
        validate_provider_input_size(self.text())
    }
}

/// Return the independently versioned Coordinate-search query contract digest.
#[must_use]
pub fn coordinate_search_query_contract_digest() -> Digest32 {
    hash_domain(
        b"carryforth.project-context-coordinate-search-contract",
        &[QUERY_CONTRACT_DESCRIPTOR.as_bytes()],
    )
}

/// Serialize one canonical natural-language Coordinate-search Provider input.
pub fn canonical_coordinate_search_query_text(query: &str) -> CoordinateSearchResult<String> {
    let query = validated_query(query)?;
    let mut output =
        String::with_capacity(COORDINATE_SEARCH_QUERY_CONTRACT.len() + query.len() + 32);
    output.push_str("{\"contract\":\"");
    output.push_str(COORDINATE_SEARCH_QUERY_CONTRACT);
    output.push_str("\",\"query\":\"");
    push_canonical_json_string_contents(&mut output, query);
    output.push_str("\"}");
    validate_provider_input_size(&output)?;
    Ok(output)
}

/// Build exactly one immutable Provider input for a canonical request.
pub fn build_coordinate_search_encoder_input(
    request: &ProjectContextCoordinateSearchQuery,
) -> CoordinateSearchResult<CoordinateSearchEncoderInput> {
    let request = request.clone().validate_and_canonicalize()?;
    let text = canonical_coordinate_search_query_text(&request.query)?;
    let semantic_input = SemanticQueryInput::new_closed(
        request.request_id,
        coordinate_search_channel_id(request.request_id),
        SemanticQueryInputKind::CoordinateSearch,
        coordinate_search_query_contract_digest(),
        coordinate_search_query_text_digest(text.as_bytes()),
        text,
    )
    .map_err(|error| CoordinateSearchError::InvalidState(error.to_string()))?;
    Ok(CoordinateSearchEncoderInput { semantic_input })
}

/// One validated ephemeral Coordinate-search query vector.
///
/// The vector is bound to the independently versioned Coordinate-search text
/// contract and to the active Foundation model space. It contains no source
/// text and is never persisted.
pub struct EncodedCoordinateSearchQuery {
    inner: ProviderEncodedSemanticInput,
}

impl EncodedCoordinateSearchQuery {
    /// Validate and bind one Provider vector to its exact input and model.
    pub fn new(
        input: &CoordinateSearchEncoderInput,
        response_model: String,
        values: Vec<f32>,
        source_contract: &SemanticModelContract,
    ) -> CoordinateSearchResult<Self> {
        input.validate()?;
        let inner = ProviderEncodedSemanticInput::new(
            input.semantic_input(),
            response_model,
            values,
            source_contract,
        )
        .map_err(|error| CoordinateSearchError::InvalidState(error.to_string()))?;
        Self::from_provider_encoded(input, inner, source_contract)
    }

    /// Restore the Coordinate compatibility wrapper around a common result.
    pub fn from_provider_encoded(
        input: &CoordinateSearchEncoderInput,
        inner: ProviderEncodedSemanticInput,
        source_contract: &SemanticModelContract,
    ) -> CoordinateSearchResult<Self> {
        input.validate()?;
        if !matches!(
            inner.channel_kind(),
            SemanticQueryInputKind::CoordinateSearch
        ) || inner.request_id() != input.request_id()
            || inner.channel_id() != input.semantic_input().channel_id()
            || inner.encoding_contract_digest() != coordinate_search_query_contract_digest()
            || inner.input_digest() != input.text_digest()
            || inner.response_model() != source_contract.model
            || inner.model_space().source_generation_contract_digest
                != source_contract
                    .digest()
                    .map_err(|error| CoordinateSearchError::InvalidState(error.to_string()))?
            || inner.model_space().embedding_space_fence
                != crate::embedding_space_fence(source_contract)
                    .map_err(|error| CoordinateSearchError::InvalidState(error.to_string()))?
        {
            return Err(CoordinateSearchError::InvalidState(
                "Coordinate-search Provider binding mismatch".to_owned(),
            ));
        }
        Ok(Self { inner })
    }

    /// Owning request identity.
    pub const fn request_id(&self) -> Uuid {
        self.inner.request_id()
    }

    /// Complete active Foundation generation contract digest.
    pub const fn source_generation_contract_digest(&self) -> Digest32 {
        self.inner.model_space().source_generation_contract_digest
    }

    /// Comparable model-space fence.
    pub const fn embedding_space_fence(&self) -> Digest32 {
        self.inner.model_space().embedding_space_fence
    }

    /// Coordinate-search query contract digest.
    pub const fn query_contract_digest(&self) -> Digest32 {
        self.inner.encoding_contract_digest()
    }

    /// Exact canonical Provider input digest.
    pub const fn query_input_digest(&self) -> Digest32 {
        self.inner.input_digest()
    }

    /// Exact response model identity.
    pub fn response_model(&self) -> &str {
        self.inner.response_model()
    }

    /// Validated finite, dimensioned, non-zero query vector.
    pub fn embedding(&self) -> &EmbeddingVector {
        self.inner.embedding()
    }

    /// Common Provider-bound representation used by the DB ticket binder.
    pub const fn provider_encoded(&self) -> &ProviderEncodedSemanticInput {
        &self.inner
    }

    /// Consume this compatibility wrapper.
    pub fn into_provider_encoded(self) -> ProviderEncodedSemanticInput {
        self.inner
    }
}

/// Exact generation and graph snapshot observations on a result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextCoordinateSearchObservations {
    /// Active semantic generation observed in the read snapshot.
    pub semantic_generation_id: Uuid,
    /// Comparable vector-space fence.
    pub embedding_space_fence: Digest32,
    /// Coordinate-search template/serializer/input-limit digest.
    pub query_contract_digest: Digest32,
    /// Applied v2 Coordinate type scope; omitted only for v1 all-type results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinate_types: Option<ProjectContextCoordinateTypeFilter>,
    /// Active Project Context projection generation.
    pub projection_generation: u64,
    /// Project Context catalog revision in the read snapshot.
    pub project_context_revision: u64,
    /// Writer-DB transaction observation time.
    pub snapshot_observed_at: DateTime<Utc>,
}

/// One ranked starting Coordinate candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextCoordinateSearchCandidate {
    /// One-based rank in the returned result.
    pub rank: u8,
    /// Canonical current Project Context Coordinate identity.
    pub coordinate: ProjectContextCoordinate,
    /// Direct fixed-point cosine score.
    pub score: Score,
}

/// Complete Relay-signed Coordinate-only search result.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextCoordinateSearchResult {
    /// Request correlation identity.
    pub request_id: Uuid,
    /// Host-derived Project identity.
    pub project_id: Uuid,
    /// Authenticated exact-request binding digest.
    pub request_binding_digest: Digest32,
    /// Generation and graph snapshot observations.
    pub observations: ProjectContextCoordinateSearchObservations,
    /// Bounded ranked Coordinate candidates.
    pub coordinates: Vec<ProjectContextCoordinateSearchCandidate>,
    /// Whether an additional eligible candidate existed in the same snapshot.
    pub truncated: bool,
}

impl std::fmt::Debug for ProjectContextCoordinateSearchResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectContextCoordinateSearchResult")
            .field("request_id", &self.request_id)
            .field("candidate_count", &self.coordinates.len())
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

impl ProjectContextCoordinateSearchResult {
    /// Validate result identities, ordering, bounds, and Coordinate identities.
    pub fn validate(&self) -> CoordinateSearchResult<()> {
        validate_uuid_v4(self.request_id, "result.request_id")?;
        validate_uuid_v4(self.project_id, "result.project_id")?;
        validate_uuid_v4(
            self.observations.semantic_generation_id,
            "result.observations.semantic_generation_id",
        )?;
        if self.observations.query_contract_digest != coordinate_search_query_contract_digest() {
            return Err(CoordinateSearchError::InvalidState(
                "result query contract digest mismatch".to_owned(),
            ));
        }
        if self
            .observations
            .coordinate_types
            .as_ref()
            .is_some_and(|filter| !filter.is_canonical())
        {
            return Err(CoordinateSearchError::InvalidState(
                "result Coordinate type filter is not canonical".to_owned(),
            ));
        }
        if self.observations.projection_generation == 0
            || self.observations.projection_generation > MAX_SAFE_REVISION
            || self.observations.project_context_revision > MAX_SAFE_REVISION
        {
            return Err(CoordinateSearchError::InvalidState(
                "result graph revision observations are out of range".to_owned(),
            ));
        }
        if self.coordinates.len() > usize::from(MAX_COORDINATE_SEARCH_LIMIT) {
            return Err(CoordinateSearchError::InvalidState(
                "result exceeds the hard candidate limit".to_owned(),
            ));
        }
        let mut seen_coordinates = BTreeSet::new();
        for (index, candidate) in self.coordinates.iter().enumerate() {
            let expected_rank =
                u8::try_from(index + 1).map_err(|_| CoordinateSearchError::Serialization)?;
            if candidate.rank != expected_rank {
                return Err(CoordinateSearchError::InvalidState(
                    "candidate ranks must be consecutive and one-based".to_owned(),
                ));
            }
            candidate
                .coordinate
                .validate_for_project(self.project_id)
                .map_err(|error| CoordinateSearchError::InvalidCoordinate(error.to_string()))?;
            if !seen_coordinates.insert(candidate.coordinate.clone()) {
                return Err(CoordinateSearchError::InvalidState(
                    "candidates must contain unique Coordinates".to_owned(),
                ));
            }
            if self
                .observations
                .coordinate_types
                .as_ref()
                .is_some_and(|filter| !filter.matches(&candidate.coordinate))
            {
                return Err(CoordinateSearchError::InvalidState(
                    "candidate violates the applied Coordinate type filter".to_owned(),
                ));
            }
        }
        if self
            .coordinates
            .windows(2)
            .any(|pair| candidate_order(&pair[0], &pair[1]) == Ordering::Greater)
        {
            return Err(CoordinateSearchError::InvalidState(
                "candidates must be unique and canonically score-sorted".to_owned(),
            ));
        }
        Ok(())
    }

    /// Validate this result against the exact canonical caller request.
    pub fn validate_for_request(
        &self,
        request: &ProjectContextCoordinateSearchQuery,
    ) -> CoordinateSearchResult<()> {
        let request = request.clone().validate_and_canonicalize()?;
        self.validate()?;
        if self.request_id != request.request_id || self.project_id != request.project_id {
            return Err(CoordinateSearchError::InvalidState(
                "result does not belong to the request".to_owned(),
            ));
        }
        if self.observations.coordinate_types != request.coordinate_types {
            return Err(CoordinateSearchError::InvalidState(
                "result Coordinate type filter does not match the request".to_owned(),
            ));
        }
        if self.coordinates.len() > usize::from(request.limit) {
            return Err(CoordinateSearchError::InvalidState(
                "result exceeds the requested candidate limit".to_owned(),
            ));
        }
        if self.truncated && self.coordinates.len() != usize::from(request.limit) {
            return Err(CoordinateSearchError::InvalidState(
                "truncated result must fill the requested limit".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Derive an independently domain-separated exact HTTP transcript binding.
pub fn derive_coordinate_search_http_request_binding(
    host_project_id: Uuid,
    authenticated_caller_pubkey: &[u8; 32],
    nip98_auth_event_id: Digest32,
    exact_authenticated_body: &[u8],
) -> CoordinateSearchResult<Digest32> {
    validate_uuid_v4(host_project_id, "host_project_id")?;
    let body_digest: [u8; 32] = Sha256::digest(exact_authenticated_body).into();
    Ok(hash_domain(
        REQUEST_BINDING_DOMAIN,
        &[
            host_project_id.as_bytes(),
            authenticated_caller_pubkey,
            nip98_auth_event_id.as_bytes(),
            &body_digest,
        ],
    ))
}

/// Verify an observed Coordinate-search binding against exact HTTP bytes.
pub fn verify_coordinate_search_http_request_binding(
    observed: Digest32,
    host_project_id: Uuid,
    authenticated_caller_pubkey: &[u8; 32],
    nip98_auth_event_id: Digest32,
    exact_authenticated_body: &[u8],
) -> CoordinateSearchResult<()> {
    let expected = derive_coordinate_search_http_request_binding(
        host_project_id,
        authenticated_caller_pubkey,
        nip98_auth_event_id,
        exact_authenticated_body,
    )?;
    if observed != expected {
        return Err(CoordinateSearchError::InvalidState(
            "HTTP request binding digest mismatch".to_owned(),
        ));
    }
    Ok(())
}

/// Derive the independently versioned filtered Coordinate-search transcript binding.
pub fn derive_coordinate_search_v2_http_request_binding(
    host_project_id: Uuid,
    authenticated_caller_pubkey: &[u8; 32],
    nip98_auth_event_id: Digest32,
    exact_authenticated_body: &[u8],
) -> CoordinateSearchResult<Digest32> {
    validate_uuid_v4(host_project_id, "host_project_id")?;
    let body_digest: [u8; 32] = Sha256::digest(exact_authenticated_body).into();
    Ok(hash_domain(
        FILTERED_REQUEST_BINDING_DOMAIN,
        &[
            host_project_id.as_bytes(),
            authenticated_caller_pubkey,
            nip98_auth_event_id.as_bytes(),
            &body_digest,
        ],
    ))
}

/// Verify a filtered Coordinate-search transcript against exact authenticated bytes.
pub fn verify_coordinate_search_v2_http_request_binding(
    observed: Digest32,
    host_project_id: Uuid,
    authenticated_caller_pubkey: &[u8; 32],
    nip98_auth_event_id: Digest32,
    exact_authenticated_body: &[u8],
) -> CoordinateSearchResult<()> {
    let expected = derive_coordinate_search_v2_http_request_binding(
        host_project_id,
        authenticated_caller_pubkey,
        nip98_auth_event_id,
        exact_authenticated_body,
    )?;
    if observed != expected {
        return Err(CoordinateSearchError::InvalidState(
            "filtered HTTP request binding digest mismatch".to_owned(),
        ));
    }
    Ok(())
}

fn candidate_order(
    left: &ProjectContextCoordinateSearchCandidate,
    right: &ProjectContextCoordinateSearchCandidate,
) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| left.coordinate.cmp(&right.coordinate))
}

fn validate_uuid_v4(value: Uuid, field: &'static str) -> CoordinateSearchResult<()> {
    if value.is_nil() || value.get_version_num() != 4 {
        return Err(CoordinateSearchError::InvalidUuid { field });
    }
    Ok(())
}

fn validated_query(query: &str) -> CoordinateSearchResult<&str> {
    let query = query.trim();
    if query.is_empty() {
        return Err(CoordinateSearchError::BlankQuery);
    }
    if query.as_bytes().contains(&0) {
        return Err(CoordinateSearchError::NulQuery);
    }
    if query.len() > MAX_COORDINATE_SEARCH_QUERY_BYTES {
        return Err(CoordinateSearchError::QueryTooLarge {
            observed: query.len(),
            maximum: MAX_COORDINATE_SEARCH_QUERY_BYTES,
        });
    }
    Ok(query)
}

fn validate_provider_input_size(text: &str) -> CoordinateSearchResult<()> {
    if text.len() > MAX_COORDINATE_SEARCH_PROVIDER_INPUT_BYTES {
        return Err(CoordinateSearchError::ProviderInputTooLarge {
            observed: text.len(),
            maximum: MAX_COORDINATE_SEARCH_PROVIDER_INPUT_BYTES,
        });
    }
    Ok(())
}

fn push_canonical_json_string_contents(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{0000}'..='\u{001f}' => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let value = character as u8;
                output.push_str("\\u00");
                output.push(HEX[(value >> 4) as usize] as char);
                output.push(HEX[(value & 0x0f) as usize] as char);
            }
            other => output.push(other),
        }
    }
}

pub(crate) fn coordinate_search_query_text_digest(text: &[u8]) -> Digest32 {
    hash_domain(
        b"carryforth.project-context-coordinate-search-query-text",
        &[text],
    )
}

fn coordinate_search_channel_id(request_id: Uuid) -> Digest32 {
    hash_domain(
        b"carryforth.project-context-coordinate-search-channel",
        &[request_id.as_bytes()],
    )
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
    use buzz_project_context::ProjectContextCoordinate;
    use buzz_project_view::ProjectViewObjectType;
    use chrono::Utc;

    use crate::ProjectContextCoordinateType;

    use super::*;

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_0000_0000 | value)
    }

    fn request() -> ProjectContextCoordinateSearchQuery {
        ProjectContextCoordinateSearchQuery {
            request_id: uuid(1),
            project_id: uuid(2),
            query: " why authorization failed? ".to_owned(),
            coordinate_types: None,
            limit: DEFAULT_COORDINATE_SEARCH_LIMIT,
        }
    }

    fn coordinate(value: u128) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: uuid(value),
        }
    }

    fn result() -> ProjectContextCoordinateSearchResult {
        ProjectContextCoordinateSearchResult {
            request_id: uuid(1),
            project_id: uuid(2),
            request_binding_digest: Digest32::from_bytes([3; 32]),
            observations: ProjectContextCoordinateSearchObservations {
                semantic_generation_id: uuid(3),
                embedding_space_fence: Digest32::from_bytes([4; 32]),
                query_contract_digest: coordinate_search_query_contract_digest(),
                coordinate_types: None,
                projection_generation: 1,
                project_context_revision: 2,
                snapshot_observed_at: Utc::now(),
            },
            coordinates: vec![
                ProjectContextCoordinateSearchCandidate {
                    rank: 1,
                    coordinate: coordinate(4),
                    score: Score::new(800_000).expect("score"),
                },
                ProjectContextCoordinateSearchCandidate {
                    rank: 2,
                    coordinate: coordinate(8),
                    score: Score::new(700_000).expect("score"),
                },
            ],
            truncated: false,
        }
    }

    #[test]
    fn request_trims_query_and_rejects_closed_bounds() {
        let canonical = request()
            .validate_and_canonicalize()
            .expect("canonical request");
        assert_eq!(canonical.query, "why authorization failed?");

        let mut invalid = request();
        invalid.limit = 0;
        assert!(matches!(
            invalid.validate_and_canonicalize(),
            Err(CoordinateSearchError::InvalidLimit { .. })
        ));
        let mut invalid = request();
        invalid.query = "\0".to_owned();
        assert!(matches!(
            invalid.validate_and_canonicalize(),
            Err(CoordinateSearchError::NulQuery)
        ));
    }

    #[test]
    fn closed_json_rejects_unknown_and_duplicate_fields() {
        let request = request().validate_and_canonicalize().expect("request");
        let mut value = serde_json::to_value(&request).expect("serialize");
        value
            .as_object_mut()
            .expect("object")
            .insert("unknown".to_owned(), serde_json::Value::Bool(true));
        assert!(ProjectContextCoordinateSearchQuery::parse_json(
            &serde_json::to_vec(&value).expect("json")
        )
        .is_err());

        let duplicate = format!(
            "{{\"request_id\":\"{}\",\"project_id\":\"{}\",\"query\":\"a\",\"query\":\"b\",\"limit\":8}}",
            request.request_id, request.project_id
        );
        assert!(ProjectContextCoordinateSearchQuery::parse_json(duplicate.as_bytes()).is_err());
    }

    #[test]
    fn one_input_has_an_independent_canonical_contract_and_redacted_debug() {
        let input = build_coordinate_search_encoder_input(&request()).expect("input");
        assert_eq!(
            input.text(),
            "{\"contract\":\"carryforth.project-context-coordinate-search.query\",\"query\":\"why authorization failed?\"}"
        );
        input.validate().expect("valid input");
        let debug = format!("{input:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("authorization"));

        let mut filtered = request();
        filtered.coordinate_types = Some(
            ProjectContextCoordinateTypeFilter::new(vec![ProjectContextCoordinateType::Work])
                .expect("filter"),
        );
        let filtered = build_coordinate_search_encoder_input(&filtered).expect("filtered input");
        assert_eq!(filtered.text(), input.text());
        assert_eq!(filtered.text_digest(), input.text_digest());
    }

    #[test]
    fn result_requires_unique_canonical_score_order_and_request_limit() {
        let result = result();
        result
            .validate_for_request(&request())
            .expect("valid result");

        let mut invalid = result.clone();
        invalid.coordinates.swap(0, 1);
        assert!(invalid.validate().is_err());

        let mut duplicate = result.clone();
        duplicate
            .coordinates
            .push(ProjectContextCoordinateSearchCandidate {
                rank: 3,
                coordinate: duplicate.coordinates[0].coordinate.clone(),
                score: Score::new(600_000).expect("score"),
            });
        assert!(duplicate.validate().is_err());

        let mut invalid = result.clone();
        invalid.coordinates[1].coordinate = invalid.coordinates[0].coordinate.clone();
        assert!(invalid.validate().is_err());

        let mut limited = request();
        limited.limit = 1;
        assert!(result.validate_for_request(&limited).is_err());

        let filter =
            ProjectContextCoordinateTypeFilter::new(vec![ProjectContextCoordinateType::Work])
                .expect("filter");
        let mut filtered_request = request();
        filtered_request.coordinate_types = Some(filter.clone());
        let mut filtered_result = result.clone();
        filtered_result.observations.coordinate_types = Some(filter);
        filtered_result
            .validate_for_request(&filtered_request)
            .expect("filtered result");
        filtered_result.coordinates[0].coordinate = ProjectContextCoordinate::Document {
            document_id: uuid(20),
        };
        assert!(filtered_result
            .validate_for_request(&filtered_request)
            .is_err());
    }

    #[test]
    fn truncated_result_must_fill_the_requested_limit() {
        let mut request = request();
        request.limit = 3;
        let mut result = result();
        result.truncated = true;
        assert!(result.validate_for_request(&request).is_err());
    }

    #[test]
    fn binding_is_domain_separated_and_binds_every_body_byte() {
        let body = br#"[{\"kinds\":[40913]}]"#;
        let binding = derive_coordinate_search_http_request_binding(
            uuid(2),
            &[7; 32],
            Digest32::from_bytes([8; 32]),
            body,
        )
        .expect("binding");
        verify_coordinate_search_http_request_binding(
            binding,
            uuid(2),
            &[7; 32],
            Digest32::from_bytes([8; 32]),
            body,
        )
        .expect("verify");
        let v2 = derive_coordinate_search_v2_http_request_binding(
            uuid(2),
            &[7; 32],
            Digest32::from_bytes([8; 32]),
            body,
        )
        .expect("v2 binding");
        assert_ne!(binding, v2);
        verify_coordinate_search_v2_http_request_binding(
            v2,
            uuid(2),
            &[7; 32],
            Digest32::from_bytes([8; 32]),
            body,
        )
        .expect("verify v2");
        assert_ne!(
            binding,
            derive_coordinate_search_http_request_binding(
                uuid(2),
                &[7; 32],
                Digest32::from_bytes([8; 32]),
                br#"[{\"kinds\":[40913]} ]"#,
            )
            .expect("changed")
        );
    }
}
