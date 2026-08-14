#![deny(unsafe_code)]
#![warn(missing_docs)]
//! Pure contracts and deterministic algorithms for Project Context semantic
//! graph queries.
//!
//! This crate performs no database, network, authorization, Relay, or agent
//! work. Callers must supply current, authorized canonical observations.

mod binding;
mod contract;
mod coordinate_search;
mod encoder;
mod fence;
mod fleet;
mod frontier;
mod query_text;
mod result;
mod root;
mod score;

pub use binding::{derive_http_request_binding, verify_http_request_binding};
pub use contract::{
    budget_profile_digest, LifecycleFilter, QueryContractResult, SemanticGraphQuery,
    SemanticGraphQueryBudget, SemanticGraphQueryError, DEFAULT_QUERY_BUDGET, MAX_BEAM_WIDTH,
    MAX_CONTEXT_COORDINATES, MAX_EXPANDED_COORDINATES, MAX_HOPS_PER_PATH,
    MAX_HYPEREDGE_IDENTITY_BYTES, MAX_INCIDENT_EDGES_MATERIALIZED, MAX_INITIAL_COORDINATES,
    MAX_PATHS, MAX_PROBLEM_BYTES, MAX_PROVIDER_QUERY_INPUT_BYTES, MAX_QUERY_CHANNELS,
    MAX_QUERY_REQUEST_BYTES, MAX_RECALL_PER_CHANNEL, MAX_RELATION_OPTIONS_MATERIALIZED,
    MAX_RESPONSE_BYTES, MAX_SEMANTIC_ROOTS, MAX_TARGET_OPTIONS_MATERIALIZED,
    MAX_TRUNCATION_SAMPLES, MAX_WALL_TIME_MS, RESPONSE_TAIL_RESERVE_MS, SNAPSHOT_CLOSE_RESERVE_MS,
};
pub use coordinate_search::*;
pub use encoder::{
    DeterministicFakeQueryEncoder, EncodedSemanticQuery, SemanticQueryEncoder,
    SemanticQueryEncoderFuture,
};
pub use fence::{embedding_space_fence, QueryCompatibilityFences};
pub use fleet::{
    semantic_graph_http_runtime_digest, ParseSemanticGraphQueryFleetPolicyError,
    SemanticGraphFleetInventoryError, SemanticGraphHttpFleetInstance,
    SemanticGraphHttpFleetInventory, SemanticGraphQueryEnableRequirement,
    SemanticGraphQueryFleetPolicy, SemanticGraphQueryRoutingTrust,
    MAX_SEMANTIC_GRAPH_FLEET_INSTANCES, MAX_SEMANTIC_GRAPH_FLEET_INVENTORY_BYTES,
    SEMANTIC_GRAPH_HTTP_RUNTIME_CONTRACT, SEMANTIC_GRAPH_HTTP_TRANSPORT,
};
pub use frontier::{
    first_wave_slice, highest_precedence_stop, BoundedSuccessorAccumulator, CounterAdmission,
    ExpansionContinuation, FrontierPathState, IncidentExpansionContinuation, RelationRankCursor,
    TargetExpansionContinuation, TargetRankCursor, TraversalMaterializationCounters,
};
pub use query_text::{
    build_query_encoder_inputs, canonical_conditioned_query_text, canonical_problem_query_text,
    query_contract_digest, ConditionedContextOverview, ConditionedInputOmissionReason,
    OmittedConditionedInput, SemanticQueryChannelKind, SemanticQueryEncoderInput,
    SemanticQueryInputBuildOutcome, CONDITIONED_CONTEXT_CONTRACT, PROBLEM_CONTRACT,
    QUERY_SERIALIZER_CONTRACT,
};
pub use result::*;
pub use root::{
    select_automatic_roots, AutomaticRootLane, RootPairRedundancy, RootSelectionCandidate,
    SelectedAutomaticRoot,
};
pub use score::{
    candidate_score, context_kind_weight, document_score, environment_gain, harmonic_score,
    mul_score, path_score, ranking_contract_digest, root_diversity_priority,
    target_coordinate_score, weighted_score, AnchorGain, ConditionedEvidence,
    EnvironmentScoreExplanation, PathScoreExplanation, Score, ScoreError, BASE_ENTRY_FLOOR,
    DISCOUNT_FACTOR, HOP_PENALTY, RELATION_FLOOR, SCORE_SCALE, TARGET_FLOOR, TRANSITION_FLOOR,
};
