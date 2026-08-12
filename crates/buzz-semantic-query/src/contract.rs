use buzz_project_context::ProjectContextCoordinate;
use buzz_semantic::Digest32;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

/// Maximum authenticated JSON request size accepted by the closed parser.
pub const MAX_QUERY_REQUEST_BYTES: usize = 64 * 1024;
/// Maximum UTF-8 byte length of the trimmed problem.
pub const MAX_PROBLEM_BYTES: usize = 16 * 1024;
/// Maximum raw initial-coordinate entries before canonical deduplication.
pub const MAX_INITIAL_COORDINATES: usize = 16;
/// Maximum raw context-coordinate entries before canonical deduplication.
pub const MAX_CONTEXT_COORDINATES: usize = 8;
/// Maximum problem plus conditioned query-vector branches.
pub const MAX_QUERY_CHANNELS: usize = 1 + MAX_CONTEXT_COORDINATES;
/// Maximum canonical UTF-8 bytes in one Provider query item.
pub const MAX_PROVIDER_QUERY_INPUT_BYTES: usize = 64 * 1024;
/// Hard cap for exact source recall returned by any one query-vector branch.
pub const MAX_RECALL_PER_CHANNEL: u16 = 256;
/// Hard cap for automatic semantic roots.
pub const MAX_SEMANTIC_ROOTS: u16 = 16;
/// Hard cap for complete hops in one path.
pub const MAX_HOPS_PER_PATH: u8 = 6;
/// Hard cap for successors retained per logical path state.
pub const MAX_BEAM_WIDTH: u16 = 32;
/// Hard cap for provenance-distinct Coordinate expansions.
pub const MAX_EXPANDED_COORDINATES: u16 = 512;
/// Hard cap for unique complete Edge identities materialized.
pub const MAX_INCIDENT_EDGES_MATERIALIZED: u16 = 1_024;
/// Hard cap for unique relation options materialized.
pub const MAX_RELATION_OPTIONS_MATERIALIZED: u16 = 2_048;
/// Hard cap for unique target options materialized.
pub const MAX_TARGET_OPTIONS_MATERIALIZED: u16 = 4_096;
/// Hard cap for retained result paths.
pub const MAX_PATHS: u16 = 64;
/// Hard cap for absolute query wall time.
pub const MAX_WALL_TIME_MS: u32 = 180_000;
/// Hard cap for a serialized result Event array.
pub const MAX_RESPONSE_BYTES: u32 = 256 * 1024;
/// Fixed maximum canonical Hyperedge identity JSON size.
pub const MAX_HYPEREDGE_IDENTITY_BYTES: usize = 64 * 1024;
/// Fixed cap on diagnostic truncation samples.
pub const MAX_TRUNCATION_SAMPLES: usize = 32;
/// Time reserved after traversal for closing its read-only database snapshot.
pub const SNAPSHOT_CLOSE_RESERVE_MS: u32 = 5_000;
/// Response tail reserved for packing, postflight, and signing.
pub const RESPONSE_TAIL_RESERVE_MS: u32 = 1_000;

const _: () = assert!(SNAPSHOT_CLOSE_RESERVE_MS + RESPONSE_TAIL_RESERVE_MS < MAX_WALL_TIME_MS);

/// Closed lifecycle selection applied to automatic Coordinate entrypoints and
/// continued target Coordinates.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleFilter {
    /// Active, finalizing, and terminal current sources.
    #[default]
    AllCurrent,
    /// Active and finalizing current sources only.
    NonTerminal,
    /// Terminal current sources only.
    TerminalOnly,
}

/// Caller-controlled logical work limits. Every value is positive and capped
/// by the server profile in [`SemanticGraphQueryBudget::validate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SemanticGraphQueryBudget {
    /// Maximum source recall per query-vector branch.
    pub max_recall_per_channel: u16,
    /// Maximum automatic semantic source roots; explicit roots do not count.
    pub max_semantic_roots: u16,
    /// Maximum complete Hyperedge hops in one path.
    pub max_hops_per_path: u8,
    /// Maximum successors retained per logical path state.
    pub beam_width: u16,
    /// Maximum provenance-distinct Coordinate expansions.
    pub max_expanded_coordinates: u16,
    /// Maximum globally unique complete Edge identities materialized.
    pub max_incident_edges_materialized: u16,
    /// Maximum unique `(U?, E, D)` relation options materialized.
    pub max_relation_options_materialized: u16,
    /// Maximum unique `(U?, E, D, V)` target options materialized.
    pub max_target_options_materialized: u16,
    /// Maximum stopped paths retained after search.
    pub max_paths: u16,
    /// Absolute request wall-clock budget in milliseconds.
    pub max_wall_time_ms: u32,
    /// Maximum serialized virtual Event array size in bytes.
    pub max_response_bytes: u32,
}

/// Frozen default query budget.
pub const DEFAULT_QUERY_BUDGET: SemanticGraphQueryBudget = SemanticGraphQueryBudget {
    max_recall_per_channel: 64,
    max_semantic_roots: 6,
    max_hops_per_path: 3,
    beam_width: 8,
    max_expanded_coordinates: 64,
    max_incident_edges_materialized: 96,
    max_relation_options_materialized: 128,
    max_target_options_materialized: 192,
    max_paths: 12,
    max_wall_time_ms: 180_000,
    max_response_bytes: 128 * 1024,
};

impl Default for SemanticGraphQueryBudget {
    fn default() -> Self {
        DEFAULT_QUERY_BUDGET
    }
}

impl SemanticGraphQueryBudget {
    /// Validate positive values against the frozen server hard caps.
    pub fn validate(&self) -> QueryContractResult<()> {
        validate_cap(
            "max_recall_per_channel",
            self.max_recall_per_channel,
            MAX_RECALL_PER_CHANNEL,
        )?;
        validate_cap(
            "max_semantic_roots",
            self.max_semantic_roots,
            MAX_SEMANTIC_ROOTS,
        )?;
        validate_cap(
            "max_hops_per_path",
            self.max_hops_per_path,
            MAX_HOPS_PER_PATH,
        )?;
        validate_cap("beam_width", self.beam_width, MAX_BEAM_WIDTH)?;
        validate_cap(
            "max_expanded_coordinates",
            self.max_expanded_coordinates,
            MAX_EXPANDED_COORDINATES,
        )?;
        validate_cap(
            "max_incident_edges_materialized",
            self.max_incident_edges_materialized,
            MAX_INCIDENT_EDGES_MATERIALIZED,
        )?;
        validate_cap(
            "max_relation_options_materialized",
            self.max_relation_options_materialized,
            MAX_RELATION_OPTIONS_MATERIALIZED,
        )?;
        validate_cap(
            "max_target_options_materialized",
            self.max_target_options_materialized,
            MAX_TARGET_OPTIONS_MATERIALIZED,
        )?;
        validate_cap("max_paths", self.max_paths, MAX_PATHS)?;
        validate_cap("max_wall_time_ms", self.max_wall_time_ms, MAX_WALL_TIME_MS)?;
        validate_cap(
            "max_response_bytes",
            self.max_response_bytes,
            MAX_RESPONSE_BYTES,
        )?;
        Ok(())
    }
}

/// Digest the complete current budget defaults, hard caps, materialization
/// counters, response packing, and deadline-tail contract.
pub fn budget_profile_digest() -> QueryContractResult<Digest32> {
    let canonical = postcard::to_stdvec(&(
        "semantic-graph-budget",
        DEFAULT_QUERY_BUDGET,
        (
            MAX_RECALL_PER_CHANNEL,
            MAX_SEMANTIC_ROOTS,
            MAX_HOPS_PER_PATH,
            MAX_BEAM_WIDTH,
            MAX_EXPANDED_COORDINATES,
            MAX_INCIDENT_EDGES_MATERIALIZED,
            MAX_RELATION_OPTIONS_MATERIALIZED,
            MAX_TARGET_OPTIONS_MATERIALIZED,
            MAX_PATHS,
        ),
        (
            MAX_WALL_TIME_MS,
            MAX_RESPONSE_BYTES,
            MAX_HYPEREDGE_IDENTITY_BYTES,
            MAX_TRUNCATION_SAMPLES,
            SNAPSHOT_CLOSE_RESERVE_MS,
            RESPONSE_TAIL_RESERVE_MS,
        ),
        (
            "counts=recall_unique_source;roots_unique_automatic_source;hops_complete;beam_successor;expanded_path_provenance;edges_global_edge_key;relations_u_edge_document;targets_u_edge_document_v;paths_stopped;response_event_array",
            "exhausted=k-plus-one-except-hop-conservative",
            "materialize=relation-then-edge-then-target-then-complete-hop",
            "packing=envelope-explicit-roots-automatic-roots-paths-whole-summary-whole-edge",
            "deadline=one-monotonic-absolute-minus-response-tail-minus-snapshot-close-reserve",
        ),
    ))
    .map_err(|_| SemanticGraphQueryError::Serialization)?;
    let mut hasher = Sha256::new();
    let domain = b"buzz.semantic-graph-budget";
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain);
    hasher.update((canonical.len() as u64).to_be_bytes());
    hasher.update(canonical);
    Ok(Digest32::from_bytes(hasher.finalize().into()))
}

fn validate_cap<T>(field: &'static str, value: T, hard_cap: T) -> QueryContractResult<()>
where
    T: Copy + PartialEq + PartialOrd + From<u8> + Into<u64>,
{
    if value == T::from(0) || value > hard_cap {
        return Err(SemanticGraphQueryError::BudgetOutOfRange {
            field,
            value: value.into(),
            hard_cap: hard_cap.into(),
        });
    }
    Ok(())
}

/// Unversioned, closed semantic graph query request.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticGraphQuery {
    /// Caller-generated result correlation and authenticated replay-binding UUID.
    pub request_id: Uuid,
    /// Host-derived Project identity repeated for confused-deputy protection.
    pub project_id: Uuid,
    /// Natural-language problem that remains the dominant ranking signal.
    pub problem: String,
    /// Optional explicit graph traversal roots.
    #[serde(default)]
    pub initial_coordinates: Vec<ProjectContextCoordinate>,
    /// Optional soft semantic environment lenses.
    #[serde(default)]
    pub context_coordinates: Vec<ProjectContextCoordinate>,
    /// Lifecycle selection for automatic Coordinate roles and targets.
    #[serde(default)]
    pub lifecycle_filter: LifecycleFilter,
    /// Caller-requested bounded work profile.
    #[serde(default)]
    pub budget: SemanticGraphQueryBudget,
}

impl std::fmt::Debug for SemanticGraphQuery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticGraphQuery")
            .field("problem", &"<redacted>")
            .field("problem_bytes", &self.problem.len())
            .field("initial_coordinate_count", &self.initial_coordinates.len())
            .field("context_coordinate_count", &self.context_coordinates.len())
            .field("lifecycle_filter", &self.lifecycle_filter)
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

impl SemanticGraphQuery {
    /// Parse an exact JSON body, reject unknown fields, validate raw resource
    /// bounds, and return canonicalized Coordinate arrays and trimmed problem.
    pub fn parse_json(bytes: &[u8]) -> QueryContractResult<Self> {
        if bytes.len() > MAX_QUERY_REQUEST_BYTES {
            return Err(SemanticGraphQueryError::RequestTooLarge {
                observed: bytes.len(),
                maximum: MAX_QUERY_REQUEST_BYTES,
            });
        }
        let query: Self = serde_json::from_slice(bytes)
            .map_err(|error| SemanticGraphQueryError::InvalidJson(error.to_string()))?;
        query.validate_and_canonicalize()
    }

    /// Validate and canonicalize a request constructed by a trusted internal
    /// caller. Raw Coordinate count limits are checked before deduplication.
    pub fn validate_and_canonicalize(mut self) -> QueryContractResult<Self> {
        validate_uuid_v4(self.request_id, "request_id")?;
        validate_uuid_v4(self.project_id, "project_id")?;

        let problem = self.problem.trim();
        if problem.is_empty() {
            return Err(SemanticGraphQueryError::BlankProblem);
        }
        if problem.as_bytes().contains(&0) {
            return Err(SemanticGraphQueryError::NulText { field: "problem" });
        }
        if problem.len() > MAX_PROBLEM_BYTES {
            return Err(SemanticGraphQueryError::ProblemTooLarge {
                observed: problem.len(),
                maximum: MAX_PROBLEM_BYTES,
            });
        }
        if self.initial_coordinates.len() > MAX_INITIAL_COORDINATES {
            return Err(SemanticGraphQueryError::TooManyCoordinates {
                field: "initial_coordinates",
                observed: self.initial_coordinates.len(),
                maximum: MAX_INITIAL_COORDINATES,
            });
        }
        if self.context_coordinates.len() > MAX_CONTEXT_COORDINATES {
            return Err(SemanticGraphQueryError::TooManyCoordinates {
                field: "context_coordinates",
                observed: self.context_coordinates.len(),
                maximum: MAX_CONTEXT_COORDINATES,
            });
        }
        canonicalize_query_coordinates(
            self.project_id,
            "initial_coordinates",
            &mut self.initial_coordinates,
        )?;
        canonicalize_query_coordinates(
            self.project_id,
            "context_coordinates",
            &mut self.context_coordinates,
        )?;
        self.budget.validate()?;
        self.problem = problem.to_owned();
        Ok(self)
    }
}

fn canonicalize_query_coordinates(
    project_id: Uuid,
    field: &'static str,
    coordinates: &mut Vec<ProjectContextCoordinate>,
) -> QueryContractResult<()> {
    for coordinate in coordinates.iter() {
        coordinate
            .validate_for_project(project_id)
            .map_err(|error| SemanticGraphQueryError::InvalidCoordinate {
                field,
                reason: error.to_string(),
            })?;
    }
    coordinates.sort();
    coordinates.dedup();
    Ok(())
}

fn validate_uuid_v4(value: Uuid, field: &'static str) -> QueryContractResult<()> {
    if value.is_nil() || value.get_version_num() != 4 {
        return Err(SemanticGraphQueryError::InvalidUuid { field });
    }
    Ok(())
}

/// Errors returned by pure closed-query parsing, validation, and algorithms.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SemanticGraphQueryError {
    /// Exact JSON request bytes exceed the public resource boundary.
    #[error("semantic graph query request is {observed} bytes; maximum is {maximum}")]
    RequestTooLarge {
        /// Observed byte length.
        observed: usize,
        /// Maximum accepted byte length.
        maximum: usize,
    },
    /// JSON is malformed or violates a closed serde schema.
    #[error("invalid semantic graph query JSON: {0}")]
    InvalidJson(String),
    /// A required UUID is nil or is not UUIDv4.
    #[error("semantic graph query {field} must be UUIDv4")]
    InvalidUuid {
        /// Rejected field.
        field: &'static str,
    },
    /// The trimmed problem is empty.
    #[error("semantic graph query problem must not be blank")]
    BlankProblem,
    /// Project text contains a forbidden NUL byte.
    #[error("semantic graph query {field} must not contain NUL")]
    NulText {
        /// Rejected field.
        field: &'static str,
    },
    /// Problem exceeds its request resource boundary.
    #[error("semantic graph query problem is {observed} bytes; maximum is {maximum}")]
    ProblemTooLarge {
        /// Observed UTF-8 byte length.
        observed: usize,
        /// Maximum accepted UTF-8 byte length.
        maximum: usize,
    },
    /// A raw Coordinate array exceeds its pre-deduplication limit.
    #[error("semantic graph query {field} has {observed} entries; maximum is {maximum}")]
    TooManyCoordinates {
        /// Rejected array.
        field: &'static str,
        /// Raw entry count.
        observed: usize,
        /// Maximum raw entry count.
        maximum: usize,
    },
    /// A Coordinate identity is malformed or crosses its Project boundary.
    #[error("invalid semantic graph query {field}: {reason}")]
    InvalidCoordinate {
        /// Rejected array.
        field: &'static str,
        /// Content-free domain validation reason.
        reason: String,
    },
    /// One budget value is zero or exceeds the server hard cap.
    #[error("semantic graph query {field}={value} is outside 1..={hard_cap}")]
    BudgetOutOfRange {
        /// Rejected budget field.
        field: &'static str,
        /// Rejected value.
        value: u64,
        /// Frozen hard cap.
        hard_cap: u64,
    },
    /// A canonical Provider query item exceeds the fixed Provider input bound.
    #[error("semantic graph query Provider input is {observed} bytes; maximum is {maximum}")]
    ProviderInputTooLarge {
        /// Observed canonical byte length.
        observed: usize,
        /// Maximum canonical byte length.
        maximum: usize,
    },
    /// A score value or arithmetic input violates the closed score contract.
    #[error("invalid semantic graph query score: {0}")]
    InvalidScore(String),
    /// A pure result or frontier contract violates a closed invariant.
    #[error("invalid semantic graph query state: {0}")]
    InvalidState(String),
    /// A deterministic contract could not be serialized.
    #[error("semantic graph query contract serialization failed")]
    Serialization,
    /// The approved query Provider could not be reached or timed out.
    #[error("semantic graph query Provider transport failed")]
    ProviderTransport,
    /// The Provider asked the interactive query to retry later.
    #[error("semantic graph query Provider rate limited the request")]
    ProviderRateLimited {
        /// Optional Provider-supplied retry delay.
        retry_after_seconds: Option<u64>,
    },
    /// The Provider returned a retryable server status.
    #[error("semantic graph query Provider returned retryable status {status}")]
    ProviderRetryable {
        /// HTTP status without response body or project data.
        status: u16,
    },
    /// The Provider permanently rejected the bounded request.
    #[error("semantic graph query Provider rejected request with status {status}")]
    ProviderRejected {
        /// HTTP status without response body or project data.
        status: u16,
    },
    /// Provider JSON, model, output count, order, or vectors violate contract.
    #[error("semantic graph query Provider response violated its closed contract")]
    ProviderResponse,
}

/// Result returned by pure query-contract operations.
pub type QueryContractResult<T> = Result<T, SemanticGraphQueryError>;

#[cfg(test)]
mod tests {
    use buzz_project_context::ProjectContextCoordinate;
    use buzz_project_view::ProjectViewObjectType;
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        SemanticGraphQuery, SemanticGraphQueryError, DEFAULT_QUERY_BUDGET, MAX_CONTEXT_COORDINATES,
        MAX_WALL_TIME_MS,
    };

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_0000_0000 | value)
    }

    fn work(value: u128) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: uuid(value),
        }
    }

    #[test]
    fn closed_parse_trims_problem_and_canonicalizes_each_array() {
        let body = json!({
            "request_id": uuid(1),
            "project_id": uuid(2),
            "problem": "  为什么复发？\n",
            "initial_coordinates": [work(9), work(3), work(9)],
            "context_coordinates": [work(8), work(4)],
            "lifecycle_filter": "all_current",
            "budget": {}
        });
        let parsed = SemanticGraphQuery::parse_json(&serde_json::to_vec(&body).expect("JSON"))
            .expect("valid query");
        assert_eq!(parsed.problem, "为什么复发？");
        assert_eq!(parsed.initial_coordinates, vec![work(3), work(9)]);
        assert_eq!(parsed.context_coordinates, vec![work(4), work(8)]);
    }

    #[test]
    fn unknown_fields_fail_closed() {
        let body = json!({
            "request_id": uuid(1),
            "project_id": uuid(2),
            "problem": "problem",
            "budget": {},
            "schema_version": 1
        });
        assert!(matches!(
            SemanticGraphQuery::parse_json(&serde_json::to_vec(&body).expect("JSON")),
            Err(SemanticGraphQueryError::InvalidJson(_))
        ));
    }

    #[test]
    fn raw_coordinate_limit_precedes_deduplication() {
        let body = json!({
            "request_id": uuid(1),
            "project_id": uuid(2),
            "problem": "problem",
            "context_coordinates": vec![work(8); MAX_CONTEXT_COORDINATES + 1],
            "budget": {}
        });
        assert!(matches!(
            SemanticGraphQuery::parse_json(&serde_json::to_vec(&body).expect("JSON")),
            Err(SemanticGraphQueryError::TooManyCoordinates {
                field: "context_coordinates",
                ..
            })
        ));
    }

    #[test]
    fn budget_is_closed_positive_and_capped() {
        let body = json!({
            "request_id": uuid(1),
            "project_id": uuid(2),
            "problem": "problem",
            "budget": {"beam_width": 0}
        });
        assert!(matches!(
            SemanticGraphQuery::parse_json(&serde_json::to_vec(&body).expect("JSON")),
            Err(SemanticGraphQueryError::BudgetOutOfRange {
                field: "beam_width",
                ..
            })
        ));
        assert_eq!(DEFAULT_QUERY_BUDGET.max_wall_time_ms, MAX_WALL_TIME_MS);
        assert_eq!(MAX_WALL_TIME_MS, 180_000);
    }

    #[test]
    fn query_debug_redacts_problem_and_all_request_identities() {
        let secret = "CONFIDENTIAL-PROBLEM-中文";
        let body = json!({
            "request_id": uuid(1),
            "project_id": uuid(2),
            "problem": secret,
            "initial_coordinates": [work(9)],
            "context_coordinates": [work(8)],
            "budget": {}
        });
        let query = SemanticGraphQuery::parse_json(&serde_json::to_vec(&body).expect("JSON"))
            .expect("valid query");
        let debug = format!("{query:?}");

        assert!(!debug.contains(secret));
        assert!(!debug.contains(&uuid(1).to_string()));
        assert!(!debug.contains(&uuid(2).to_string()));
        assert!(!debug.contains(&uuid(8).to_string()));
        assert!(!debug.contains(&uuid(9).to_string()));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("context_coordinate_count: 1"));
    }
}
