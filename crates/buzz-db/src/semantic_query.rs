//! Current, authorized Project Context semantic graph exact reads.
//!
//! This module deliberately owns no query text encoding, public result DTO, or
//! graph-ranking policy. It exposes writer-database, transaction-bound scalar
//! observations to the Relay query orchestrator. Raw source vectors never
//! cross this API.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use buzz_core::{CommunityId, PublicKey};
use buzz_project_context::{EdgeKey, ProjectContextCoordinate};
use buzz_project_view::ProjectViewObjectType;
use buzz_semantic::{
    CanonicalSemanticSourceObservation, Digest32, EmbeddingVector, IneligibilityReason,
    ProjectViewSemanticType, SemanticCoverage, SemanticEligibility, SemanticLifecycleClass,
    SemanticSourceBasis, SemanticSourceIdentity, SemanticSourceKind,
};
use buzz_semantic_query::{
    document_score, environment_gain, harmonic_score, target_coordinate_score, ConditionedEvidence,
    ContextDocumentBindingObservation, LifecycleFilter, ProjectContextBindingProvenance,
    ProjectContextEdgeProvenance, QueryCompatibilityFences, RelationRankCursor, Score,
    SemanticEdgeObservation, SemanticGraphQueryRoutingTrust, TargetRankCursor,
    MAX_CONTEXT_COORDINATES, MAX_HYPEREDGE_IDENTITY_BYTES, MAX_INITIAL_COORDINATES,
    MAX_QUERY_CHANNELS, MAX_RECALL_PER_CHANNEL, MAX_RELATION_OPTIONS_MATERIALIZED,
    MAX_TARGET_OPTIONS_MATERIALIZED, RELATION_FLOOR, TARGET_FLOOR, TRANSITION_FLOOR,
};
use chrono::{DateTime, Utc};
use pgvector::Vector;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::semantic::{
    observe_semantic_source_in_connection, reserve_semantic_provider_slot_in_tx,
    semantic_generation_from_row, semantic_source_kind_from_db, SemanticProviderReservation,
    SemanticProviderWorkload,
};
use crate::{Db, DbError, Result};

const MAX_SCORE_MATRIX_SOURCES: usize =
    MAX_QUERY_CHANNELS * MAX_RECALL_PER_CHANNEL as usize + MAX_INITIAL_COORDINATES;
const MAX_CONTEXT_OVERVIEWS: usize = 8;
const MAX_RELATION_SCORE_SET: usize = MAX_RELATION_OPTIONS_MATERIALIZED as usize;
const MAX_TARGET_SCORE_SET: usize = MAX_TARGET_OPTIONS_MATERIALIZED as usize;
const MAX_SOURCE_PAIR_SCORES: usize = MAX_TARGET_SCORE_SET;

/// An active-generation and Project Context observation established by a
/// current, authorized writer-database snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticGraphQueryTicket {
    /// Host-derived Community/Project identity.
    pub community_id: CommunityId,
    /// Current active semantic generation.
    pub generation: crate::semantic::SemanticGenerationRecord,
    /// Closed source-generation/vector-space/query-template compatibility
    /// fences derived from the active generation.
    pub query_fences: QueryCompatibilityFences,
    /// Project Context projection generation observed with the ticket.
    pub projection_generation: u64,
    /// Project Context catalog revision observed with the generation pointer.
    pub project_context_revision: u64,
    /// Database time at which this ticket snapshot was observed.
    pub observed_at: DateTime<Utc>,
}

/// Server-owned transaction timeouts for one Stage C exact read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticGraphReadTimeouts {
    /// Maximum duration of each statement.
    pub statement: Duration,
    /// Maximum duration waiting for a database lock.
    pub lock: Duration,
    /// Maximum idle time while the read transaction is open.
    pub idle_in_transaction: Duration,
}

impl Default for SemanticGraphReadTimeouts {
    fn default() -> Self {
        Self {
            statement: Duration::from_secs(5),
            lock: Duration::from_millis(250),
            idle_in_transaction: Duration::from_secs(10),
        }
    }
}

/// One validated query vector branch supplied by the query encoder.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticExactQueryVector {
    /// Stable, request-local branch identity.
    channel_id: Digest32,
    /// Three compatibility fences observed by the query encoder.
    query_fences: QueryCompatibilityFences,
    /// Finite vector already validated against the ticket model contract.
    embedding: EmbeddingVector,
}

impl SemanticExactQueryVector {
    /// Bind one encoded branch to the exact ticket and closed compatibility
    /// fences before it can reach SQL.
    pub fn new(
        ticket: &SemanticGraphQueryTicket,
        channel_id: Digest32,
        query_fences: QueryCompatibilityFences,
        embedding: EmbeddingVector,
    ) -> Result<Self> {
        let vector = Self {
            channel_id,
            query_fences,
            embedding,
        };
        validate_query_vectors(ticket, std::slice::from_ref(&vector))?;
        Ok(vector)
    }

    /// Stable request-local branch identity.
    pub const fn channel_id(&self) -> Digest32 {
        self.channel_id
    }
}

/// Current source-head evidence returned without its raw embedding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCurrentHead {
    /// Current invalidation epoch.
    pub invalidation_epoch: u64,
    /// Current canonical snapshot digest.
    pub snapshot_digest: Digest32,
    /// Typed canonical source basis.
    pub source_basis: SemanticSourceBasis,
    /// Active unit set selected by the generation head.
    pub unit_set_id: Uuid,
    /// Stable overview unit key.
    pub unit_key: String,
    /// Digest of the overview semantic text.
    pub semantic_text_digest: Digest32,
    /// Whether title alone or title plus summary formed the overview.
    pub summary_coverage: SemanticCoverage,
}

/// Current provenance for one active Context Document binding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemanticContextDocumentBinding {
    /// Exact hyperedge identity.
    pub edge_key: EdgeKey,
    /// Edge-local revision carrying this active relation.
    pub edge_last_context_revision: u64,
    /// Canonical Edge source-change identity.
    pub edge_source_change_id: Digest32,
    /// Binding-local Context revision.
    pub binding_context_revision: u64,
    /// Canonical Binding source-change identity.
    pub binding_source_change_id: Digest32,
    /// Current signed Binding projection event identity.
    pub binding_projection_event_id: Digest32,
}

/// Structural roles for one canonical source identity in the current graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticGraphStructuralRoles {
    /// Whether the source is a Coordinate on at least one active Edge.
    pub coordinate: bool,
    /// Whether that Coordinate role passes the selected lifecycle filter (or
    /// the explicit-initial exception).
    pub coordinate_entry_eligible: bool,
    /// Canonically sorted active incident Edges for the Coordinate role.
    /// Empty for sources without a current Coordinate role.
    pub coordinate_incident_edge_keys: Vec<EdgeKey>,
    /// Active Context Document relation entrypoints for this source.
    pub context_document_bindings: Vec<SemanticContextDocumentBinding>,
}

impl SemanticGraphStructuralRoles {
    /// Return whether at least one role may enter graph retrieval.
    pub fn has_eligible_entrypoint(&self) -> bool {
        self.coordinate_entry_eligible || !self.context_document_bindings.is_empty()
    }
}

/// Snapshot fences shared by transaction-bound hydration and input
/// observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticGraphSnapshotBinding {
    /// Host-derived Community/Project identity.
    pub community_id: CommunityId,
    /// Active semantic generation observed by the transaction ticket.
    pub generation_id: Uuid,
    /// Source-generation, embedding-space, and query-template compatibility
    /// fences carried by that exact ticket.
    pub query_fences: QueryCompatibilityFences,
    /// Overview extractor contract frozen by the active generation.
    pub extractor_version: String,
    /// Current Project Context catalog revision in the same snapshot.
    pub project_context_revision: u64,
    /// Database observation time from the Stage C ticket.
    pub observed_at: DateTime<Utc>,
}

/// Source-owned canonical preview and currentness, reconstructed from the
/// authoritative source table through its typed parser.
#[derive(Clone, PartialEq, Eq)]
pub struct SemanticCanonicalSourceSnapshot {
    /// Canonical source identity.
    pub source: SemanticSourceIdentity,
    /// Foundation invalidation epoch current in this snapshot.
    pub source_invalidation_epoch: u64,
    /// Typed source-family currentness basis.
    pub source_basis: SemanticSourceBasis,
    /// Digest of the complete typed canonical observation.
    pub source_snapshot_digest: Digest32,
    /// Current source lifecycle computed by the source adapter.
    pub lifecycle: SemanticLifecycleClass,
    /// Optional source-native status computed by the source adapter.
    pub source_status: Option<String>,
    /// Current source-owned title or name.
    pub title: String,
    /// Current source-owned summary. No source body is exposed.
    pub summary: Option<String>,
}

impl std::fmt::Debug for SemanticCanonicalSourceSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticCanonicalSourceSnapshot")
            .field("source_kind", &self.source.kind)
            .field("lifecycle", &self.lifecycle)
            .field("has_source_status", &self.source_status.is_some())
            .field("title", &"<redacted>")
            .field("title_bytes", &self.title.len())
            .field("summary", &self.summary.as_ref().map(|_| "<redacted>"))
            .field("summary_bytes", &self.summary.as_ref().map(String::len))
            .finish_non_exhaustive()
    }
}

/// One exact-current source hydrated through its canonical source adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticHydratedCurrentSource {
    /// Current source-owned material and provenance.
    pub canonical: SemanticCanonicalSourceSnapshot,
    /// Exact active-generation overview head that was hydrated.
    pub semantic_head: SemanticCurrentHead,
}

/// Deterministically ordered canonical hydration results bound to one query
/// transaction snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCanonicalHydrationBatch {
    /// Snapshot shared by every hydrated source.
    pub snapshot: SemanticGraphSnapshotBinding,
    /// Sources sorted by canonical semantic source identity.
    pub sources: Vec<SemanticHydratedCurrentSource>,
}

/// Current active-Edge membership for one explicit initial Coordinate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCoordinateGraphMembership {
    /// Project Context revision in which membership was observed.
    pub project_context_revision: u64,
    /// Canonically sorted current incident Edge identities.
    pub incident_edge_keys: Vec<EdgeKey>,
}

/// Closed semantic availability of an eligible in-graph explicit initial
/// Coordinate. Missing embeddings do not remove the explicit root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticInitialHeadState {
    /// Exact current-generation overview embedding.
    Current(SemanticCurrentHead),
    /// No exact current-generation head exists.
    Missing,
    /// Current-generation indexing is pending, claimed, or retrying.
    Building,
    /// Current-generation indexing failed or produced no queryable head.
    Failed,
    /// The active generation cannot represent this source.
    Unsupported,
}

/// Closed reason an in-graph explicit initial Coordinate cannot be a root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticInitialOmissionReason {
    /// The canonical source identity does not currently exist.
    SourceNotFound,
    /// The canonical source was hard deleted.
    SourceDeleted,
    /// The canonical source is a bodyless tombstone.
    SourceTombstoned,
    /// The canonical source is otherwise ineligible.
    SourceIneligible,
}

/// Transaction-bound observation for exactly one caller-supplied initial
/// Coordinate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticInitialCoordinateObservation {
    /// Current graph member with a current, readable canonical source.
    Accepted {
        /// Explicit initial Coordinate.
        coordinate: ProjectContextCoordinate,
        /// Current incident-Edge evidence.
        graph_membership: SemanticCoordinateGraphMembership,
        /// Canonical source material; no body is included.
        canonical: Box<SemanticCanonicalSourceSnapshot>,
        /// Current active-generation semantic availability.
        semantic_state: SemanticInitialHeadState,
    },
    /// Coordinate is not a member of any current active Edge.
    NotInGraph {
        /// Graph-external Coordinate.
        coordinate: ProjectContextCoordinate,
        /// Exact revision at which absence was observed.
        project_context_revision: u64,
    },
    /// Coordinate remains in the graph but its canonical source is unavailable.
    Omitted {
        /// Omitted Coordinate.
        coordinate: ProjectContextCoordinate,
        /// Current incident-Edge evidence.
        graph_membership: SemanticCoordinateGraphMembership,
        /// Closed source-local reason. Permission/readiness failures remain
        /// whole-request errors and never appear here.
        reason: SemanticInitialOmissionReason,
    },
}

/// Deterministically ordered initial Coordinate observations bound to one
/// Stage C snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticInitialCoordinateObservationBatch {
    /// Snapshot shared by every input observation.
    pub snapshot: SemanticGraphSnapshotBinding,
    /// One mutually exclusive observation per canonicalized input Coordinate.
    pub observations: Vec<SemanticInitialCoordinateObservation>,
}

/// Closed reason a context Coordinate could not produce a conditioned query
/// vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticContextOmissionReason {
    /// The canonical source identity does not currently exist.
    SourceNotFound,
    /// The canonical source is deleted, tombstoned, or otherwise ineligible.
    SourceIneligible,
    /// No exact current-generation head exists.
    SemanticHeadMissing,
    /// Current-generation indexing is pending, claimed, or retrying.
    SemanticHeadBuilding,
    /// Current-generation indexing failed or produced no queryable head.
    SemanticHeadFailed,
}

/// Current context Coordinate that can produce one conditioned query vector.
#[derive(Clone, PartialEq, Eq)]
pub struct SemanticAcceptedContextCoordinate {
    /// Caller-supplied context Coordinate.
    pub coordinate: ProjectContextCoordinate,
    /// Canonical source material; no body is included.
    pub canonical: SemanticCanonicalSourceSnapshot,
    /// Exact current head represented by `semantic_text`.
    pub semantic_head: SemanticCurrentHead,
    /// Relay-internal overview text. It must never enter public DTOs or logs.
    pub semantic_text: String,
}

impl std::fmt::Debug for SemanticAcceptedContextCoordinate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticAcceptedContextCoordinate")
            .field("coordinate", &"<redacted>")
            .field("canonical", &self.canonical)
            .field("semantic_text", &"<redacted>")
            .field("semantic_text_bytes", &self.semantic_text.len())
            .finish_non_exhaustive()
    }
}

/// Content-free source-currentness evidence retained for an omitted context
/// Coordinate so Stage B can detect churn even when its closed reason remains
/// unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticOmittedContextEvidence {
    /// Canonical semantic source identity derived from the Coordinate.
    pub source: SemanticSourceIdentity,
    /// Foundation invalidation epoch when a source catalog row exists.
    pub source_invalidation_epoch: Option<u64>,
    /// Typed canonical basis when the source exists and parses.
    pub source_basis: Option<SemanticSourceBasis>,
    /// Canonical observation digest when the source exists and parses.
    pub source_snapshot_digest: Option<Digest32>,
}

/// Transaction-bound observation for one context Coordinate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticContextCoordinateObservation {
    /// Current canonical overview is safe to use as a conditioned query input.
    Accepted(Box<SemanticAcceptedContextCoordinate>),
    /// Context input was omitted for a source-local closed reason.
    Omitted {
        /// Omitted context Coordinate.
        coordinate: ProjectContextCoordinate,
        /// Closed source-local reason.
        reason: SemanticContextOmissionReason,
        /// Exact content-free source evidence observed with that reason.
        evidence: SemanticOmittedContextEvidence,
    },
}

/// Deterministically ordered context Coordinate observations bound to one
/// query transaction snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticContextCoordinateObservationBatch {
    /// Snapshot shared by every input observation.
    pub snapshot: SemanticGraphSnapshotBinding,
    /// One mutually exclusive observation per canonicalized context input.
    pub observations: Vec<SemanticContextCoordinateObservation>,
}

/// Content-free exact context-head expectation carried from Stage A into the
/// atomic Stage B egress check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticContextHeadExpectation {
    /// Canonical context source identity.
    pub source: SemanticSourceIdentity,
    /// Exact current head whose semantic text would be sent to the Provider.
    pub semantic_head: SemanticCurrentHead,
}

/// Complete expected Stage A state for one context input that participates in
/// the Provider batch decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticContextEgressExpectation {
    /// Context produced one Q_i and must retain this exact head.
    Accepted(SemanticContextHeadExpectation),
    /// Context produced no Q_i and must retain the same closed reason and
    /// source epoch/basis/snapshot evidence.
    Omitted {
        /// Closed Stage A omission reason.
        reason: SemanticContextOmissionReason,
        /// Exact content-free source evidence.
        evidence: SemanticOmittedContextEvidence,
    },
}

impl SemanticContextEgressExpectation {
    /// Copy a content-free Stage B expectation from a closed Stage A
    /// observation. Callers must include every request context, including an
    /// accepted context later omitted from Provider serialization: its closed
    /// result observation and exact head still belong to the query snapshot.
    pub fn from_observation(observation: &SemanticContextCoordinateObservation) -> Self {
        match observation {
            SemanticContextCoordinateObservation::Accepted(context) => {
                Self::Accepted(SemanticContextHeadExpectation::from_accepted(context))
            }
            SemanticContextCoordinateObservation::Omitted {
                reason, evidence, ..
            } => Self::Omitted {
                reason: *reason,
                evidence: evidence.clone(),
            },
        }
    }

    fn source(&self) -> &SemanticSourceIdentity {
        match self {
            Self::Accepted(context) => &context.source,
            Self::Omitted { evidence, .. } => &evidence.source,
        }
    }
}

impl SemanticContextHeadExpectation {
    /// Copy the content-free identity/currentness fields from one accepted
    /// context observation. The internal semantic text is deliberately not
    /// retained here.
    pub fn from_accepted(context: &SemanticAcceptedContextCoordinate) -> Self {
        Self {
            source: context.canonical.source.clone(),
            semantic_head: context.semantic_head.clone(),
        }
    }
}

/// One committed Provider slot reservation.
///
/// This is deliberately not an egress authorization: its wait may outlive the
/// principal, query gate, generation, graph revision, or conditioned source
/// heads that were observed while reserving it. The Relay must obtain a fresh
/// [`SemanticGraphQueryEgressPermit`] after this wait and immediately hand that
/// permit to the Provider call.
#[derive(Debug)]
pub struct SemanticGraphQueryProviderReservation {
    wait: Duration,
    generation_id: Uuid,
    context_state_set_digest: Digest32,
}

impl SemanticGraphQueryProviderReservation {
    /// Consume the reservation into its wait and content-free identity fences.
    pub fn into_parts(self) -> (Duration, Uuid, Digest32) {
        (self.wait, self.generation_id, self.context_state_set_digest)
    }
}

/// Outcome of the atomic Stage B composite recheck and interactive admission.
#[derive(Debug)]
pub enum SemanticGraphQueryEgressReservation {
    /// All currentness/authorization checks passed and exactly one slot was
    /// committed.
    Reserved(SemanticGraphQueryProviderReservation),
    /// No slot can start before the deadline. The transaction was rolled back
    /// and neither gate table was changed.
    Busy,
    /// One or more expected context heads changed or disappeared before the
    /// egress linearization point. No Provider slot was consumed.
    ContextChanged,
    /// Principal, capability, generation, fence, or graph readiness no longer
    /// matches the ticket. No Provider slot was consumed.
    Unavailable,
}

/// One current, single-use Provider egress authorization observation.
///
/// This value is intentionally not `Clone`. It is issued only after any
/// provider-slot wait and must be consumed by the immediately following
/// Provider handoff without intervening waitable work.
#[derive(Debug)]
pub struct SemanticGraphQueryEgressPermit {
    generation_id: Uuid,
    context_state_set_digest: Digest32,
}

impl SemanticGraphQueryEgressPermit {
    /// Consume the permit into the content-free fences that must match the
    /// earlier provider reservation.
    pub fn into_parts(self) -> (Uuid, Digest32) {
        (self.generation_id, self.context_state_set_digest)
    }
}

/// Outcome of the final no-wait Stage B egress revalidation.
#[derive(Debug)]
pub enum SemanticGraphQueryEgressConfirmation {
    /// Every authorization and currentness fence still matches.
    Permitted(SemanticGraphQueryEgressPermit),
    /// One or more conditioned source observations changed during the slot
    /// wait, so the prepared query batch must be discarded.
    ContextChanged,
    /// Attested-Fleet mode no longer has a valid routing assertion for this
    /// exact serving instance. This outcome is unreachable in trusted mode.
    FleetUnavailable,
    /// Principal, capability, generation, graph, or read readiness changed.
    Unavailable,
}

/// Borrowed inputs for one atomic Stage B composite recheck and Provider
/// reservation.
pub struct SemanticGraphQueryEgressRequest<'a> {
    /// Exact Stage A ticket to revalidate.
    pub expected_ticket: &'a SemanticGraphQueryTicket,
    /// Current authenticated principal pubkey.
    pub reader_pubkey: &'a [u8],
    /// Relay projection signer required by all canonical read models.
    pub expected_projection_pubkey: &'a PublicKey,
    /// Complete accepted/omitted context state that affected the Provider
    /// channel set.
    pub expected_contexts: &'a [SemanticContextEgressExpectation],
    /// Provider identity frozen by the ticket's model contract.
    pub provider: &'a str,
    /// Physical Provider request interval.
    pub interval: Duration,
    /// Latest usable Provider start time for this request.
    pub latest_start_at: DateTime<Utc>,
}

/// Borrowed inputs for the final no-wait Provider egress revalidation.
pub struct SemanticGraphQueryEgressConfirmationRequest<'a> {
    /// Exact Stage A ticket to revalidate after the provider-slot wait.
    pub expected_ticket: &'a SemanticGraphQueryTicket,
    /// Current authenticated principal pubkey.
    pub reader_pubkey: &'a [u8],
    /// Relay projection signer required by all canonical read models.
    pub expected_projection_pubkey: &'a PublicKey,
    /// Complete accepted/omitted context state that affected the Provider
    /// channel set.
    pub expected_contexts: &'a [SemanticContextEgressExpectation],
    /// Explicit single-Relay or attested-Fleet routing requirement.
    pub routing_trust: SemanticGraphQueryRoutingTrust<'a>,
}

/// One exact source/channel cosine observation. No raw vector is returned.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticExactSourceScore {
    /// Query vector branch identity.
    pub channel_id: Digest32,
    /// Current canonical source identity.
    pub source: SemanticSourceIdentity,
    /// Exact current semantic head.
    pub head: SemanticCurrentHead,
    /// Current source lifecycle.
    pub lifecycle: SemanticLifecycleClass,
    /// Optional source-native status.
    pub source_status: Option<String>,
    /// Role-specific graph entrypoints.
    pub roles: SemanticGraphStructuralRoles,
    /// DB-quantized normalized cosine score.
    pub score: Score,
    /// Deterministic per-channel rank when this came from recall.
    pub channel_rank: u32,
}

/// Whether one independently recalled query-vector branch exhausted its
/// current authorized candidate set at the requested public K.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticExactRecallExhaustion {
    /// No hidden K+1 row exists for this branch.
    Exhausted,
    /// At least one additional candidate exists beyond the public K.
    Truncated,
}

/// Per-channel exact recall accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticExactRecallChannelObservation {
    /// Stable request-local query-vector branch identity.
    pub channel_id: Digest32,
    /// Number of public candidate rows returned for this branch.
    pub returned_count: u16,
    /// K+1 exhaustion observation. It never claims the exact hidden count.
    pub exhaustion: SemanticExactRecallExhaustion,
}

/// Exact recall candidates and complete per-channel K+1 observations.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticExactRecallBatch {
    /// Public top-K rows only; the internal K+1 sentinel never escapes here.
    pub scores: Vec<SemanticExactSourceScore>,
    /// One observation for every supplied channel, including zero-hit channels.
    pub channels: Vec<SemanticExactRecallChannelObservation>,
}

/// Current overview text used only to build a conditioned query vector.
#[derive(Clone, PartialEq, Eq)]
pub struct SemanticCurrentContextOverview {
    /// Canonical source identity.
    pub source: SemanticSourceIdentity,
    /// Exact current semantic head.
    pub head: SemanticCurrentHead,
    /// Internal Foundation overview text. It must not enter public DTOs or logs.
    pub semantic_text: String,
}

impl std::fmt::Debug for SemanticCurrentContextOverview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticCurrentContextOverview")
            .field("source", &"<redacted>")
            .field("semantic_text", &"<redacted>")
            .field("semantic_text_bytes", &self.semantic_text.len())
            .finish_non_exhaustive()
    }
}

/// Closed, mutually exclusive current-index coverage classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SemanticGraphEmbeddingCoverageClass {
    /// Exact current, queryable overview embedding.
    Current,
    /// Exact current embedding exists but has zero norm.
    NonQueryableZeroVector,
    /// Current-epoch job failed or succeeded without a complete current head.
    Failed,
    /// Current-epoch job is pending, claimed, or retrying.
    Building,
    /// Foundation marked this current source unsupported.
    Unsupported,
    /// No stronger current-generation observation exists.
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurrentSemanticAvailabilityClass {
    Current,
    Missing,
    Building,
    Failed,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CurrentSemanticSourceState {
    source: SemanticSourceIdentity,
    source_invalidation_epoch: Option<u64>,
    eligibility: Option<SemanticEligibility>,
    lifecycle: Option<SemanticLifecycleClass>,
    availability: CurrentSemanticAvailabilityClass,
    head: Option<SemanticCurrentHead>,
    semantic_text: Option<String>,
}

/// Authorized semantic graph coverage for one Stage C snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticGraphCoverage {
    /// Distinct current, readable graph source identities with an eligible role.
    pub authorized_graph_sources: u64,
    /// Authorized sources with an exact current queryable head.
    pub current_indexed_graph_sources: u64,
    /// Current indexed overviews containing title only.
    pub title_only_sources: u64,
    /// Mutually exclusive counts by current embedding state.
    pub embedding: BTreeMap<SemanticGraphEmbeddingCoverageClass, u64>,
}

/// One scalar exact distance between two exact-current source heads.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticCurrentSourcePairDistance {
    /// Left current source.
    pub left: SemanticSourceIdentity,
    /// Right current source.
    pub right: SemanticSourceIdentity,
    /// Left source head.
    pub left_head: SemanticCurrentHead,
    /// Right source head.
    pub right_head: SemanticCurrentHead,
    /// DB-quantized normalized cosine score.
    pub score: Score,
}

/// One requested pair for current source-to-source coherence scoring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCurrentSourcePair {
    /// Left source identity.
    pub left: SemanticSourceIdentity,
    /// Right source identity.
    pub right: SemanticSourceIdentity,
}

/// One conditioned query-vector branch and the Coordinate that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticTraversalConditionedChannel {
    /// Stable request-local channel identity.
    pub channel_id: Digest32,
    /// Canonical context Coordinate responsible for this branch.
    pub context_coordinate: ProjectContextCoordinate,
}

/// Complete Q0/Qi binding used by exact traversal scoring.
///
/// The vectors remain borrowed from the generation-bound Stage C session;
/// every channel identity and compatibility fence is revalidated by the DB
/// method that consumes this value.
#[derive(Debug, Clone, Copy)]
pub struct SemanticTraversalQueryChannels<'a> {
    /// Exact query vectors, including Q0 and every retained Qi.
    pub query_vectors: &'a [SemanticExactQueryVector],
    /// Channel identity of the problem-only Q0 vector.
    pub problem_channel_id: Digest32,
    /// Exact one-to-one Qi-to-Coordinate bindings.
    pub conditioned: &'a [SemanticTraversalConditionedChannel],
}

/// Expected current identity of one Hyperedge and, optionally, one selected
/// Context Document binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticHyperedgeExpectation {
    /// Exact Edge identity.
    pub edge_key: EdgeKey,
    /// Edge provenance observed by the root or relation ranking stage.
    pub edge_provenance: ProjectContextEdgeProvenance,
    /// Selected binding that must still be a member of the complete Edge.
    pub required_binding: Option<ContextDocumentBindingObservation>,
}

/// Closed result of loading one complete current Hyperedge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticHyperedgeReadOutcome {
    /// The complete exact Edge and all active binding observations.
    Current(Box<SemanticEdgeObservation>),
    /// The Edge or required binding is no longer current in this snapshot.
    Changed,
    /// The exact Edge identity exceeds the fixed 64 KiB contract boundary.
    HyperedgeTooLarge {
        /// Exact serialized identity byte count observed by the server.
        identity_bytes: usize,
    },
}

/// Closed current-source omission observed while expanding graph structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SemanticTraversalSourceOmissionReason {
    /// No canonical/semantic source record exists for the graph identity.
    SourceNotFound,
    /// The canonical source is a bodyless tombstone.
    SourceTombstoned,
    /// The canonical source was deleted or erased.
    SourceDeleted,
    /// Typed canonical validation or a source-family capability is unavailable.
    SourceIneligible,
    /// The Coordinate does not pass the request lifecycle selector.
    LifecycleFiltered,
    /// No exact current-generation overview head exists.
    SemanticHeadMissing,
    /// Current-generation indexing is pending, claimed, or retrying.
    SemanticHeadBuilding,
    /// Current-generation indexing failed or has no complete current head.
    SemanticHeadFailed,
    /// The active generation cannot represent the source.
    SemanticHeadUnsupported,
    /// Current graph/source role or authorization evidence disappeared.
    SourceNotReadable,
}

/// One unscorable current `(Edge, Context Document)` relation option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticRelationOptionOmission {
    /// Exact Edge identity.
    pub edge_key: EdgeKey,
    /// Bound Project Document identity.
    pub document_id: Uuid,
    /// Closed omission reason.
    pub reason: SemanticTraversalSourceOmissionReason,
}

/// One unscorable Coordinate member of a complete Hyperedge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticTargetOptionOmission {
    /// Complete Edge Coordinate identity; never removed from the Edge itself.
    pub coordinate: ProjectContextCoordinate,
    /// Closed continuation omission reason.
    pub reason: SemanticTraversalSourceOmissionReason,
}

/// One exact, qualified relation option after complete-set scoring.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticRankedRelationOption {
    /// Edge containing the selected relation Document.
    pub edge_key: EdgeKey,
    /// Exact current Edge provenance.
    pub edge_provenance: ProjectContextEdgeProvenance,
    /// Bound Project Document identity.
    pub document_id: Uuid,
    /// Exact current Binding provenance.
    pub binding_provenance: ProjectContextBindingProvenance,
    /// Complete Q0/Qi exact score rows for this source.
    pub channel_scores: Vec<SemanticExactSourceScore>,
    /// Exact U-D current-head coherence; absent only when U has no embedding.
    pub local_coherence: Option<SemanticCurrentSourcePairDistance>,
    /// Fixed-point relation Document score.
    pub document_score: Score,
}

/// Whether a ranked traversal slice has another qualifying item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticTraversalSliceExhaustion {
    /// No hidden `limit + 1` qualifying item was observed.
    Exhausted,
    /// At least one qualifying item remains after this slice.
    Truncated,
}

/// One ranked Coordinate-incident relation slice.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticIncidentRelationRankBatch {
    /// Snapshot shared by all observations.
    pub snapshot: SemanticGraphSnapshotBinding,
    /// Qualifying options sorted by `(DocumentScore DESC, EdgeKey, Document)`.
    pub options: Vec<SemanticRankedRelationOption>,
    /// Complete closed omissions for structurally current relation options.
    pub omitted: Vec<SemanticRelationOptionOmission>,
    /// Number of scorable options suppressed by `RELATION_FLOOR`.
    pub below_relation_floor: u32,
    /// Exact `limit + 1` continuation observation.
    pub exhaustion: SemanticTraversalSliceExhaustion,
}

/// Closed result of ranking a Coordinate's complete incident relation set.
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticIncidentRelationRankOutcome {
    /// Exact ranked slice and closed observations.
    Ranked(Box<SemanticIncidentRelationRankBatch>),
    /// The current structural option set exceeded the server scoring cap; no
    /// canonical prefix was scored or returned.
    OptionSetTooLarge {
        /// Lower bound proved by the cap-plus-one structural read.
        observed_at_least: usize,
    },
}

/// Borrowed inputs for one Coordinate-incident exact relation rank.
pub struct SemanticIncidentRelationRankRequest<'a> {
    /// Coordinate whose current incident relations are expanded.
    pub entered_from: &'a ProjectContextCoordinate,
    /// Exact Q0/Qi channel binding.
    pub channels: SemanticTraversalQueryChannels<'a>,
    /// Last emitted deterministic rank, if resuming this snapshot-local scan.
    pub after: Option<&'a RelationRankCursor>,
    /// Public slice size, bounded by the relation materialization hard cap.
    pub limit: u16,
}

/// One exact, qualified target option after complete-set scoring.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticRankedTargetOption {
    /// Complete Edge Coordinate identity.
    pub coordinate: ProjectContextCoordinate,
    /// Complete Q0/Qi exact score rows for this source.
    pub channel_scores: Vec<SemanticExactSourceScore>,
    /// Exact D-V current-head coherence and provenance.
    pub relation_document_coherence: SemanticCurrentSourcePairDistance,
    /// Fixed-point target Coordinate score.
    pub target_score: Score,
    /// Zero-absorbing harmonic Document/target transition score.
    pub transition_score: Score,
}

/// One ranked complete-Hyperedge target slice.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticEdgeTargetRankBatch {
    /// Snapshot shared by all observations.
    pub snapshot: SemanticGraphSnapshotBinding,
    /// Revalidated complete exact Edge identity.
    pub edge: SemanticEdgeObservation,
    /// Qualifying targets sorted by `(TransitionScore DESC, Coordinate)`.
    pub options: Vec<SemanticRankedTargetOption>,
    /// Complete closed omissions; the Edge identity itself remains unmodified.
    pub omitted: Vec<SemanticTargetOptionOmission>,
    /// Number of scorable targets suppressed by `TARGET_FLOOR`.
    pub below_target_floor: u32,
    /// Number passing target floor but suppressed by `TRANSITION_FLOOR`.
    pub below_transition_floor: u32,
    /// Exact `limit + 1` continuation observation.
    pub exhaustion: SemanticTraversalSliceExhaustion,
}

/// Closed result of ranking targets from one exact Edge/Document pair.
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticEdgeTargetRankOutcome {
    /// Exact ranked slice and closed observations.
    Ranked(Box<SemanticEdgeTargetRankBatch>),
    /// Edge or selected binding provenance changed in this snapshot.
    HyperedgeChanged,
    /// Complete Edge identity exceeds the fixed 64 KiB contract boundary.
    HyperedgeTooLarge {
        /// Exact serialized identity byte count observed by the server.
        identity_bytes: usize,
    },
    /// The complete target set exceeded the server scoring cap; no canonical
    /// prefix was scored or returned.
    OptionSetTooLarge {
        /// Lower bound proved by the cap-plus-one structural observation.
        observed_at_least: usize,
    },
}

/// Borrowed inputs for one complete-Hyperedge exact target rank.
pub struct SemanticEdgeTargetRankRequest<'a> {
    /// Exact Edge and binding identity observed before target expansion.
    pub hyperedge: &'a SemanticEdgeObservation,
    /// Selected relation Document identity.
    pub relation_document_id: Uuid,
    /// Exact current head used to score the selected relation Document.
    pub relation_document_head: &'a SemanticCurrentHead,
    /// Document score used by the transition harmonic mean.
    pub document_score: Score,
    /// Lifecycle selector applied only to continued target Coordinates.
    pub lifecycle_filter: LifecycleFilter,
    /// Exact Q0/Qi channel binding.
    pub channels: SemanticTraversalQueryChannels<'a>,
    /// Last emitted deterministic rank, if resuming this snapshot-local scan.
    pub after: Option<&'a TargetRankCursor>,
    /// Public slice size, bounded by the target materialization hard cap.
    pub limit: u16,
}

/// One current, single-use Stage D result-release authorization.
///
/// This value is intentionally not `Clone`. It is issued only after the
/// Community authorization writers, query/index gate, canonical read-model
/// readiness, and the selected routing policy have been linearized in one
/// short transaction. The Relay must consume it in the immediately following
/// synchronous signing operation without intervening awaitable work.
#[derive(Debug)]
pub struct SemanticGraphQueryReleasePermit {
    _private: (),
}

/// Outcome of the final Stage D result-release confirmation.
#[derive(Debug)]
pub enum SemanticGraphQueryReleaseConfirmation {
    /// Every current authorization, query/index, canonical-read, and selected
    /// routing-policy fence passed at the release linearization point.
    Permitted(SemanticGraphQueryReleasePermit),
    /// The principal or one of the current query/read prerequisites is no
    /// longer authorized.
    Denied,
    /// An optional caller-supplied generation/Context snapshot no longer
    /// matches the current canonical query ticket.
    SnapshotChanged,
    /// Attested-Fleet mode no longer authorizes this exact serving instance.
    /// This outcome is unreachable in trusted mode.
    FleetUnavailable,
}

/// Borrowed inputs for the final Stage D result-release confirmation.
pub struct SemanticGraphQueryReleaseRequest<'a> {
    /// Host-derived Community/Project identity.
    pub community_id: CommunityId,
    /// Current authenticated principal pubkey.
    pub reader_pubkey: &'a [u8],
    /// Relay projection signer required by all canonical read models.
    pub expected_projection_pubkey: &'a PublicKey,
    /// Optional exact Stage C snapshot that must still own the active
    /// generation, projection generation, and Context revision. Existing
    /// graph-query callers may omit this to preserve their snapshot contract.
    pub expected_snapshot: Option<&'a SemanticGraphQueryTicket>,
    /// Explicit single-Relay or attested-Fleet routing requirement.
    pub routing_trust: SemanticGraphQueryRoutingTrust<'a>,
}

/// Caller-owned writer-pool `REPEATABLE READ READ ONLY` transaction.
///
/// The principal, capability gates, active generation, and projection
/// readiness are checked before this value is returned. Every distance method
/// additionally binds its SQL to the same principal and expected generation.
pub struct SemanticGraphReadTx {
    pub(crate) tx: Transaction<'static, Postgres>,
    pub(crate) ticket: SemanticGraphQueryTicket,
    pub(crate) reader_pubkey: Vec<u8>,
    pub(crate) expected_projection_pubkey: PublicKey,
}

impl std::fmt::Debug for SemanticGraphReadTx {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticGraphReadTx")
            .field("community_id", &self.ticket.community_id)
            .field("generation_id", &self.ticket.generation.generation_id)
            .finish_non_exhaustive()
    }
}

impl Db {
    /// Create a short writer-database query ticket after current principal and
    /// complete Project Context/canonical-source readiness checks.
    pub async fn semantic_graph_query_ticket(
        &self,
        community_id: CommunityId,
        reader_pubkey: &[u8],
        expected_projection_pubkey: &PublicKey,
    ) -> Result<SemanticGraphQueryTicket> {
        validate_pubkey(reader_pubkey)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let ticket = load_authorized_ticket_in_tx(
            &mut tx,
            community_id,
            reader_pubkey,
            expected_projection_pubkey,
        )
        .await?;
        tx.commit().await?;
        Ok(ticket)
    }

    /// Atomically confirm the current Stage D security/read/routing
    /// prerequisites and issue a single-use result-release permit.
    ///
    /// The transaction deliberately uses `READ COMMITTED`: it first waits for
    /// the same shared per-Community advisory lock that canonical membership,
    /// ban, Project View, Document, and Context writers take exclusively, and
    /// only then forms the snapshots used by the following checks. Starting a
    /// `REPEATABLE READ` snapshot before that wait would allow an already
    /// committed revocation to remain invisible. The Community row and fleet
    /// row and, in strict mode, the Fleet assertion are also held `FOR SHARE`,
    /// closing query/index and routing-policy mutation races through commit.
    pub async fn confirm_semantic_graph_query_release(
        &self,
        request: SemanticGraphQueryReleaseRequest<'_>,
    ) -> Result<SemanticGraphQueryReleaseConfirmation> {
        let SemanticGraphQueryReleaseRequest {
            community_id,
            reader_pubkey,
            expected_projection_pubkey,
            expected_snapshot,
            routing_trust,
        } = request;
        validate_pubkey(reader_pubkey)?;
        let mut tx = self
            .begin_semantic_graph_final_confirmation(community_id)
            .await?;
        let locked_community =
            lock_semantic_graph_query_community_row(&mut tx, community_id).await?;
        if !locked_community {
            tx.rollback().await?;
            return Ok(SemanticGraphQueryReleaseConfirmation::Denied);
        }
        let current_ticket = match load_authorized_ticket_in_tx(
            &mut tx,
            community_id,
            reader_pubkey,
            expected_projection_pubkey,
        )
        .await
        {
            Ok(ticket) => ticket,
            Err(DbError::AccessDenied(_)) | Err(DbError::NotFound(_)) => {
                tx.rollback().await?;
                return Ok(SemanticGraphQueryReleaseConfirmation::Denied);
            }
            Err(error) => return Err(error),
        };
        if expected_snapshot
            .is_some_and(|expected| !same_release_snapshot(&current_ticket, expected))
        {
            tx.rollback().await?;
            return Ok(SemanticGraphQueryReleaseConfirmation::SnapshotChanged);
        }
        if !crate::semantic_fleet::semantic_graph_query_routing_ready_in_tx(
            &mut tx,
            community_id,
            routing_trust,
        )
        .await?
        {
            tx.rollback().await?;
            return Ok(SemanticGraphQueryReleaseConfirmation::FleetUnavailable);
        }
        tx.commit().await?;
        Ok(SemanticGraphQueryReleaseConfirmation::Permitted(
            SemanticGraphQueryReleasePermit { _private: () },
        ))
    }

    async fn begin_semantic_graph_final_confirmation(
        &self,
        community_id: CommunityId,
    ) -> Result<Transaction<'static, Postgres>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(FINAL_CONFIRMATION_ISOLATION_SQL)
            .execute(&mut *tx)
            .await?;
        // This lock statement may wait, but READ COMMITTED does not retain its
        // statement snapshot. Every authorization/currentness statement below
        // therefore observes commits that completed before this shared lock
        // was granted. Canonical writers hold the exclusive form through their
        // own commit.
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        Ok(tx)
    }

    /// Atomically recheck Stage B authorization/currentness and reserve one
    /// interactive Provider slot.
    ///
    /// The Community row and every expected semantic source row remain locked
    /// from their current observations through the reservation commit. Thus a
    /// query-disable/generation change or context-source change either commits
    /// before these checks and rejects the reservation, or follows the
    /// committed reservation. Because the returned wait is not authorization,
    /// callers must revalidate with [`Db::confirm_semantic_graph_query_egress`]
    /// after waiting. `Busy` rolls the whole transaction back and leaves both
    /// gate tables unchanged.
    pub async fn reserve_semantic_graph_query_egress(
        &self,
        request: SemanticGraphQueryEgressRequest<'_>,
    ) -> Result<SemanticGraphQueryEgressReservation> {
        let SemanticGraphQueryEgressRequest {
            expected_ticket,
            reader_pubkey,
            expected_projection_pubkey,
            expected_contexts,
            provider,
            interval,
            latest_start_at,
        } = request;
        let expected_contexts =
            validate_egress_expectations(expected_ticket, reader_pubkey, expected_contexts)?;
        if provider != expected_ticket.generation.model_contract.provider {
            return Err(DbError::InvalidData(
                "semantic egress provider does not match the query ticket".to_string(),
            ));
        }

        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
            .execute(&mut *tx)
            .await?;
        let context_state_set_digest = match recheck_semantic_graph_query_egress_in_tx(
            &mut tx,
            expected_ticket,
            reader_pubkey,
            expected_projection_pubkey,
            &expected_contexts,
        )
        .await?
        {
            SemanticGraphQueryEgressRecheck::Ready {
                context_state_set_digest,
            } => context_state_set_digest,
            SemanticGraphQueryEgressRecheck::ContextChanged => {
                tx.rollback().await?;
                return Ok(SemanticGraphQueryEgressReservation::ContextChanged);
            }
            SemanticGraphQueryEgressRecheck::Unavailable => {
                tx.rollback().await?;
                return Ok(SemanticGraphQueryEgressReservation::Unavailable);
            }
        };
        let reservation = reserve_semantic_provider_slot_in_tx(
            &mut tx,
            expected_ticket.community_id,
            provider,
            SemanticProviderWorkload::InteractiveQuery,
            interval,
            latest_start_at,
        )
        .await?;
        let SemanticProviderReservation::Reserved { wait } = reservation else {
            tx.rollback().await?;
            return Ok(SemanticGraphQueryEgressReservation::Busy);
        };
        tx.commit().await?;
        Ok(SemanticGraphQueryEgressReservation::Reserved(
            SemanticGraphQueryProviderReservation {
                wait,
                generation_id: expected_ticket.generation.generation_id,
                context_state_set_digest,
            },
        ))
    }

    /// Revalidate the complete Provider egress authorization after the
    /// provider-slot wait and issue a single-use no-wait permit.
    ///
    /// The caller must perform all other waitable readiness work before this
    /// method, then pass a successful permit directly into the Provider call.
    /// A slot reserved before this check remains consumed when the check fails;
    /// it is rate-limit capacity, not authorization.
    pub async fn confirm_semantic_graph_query_egress(
        &self,
        request: SemanticGraphQueryEgressConfirmationRequest<'_>,
    ) -> Result<SemanticGraphQueryEgressConfirmation> {
        let SemanticGraphQueryEgressConfirmationRequest {
            expected_ticket,
            reader_pubkey,
            expected_projection_pubkey,
            expected_contexts,
            routing_trust,
        } = request;
        let expected_contexts =
            validate_egress_expectations(expected_ticket, reader_pubkey, expected_contexts)?;

        let mut tx = self
            .begin_semantic_graph_final_confirmation(expected_ticket.community_id)
            .await?;
        match recheck_semantic_graph_query_egress_in_tx(
            &mut tx,
            expected_ticket,
            reader_pubkey,
            expected_projection_pubkey,
            &expected_contexts,
        )
        .await?
        {
            SemanticGraphQueryEgressRecheck::Ready {
                context_state_set_digest,
            } => {
                if !crate::semantic_fleet::semantic_graph_query_routing_ready_in_tx(
                    &mut tx,
                    expected_ticket.community_id,
                    routing_trust,
                )
                .await?
                {
                    tx.rollback().await?;
                    return Ok(SemanticGraphQueryEgressConfirmation::FleetUnavailable);
                }
                tx.commit().await?;
                Ok(SemanticGraphQueryEgressConfirmation::Permitted(
                    SemanticGraphQueryEgressPermit {
                        generation_id: expected_ticket.generation.generation_id,
                        context_state_set_digest,
                    },
                ))
            }
            SemanticGraphQueryEgressRecheck::ContextChanged => {
                tx.rollback().await?;
                Ok(SemanticGraphQueryEgressConfirmation::ContextChanged)
            }
            SemanticGraphQueryEgressRecheck::Unavailable => {
                tx.rollback().await?;
                Ok(SemanticGraphQueryEgressConfirmation::Unavailable)
            }
        }
    }

    /// Begin the writer-pool Stage C snapshot and recheck the ticket's active
    /// generation plus the current principal and all read prerequisites.
    pub async fn begin_semantic_graph_read(
        &self,
        expected_ticket: &SemanticGraphQueryTicket,
        reader_pubkey: &[u8],
        expected_projection_pubkey: PublicKey,
        timeouts: SemanticGraphReadTimeouts,
    ) -> Result<SemanticGraphReadTx> {
        validate_pubkey(reader_pubkey)?;
        validate_timeouts(timeouts)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        set_local_timeout(&mut tx, "statement_timeout", timeouts.statement).await?;
        set_local_timeout(&mut tx, "lock_timeout", timeouts.lock).await?;
        set_local_timeout(
            &mut tx,
            "idle_in_transaction_session_timeout",
            timeouts.idle_in_transaction,
        )
        .await?;
        let ticket = load_authorized_ticket_in_tx(
            &mut tx,
            expected_ticket.community_id,
            reader_pubkey,
            &expected_projection_pubkey,
        )
        .await?;
        if !same_generation_contract(&ticket, expected_ticket) {
            return Err(unavailable());
        }
        Ok(SemanticGraphReadTx {
            tx,
            ticket,
            reader_pubkey: reader_pubkey.to_vec(),
            expected_projection_pubkey,
        })
    }
}

impl SemanticGraphReadTx {
    /// Return the exact Stage C snapshot observation.
    pub const fn ticket(&self) -> &SemanticGraphQueryTicket {
        &self.ticket
    }

    /// Commit the read-only transaction.
    pub async fn commit(self) -> Result<()> {
        self.tx.commit().await.map_err(Into::into)
    }

    /// Explicitly roll back the read-only transaction.
    pub async fn rollback(self) -> Result<()> {
        self.tx.rollback().await.map_err(Into::into)
    }

    /// Load exact-current overview text for conditioned context Coordinates.
    ///
    /// The returned text is restricted to Relay-internal query orchestration;
    /// callers must not serialize or log it.
    pub async fn load_current_context_overviews(
        &mut self,
        sources: &[SemanticSourceIdentity],
    ) -> Result<Vec<SemanticCurrentContextOverview>> {
        validate_source_inputs(
            self.ticket.community_id,
            sources,
            MAX_CONTEXT_OVERVIEWS,
            "context source",
        )?;
        if sources.is_empty() {
            return Ok(Vec::new());
        }
        let requested = source_key_arrays(sources);
        let dimensions = i32::try_from(self.ticket.generation.model_contract.dimensions)
            .map_err(|_| DbError::InvalidData("semantic dimensions exceed int4".to_string()))?;
        let rows = sqlx::query(CURRENT_CONTEXT_OVERVIEWS_SQL)
            .bind(self.ticket.community_id.as_uuid())
            .bind(self.reader_pubkey.as_slice())
            .bind(self.expected_projection_pubkey.as_bytes())
            .bind(self.ticket.generation.generation_id)
            .bind(
                self.ticket
                    .generation
                    .model_contract_digest
                    .as_bytes()
                    .as_slice(),
            )
            .bind(self.ticket.generation.extractor_version.as_str())
            .bind(self.ticket.generation.model_contract.model.as_str())
            .bind(dimensions)
            .bind(&requested.families)
            .bind(&requested.subtypes)
            .bind(&requested.ids)
            .fetch_all(&mut *self.tx)
            .await?;
        rows.iter()
            .map(|row| {
                let source =
                    source_identity_from_row(row, "source_family", "source_subtype", "source_id")?;
                let head = semantic_head_from_row(row, "")?;
                validate_head_for_source(&head, &source)?;
                Ok(SemanticCurrentContextOverview {
                    source,
                    head,
                    semantic_text: row.try_get("semantic_text")?,
                })
            })
            .collect()
    }

    /// Hydrate exact-current scored sources through their authoritative
    /// source-family tables and typed parsers.
    ///
    /// Repeated per-channel hits are deduplicated by source identity. Every
    /// supplied epoch/basis/snapshot/head is rechecked in this transaction;
    /// stale or caller-forged score observations fail closed. Source body and
    /// internal semantic text are never returned.
    pub async fn hydrate_current_exact_sources(
        &mut self,
        scored_sources: &[SemanticExactSourceScore],
    ) -> Result<SemanticCanonicalHydrationBatch> {
        if scored_sources.len() > MAX_SCORE_MATRIX_SOURCES * MAX_QUERY_CHANNELS {
            return Err(DbError::InvalidData(
                "semantic hydration score count exceeds the server bound".to_string(),
            ));
        }

        let mut expected = BTreeMap::new();
        for scored in scored_sources {
            validate_source_inputs(
                self.ticket.community_id,
                std::slice::from_ref(&scored.source),
                1,
                "hydration source",
            )?;
            let key = semantic_source_sort_key(&scored.source);
            if let Some(previous) = expected.get(&key) {
                let previous: &&SemanticExactSourceScore = previous;
                if previous.source != scored.source
                    || previous.head != scored.head
                    || previous.lifecycle != scored.lifecycle
                    || previous.source_status != scored.source_status
                    || previous.roles != scored.roles
                {
                    return Err(DbError::InvalidData(
                        "semantic hydration contains conflicting observations for one source"
                            .to_string(),
                    ));
                }
            } else {
                expected.insert(key, scored);
            }
        }

        let sources: Vec<SemanticSourceIdentity> = expected
            .values()
            .map(|scored| scored.source.clone())
            .collect();
        let states = self
            .load_current_semantic_source_states(&sources, MAX_SCORE_MATRIX_SOURCES)
            .await?;
        if states.len() != sources.len() {
            return Err(DbError::InvalidData(
                "semantic hydration state result is incomplete".to_string(),
            ));
        }

        let mut hydrated = Vec::with_capacity(sources.len());
        for ((_, expected), state) in expected.into_iter().zip(states) {
            if state.source != expected.source
                || state.availability != CurrentSemanticAvailabilityClass::Current
                || state.head.as_ref() != Some(&expected.head)
            {
                return Err(DbError::InvalidData(
                    "semantic exact source is no longer bound to its supplied current head"
                        .to_string(),
                ));
            }
            let invalidation_epoch = state.source_invalidation_epoch.ok_or_else(|| {
                DbError::InvalidData(
                    "semantic current source lacks an invalidation epoch".to_string(),
                )
            })?;
            let observation = self
                .observe_optional_canonical_source(&expected.source)
                .await?
                .ok_or_else(|| {
                    DbError::InvalidData(
                        "semantic exact source disappeared from canonical storage".to_string(),
                    )
                })?;
            ensure_eligible_canonical_observation(&observation)?;
            validate_canonical_against_head(&observation, &expected.head)?;
            if observation.filter.lifecycle != expected.lifecycle
                || observation.filter.source_status != expected.source_status
            {
                return Err(DbError::InvalidData(
                    "semantic exact source filter metadata disagrees with its canonical source"
                        .to_string(),
                ));
            }
            hydrated.push(SemanticHydratedCurrentSource {
                canonical: canonical_snapshot(observation, invalidation_epoch),
                semantic_head: expected.head.clone(),
            });
        }

        Ok(SemanticCanonicalHydrationBatch {
            snapshot: self.snapshot_binding(),
            sources: hydrated,
        })
    }

    /// Observe every explicit initial Coordinate against current active graph
    /// membership and its authoritative canonical source, even when no current
    /// embedding exists.
    pub async fn observe_initial_coordinates(
        &mut self,
        coordinates: &[ProjectContextCoordinate],
    ) -> Result<SemanticInitialCoordinateObservationBatch> {
        let coordinates = canonical_query_coordinates(
            self.ticket.community_id,
            coordinates,
            MAX_INITIAL_COORDINATES,
            "initial Coordinate",
        )?;
        if coordinates.is_empty() {
            return Ok(SemanticInitialCoordinateObservationBatch {
                snapshot: self.snapshot_binding(),
                observations: Vec::new(),
            });
        }
        let memberships = self.load_coordinate_memberships(&coordinates).await?;
        if memberships.len() != coordinates.len() {
            return Err(DbError::InvalidData(
                "semantic initial membership result is incomplete".to_string(),
            ));
        }
        let sources: Vec<SemanticSourceIdentity> = coordinates
            .iter()
            .map(|coordinate| {
                semantic_source_identity_for_coordinate(self.ticket.community_id, coordinate)
            })
            .collect::<Result<_>>()?;
        let states = self
            .load_current_semantic_source_states(&sources, MAX_INITIAL_COORDINATES)
            .await?;
        if states.len() != coordinates.len() {
            return Err(DbError::InvalidData(
                "semantic initial source-state result is incomplete".to_string(),
            ));
        }

        let mut observations = Vec::with_capacity(coordinates.len());
        for (((coordinate, source), membership), state) in coordinates
            .into_iter()
            .zip(sources)
            .zip(memberships)
            .zip(states)
        {
            if state.source != source {
                return Err(DbError::InvalidData(
                    "semantic initial source-state order is inconsistent".to_string(),
                ));
            }
            if membership.incident_edge_keys.is_empty() {
                observations.push(SemanticInitialCoordinateObservation::NotInGraph {
                    coordinate,
                    project_context_revision: self.ticket.project_context_revision,
                });
                continue;
            }
            let canonical = self.observe_optional_canonical_source(&source).await?;
            let Some(canonical) = canonical else {
                observations.push(SemanticInitialCoordinateObservation::Omitted {
                    coordinate,
                    graph_membership: membership,
                    reason: SemanticInitialOmissionReason::SourceNotFound,
                });
                continue;
            };
            if let SemanticEligibility::Ineligible(reason) = canonical.eligibility {
                observations.push(SemanticInitialCoordinateObservation::Omitted {
                    coordinate,
                    graph_membership: membership,
                    reason: initial_omission_reason(reason),
                });
                continue;
            }
            let invalidation_epoch = state.source_invalidation_epoch.ok_or_else(|| {
                DbError::InvalidData(
                    "eligible semantic initial source lacks an invalidation epoch".to_string(),
                )
            })?;
            let semantic_state = match state.availability {
                CurrentSemanticAvailabilityClass::Current => {
                    let head = state.head.ok_or_else(|| {
                        DbError::InvalidData(
                            "current semantic initial source lacks its head".to_string(),
                        )
                    })?;
                    validate_canonical_against_head(&canonical, &head)?;
                    SemanticInitialHeadState::Current(head)
                }
                CurrentSemanticAvailabilityClass::Missing => SemanticInitialHeadState::Missing,
                CurrentSemanticAvailabilityClass::Building => SemanticInitialHeadState::Building,
                CurrentSemanticAvailabilityClass::Failed => SemanticInitialHeadState::Failed,
                CurrentSemanticAvailabilityClass::Unsupported => {
                    SemanticInitialHeadState::Unsupported
                }
            };
            observations.push(SemanticInitialCoordinateObservation::Accepted {
                coordinate,
                graph_membership: membership,
                canonical: Box::new(canonical_snapshot(canonical, invalidation_epoch)),
                semantic_state,
            });
        }

        Ok(SemanticInitialCoordinateObservationBatch {
            snapshot: self.snapshot_binding(),
            observations,
        })
    }

    /// Observe context Coordinates through canonical source adapters and
    /// return a closed accepted/omitted result for every input.
    ///
    /// Accepted values carry Relay-internal semantic text so Stage A/B/C can
    /// build and revalidate Q_i without interpreting a missing row as a single
    /// ambiguous state. Permission or capability-readiness failures remain
    /// whole-request errors established by the transaction ticket.
    pub async fn observe_context_coordinates(
        &mut self,
        coordinates: &[ProjectContextCoordinate],
    ) -> Result<SemanticContextCoordinateObservationBatch> {
        let coordinates = canonical_query_coordinates(
            self.ticket.community_id,
            coordinates,
            MAX_CONTEXT_COORDINATES,
            "context Coordinate",
        )?;
        if coordinates.is_empty() {
            return Ok(SemanticContextCoordinateObservationBatch {
                snapshot: self.snapshot_binding(),
                observations: Vec::new(),
            });
        }
        let sources: Vec<SemanticSourceIdentity> = coordinates
            .iter()
            .map(|coordinate| {
                semantic_source_identity_for_coordinate(self.ticket.community_id, coordinate)
            })
            .collect::<Result<_>>()?;
        let states = self
            .load_current_semantic_source_states(&sources, MAX_CONTEXT_COORDINATES)
            .await?;
        if states.len() != sources.len() {
            return Err(DbError::InvalidData(
                "semantic context source-state result is incomplete".to_string(),
            ));
        }

        let mut observations = Vec::with_capacity(coordinates.len());
        for ((coordinate, source), state) in coordinates.into_iter().zip(sources).zip(states) {
            if state.source != source {
                return Err(DbError::InvalidData(
                    "semantic context source-state order is inconsistent".to_string(),
                ));
            }
            let canonical = self.observe_optional_canonical_source(&source).await?;
            let Some(canonical) = canonical else {
                observations.push(SemanticContextCoordinateObservation::Omitted {
                    coordinate,
                    reason: SemanticContextOmissionReason::SourceNotFound,
                    evidence: omitted_context_evidence(&source, &state, None),
                });
                continue;
            };
            if !matches!(canonical.eligibility, SemanticEligibility::Eligible) {
                observations.push(SemanticContextCoordinateObservation::Omitted {
                    coordinate,
                    reason: SemanticContextOmissionReason::SourceIneligible,
                    evidence: omitted_context_evidence(&source, &state, Some(&canonical)),
                });
                continue;
            }
            let reason = match state.availability {
                CurrentSemanticAvailabilityClass::Missing => {
                    Some(SemanticContextOmissionReason::SemanticHeadMissing)
                }
                CurrentSemanticAvailabilityClass::Building => {
                    Some(SemanticContextOmissionReason::SemanticHeadBuilding)
                }
                CurrentSemanticAvailabilityClass::Failed => {
                    Some(SemanticContextOmissionReason::SemanticHeadFailed)
                }
                CurrentSemanticAvailabilityClass::Unsupported => {
                    Some(SemanticContextOmissionReason::SourceIneligible)
                }
                CurrentSemanticAvailabilityClass::Current => None,
            };
            if let Some(reason) = reason {
                observations.push(SemanticContextCoordinateObservation::Omitted {
                    coordinate,
                    reason,
                    evidence: omitted_context_evidence(&source, &state, Some(&canonical)),
                });
                continue;
            }
            let invalidation_epoch = state.source_invalidation_epoch.ok_or_else(|| {
                DbError::InvalidData(
                    "current semantic context source lacks an invalidation epoch".to_string(),
                )
            })?;
            let head = state.head.ok_or_else(|| {
                DbError::InvalidData("current semantic context source lacks its head".to_string())
            })?;
            validate_canonical_against_head(&canonical, &head)?;
            let semantic_text = state.semantic_text.ok_or_else(|| {
                DbError::InvalidData(
                    "current semantic context source lacks overview text".to_string(),
                )
            })?;
            if semantic_text.trim().is_empty() || semantic_text.contains('\0') {
                return Err(DbError::InvalidData(
                    "current semantic context overview text is invalid".to_string(),
                ));
            }
            observations.push(SemanticContextCoordinateObservation::Accepted(Box::new(
                SemanticAcceptedContextCoordinate {
                    coordinate,
                    canonical: canonical_snapshot(canonical, invalidation_epoch),
                    semantic_head: head,
                    semantic_text,
                },
            )));
        }

        Ok(SemanticContextCoordinateObservationBatch {
            snapshot: self.snapshot_binding(),
            observations,
        })
    }

    /// Recall a deterministic top-K independently for every query vector
    /// branch using one materialized current/authorized eligible set.
    pub async fn recall_current_graph_sources_exact(
        &mut self,
        lifecycle_filter: LifecycleFilter,
        explicit_initial_sources: &[SemanticSourceIdentity],
        query_vectors: &[SemanticExactQueryVector],
        recall_per_channel: u16,
    ) -> Result<SemanticExactRecallBatch> {
        if recall_per_channel == 0 || recall_per_channel > MAX_RECALL_PER_CHANNEL {
            return Err(DbError::InvalidData(
                "semantic recall_per_channel is outside the server bound".to_string(),
            ));
        }
        let observed_limit = u32::from(recall_per_channel)
            .checked_add(1)
            .ok_or_else(|| {
                DbError::InvalidData("semantic recall observation limit overflow".to_string())
            })?;
        let observed = self
            .query_exact_source_scores(
                lifecycle_filter,
                explicit_initial_sources,
                query_vectors,
                None,
                Some(observed_limit),
            )
            .await?;
        let channel_ids: Vec<Digest32> = query_vectors
            .iter()
            .map(|channel| channel.channel_id)
            .collect();
        partition_exact_recall(&channel_ids, observed, recall_per_channel)
    }

    /// Compute the complete channel/source scalar matrix for a bounded source
    /// identity union. Sources are re-resolved through exact current heads;
    /// stale or newly ineligible identities disappear rather than being scored.
    pub async fn score_candidate_matrix_exact(
        &mut self,
        lifecycle_filter: LifecycleFilter,
        explicit_initial_sources: &[SemanticSourceIdentity],
        query_vectors: &[SemanticExactQueryVector],
        candidates: &[SemanticSourceIdentity],
    ) -> Result<Vec<SemanticExactSourceScore>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        if candidates.len() > MAX_SCORE_MATRIX_SOURCES {
            return Err(DbError::InvalidData(
                "semantic score matrix source count exceeds the server bound".to_string(),
            ));
        }
        self.query_exact_source_scores(
            lifecycle_filter,
            explicit_initial_sources,
            query_vectors,
            Some(candidates),
            None,
        )
        .await
    }

    /// Load one complete current Hyperedge and all active Binding provenance.
    /// Coordinates are never lifecycle- or semantic-readiness-filtered.
    pub async fn load_complete_hyperedge(
        &mut self,
        expectation: &SemanticHyperedgeExpectation,
    ) -> Result<SemanticHyperedgeReadOutcome> {
        let Some(edge) =
            load_complete_hyperedge_in_tx(&mut self.tx, &self.ticket, expectation.edge_key).await?
        else {
            return Ok(SemanticHyperedgeReadOutcome::Changed);
        };
        if edge.provenance != expectation.edge_provenance
            || expectation
                .required_binding
                .as_ref()
                .is_some_and(|required| !edge.current_context_document_bindings.contains(required))
        {
            return Ok(SemanticHyperedgeReadOutcome::Changed);
        }
        let identity_bytes = semantic_hyperedge_identity_bytes(&edge)?;
        if identity_bytes > MAX_HYPEREDGE_IDENTITY_BYTES {
            return Ok(SemanticHyperedgeReadOutcome::HyperedgeTooLarge { identity_bytes });
        }
        Ok(SemanticHyperedgeReadOutcome::Current(Box::new(edge)))
    }

    /// Rank every current relation Document incident to one Coordinate before
    /// applying a deterministic keyset slice. Structural refs are loaded in
    /// full up to the server cap; a canonical prefix is never preselected for
    /// semantic scoring.
    pub async fn rank_incident_relation_options_exact(
        &mut self,
        request: SemanticIncidentRelationRankRequest<'_>,
    ) -> Result<SemanticIncidentRelationRankOutcome> {
        validate_coordinate(self.ticket.community_id, request.entered_from)?;
        validate_traversal_channels(&self.ticket, request.channels)?;
        validate_traversal_limit(request.limit, MAX_RELATION_OPTIONS_MATERIALIZED, "relation")?;
        let refs = load_incident_relation_refs_in_tx(
            &mut self.tx,
            &self.ticket,
            request.entered_from,
            MAX_RELATION_SCORE_SET + 1,
        )
        .await?;
        if refs.len() > MAX_RELATION_SCORE_SET {
            return Ok(SemanticIncidentRelationRankOutcome::OptionSetTooLarge {
                observed_at_least: refs.len(),
            });
        }
        if refs.is_empty() {
            return Ok(SemanticIncidentRelationRankOutcome::Ranked(Box::new(
                SemanticIncidentRelationRankBatch {
                    snapshot: self.snapshot_binding(),
                    options: Vec::new(),
                    omitted: Vec::new(),
                    below_relation_floor: 0,
                    exhaustion: SemanticTraversalSliceExhaustion::Exhausted,
                },
            )));
        }

        let sources = refs
            .iter()
            .map(|item| project_document_source(self.ticket.community_id, item.document_id))
            .collect::<Vec<_>>();
        validate_unique_sources(&sources, "incident relation")?;
        let matrix = self
            .query_exact_source_scores(
                LifecycleFilter::AllCurrent,
                &[],
                request.channels.query_vectors,
                Some(&sources),
                None,
            )
            .await?;
        let scored = traversal_source_score_sets(request.channels, matrix)?;
        let entered_source = semantic_source_identity_for_coordinate(
            self.ticket.community_id,
            request.entered_from,
        )?;
        let entered_state = self
            .load_current_semantic_source_states(std::slice::from_ref(&entered_source), 1)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                DbError::InvalidData(
                    "semantic entered Coordinate source state is missing".to_string(),
                )
            })?;
        let pairs = scored
            .keys()
            .map(|source| SemanticCurrentSourcePair {
                left: source.clone(),
                right: entered_source.clone(),
            })
            .collect::<Vec<_>>();
        let coherences = self.score_current_source_pairs_exact(&pairs).await?;
        let coherences = pair_scores_by_left(coherences, Some(&entered_source), None)?;
        let entered_has_current_head = matches!(
            entered_state.availability,
            CurrentSemanticAvailabilityClass::Current
        ) && matches!(
            entered_state.eligibility,
            Some(SemanticEligibility::Eligible)
        );
        if (entered_has_current_head && coherences.len() != scored.len())
            || (!entered_has_current_head && !coherences.is_empty())
        {
            return Err(DbError::InvalidData(
                "semantic U-D coherence completeness disagrees with entered Coordinate head"
                    .to_string(),
            ));
        }
        let states = self
            .load_current_semantic_source_states(&sources, MAX_RELATION_SCORE_SET)
            .await?;

        let mut options = Vec::with_capacity(scored.len());
        let mut omitted = Vec::new();
        let mut below_relation_floor = 0_u32;
        for (relation, state) in refs.into_iter().zip(states) {
            let source = project_document_source(self.ticket.community_id, relation.document_id);
            let Some(source_scores) = scored.get(&source) else {
                omitted.push(SemanticRelationOptionOmission {
                    edge_key: relation.edge_key,
                    document_id: relation.document_id,
                    reason: traversal_omission_reason(&state, false, LifecycleFilter::AllCurrent),
                });
                continue;
            };
            let representative = source_scores.channel_scores.first().ok_or_else(|| {
                DbError::InvalidData("semantic relation score set is empty".to_string())
            })?;
            let expected_binding = SemanticContextDocumentBinding {
                edge_key: relation.edge_key,
                edge_last_context_revision: relation.edge_provenance.last_context_revision,
                edge_source_change_id: relation.edge_provenance.source_change_id,
                binding_context_revision: relation.binding_provenance.binding_context_revision,
                binding_source_change_id: relation.binding_provenance.source_change_id,
                binding_projection_event_id: relation.binding_provenance.projection_event_id,
            };
            if !representative
                .roles
                .context_document_bindings
                .contains(&expected_binding)
            {
                return Err(DbError::InvalidData(
                    "semantic relation score lost its exact Binding role".to_string(),
                ));
            }
            let local_coherence = coherences.get(&source).cloned();
            let score = document_score(
                source_scores.problem_score,
                source_scores.environment_gain,
                local_coherence.as_ref().map(|item| item.score),
            );
            if score < RELATION_FLOOR {
                below_relation_floor = below_relation_floor.checked_add(1).ok_or_else(|| {
                    DbError::InvalidData("semantic relation floor count overflow".to_string())
                })?;
                continue;
            }
            options.push(SemanticRankedRelationOption {
                edge_key: relation.edge_key,
                edge_provenance: relation.edge_provenance,
                document_id: relation.document_id,
                binding_provenance: relation.binding_provenance,
                channel_scores: source_scores.channel_scores.clone(),
                local_coherence,
                document_score: score,
            });
        }
        options.sort_by(compare_ranked_relations);
        let (options, exhaustion) = slice_ranked_relations(options, request.after, request.limit)?;
        omitted.sort_by(|left, right| {
            left.edge_key
                .cmp(&right.edge_key)
                .then_with(|| left.document_id.cmp(&right.document_id))
                .then_with(|| left.reason.cmp(&right.reason))
        });
        Ok(SemanticIncidentRelationRankOutcome::Ranked(Box::new(
            SemanticIncidentRelationRankBatch {
                snapshot: self.snapshot_binding(),
                options,
                omitted,
                below_relation_floor,
                exhaustion,
            },
        )))
    }

    /// Rank every semantically continuable Coordinate in one complete current
    /// Hyperedge. The Edge identity remains complete even when individual
    /// targets are lifecycle-filtered or lack a current embedding.
    pub async fn rank_edge_target_options_exact(
        &mut self,
        request: SemanticEdgeTargetRankRequest<'_>,
    ) -> Result<SemanticEdgeTargetRankOutcome> {
        validate_traversal_channels(&self.ticket, request.channels)?;
        validate_traversal_limit(request.limit, MAX_TARGET_OPTIONS_MATERIALIZED, "target")?;
        let expected_binding = request
            .hyperedge
            .current_context_document_bindings
            .iter()
            .find(|binding| binding.document_id == request.relation_document_id)
            .cloned()
            .ok_or_else(|| {
                DbError::InvalidData(
                    "semantic target rank relation Document is not bound to its Edge".to_string(),
                )
            })?;
        let expectation = SemanticHyperedgeExpectation {
            edge_key: request.hyperedge.edge_key,
            edge_provenance: request.hyperedge.provenance.clone(),
            required_binding: Some(expected_binding),
        };
        let current_edge = match self.load_complete_hyperedge(&expectation).await? {
            SemanticHyperedgeReadOutcome::Current(edge) if edge.as_ref() == request.hyperedge => {
                *edge
            }
            SemanticHyperedgeReadOutcome::Current(_) | SemanticHyperedgeReadOutcome::Changed => {
                return Ok(SemanticEdgeTargetRankOutcome::HyperedgeChanged);
            }
            SemanticHyperedgeReadOutcome::HyperedgeTooLarge { identity_bytes } => {
                return Ok(SemanticEdgeTargetRankOutcome::HyperedgeTooLarge { identity_bytes });
            }
        };
        if current_edge.complete_coordinates.len() > MAX_TARGET_SCORE_SET {
            return Ok(SemanticEdgeTargetRankOutcome::OptionSetTooLarge {
                observed_at_least: current_edge.complete_coordinates.len(),
            });
        }

        let sources = current_edge
            .complete_coordinates
            .iter()
            .map(|coordinate| {
                semantic_source_identity_for_coordinate(self.ticket.community_id, coordinate)
            })
            .collect::<Result<Vec<_>>>()?;
        validate_unique_sources(&sources, "Hyperedge target")?;
        let matrix = self
            .query_exact_source_scores(
                request.lifecycle_filter,
                &[],
                request.channels.query_vectors,
                Some(&sources),
                None,
            )
            .await?;
        let scored = traversal_source_score_sets(request.channels, matrix)?;
        let relation_source =
            project_document_source(self.ticket.community_id, request.relation_document_id);
        validate_head_for_source(request.relation_document_head, &relation_source)?;
        let pairs = scored
            .keys()
            .map(|source| SemanticCurrentSourcePair {
                left: relation_source.clone(),
                right: source.clone(),
            })
            .collect::<Vec<_>>();
        let coherences = self.score_current_source_pairs_exact(&pairs).await?;
        let coherences = pair_scores_by_right(
            coherences,
            Some(&relation_source),
            Some(request.relation_document_head),
        )?;
        let states = self
            .load_current_semantic_source_states(&sources, MAX_TARGET_SCORE_SET)
            .await?;

        let mut options = Vec::with_capacity(scored.len());
        let mut omitted = Vec::new();
        let mut below_target_floor = 0_u32;
        let mut below_transition_floor = 0_u32;
        for ((coordinate, source), state) in current_edge
            .complete_coordinates
            .iter()
            .cloned()
            .zip(sources)
            .zip(states)
        {
            let Some(source_scores) = scored.get(&source) else {
                omitted.push(SemanticTargetOptionOmission {
                    coordinate,
                    reason: traversal_omission_reason(&state, true, request.lifecycle_filter),
                });
                continue;
            };
            let representative = source_scores.channel_scores.first().ok_or_else(|| {
                DbError::InvalidData("semantic target score set is empty".to_string())
            })?;
            if !representative.roles.coordinate
                || !representative
                    .roles
                    .coordinate_incident_edge_keys
                    .contains(&current_edge.edge_key)
            {
                return Err(DbError::InvalidData(
                    "semantic target score lost its exact Coordinate role".to_string(),
                ));
            }
            if !representative.roles.coordinate_entry_eligible {
                omitted.push(SemanticTargetOptionOmission {
                    coordinate,
                    reason: SemanticTraversalSourceOmissionReason::LifecycleFiltered,
                });
                continue;
            }
            let coherence = coherences.get(&source).cloned().ok_or_else(|| {
                DbError::InvalidData(
                    "semantic D-V coherence is missing for an exact-current target".to_string(),
                )
            })?;
            let target_score = target_coordinate_score(
                source_scores.problem_score,
                source_scores.environment_gain,
                coherence.score,
            );
            if target_score < TARGET_FLOOR {
                below_target_floor = below_target_floor.checked_add(1).ok_or_else(|| {
                    DbError::InvalidData("semantic target floor count overflow".to_string())
                })?;
                continue;
            }
            let transition_score = harmonic_score(request.document_score, target_score);
            if transition_score < TRANSITION_FLOOR {
                below_transition_floor =
                    below_transition_floor.checked_add(1).ok_or_else(|| {
                        DbError::InvalidData("semantic transition floor count overflow".to_string())
                    })?;
                continue;
            }
            options.push(SemanticRankedTargetOption {
                coordinate,
                channel_scores: source_scores.channel_scores.clone(),
                relation_document_coherence: coherence,
                target_score,
                transition_score,
            });
        }
        options.sort_by(compare_ranked_targets);
        let (options, exhaustion) = slice_ranked_targets(options, request.after, request.limit)?;
        omitted.sort_by(|left, right| {
            left.coordinate
                .cmp(&right.coordinate)
                .then_with(|| left.reason.cmp(&right.reason))
        });
        Ok(SemanticEdgeTargetRankOutcome::Ranked(Box::new(
            SemanticEdgeTargetRankBatch {
                snapshot: self.snapshot_binding(),
                edge: current_edge,
                options,
                omitted,
                below_target_floor,
                below_transition_floor,
                exhaustion,
            },
        )))
    }

    /// Count the authorized graph source set and classify every source into
    /// exactly one active-generation embedding coverage state.
    pub async fn semantic_graph_coverage(
        &mut self,
        lifecycle_filter: LifecycleFilter,
        explicit_initial_sources: &[SemanticSourceIdentity],
    ) -> Result<SemanticGraphCoverage> {
        validate_source_inputs(
            self.ticket.community_id,
            explicit_initial_sources,
            MAX_INITIAL_COORDINATES,
            "explicit initial source",
        )?;
        let initial = source_key_arrays(explicit_initial_sources);
        let dimensions = i32::try_from(self.ticket.generation.model_contract.dimensions)
            .map_err(|_| DbError::InvalidData("semantic dimensions exceed int4".to_string()))?;
        let rows = sqlx::query(SEMANTIC_GRAPH_COVERAGE_SQL)
            .bind(self.ticket.community_id.as_uuid())
            .bind(self.reader_pubkey.as_slice())
            .bind(self.expected_projection_pubkey.as_bytes())
            .bind(self.ticket.generation.generation_id)
            .bind(
                self.ticket
                    .generation
                    .model_contract_digest
                    .as_bytes()
                    .as_slice(),
            )
            .bind(self.ticket.generation.extractor_version.as_str())
            .bind(self.ticket.generation.model_contract.model.as_str())
            .bind(dimensions)
            .bind(lifecycle_filter_db(lifecycle_filter))
            .bind(&initial.families)
            .bind(&initial.subtypes)
            .bind(&initial.ids)
            .fetch_all(&mut *self.tx)
            .await?;
        let mut embedding = BTreeMap::new();
        let mut title_only_sources = 0_u64;
        for row in rows {
            let class =
                coverage_class_from_db(row.try_get::<String, _>("coverage_class")?.as_str())?;
            let count = nonnegative_u64(row.try_get("source_count")?, "coverage source_count")?;
            embedding.insert(class, count);
            title_only_sources = title_only_sources
                .checked_add(nonnegative_u64(
                    row.try_get("title_only_count")?,
                    "coverage title_only_count",
                )?)
                .ok_or_else(|| {
                    DbError::InvalidData("semantic coverage count overflow".to_string())
                })?;
        }
        for class in [
            SemanticGraphEmbeddingCoverageClass::Current,
            SemanticGraphEmbeddingCoverageClass::NonQueryableZeroVector,
            SemanticGraphEmbeddingCoverageClass::Failed,
            SemanticGraphEmbeddingCoverageClass::Building,
            SemanticGraphEmbeddingCoverageClass::Unsupported,
            SemanticGraphEmbeddingCoverageClass::Missing,
        ] {
            embedding.entry(class).or_insert(0);
        }
        let authorized_graph_sources = embedding.values().try_fold(0_u64, |sum, value| {
            sum.checked_add(*value)
                .ok_or_else(|| DbError::InvalidData("semantic coverage count overflow".to_string()))
        })?;
        let current_indexed_graph_sources = *embedding
            .get(&SemanticGraphEmbeddingCoverageClass::Current)
            .unwrap_or(&0);
        if title_only_sources > current_indexed_graph_sources {
            return Err(DbError::InvalidData(
                "semantic title-only coverage exceeds current coverage".to_string(),
            ));
        }
        Ok(SemanticGraphCoverage {
            authorized_graph_sources,
            current_indexed_graph_sources,
            title_only_sources,
            embedding,
        })
    }

    /// Score a bounded list of current source pairs for root redundancy and
    /// local path coherence. Both endpoints are rejoined through the same
    /// active generation and exact current-head fences.
    pub async fn score_current_source_pairs_exact(
        &mut self,
        pairs: &[SemanticCurrentSourcePair],
    ) -> Result<Vec<SemanticCurrentSourcePairDistance>> {
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        if pairs.len() > MAX_SOURCE_PAIR_SCORES {
            return Err(DbError::InvalidData(
                "semantic source pair count exceeds the server bound".to_string(),
            ));
        }
        let left: Vec<SemanticSourceIdentity> =
            pairs.iter().map(|pair| pair.left.clone()).collect();
        let right: Vec<SemanticSourceIdentity> =
            pairs.iter().map(|pair| pair.right.clone()).collect();
        validate_source_inputs(
            self.ticket.community_id,
            &left,
            MAX_SOURCE_PAIR_SCORES,
            "source pair left endpoint",
        )?;
        validate_source_inputs(
            self.ticket.community_id,
            &right,
            MAX_SOURCE_PAIR_SCORES,
            "source pair right endpoint",
        )?;
        let left = source_key_arrays(&left);
        let right = source_key_arrays(&right);
        let ordinals: Vec<i32> = (0..pairs.len())
            .map(|ordinal| {
                i32::try_from(ordinal).map_err(|_| {
                    DbError::InvalidData("semantic source pair ordinal exceeds int4".to_string())
                })
            })
            .collect::<Result<_>>()?;
        let dimensions = i32::try_from(self.ticket.generation.model_contract.dimensions)
            .map_err(|_| DbError::InvalidData("semantic dimensions exceed int4".to_string()))?;
        let rows = sqlx::query(CURRENT_SOURCE_PAIR_SCORES_SQL)
            .bind(self.ticket.community_id.as_uuid())
            .bind(self.reader_pubkey.as_slice())
            .bind(self.expected_projection_pubkey.as_bytes())
            .bind(self.ticket.generation.generation_id)
            .bind(
                self.ticket
                    .generation
                    .model_contract_digest
                    .as_bytes()
                    .as_slice(),
            )
            .bind(self.ticket.generation.extractor_version.as_str())
            .bind(self.ticket.generation.model_contract.model.as_str())
            .bind(dimensions)
            .bind(&ordinals)
            .bind(&left.families)
            .bind(&left.subtypes)
            .bind(&left.ids)
            .bind(&right.families)
            .bind(&right.subtypes)
            .bind(&right.ids)
            .fetch_all(&mut *self.tx)
            .await?;
        rows.iter().map(source_pair_from_row).collect()
    }

    fn snapshot_binding(&self) -> SemanticGraphSnapshotBinding {
        SemanticGraphSnapshotBinding {
            community_id: self.ticket.community_id,
            generation_id: self.ticket.generation.generation_id,
            query_fences: self.ticket.query_fences,
            extractor_version: self.ticket.generation.extractor_version.clone(),
            project_context_revision: self.ticket.project_context_revision,
            observed_at: self.ticket.observed_at,
        }
    }

    async fn observe_optional_canonical_source(
        &mut self,
        source: &SemanticSourceIdentity,
    ) -> Result<Option<CanonicalSemanticSourceObservation>> {
        match observe_semantic_source_in_connection(&mut self.tx, source).await {
            Ok(observation) => Ok(Some(observation)),
            Err(DbError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn load_coordinate_memberships(
        &mut self,
        coordinates: &[ProjectContextCoordinate],
    ) -> Result<Vec<SemanticCoordinateGraphMembership>> {
        let requested = coordinate_key_arrays(coordinates);
        let context_revision =
            i64::try_from(self.ticket.project_context_revision).map_err(|_| {
                DbError::InvalidData("semantic Project Context revision exceeds int8".to_string())
            })?;
        let rows = sqlx::query(CURRENT_COORDINATE_MEMBERSHIPS_SQL)
            .bind(self.ticket.community_id.as_uuid())
            .bind(&requested.types)
            .bind(&requested.subtypes)
            .bind(&requested.ids)
            .bind(context_revision)
            .fetch_all(&mut *self.tx)
            .await?;
        let mut memberships = vec![
            SemanticCoordinateGraphMembership {
                project_context_revision: self.ticket.project_context_revision,
                incident_edge_keys: Vec::new(),
            };
            coordinates.len()
        ];
        let mut observed_ordinals = BTreeSet::new();
        for row in rows {
            let ordinal = positive_usize(row.try_get("request_ordinal")?, "request_ordinal")?;
            let index = ordinal.checked_sub(1).ok_or_else(|| {
                DbError::InvalidData(
                    "semantic membership request ordinal must be one-based".to_string(),
                )
            })?;
            let membership = memberships.get_mut(index).ok_or_else(|| {
                DbError::InvalidData(
                    "semantic membership request ordinal exceeds its input".to_string(),
                )
            })?;
            observed_ordinals.insert(ordinal);
            if let Some(edge_key) = row.try_get::<Option<Vec<u8>>, _>("edge_key")? {
                membership.incident_edge_keys.push(edge_key_from_bytes(
                    edge_key,
                    "coordinate incident edge_key",
                )?);
            }
        }
        if observed_ordinals.len() != coordinates.len() {
            return Err(unavailable());
        }
        for membership in &mut memberships {
            membership.incident_edge_keys.sort();
            if membership
                .incident_edge_keys
                .windows(2)
                .any(|pair| pair[0] == pair[1])
            {
                return Err(DbError::InvalidData(
                    "semantic Coordinate membership contains duplicate Edges".to_string(),
                ));
            }
        }
        Ok(memberships)
    }

    async fn load_current_semantic_source_states(
        &mut self,
        sources: &[SemanticSourceIdentity],
        maximum: usize,
    ) -> Result<Vec<CurrentSemanticSourceState>> {
        load_current_semantic_source_states_in_tx(&mut self.tx, &self.ticket, sources, maximum)
            .await
    }

    async fn query_exact_source_scores(
        &mut self,
        lifecycle_filter: LifecycleFilter,
        explicit_initial_sources: &[SemanticSourceIdentity],
        query_vectors: &[SemanticExactQueryVector],
        candidates: Option<&[SemanticSourceIdentity]>,
        top_per_channel: Option<u32>,
    ) -> Result<Vec<SemanticExactSourceScore>> {
        validate_source_inputs(
            self.ticket.community_id,
            explicit_initial_sources,
            MAX_INITIAL_COORDINATES,
            "explicit initial source",
        )?;
        validate_query_vectors(&self.ticket, query_vectors)?;
        if let Some(candidates) = candidates {
            validate_source_inputs(
                self.ticket.community_id,
                candidates,
                MAX_TARGET_SCORE_SET,
                "candidate source",
            )?;
        }

        let initial = source_key_arrays(explicit_initial_sources);
        let restrict_candidates = candidates.is_some();
        let candidates = source_key_arrays(candidates.unwrap_or_default());
        let channel_ids: Vec<Vec<u8>> = query_vectors
            .iter()
            .map(|channel| channel.channel_id.as_bytes().to_vec())
            .collect();
        let vectors: Vec<Vector> = query_vectors
            .iter()
            .map(|channel| Vector::from(channel.embedding.as_slice().to_vec()))
            .collect();
        let dimensions = i32::try_from(self.ticket.generation.model_contract.dimensions)
            .map_err(|_| DbError::InvalidData("semantic dimensions exceed int4".to_string()))?;
        let top_per_channel = top_per_channel.map(i64::from);

        let rows = sqlx::query(EXACT_SOURCE_SCORES_SQL)
            .bind(self.ticket.community_id.as_uuid())
            .bind(self.reader_pubkey.as_slice())
            .bind(self.expected_projection_pubkey.as_bytes())
            .bind(self.ticket.generation.generation_id)
            .bind(
                self.ticket
                    .generation
                    .model_contract_digest
                    .as_bytes()
                    .as_slice(),
            )
            .bind(self.ticket.generation.extractor_version.as_str())
            .bind(self.ticket.generation.model_contract.model.as_str())
            .bind(dimensions)
            .bind(lifecycle_filter_db(lifecycle_filter))
            .bind(&initial.families)
            .bind(&initial.subtypes)
            .bind(&initial.ids)
            .bind(&channel_ids)
            .bind(&vectors)
            .bind(restrict_candidates)
            .bind(&candidates.families)
            .bind(&candidates.subtypes)
            .bind(&candidates.ids)
            .bind(top_per_channel)
            .fetch_all(&mut *self.tx)
            .await?;
        rows.iter().map(exact_score_from_row).collect()
    }
}

#[derive(Debug, Clone)]
struct IncidentRelationRef {
    edge_key: EdgeKey,
    edge_provenance: ProjectContextEdgeProvenance,
    document_id: Uuid,
    binding_provenance: ProjectContextBindingProvenance,
}

#[derive(Debug, Clone)]
struct TraversalSourceScoreSet {
    channel_scores: Vec<SemanticExactSourceScore>,
    problem_score: Score,
    environment_gain: Score,
}

async fn load_complete_hyperedge_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    ticket: &SemanticGraphQueryTicket,
    edge_key: EdgeKey,
) -> Result<Option<SemanticEdgeObservation>> {
    let context_revision = i64::try_from(ticket.project_context_revision).map_err(|_| {
        DbError::InvalidData("semantic Project Context revision exceeds int8".to_string())
    })?;
    let rows = sqlx::query(COMPLETE_HYPEREDGE_SQL)
        .bind(ticket.community_id.as_uuid())
        .bind(edge_key.as_bytes().as_slice())
        .bind(context_revision)
        .fetch_all(&mut **tx)
        .await?;
    if rows.is_empty() {
        return Ok(None);
    }

    let first = rows.first().ok_or_else(|| {
        DbError::InvalidData("semantic complete Hyperedge result is empty".to_string())
    })?;
    let observed_key = edge_key_from_bytes(first.try_get("edge_key")?, "complete edge_key")?;
    if observed_key != edge_key {
        return Err(DbError::InvalidData(
            "semantic complete Hyperedge returned a different Edge key".to_string(),
        ));
    }
    let complete_coordinates: Vec<ProjectContextCoordinate> =
        serde_json::from_value(first.try_get::<Value, _>("canonical_coordinates")?)?;
    if complete_coordinates.len() < 2
        || complete_coordinates
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || EdgeKey::derive(*ticket.community_id.as_uuid(), &complete_coordinates).map_err(
            |error| {
                DbError::InvalidData(format!(
                    "semantic complete Hyperedge coordinates are invalid: {error}"
                ))
            },
        )? != edge_key
    {
        return Err(DbError::InvalidData(
            "semantic complete Hyperedge canonical identity is inconsistent".to_string(),
        ));
    }
    let provenance = ProjectContextEdgeProvenance {
        last_context_revision: positive_u64(
            first.try_get("last_context_revision")?,
            "edge last_context_revision",
        )?,
        source_change_id: digest_from_bytes(
            first.try_get("edge_source_change_id")?,
            "edge source_change_id",
        )?,
    };
    let mut bindings = Vec::with_capacity(rows.len());
    for row in rows {
        if edge_key_from_bytes(row.try_get("edge_key")?, "complete edge_key")? != edge_key
            || row.try_get::<Value, _>("canonical_coordinates")?
                != serde_json::to_value(&complete_coordinates)?
            || positive_u64(
                row.try_get("last_context_revision")?,
                "edge last_context_revision",
            )? != provenance.last_context_revision
            || digest_from_bytes(
                row.try_get("edge_source_change_id")?,
                "edge source_change_id",
            )? != provenance.source_change_id
        {
            return Err(DbError::InvalidData(
                "semantic complete Hyperedge rows disagree on Edge identity".to_string(),
            ));
        }
        let document_id: Uuid = row.try_get("context_document_id")?;
        validate_coordinate(
            ticket.community_id,
            &ProjectContextCoordinate::Document { document_id },
        )?;
        bindings.push(ContextDocumentBindingObservation {
            document_id,
            provenance: ProjectContextBindingProvenance {
                binding_context_revision: positive_u64(
                    row.try_get("binding_context_revision")?,
                    "binding_context_revision",
                )?,
                source_change_id: digest_from_bytes(
                    row.try_get("binding_source_change_id")?,
                    "binding source_change_id",
                )?,
                projection_event_id: digest_from_bytes(
                    row.try_get("binding_projection_event_id")?,
                    "binding projection_event_id",
                )?,
            },
        });
    }
    bindings.sort_by_key(|binding| binding.document_id);
    if bindings.is_empty()
        || bindings
            .windows(2)
            .any(|pair| pair[0].document_id == pair[1].document_id)
    {
        return Err(DbError::InvalidData(
            "semantic complete active Hyperedge has invalid Binding membership".to_string(),
        ));
    }
    Ok(Some(SemanticEdgeObservation {
        edge_key,
        complete_coordinates,
        provenance,
        current_context_document_bindings: bindings,
    }))
}

async fn load_incident_relation_refs_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    ticket: &SemanticGraphQueryTicket,
    coordinate: &ProjectContextCoordinate,
    maximum: usize,
) -> Result<Vec<IncidentRelationRef>> {
    let requested = coordinate_key_arrays(std::slice::from_ref(coordinate));
    let context_revision = i64::try_from(ticket.project_context_revision).map_err(|_| {
        DbError::InvalidData("semantic Project Context revision exceeds int8".to_string())
    })?;
    let maximum = i64::try_from(maximum).map_err(|_| {
        DbError::InvalidData("semantic incident relation cap exceeds int8".to_string())
    })?;
    let rows = sqlx::query(INCIDENT_RELATION_REFS_SQL)
        .bind(ticket.community_id.as_uuid())
        .bind(requested.types[0])
        .bind(requested.subtypes[0])
        .bind(requested.ids[0])
        .bind(context_revision)
        .bind(maximum)
        .fetch_all(&mut **tx)
        .await?;
    let mut refs = Vec::with_capacity(rows.len());
    for row in rows {
        let document_id: Uuid = row.try_get("context_document_id")?;
        validate_coordinate(
            ticket.community_id,
            &ProjectContextCoordinate::Document { document_id },
        )?;
        refs.push(IncidentRelationRef {
            edge_key: edge_key_from_bytes(row.try_get("edge_key")?, "incident edge_key")?,
            edge_provenance: ProjectContextEdgeProvenance {
                last_context_revision: positive_u64(
                    row.try_get("edge_last_context_revision")?,
                    "incident edge last_context_revision",
                )?,
                source_change_id: digest_from_bytes(
                    row.try_get("edge_source_change_id")?,
                    "incident edge source_change_id",
                )?,
            },
            document_id,
            binding_provenance: ProjectContextBindingProvenance {
                binding_context_revision: positive_u64(
                    row.try_get("binding_context_revision")?,
                    "incident binding_context_revision",
                )?,
                source_change_id: digest_from_bytes(
                    row.try_get("binding_source_change_id")?,
                    "incident binding source_change_id",
                )?,
                projection_event_id: digest_from_bytes(
                    row.try_get("binding_projection_event_id")?,
                    "incident binding projection_event_id",
                )?,
            },
        });
    }
    if refs.windows(2).any(|pair| {
        (pair[0].edge_key, pair[0].document_id) >= (pair[1].edge_key, pair[1].document_id)
    }) {
        return Err(DbError::InvalidData(
            "semantic incident relation refs are not uniquely canonical".to_string(),
        ));
    }
    Ok(refs)
}

fn semantic_hyperedge_identity_bytes(edge: &SemanticEdgeObservation) -> Result<usize> {
    Ok(serde_json::to_vec(edge)?.len())
}

fn validate_coordinate(
    community_id: CommunityId,
    coordinate: &ProjectContextCoordinate,
) -> Result<()> {
    coordinate
        .validate_for_project(*community_id.as_uuid())
        .map_err(|error| {
            DbError::InvalidData(format!("invalid semantic traversal Coordinate: {error}"))
        })
}

fn validate_traversal_channels(
    ticket: &SemanticGraphQueryTicket,
    channels: SemanticTraversalQueryChannels<'_>,
) -> Result<()> {
    validate_query_vectors(ticket, channels.query_vectors)?;
    if channels.conditioned.len().checked_add(1) != Some(channels.query_vectors.len()) {
        return Err(DbError::InvalidData(
            "semantic traversal Q0/Qi binding count is inconsistent".to_string(),
        ));
    }
    let vector_ids = channels
        .query_vectors
        .iter()
        .map(SemanticExactQueryVector::channel_id)
        .collect::<BTreeSet<_>>();
    if !vector_ids.contains(&channels.problem_channel_id) {
        return Err(DbError::InvalidData(
            "semantic traversal channel binding lacks Q0".to_string(),
        ));
    }
    let mut bound_ids = BTreeSet::from([channels.problem_channel_id]);
    let mut coordinates = BTreeSet::new();
    for conditioned in channels.conditioned {
        validate_coordinate(ticket.community_id, &conditioned.context_coordinate)?;
        if conditioned.channel_id == channels.problem_channel_id
            || !vector_ids.contains(&conditioned.channel_id)
            || !bound_ids.insert(conditioned.channel_id)
            || !coordinates.insert(conditioned.context_coordinate.clone())
        {
            return Err(DbError::InvalidData(
                "semantic traversal Qi binding is duplicate or unknown".to_string(),
            ));
        }
    }
    if bound_ids != vector_ids {
        return Err(DbError::InvalidData(
            "semantic traversal query vector is not bound to a closed channel".to_string(),
        ));
    }
    Ok(())
}

fn validate_traversal_limit(limit: u16, hard_cap: u16, label: &str) -> Result<()> {
    if limit == 0 || limit > hard_cap {
        return Err(DbError::InvalidData(format!(
            "semantic {label} rank limit is outside the server bound"
        )));
    }
    Ok(())
}

fn validate_unique_sources(sources: &[SemanticSourceIdentity], label: &str) -> Result<()> {
    let mut observed = std::collections::HashSet::with_capacity(sources.len());
    if sources.iter().any(|source| !observed.insert(source)) {
        return Err(DbError::InvalidData(format!(
            "semantic {label} source identities are not unique"
        )));
    }
    Ok(())
}

fn traversal_source_score_sets(
    channels: SemanticTraversalQueryChannels<'_>,
    matrix: Vec<SemanticExactSourceScore>,
) -> Result<HashMap<SemanticSourceIdentity, TraversalSourceScoreSet>> {
    let mut grouped: HashMap<SemanticSourceIdentity, Vec<SemanticExactSourceScore>> =
        HashMap::new();
    for row in matrix {
        grouped.entry(row.source.clone()).or_default().push(row);
    }
    let mut ordered_channel_ids = Vec::with_capacity(channels.query_vectors.len());
    ordered_channel_ids.push(channels.problem_channel_id);
    ordered_channel_ids.extend(channels.conditioned.iter().map(|item| item.channel_id));
    let mut result = HashMap::with_capacity(grouped.len());
    for (source, rows) in grouped {
        if rows.len() != ordered_channel_ids.len() {
            return Err(DbError::InvalidData(
                "semantic traversal source score matrix is incomplete".to_string(),
            ));
        }
        let mut ordered = Vec::with_capacity(rows.len());
        for channel_id in &ordered_channel_ids {
            let mut matches = rows.iter().filter(|row| row.channel_id == *channel_id);
            let row = matches.next().ok_or_else(|| {
                DbError::InvalidData(
                    "semantic traversal source score matrix lacks a channel".to_string(),
                )
            })?;
            if matches.next().is_some() {
                return Err(DbError::InvalidData(
                    "semantic traversal source score matrix duplicates a channel".to_string(),
                ));
            }
            ordered.push(row.clone());
        }
        let representative = ordered.first().ok_or_else(|| {
            DbError::InvalidData("semantic traversal source score set is empty".to_string())
        })?;
        if ordered.iter().any(|row| {
            row.source != representative.source
                || row.head != representative.head
                || row.lifecycle != representative.lifecycle
                || row.source_status != representative.source_status
                || row.roles != representative.roles
        }) {
            return Err(DbError::InvalidData(
                "semantic traversal score rows disagree on current source provenance".to_string(),
            ));
        }
        let problem_score = representative.score;
        let evidence = channels
            .conditioned
            .iter()
            .zip(ordered.iter().skip(1))
            .map(|(channel, row)| {
                ConditionedEvidence::new(
                    channel.context_coordinate.clone(),
                    problem_score,
                    row.score,
                )
            })
            .collect::<Vec<_>>();
        let environment = environment_gain(&evidence);
        result.insert(
            source,
            TraversalSourceScoreSet {
                channel_scores: ordered,
                problem_score,
                environment_gain: environment.environment_gain,
            },
        );
    }
    Ok(result)
}

fn pair_scores_by_left(
    scores: Vec<SemanticCurrentSourcePairDistance>,
    expected_right: Option<&SemanticSourceIdentity>,
    expected_right_head: Option<&SemanticCurrentHead>,
) -> Result<HashMap<SemanticSourceIdentity, SemanticCurrentSourcePairDistance>> {
    let mut result = HashMap::with_capacity(scores.len());
    for score in scores {
        if expected_right.is_some_and(|expected| expected != &score.right)
            || expected_right_head.is_some_and(|expected| expected != &score.right_head)
            || result.insert(score.left.clone(), score).is_some()
        {
            return Err(DbError::InvalidData(
                "semantic source-pair left coherence provenance is inconsistent".to_string(),
            ));
        }
    }
    Ok(result)
}

fn pair_scores_by_right(
    scores: Vec<SemanticCurrentSourcePairDistance>,
    expected_left: Option<&SemanticSourceIdentity>,
    expected_left_head: Option<&SemanticCurrentHead>,
) -> Result<HashMap<SemanticSourceIdentity, SemanticCurrentSourcePairDistance>> {
    let mut result = HashMap::with_capacity(scores.len());
    for score in scores {
        if expected_left.is_some_and(|expected| expected != &score.left)
            || expected_left_head.is_some_and(|expected| expected != &score.left_head)
            || result.insert(score.right.clone(), score).is_some()
        {
            return Err(DbError::InvalidData(
                "semantic source-pair right coherence provenance is inconsistent".to_string(),
            ));
        }
    }
    Ok(result)
}

fn traversal_omission_reason(
    state: &CurrentSemanticSourceState,
    apply_lifecycle: bool,
    lifecycle_filter: LifecycleFilter,
) -> SemanticTraversalSourceOmissionReason {
    let Some(eligibility) = state.eligibility else {
        return SemanticTraversalSourceOmissionReason::SourceNotFound;
    };
    match eligibility {
        SemanticEligibility::Ineligible(IneligibilityReason::Tombstone) => {
            return SemanticTraversalSourceOmissionReason::SourceTombstoned;
        }
        SemanticEligibility::Ineligible(IneligibilityReason::Deleted) => {
            return SemanticTraversalSourceOmissionReason::SourceDeleted;
        }
        SemanticEligibility::Ineligible(
            IneligibilityReason::InvalidCanonicalState
            | IneligibilityReason::SourceCapabilityUnavailable,
        ) => return SemanticTraversalSourceOmissionReason::SourceIneligible,
        SemanticEligibility::Eligible => {}
    }
    if apply_lifecycle
        && state
            .lifecycle
            .is_some_and(|lifecycle| !lifecycle_matches(lifecycle_filter, lifecycle))
    {
        return SemanticTraversalSourceOmissionReason::LifecycleFiltered;
    }
    match state.availability {
        CurrentSemanticAvailabilityClass::Missing => {
            SemanticTraversalSourceOmissionReason::SemanticHeadMissing
        }
        CurrentSemanticAvailabilityClass::Building => {
            SemanticTraversalSourceOmissionReason::SemanticHeadBuilding
        }
        CurrentSemanticAvailabilityClass::Failed => {
            SemanticTraversalSourceOmissionReason::SemanticHeadFailed
        }
        CurrentSemanticAvailabilityClass::Unsupported => {
            SemanticTraversalSourceOmissionReason::SemanticHeadUnsupported
        }
        CurrentSemanticAvailabilityClass::Current => {
            SemanticTraversalSourceOmissionReason::SourceNotReadable
        }
    }
}

const fn lifecycle_matches(filter: LifecycleFilter, lifecycle: SemanticLifecycleClass) -> bool {
    match filter {
        LifecycleFilter::AllCurrent => matches!(
            lifecycle,
            SemanticLifecycleClass::Active
                | SemanticLifecycleClass::Finalizing
                | SemanticLifecycleClass::Terminal
        ),
        LifecycleFilter::NonTerminal => matches!(
            lifecycle,
            SemanticLifecycleClass::Active | SemanticLifecycleClass::Finalizing
        ),
        LifecycleFilter::TerminalOnly => matches!(lifecycle, SemanticLifecycleClass::Terminal),
    }
}

fn compare_ranked_relations(
    left: &SemanticRankedRelationOption,
    right: &SemanticRankedRelationOption,
) -> std::cmp::Ordering {
    right
        .document_score
        .cmp(&left.document_score)
        .then_with(|| left.edge_key.cmp(&right.edge_key))
        .then_with(|| left.document_id.cmp(&right.document_id))
}

fn slice_ranked_relations(
    options: Vec<SemanticRankedRelationOption>,
    after: Option<&RelationRankCursor>,
    limit: u16,
) -> Result<(
    Vec<SemanticRankedRelationOption>,
    SemanticTraversalSliceExhaustion,
)> {
    let mut eligible = options
        .into_iter()
        .filter(|option| {
            after.is_none_or(|cursor| {
                option.document_score < cursor.document_score
                    || (option.document_score == cursor.document_score
                        && (option.edge_key, option.document_id)
                            > (cursor.edge_key, cursor.document_id))
            })
        })
        .take(usize::from(limit) + 1)
        .collect::<Vec<_>>();
    let exhaustion = if eligible.len() > usize::from(limit) {
        eligible.pop();
        SemanticTraversalSliceExhaustion::Truncated
    } else {
        SemanticTraversalSliceExhaustion::Exhausted
    };
    Ok((eligible, exhaustion))
}

fn compare_ranked_targets(
    left: &SemanticRankedTargetOption,
    right: &SemanticRankedTargetOption,
) -> std::cmp::Ordering {
    right
        .transition_score
        .cmp(&left.transition_score)
        .then_with(|| left.coordinate.cmp(&right.coordinate))
}

fn slice_ranked_targets(
    options: Vec<SemanticRankedTargetOption>,
    after: Option<&TargetRankCursor>,
    limit: u16,
) -> Result<(
    Vec<SemanticRankedTargetOption>,
    SemanticTraversalSliceExhaustion,
)> {
    let mut eligible = options
        .into_iter()
        .filter(|option| {
            after.is_none_or(|cursor| {
                option.transition_score < cursor.transition_score
                    || (option.transition_score == cursor.transition_score
                        && option.coordinate > cursor.target_coordinate)
            })
        })
        .take(usize::from(limit) + 1)
        .collect::<Vec<_>>();
    let exhaustion = if eligible.len() > usize::from(limit) {
        eligible.pop();
        SemanticTraversalSliceExhaustion::Truncated
    } else {
        SemanticTraversalSliceExhaustion::Exhausted
    };
    Ok((eligible, exhaustion))
}

fn project_document_source(community_id: CommunityId, document_id: Uuid) -> SemanticSourceIdentity {
    SemanticSourceIdentity {
        community_id: *community_id.as_uuid(),
        kind: SemanticSourceKind::ProjectDocument,
        source_id: document_id,
    }
}

async fn load_current_semantic_source_states_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    ticket: &SemanticGraphQueryTicket,
    sources: &[SemanticSourceIdentity],
    maximum: usize,
) -> Result<Vec<CurrentSemanticSourceState>> {
    validate_source_inputs(
        ticket.community_id,
        sources,
        maximum,
        "current source-state input",
    )?;
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let requested = source_key_arrays(sources);
    let dimensions = i32::try_from(ticket.generation.model_contract.dimensions)
        .map_err(|_| DbError::InvalidData("semantic dimensions exceed int4".to_string()))?;
    let rows = sqlx::query(CURRENT_SEMANTIC_SOURCE_STATES_SQL)
        .bind(ticket.community_id.as_uuid())
        .bind(ticket.generation.generation_id)
        .bind(
            ticket
                .generation
                .model_contract_digest
                .as_bytes()
                .as_slice(),
        )
        .bind(ticket.generation.extractor_version.as_str())
        .bind(ticket.generation.model_contract.model.as_str())
        .bind(dimensions)
        .bind(&requested.families)
        .bind(&requested.subtypes)
        .bind(&requested.ids)
        .fetch_all(&mut **tx)
        .await?;
    if rows.len() != sources.len() {
        return Err(unavailable());
    }

    rows.iter()
        .zip(sources)
        .map(|(row, expected_source)| {
            let source =
                source_identity_from_row(row, "source_family", "source_subtype", "source_id")?;
            if &source != expected_source {
                return Err(DbError::InvalidData(
                    "semantic current source-state order is inconsistent".to_string(),
                ));
            }
            let availability =
                current_availability_from_db(row.try_get::<String, _>("availability")?.as_str())?;
            let source_invalidation_epoch = row
                .try_get::<Option<i64>, _>("source_invalidation_epoch")?
                .map(|value| positive_u64(value, "source_invalidation_epoch"))
                .transpose()?;
            let eligibility = source_eligibility_from_db(
                row.try_get::<Option<String>, _>("source_eligibility")?
                    .as_deref(),
                row.try_get::<Option<String>, _>("source_ineligibility_reason")?
                    .as_deref(),
            )?;
            let lifecycle = row
                .try_get::<Option<String>, _>("source_lifecycle_class")?
                .map(|value| lifecycle_from_db(&value))
                .transpose()?;
            let (head, semantic_text) = if availability == CurrentSemanticAvailabilityClass::Current
            {
                let head = semantic_head_from_row(row, "")?;
                validate_head_for_source(&head, &source)?;
                if source_invalidation_epoch != Some(head.invalidation_epoch) {
                    return Err(DbError::InvalidData(
                        "semantic current head epoch disagrees with its source".to_string(),
                    ));
                }
                (Some(head), row.try_get("semantic_text")?)
            } else {
                (None, None)
            };
            Ok(CurrentSemanticSourceState {
                source,
                source_invalidation_epoch,
                eligibility,
                lifecycle,
                availability,
                head,
                semantic_text,
            })
        })
        .collect()
}

fn partition_exact_recall(
    channel_ids: &[Digest32],
    observed: Vec<SemanticExactSourceScore>,
    recall_per_channel: u16,
) -> Result<SemanticExactRecallBatch> {
    let public_limit = u32::from(recall_per_channel);
    let observed_limit = public_limit.checked_add(1).ok_or_else(|| {
        DbError::InvalidData("semantic recall observation limit overflow".to_string())
    })?;
    let mut channel_counts: BTreeMap<Digest32, (u16, bool)> = channel_ids
        .iter()
        .copied()
        .map(|channel_id| (channel_id, (0, false)))
        .collect();
    if channel_counts.len() != channel_ids.len() {
        return Err(DbError::InvalidData(
            "semantic recall channel ids must be unique".to_string(),
        ));
    }
    let mut scores = Vec::with_capacity(observed.len());
    for score in observed {
        let state = channel_counts.get_mut(&score.channel_id).ok_or_else(|| {
            DbError::InvalidData("semantic recall returned an unknown query channel".to_string())
        })?;
        if score.channel_rank <= public_limit {
            state.0 = state.0.checked_add(1).ok_or_else(|| {
                DbError::InvalidData("semantic recall channel count overflow".to_string())
            })?;
            scores.push(score);
        } else if score.channel_rank == observed_limit {
            state.1 = true;
        } else {
            return Err(DbError::InvalidData(
                "semantic recall returned a rank beyond its K+1 observation".to_string(),
            ));
        }
    }
    let channels = channel_counts
        .into_iter()
        .map(
            |(channel_id, (returned_count, truncated))| SemanticExactRecallChannelObservation {
                channel_id,
                returned_count,
                exhaustion: if truncated {
                    SemanticExactRecallExhaustion::Truncated
                } else {
                    SemanticExactRecallExhaustion::Exhausted
                },
            },
        )
        .collect();
    Ok(SemanticExactRecallBatch { scores, channels })
}

/// Convert a validated Project Context Coordinate into the corresponding
/// tenant-scoped canonical semantic source identity.
pub fn semantic_source_identity_for_coordinate(
    community_id: CommunityId,
    coordinate: &ProjectContextCoordinate,
) -> Result<SemanticSourceIdentity> {
    coordinate
        .validate_for_project(*community_id.as_uuid())
        .map_err(|error| {
            DbError::InvalidData(format!("invalid semantic query Coordinate: {error}"))
        })?;
    let (kind, source_id) = match coordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        } => (
            SemanticSourceKind::ProjectView(project_view_semantic_type_for_coordinate(
                *object_type,
            )),
            *object_id,
        ),
        ProjectContextCoordinate::Document { document_id } => {
            (SemanticSourceKind::ProjectDocument, *document_id)
        }
        ProjectContextCoordinate::Meeting { meeting_id } => {
            (SemanticSourceKind::Meeting, *meeting_id)
        }
    };
    let identity = SemanticSourceIdentity {
        community_id: *community_id.as_uuid(),
        kind,
        source_id,
    };
    identity.validate().map_err(|error| {
        DbError::InvalidData(format!(
            "invalid semantic Coordinate source identity: {error}"
        ))
    })?;
    Ok(identity)
}

fn project_view_semantic_type_for_coordinate(
    object_type: ProjectViewObjectType,
) -> ProjectViewSemanticType {
    match object_type {
        ProjectViewObjectType::ProjectProfile => ProjectViewSemanticType::ProjectProfile,
        ProjectViewObjectType::Goal => ProjectViewSemanticType::Goal,
        ProjectViewObjectType::Role => ProjectViewSemanticType::Role,
        ProjectViewObjectType::Plan => ProjectViewSemanticType::Plan,
        ProjectViewObjectType::Stage => ProjectViewSemanticType::Stage,
        ProjectViewObjectType::Requirement => ProjectViewSemanticType::Requirement,
        ProjectViewObjectType::Issue => ProjectViewSemanticType::Issue,
        ProjectViewObjectType::Work => ProjectViewSemanticType::Work,
        ProjectViewObjectType::Resource => ProjectViewSemanticType::Resource,
    }
}

fn canonical_query_coordinates(
    community_id: CommunityId,
    coordinates: &[ProjectContextCoordinate],
    maximum: usize,
    label: &str,
) -> Result<Vec<ProjectContextCoordinate>> {
    if coordinates.len() > maximum {
        return Err(DbError::InvalidData(format!(
            "semantic {label} count exceeds the server bound"
        )));
    }
    let mut canonical = coordinates.to_vec();
    for coordinate in &canonical {
        coordinate
            .validate_for_project(*community_id.as_uuid())
            .map_err(|error| DbError::InvalidData(format!("invalid semantic {label}: {error}")))?;
    }
    canonical.sort();
    if canonical.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(DbError::InvalidData(format!(
            "semantic {label} identities must be unique"
        )));
    }
    Ok(canonical)
}

fn semantic_source_sort_key(source: &SemanticSourceIdentity) -> (&'static str, &'static str, Uuid) {
    let (family, subtype) = semantic_source_db_key(source.kind);
    (family, subtype, source.source_id)
}

fn canonical_snapshot(
    observation: CanonicalSemanticSourceObservation,
    source_invalidation_epoch: u64,
) -> SemanticCanonicalSourceSnapshot {
    SemanticCanonicalSourceSnapshot {
        source: observation.identity,
        source_invalidation_epoch,
        source_basis: observation.basis,
        source_snapshot_digest: observation.snapshot_digest,
        lifecycle: observation.filter.lifecycle,
        source_status: observation.filter.source_status,
        title: observation.title,
        summary: observation.summary,
    }
}

fn omitted_context_evidence(
    source: &SemanticSourceIdentity,
    state: &CurrentSemanticSourceState,
    canonical: Option<&CanonicalSemanticSourceObservation>,
) -> SemanticOmittedContextEvidence {
    SemanticOmittedContextEvidence {
        source: source.clone(),
        source_invalidation_epoch: state.source_invalidation_epoch,
        source_basis: canonical.map(|observation| observation.basis.clone()),
        source_snapshot_digest: canonical.map(|observation| observation.snapshot_digest),
    }
}

fn ensure_eligible_canonical_observation(
    observation: &CanonicalSemanticSourceObservation,
) -> Result<()> {
    if !matches!(observation.eligibility, SemanticEligibility::Eligible) {
        return Err(DbError::InvalidData(
            "semantic current head resolves to an ineligible canonical source".to_string(),
        ));
    }
    Ok(())
}

fn validate_canonical_against_head(
    observation: &CanonicalSemanticSourceObservation,
    head: &SemanticCurrentHead,
) -> Result<()> {
    ensure_eligible_canonical_observation(observation)?;
    let expected_coverage = if observation.summary.is_some() {
        SemanticCoverage::TitleAndSummary
    } else {
        SemanticCoverage::TitleOnly
    };
    if observation.basis != head.source_basis
        || observation.snapshot_digest != head.snapshot_digest
        || expected_coverage != head.summary_coverage
    {
        return Err(DbError::InvalidData(
            "semantic current head disagrees with its canonical source snapshot".to_string(),
        ));
    }
    Ok(())
}

const fn initial_omission_reason(reason: IneligibilityReason) -> SemanticInitialOmissionReason {
    match reason {
        IneligibilityReason::Deleted => SemanticInitialOmissionReason::SourceDeleted,
        IneligibilityReason::Tombstone => SemanticInitialOmissionReason::SourceTombstoned,
        IneligibilityReason::InvalidCanonicalState
        | IneligibilityReason::SourceCapabilityUnavailable => {
            SemanticInitialOmissionReason::SourceIneligible
        }
    }
}

fn current_availability_from_db(value: &str) -> Result<CurrentSemanticAvailabilityClass> {
    match value {
        "current" => Ok(CurrentSemanticAvailabilityClass::Current),
        "missing" => Ok(CurrentSemanticAvailabilityClass::Missing),
        "building" => Ok(CurrentSemanticAvailabilityClass::Building),
        "failed" => Ok(CurrentSemanticAvailabilityClass::Failed),
        "unsupported" => Ok(CurrentSemanticAvailabilityClass::Unsupported),
        _ => Err(DbError::InvalidData(
            "semantic current source availability is invalid".to_string(),
        )),
    }
}

fn source_eligibility_from_db(
    eligibility: Option<&str>,
    reason: Option<&str>,
) -> Result<Option<SemanticEligibility>> {
    match (eligibility, reason) {
        (None, None) => Ok(None),
        (Some("eligible"), None) => Ok(Some(SemanticEligibility::Eligible)),
        (Some("ineligible"), Some("tombstone")) => Ok(Some(SemanticEligibility::Ineligible(
            IneligibilityReason::Tombstone,
        ))),
        (Some("ineligible"), Some("deleted")) => Ok(Some(SemanticEligibility::Ineligible(
            IneligibilityReason::Deleted,
        ))),
        (Some("ineligible"), Some("invalid_canonical_state")) => Ok(Some(
            SemanticEligibility::Ineligible(IneligibilityReason::InvalidCanonicalState),
        )),
        (Some("ineligible"), Some("source_capability_unavailable")) => Ok(Some(
            SemanticEligibility::Ineligible(IneligibilityReason::SourceCapabilityUnavailable),
        )),
        _ => Err(DbError::InvalidData(
            "semantic source eligibility row is inconsistent".to_string(),
        )),
    }
}

fn edge_key_from_bytes(value: Vec<u8>, field: &str) -> Result<EdgeKey> {
    let bytes: [u8; 32] = value
        .try_into()
        .map_err(|_| DbError::InvalidData(format!("semantic {field} must contain 32 bytes")))?;
    EdgeKey::from_hex(&hex::encode(bytes))
        .map_err(|error| DbError::InvalidData(format!("semantic {field} is invalid: {error}")))
}

fn positive_usize(value: i64, field: &str) -> Result<usize> {
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| DbError::InvalidData(format!("semantic {field} must be positive")))
}

struct SourceKeyArrays {
    families: Vec<&'static str>,
    subtypes: Vec<&'static str>,
    ids: Vec<Uuid>,
}

struct CoordinateKeyArrays {
    types: Vec<&'static str>,
    subtypes: Vec<Option<&'static str>>,
    ids: Vec<Uuid>,
}

fn coordinate_key_arrays(coordinates: &[ProjectContextCoordinate]) -> CoordinateKeyArrays {
    let mut types = Vec::with_capacity(coordinates.len());
    let mut subtypes = Vec::with_capacity(coordinates.len());
    let mut ids = Vec::with_capacity(coordinates.len());
    for coordinate in coordinates {
        match coordinate {
            ProjectContextCoordinate::ProjectViewObject {
                object_type,
                object_id,
            } => {
                types.push("project_view_object");
                subtypes.push(Some(object_type.as_str()));
                ids.push(*object_id);
            }
            ProjectContextCoordinate::Document { document_id } => {
                types.push("document");
                subtypes.push(None);
                ids.push(*document_id);
            }
            ProjectContextCoordinate::Meeting { meeting_id } => {
                types.push("meeting");
                subtypes.push(None);
                ids.push(*meeting_id);
            }
        }
    }
    CoordinateKeyArrays {
        types,
        subtypes,
        ids,
    }
}

fn source_key_arrays(sources: &[SemanticSourceIdentity]) -> SourceKeyArrays {
    let mut families = Vec::with_capacity(sources.len());
    let mut subtypes = Vec::with_capacity(sources.len());
    let mut ids = Vec::with_capacity(sources.len());
    for source in sources {
        let (family, subtype) = semantic_source_db_key(source.kind);
        families.push(family);
        subtypes.push(subtype);
        ids.push(source.source_id);
    }
    SourceKeyArrays {
        families,
        subtypes,
        ids,
    }
}

fn validate_source_inputs(
    community_id: CommunityId,
    sources: &[SemanticSourceIdentity],
    maximum: usize,
    label: &str,
) -> Result<()> {
    if sources.len() > maximum {
        return Err(DbError::InvalidData(format!(
            "semantic {label} count exceeds the server bound"
        )));
    }
    for source in sources {
        source
            .validate()
            .map_err(|error| DbError::InvalidData(format!("invalid semantic {label}: {error}")))?;
        if source.community_id != *community_id.as_uuid() {
            return Err(DbError::InvalidData(format!(
                "semantic {label} crosses the host-derived Community"
            )));
        }
    }
    Ok(())
}

fn validate_query_vectors(
    ticket: &SemanticGraphQueryTicket,
    query_vectors: &[SemanticExactQueryVector],
) -> Result<()> {
    if query_vectors.is_empty() || query_vectors.len() > MAX_QUERY_CHANNELS {
        return Err(DbError::InvalidData(
            "semantic query vector count is outside the server bound".to_string(),
        ));
    }
    let dimensions = ticket.generation.model_contract.dimensions;
    for (index, channel) in query_vectors.iter().enumerate() {
        QueryCompatibilityFences::validate_observed(
            &ticket.generation.model_contract,
            channel.query_fences.source_generation_contract_digest,
            channel.query_fences.embedding_space_fence,
            channel.query_fences.query_contract_digest,
        )
        .map_err(|error| {
            DbError::InvalidData(format!(
                "semantic query vector {index} compatibility fence mismatch: {error}"
            ))
        })?;
        if channel.query_fences != ticket.query_fences {
            return Err(DbError::InvalidData(format!(
                "semantic query vector {index} does not match the Stage C ticket"
            )));
        }
        if channel.embedding.as_slice().len() != dimensions
            || channel
                .embedding
                .as_slice()
                .iter()
                .any(|value| !value.is_finite())
            || vector_norm_squared(channel.embedding.as_slice()) <= 0.0
        {
            return Err(DbError::InvalidData(format!(
                "semantic query vector {index} is not a finite non-zero ticket-space vector"
            )));
        }
    }
    let mut ids: Vec<Digest32> = query_vectors
        .iter()
        .map(|channel| channel.channel_id)
        .collect();
    ids.sort_unstable();
    if ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(DbError::InvalidData(
            "semantic query channel ids must be unique".to_string(),
        ));
    }
    Ok(())
}

fn vector_norm_squared(values: &[f32]) -> f64 {
    values
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum()
}

const fn lifecycle_filter_db(filter: LifecycleFilter) -> &'static str {
    match filter {
        LifecycleFilter::AllCurrent => "all_current",
        LifecycleFilter::NonTerminal => "non_terminal",
        LifecycleFilter::TerminalOnly => "terminal_only",
    }
}

fn exact_score_from_row(row: &sqlx::postgres::PgRow) -> Result<SemanticExactSourceScore> {
    let source = source_identity_from_row(row, "source_family", "source_subtype", "source_id")?;
    let head = semantic_head_from_row(row, "")?;
    validate_head_for_source(&head, &source)?;
    let bindings: Value = row.try_get("bindings")?;
    let coordinate_incident_edge_keys: Value = row.try_get("coordinate_incident_edge_keys")?;
    let coordinate: bool = row.try_get("is_coordinate")?;
    let coordinate_entry_eligible: bool = row.try_get("coordinate_entry_eligible")?;
    let coordinate_incident_edge_keys =
        edge_keys_from_json(coordinate_incident_edge_keys, "coordinate incident Edge")?;
    if coordinate == coordinate_incident_edge_keys.is_empty()
        || (coordinate_entry_eligible && !coordinate)
    {
        return Err(DbError::InvalidData(
            "semantic Coordinate structural role metadata is inconsistent".to_string(),
        ));
    }
    Ok(SemanticExactSourceScore {
        channel_id: digest_from_bytes(row.try_get("channel_id")?, "channel_id")?,
        source,
        head,
        lifecycle: lifecycle_from_db(row.try_get::<String, _>("lifecycle_class")?.as_str())?,
        source_status: row.try_get("source_status")?,
        roles: SemanticGraphStructuralRoles {
            coordinate,
            coordinate_entry_eligible,
            coordinate_incident_edge_keys,
            context_document_bindings: bindings_from_json(bindings)?,
        },
        score: score_from_i64(row.try_get("semantic_score")?)?,
        channel_rank: positive_u32(row.try_get("channel_rank")?, "channel_rank")?,
    })
}

fn source_pair_from_row(row: &sqlx::postgres::PgRow) -> Result<SemanticCurrentSourcePairDistance> {
    let left = source_identity_from_row(
        row,
        "left_source_family",
        "left_source_subtype",
        "left_source_id",
    )?;
    let right = source_identity_from_row(
        row,
        "right_source_family",
        "right_source_subtype",
        "right_source_id",
    )?;
    let left_head = semantic_head_from_row(row, "left_")?;
    let right_head = semantic_head_from_row(row, "right_")?;
    validate_head_for_source(&left_head, &left)?;
    validate_head_for_source(&right_head, &right)?;
    Ok(SemanticCurrentSourcePairDistance {
        left,
        right,
        left_head,
        right_head,
        score: score_from_i64(row.try_get("semantic_score")?)?,
    })
}

fn source_identity_from_row(
    row: &sqlx::postgres::PgRow,
    family_field: &str,
    subtype_field: &str,
    id_field: &str,
) -> Result<SemanticSourceIdentity> {
    let family: String = row.try_get(family_field)?;
    let subtype: String = row.try_get(subtype_field)?;
    let identity = SemanticSourceIdentity {
        community_id: row.try_get::<Uuid, _>("community_id")?,
        kind: semantic_source_kind_from_db(&family, &subtype)?,
        source_id: row.try_get(id_field)?,
    };
    identity
        .validate()
        .map_err(|error| DbError::InvalidData(format!("invalid semantic source row: {error}")))?;
    Ok(identity)
}

fn semantic_head_from_row(
    row: &sqlx::postgres::PgRow,
    prefix: &str,
) -> Result<SemanticCurrentHead> {
    let field = |name: &str| format!("{prefix}{name}");
    let source_basis: Value = row.try_get(field("source_basis").as_str())?;
    Ok(SemanticCurrentHead {
        invalidation_epoch: positive_u64(
            row.try_get(field("invalidation_epoch").as_str())?,
            "invalidation_epoch",
        )?,
        snapshot_digest: digest_from_bytes(
            row.try_get(field("snapshot_digest").as_str())?,
            "snapshot_digest",
        )?,
        source_basis: serde_json::from_value(source_basis).map_err(|error| {
            DbError::InvalidData(format!("semantic source basis is invalid: {error}"))
        })?,
        unit_set_id: row.try_get(field("unit_set_id").as_str())?,
        unit_key: row.try_get(field("unit_key").as_str())?,
        semantic_text_digest: digest_from_bytes(
            row.try_get(field("semantic_text_digest").as_str())?,
            "semantic_text_digest",
        )?,
        summary_coverage: coverage_from_db(
            row.try_get::<String, _>(field("summary_coverage").as_str())?
                .as_str(),
        )?,
    })
}

fn validate_head_for_source(
    head: &SemanticCurrentHead,
    source: &SemanticSourceIdentity,
) -> Result<()> {
    let valid = match (&head.source_basis, source.kind) {
        (SemanticSourceBasis::ProjectView(basis), SemanticSourceKind::ProjectView(_)) => {
            basis.schema_version > 0 && basis.object_revision > 0
        }
        (SemanticSourceBasis::ProjectDocument(basis), SemanticSourceKind::ProjectDocument) => {
            basis.document_revision > 0
        }
        (SemanticSourceBasis::Meeting(_), SemanticSourceKind::Meeting) => true,
        _ => false,
    };
    if !valid {
        return Err(DbError::InvalidData(
            "semantic current head basis does not match its source identity".to_string(),
        ));
    }
    Ok(())
}

fn bindings_from_json(value: Value) -> Result<Vec<SemanticContextDocumentBinding>> {
    let values = value.as_array().ok_or_else(|| {
        DbError::InvalidData("semantic structural bindings must be a JSON array".to_string())
    })?;
    let mut bindings = Vec::with_capacity(values.len());
    for value in values {
        let object = value.as_object().ok_or_else(|| {
            DbError::InvalidData("semantic structural binding must be an object".to_string())
        })?;
        let string = |field: &str| -> Result<&str> {
            object.get(field).and_then(Value::as_str).ok_or_else(|| {
                DbError::InvalidData(format!("semantic structural binding {field} is invalid"))
            })
        };
        let integer = |field: &str| -> Result<u64> {
            object
                .get(field)
                .and_then(Value::as_u64)
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    DbError::InvalidData(format!("semantic structural binding {field} is invalid"))
                })
        };
        bindings.push(SemanticContextDocumentBinding {
            edge_key: EdgeKey::from_hex(string("edge_key")?).map_err(|error| {
                DbError::InvalidData(format!("semantic structural edge key is invalid: {error}"))
            })?,
            edge_last_context_revision: integer("edge_last_context_revision")?,
            edge_source_change_id: digest_from_hex(
                string("edge_source_change_id")?,
                "edge_source_change_id",
            )?,
            binding_context_revision: integer("binding_context_revision")?,
            binding_source_change_id: digest_from_hex(
                string("binding_source_change_id")?,
                "binding_source_change_id",
            )?,
            binding_projection_event_id: digest_from_hex(
                string("binding_projection_event_id")?,
                "binding_projection_event_id",
            )?,
        });
    }
    bindings.sort();
    if bindings.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(DbError::InvalidData(
            "semantic structural binding metadata is duplicated".to_string(),
        ));
    }
    Ok(bindings)
}

fn edge_keys_from_json(value: Value, field: &str) -> Result<Vec<EdgeKey>> {
    let values = value.as_array().ok_or_else(|| {
        DbError::InvalidData(format!("semantic {field} keys must be a JSON array"))
    })?;
    let mut keys = values
        .iter()
        .map(|value| {
            let value = value.as_str().ok_or_else(|| {
                DbError::InvalidData(format!("semantic {field} key must be a string"))
            })?;
            EdgeKey::from_hex(value).map_err(|error| {
                DbError::InvalidData(format!("semantic {field} key is invalid: {error}"))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    keys.sort();
    if keys.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(DbError::InvalidData(format!(
            "semantic {field} keys are duplicated"
        )));
    }
    Ok(keys)
}

fn digest_from_bytes(value: Vec<u8>, field: &str) -> Result<Digest32> {
    let bytes: [u8; 32] = value
        .try_into()
        .map_err(|_| DbError::InvalidData(format!("semantic {field} must contain 32 bytes")))?;
    Ok(Digest32::from_bytes(bytes))
}

fn digest_from_hex(value: &str, field: &str) -> Result<Digest32> {
    Digest32::from_hex(value)
        .map_err(|_| DbError::InvalidData(format!("semantic {field} must be a 32-byte digest")))
}

fn positive_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| DbError::InvalidData(format!("semantic {field} must be positive")))
}

fn nonnegative_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| DbError::InvalidData(format!("semantic {field} must not be negative")))
}

fn positive_u32(value: i64, field: &str) -> Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| DbError::InvalidData(format!("semantic {field} must be positive")))
}

fn score_from_i64(value: i64) -> Result<Score> {
    let raw = u32::try_from(value)
        .map_err(|_| DbError::InvalidData("semantic DB score is outside u32".to_string()))?;
    Score::new(raw)
        .map_err(|error| DbError::InvalidData(format!("semantic DB score is invalid: {error}")))
}

fn coverage_from_db(value: &str) -> Result<SemanticCoverage> {
    match value {
        "title_only" => Ok(SemanticCoverage::TitleOnly),
        "title_and_summary" => Ok(SemanticCoverage::TitleAndSummary),
        _ => Err(DbError::InvalidData(
            "semantic summary coverage is invalid".to_string(),
        )),
    }
}

fn lifecycle_from_db(value: &str) -> Result<SemanticLifecycleClass> {
    match value {
        "active" => Ok(SemanticLifecycleClass::Active),
        "finalizing" => Ok(SemanticLifecycleClass::Finalizing),
        "terminal" => Ok(SemanticLifecycleClass::Terminal),
        "tombstone" => Ok(SemanticLifecycleClass::Tombstone),
        "deleted" => Ok(SemanticLifecycleClass::Deleted),
        _ => Err(DbError::InvalidData(
            "semantic lifecycle class is invalid".to_string(),
        )),
    }
}

fn coverage_class_from_db(value: &str) -> Result<SemanticGraphEmbeddingCoverageClass> {
    match value {
        "current" => Ok(SemanticGraphEmbeddingCoverageClass::Current),
        "non_queryable_zero_vector" => {
            Ok(SemanticGraphEmbeddingCoverageClass::NonQueryableZeroVector)
        }
        "failed" => Ok(SemanticGraphEmbeddingCoverageClass::Failed),
        "building" => Ok(SemanticGraphEmbeddingCoverageClass::Building),
        "unsupported" => Ok(SemanticGraphEmbeddingCoverageClass::Unsupported),
        "missing" => Ok(SemanticGraphEmbeddingCoverageClass::Missing),
        _ => Err(DbError::InvalidData(
            "semantic graph coverage class is invalid".to_string(),
        )),
    }
}

const fn semantic_source_db_key(kind: SemanticSourceKind) -> (&'static str, &'static str) {
    match kind {
        SemanticSourceKind::ProjectView(subtype) => (
            "project_view",
            match subtype {
                buzz_semantic::ProjectViewSemanticType::ProjectProfile => "project_profile",
                buzz_semantic::ProjectViewSemanticType::Goal => "goal",
                buzz_semantic::ProjectViewSemanticType::Role => "role",
                buzz_semantic::ProjectViewSemanticType::Plan => "plan",
                buzz_semantic::ProjectViewSemanticType::Stage => "stage",
                buzz_semantic::ProjectViewSemanticType::Requirement => "requirement",
                buzz_semantic::ProjectViewSemanticType::Issue => "issue",
                buzz_semantic::ProjectViewSemanticType::Work => "work",
                buzz_semantic::ProjectViewSemanticType::Resource => "resource",
            },
        ),
        SemanticSourceKind::ProjectDocument => ("project_document", "document"),
        SemanticSourceKind::Meeting => ("meeting", "meeting"),
    }
}

const CURRENT_SOURCE_PAIR_SCORES_SQL: &str = r#"
WITH requested_reader(pubkey) AS (VALUES ($2::bytea)),
authorized_reader AS MATERIALIZED (
  SELECT requested_reader.pubkey
  FROM requested_reader
  LEFT JOIN users actor
    ON actor.community_id = $1 AND actor.pubkey = requested_reader.pubkey
  WHERE (
    (actor.agent_owner_pubkey IS NULL AND EXISTS (
      SELECT 1 FROM relay_members member
      WHERE member.community_id = $1
        AND member.pubkey = encode(requested_reader.pubkey, 'hex')
    ))
    OR (
      actor.agent_owner_pubkey IS NOT NULL
      AND EXISTS (
        SELECT 1 FROM relay_members owner_member
        WHERE owner_member.community_id = $1
          AND owner_member.pubkey = encode(actor.agent_owner_pubkey, 'hex')
      )
      AND NOT EXISTS (
        SELECT 1 FROM community_bans owner_ban
        WHERE owner_ban.community_id = $1
          AND owner_ban.pubkey = actor.agent_owner_pubkey
          AND owner_ban.banned
          AND (owner_ban.ban_expires_at IS NULL
               OR owner_ban.ban_expires_at > clock_timestamp())
      )
      AND NOT EXISTS (
        SELECT 1 FROM users owner_actor
        WHERE owner_actor.community_id = $1
          AND owner_actor.pubkey = actor.agent_owner_pubkey
          AND owner_actor.agent_owner_pubkey IS NOT NULL
      )
    )
  )
  AND NOT EXISTS (
    SELECT 1 FROM community_bans actor_ban
    WHERE actor_ban.community_id = $1
      AND actor_ban.pubkey = requested_reader.pubkey
      AND actor_ban.banned
      AND (actor_ban.ban_expires_at IS NULL
           OR actor_ban.ban_expires_at > clock_timestamp())
  )
),
authorized_project AS MATERIALIZED (
  SELECT community.id AS community_id,
         community.semantic_active_generation_id AS generation_id
  FROM communities community CROSS JOIN authorized_reader
  JOIN project_view_maintenance maintenance ON maintenance.community_id = community.id
  JOIN project_view_state view_state ON view_state.community_id = community.id
  JOIN project_document_state document_state ON document_state.community_id = community.id
  JOIN project_context_edge_state context_state ON context_state.community_id = community.id
  WHERE community.id = $1
    AND community.archived_at IS NULL
    AND community.project_view_schema_version = 3
    AND community.project_view_enabled
    AND community.project_document_enabled
    AND community.meeting_community_read_enabled
    AND community.project_context_edge_enabled
    AND community.semantic_index_enabled
    AND community.semantic_graph_query_enabled
    AND maintenance.state = 'normal'
    AND view_state.schema_version = 3
    AND document_state.schema_version = 1
    AND context_state.schema_version = 2
    AND view_state.projection_pubkey = $3
    AND document_state.projection_pubkey = $3
    AND context_state.projection_pubkey = $3
    AND community.semantic_active_generation_id = $4
),
active_generation AS MATERIALIZED (
  SELECT generation.*
  FROM authorized_project project
  JOIN semantic_index_generations generation
    ON generation.community_id = project.community_id
   AND generation.generation_id = project.generation_id
  WHERE generation.lifecycle = 'active'
    AND generation.model_contract_digest = $5
    AND generation.extractor_version = $6
    AND generation.model = $7
    AND generation.dimensions = $8
    AND generation.distance_metric = 'cosine'
),
requested_pairs(
  ordinal, left_family, left_subtype, left_id,
  right_family, right_subtype, right_id
) AS MATERIALIZED (
  SELECT * FROM unnest(
    $9::int4[], $10::text[], $11::text[], $12::uuid[],
    $13::text[], $14::text[], $15::uuid[]
  )
),
requested_sources(source_family, source_subtype, source_id) AS MATERIALIZED (
  SELECT left_family, left_subtype, left_id FROM requested_pairs
  UNION
  SELECT right_family, right_subtype, right_id FROM requested_pairs
),
current_embeddings AS MATERIALIZED (
  SELECT source.community_id, source.source_family, source.source_subtype,
    source.source_id, source.invalidation_epoch, source.snapshot_digest,
    source.source_basis, unit_set.unit_set_id, unit.unit_key,
    unit.semantic_text_digest, unit.summary_coverage, embedding.embedding
  FROM active_generation generation
  JOIN semantic_source_generation_heads head
    ON head.community_id = generation.community_id
   AND head.generation_id = generation.generation_id
  JOIN requested_sources requested
    ON requested.source_family = head.source_family
   AND requested.source_subtype = head.source_subtype
   AND requested.source_id = head.source_id
  JOIN semantic_sources source
    ON source.community_id = head.community_id
   AND source.source_family = head.source_family
   AND source.source_subtype = head.source_subtype
   AND source.source_id = head.source_id
   AND source.invalidation_epoch = head.source_invalidation_epoch
   AND source.snapshot_digest = head.source_snapshot_digest
  JOIN semantic_unit_sets unit_set
    ON unit_set.community_id = head.community_id
   AND unit_set.unit_set_id = head.unit_set_id
   AND unit_set.source_family = head.source_family
   AND unit_set.source_subtype = head.source_subtype
   AND unit_set.source_id = head.source_id
   AND unit_set.source_invalidation_epoch = head.source_invalidation_epoch
   AND unit_set.source_snapshot_digest = head.source_snapshot_digest
   AND unit_set.state = 'active'
   AND unit_set.extractor_version = generation.extractor_version
  JOIN semantic_units unit
    ON unit.community_id = unit_set.community_id
   AND unit.unit_set_id = unit_set.unit_set_id
   AND unit.unit_kind = 'overview' AND unit.unit_key = 'overview'
  JOIN semantic_embeddings embedding
    ON embedding.community_id = unit.community_id
   AND embedding.unit_set_id = unit.unit_set_id
   AND embedding.unit_key = unit.unit_key
   AND embedding.generation_id = generation.generation_id
   AND embedding.model_contract_digest = generation.model_contract_digest
   AND embedding.dimensions = generation.dimensions
   AND embedding.response_model = generation.model
   AND vector_dims(embedding.embedding) = generation.dimensions
   AND vector_norm(embedding.embedding) > 0
  WHERE source.eligibility = 'eligible'
),
distances AS MATERIALIZED (
  SELECT pair.ordinal,
    left_source.community_id,
    left_source.source_family AS left_source_family,
    left_source.source_subtype AS left_source_subtype,
    left_source.source_id AS left_source_id,
    left_source.invalidation_epoch AS left_invalidation_epoch,
    left_source.snapshot_digest AS left_snapshot_digest,
    left_source.source_basis AS left_source_basis,
    left_source.unit_set_id AS left_unit_set_id,
    left_source.unit_key AS left_unit_key,
    left_source.semantic_text_digest AS left_semantic_text_digest,
    left_source.summary_coverage AS left_summary_coverage,
    right_source.source_family AS right_source_family,
    right_source.source_subtype AS right_source_subtype,
    right_source.source_id AS right_source_id,
    right_source.invalidation_epoch AS right_invalidation_epoch,
    right_source.snapshot_digest AS right_snapshot_digest,
    right_source.source_basis AS right_source_basis,
    right_source.unit_set_id AS right_unit_set_id,
    right_source.unit_key AS right_unit_key,
    right_source.semantic_text_digest AS right_semantic_text_digest,
    right_source.summary_coverage AS right_summary_coverage,
    left_source.embedding <=> right_source.embedding AS distance
  FROM requested_pairs pair
  JOIN current_embeddings left_source
    ON left_source.source_family = pair.left_family
   AND left_source.source_subtype = pair.left_subtype
   AND left_source.source_id = pair.left_id
  JOIN current_embeddings right_source
    ON right_source.source_family = pair.right_family
   AND right_source.source_subtype = pair.right_subtype
   AND right_source.source_id = pair.right_id
)
SELECT *, floor((
  (greatest(-1.0, least(1.0, 1.0 - distance)) + 1.0) / 2.0
) * 1000000.0 + 0.5)::bigint AS semantic_score
FROM distances
WHERE distance > '-Infinity'::double precision
  AND distance < 'Infinity'::double precision
ORDER BY ordinal
"#;

const CURRENT_COORDINATE_MEMBERSHIPS_SQL: &str = r#"
WITH authorized_snapshot AS MATERIALIZED (
    SELECT state.community_id
    FROM project_context_edge_state state
    WHERE state.community_id = $1
      AND state.schema_version = 2
      AND state.context_revision = $5
),
requested(coordinate_type, coordinate_subtype, coordinate_id, request_ordinal) AS MATERIALIZED (
    SELECT coordinate_type, coordinate_subtype, coordinate_id, request_ordinal
    FROM unnest($2::text[], $3::text[], $4::uuid[])
         WITH ORDINALITY AS input(
             coordinate_type, coordinate_subtype, coordinate_id, request_ordinal
         )
)
SELECT requested.request_ordinal, edge.edge_key
FROM authorized_snapshot
CROSS JOIN requested
LEFT JOIN project_context_edge_coordinates coordinate
  ON coordinate.community_id = authorized_snapshot.community_id
 AND coordinate.coordinate_type = requested.coordinate_type
 AND coordinate.coordinate_subtype IS NOT DISTINCT FROM requested.coordinate_subtype
 AND coordinate.coordinate_id = requested.coordinate_id
LEFT JOIN project_context_edges edge
  ON edge.community_id = coordinate.community_id
 AND edge.edge_key = coordinate.edge_key
 AND edge.state = 'active'
ORDER BY requested.request_ordinal, edge.edge_key
"#;

const COMPLETE_HYPEREDGE_SQL: &str = r#"
WITH authorized_snapshot AS MATERIALIZED (
    SELECT state.community_id
    FROM project_context_edge_state state
    WHERE state.community_id = $1
      AND state.schema_version = 2
      AND state.context_revision = $3
)
SELECT edge.edge_key, edge.canonical_coordinates,
       edge.last_context_revision,
       edge.current_source_change_id AS edge_source_change_id,
       binding.context_document_id, binding.binding_context_revision,
       binding.current_source_change_id AS binding_source_change_id,
       binding.current_projection_event_id AS binding_projection_event_id
FROM authorized_snapshot snapshot
JOIN project_context_edges edge
  ON edge.community_id = snapshot.community_id
 AND edge.edge_key = $2
 AND edge.state = 'active'
JOIN project_context_document_bindings binding
  ON binding.community_id = edge.community_id
 AND binding.edge_key = edge.edge_key
 AND binding.state = 'active'
ORDER BY binding.context_document_id
"#;

const INCIDENT_RELATION_REFS_SQL: &str = r#"
WITH authorized_snapshot AS MATERIALIZED (
    SELECT state.community_id
    FROM project_context_edge_state state
    WHERE state.community_id = $1
      AND state.schema_version = 2
      AND state.context_revision = $5
)
SELECT edge.edge_key,
       edge.last_context_revision AS edge_last_context_revision,
       edge.current_source_change_id AS edge_source_change_id,
       binding.context_document_id, binding.binding_context_revision,
       binding.current_source_change_id AS binding_source_change_id,
       binding.current_projection_event_id AS binding_projection_event_id
FROM authorized_snapshot snapshot
JOIN project_context_edge_coordinates coordinate
  ON coordinate.community_id = snapshot.community_id
 AND coordinate.coordinate_type = $2
 AND coordinate.coordinate_subtype IS NOT DISTINCT FROM $3
 AND coordinate.coordinate_id = $4
JOIN project_context_edges edge
  ON edge.community_id = coordinate.community_id
 AND edge.edge_key = coordinate.edge_key
 AND edge.state = 'active'
JOIN project_context_document_bindings binding
  ON binding.community_id = edge.community_id
 AND binding.edge_key = edge.edge_key
 AND binding.state = 'active'
ORDER BY edge.edge_key, binding.context_document_id
LIMIT $6
"#;

const CURRENT_SEMANTIC_SOURCE_STATES_SQL: &str = r#"
WITH active_generation AS MATERIALIZED (
    SELECT generation.*
    FROM semantic_index_generations generation
    WHERE generation.community_id = $1
      AND generation.generation_id = $2
      AND generation.lifecycle = 'active'
      AND generation.model_contract_digest = $3
      AND generation.extractor_version = $4
      AND generation.model = $5
      AND generation.dimensions = $6
      AND generation.distance_metric = 'cosine'
),
requested(source_family, source_subtype, source_id, request_ordinal) AS MATERIALIZED (
    SELECT source_family, source_subtype, source_id, request_ordinal
    FROM unnest($7::text[], $8::text[], $9::uuid[])
         WITH ORDINALITY AS input(
             source_family, source_subtype, source_id, request_ordinal
         )
)
SELECT generation.community_id,
       requested.source_family, requested.source_subtype, requested.source_id,
       source.invalidation_epoch AS source_invalidation_epoch,
       source.eligibility AS source_eligibility,
       source.ineligibility_reason AS source_ineligibility_reason,
       source.lifecycle_class AS source_lifecycle_class,
       current_head.invalidation_epoch,
       current_head.snapshot_digest,
       current_head.source_basis,
       current_head.unit_set_id,
       current_head.unit_key,
       current_head.semantic_text_digest,
       current_head.summary_coverage,
       current_head.semantic_text,
       CASE
         WHEN current_head.unit_set_id IS NOT NULL THEN 'current'
         WHEN job.state = 'poison'
           OR (job.state = 'succeeded' AND current_head.unit_set_id IS NULL)
           OR source.coverage_state = 'failed'
           OR source.ineligibility_reason = 'invalid_canonical_state'
           THEN 'failed'
         WHEN job.state IN ('pending', 'claimed', 'retry') THEN 'building'
         WHEN source.coverage_state = 'unsupported'
           OR source.ineligibility_reason = 'source_capability_unavailable'
           THEN 'unsupported'
         ELSE 'missing'
       END AS availability
FROM active_generation generation
CROSS JOIN requested
LEFT JOIN semantic_sources source
  ON source.community_id = generation.community_id
 AND source.source_family = requested.source_family
 AND source.source_subtype = requested.source_subtype
 AND source.source_id = requested.source_id
LEFT JOIN semantic_index_jobs job
  ON job.community_id = source.community_id
 AND job.generation_id = generation.generation_id
 AND job.source_family = source.source_family
 AND job.source_subtype = source.source_subtype
 AND job.source_id = source.source_id
 AND job.desired_invalidation_epoch = source.invalidation_epoch
LEFT JOIN LATERAL (
    SELECT head.source_invalidation_epoch AS invalidation_epoch,
           head.source_snapshot_digest AS snapshot_digest,
           unit_set.source_basis,
           unit_set.unit_set_id,
           unit.unit_key,
           unit.semantic_text_digest,
           unit.summary_coverage,
           unit.semantic_text
    FROM semantic_source_generation_heads head
    JOIN semantic_unit_sets unit_set
      ON unit_set.community_id = head.community_id
     AND unit_set.unit_set_id = head.unit_set_id
     AND unit_set.source_family = head.source_family
     AND unit_set.source_subtype = head.source_subtype
     AND unit_set.source_id = head.source_id
     AND unit_set.source_invalidation_epoch = head.source_invalidation_epoch
     AND unit_set.source_snapshot_digest = head.source_snapshot_digest
     AND unit_set.state = 'active'
     AND unit_set.extractor_version = generation.extractor_version
     AND unit_set.source_basis = source.source_basis
    JOIN semantic_units unit
      ON unit.community_id = unit_set.community_id
     AND unit.unit_set_id = unit_set.unit_set_id
     AND unit.unit_kind = 'overview'
     AND unit.unit_key = 'overview'
    JOIN semantic_embeddings embedding
      ON embedding.community_id = unit.community_id
     AND embedding.unit_set_id = unit.unit_set_id
     AND embedding.unit_key = unit.unit_key
     AND embedding.generation_id = generation.generation_id
     AND embedding.model_contract_digest = generation.model_contract_digest
     AND embedding.dimensions = generation.dimensions
     AND embedding.response_model = generation.model
     AND vector_dims(embedding.embedding) = generation.dimensions
     AND vector_norm(embedding.embedding) > 0
    WHERE head.community_id = source.community_id
      AND head.generation_id = generation.generation_id
      AND head.source_family = source.source_family
      AND head.source_subtype = source.source_subtype
      AND head.source_id = source.source_id
      AND head.source_invalidation_epoch = source.invalidation_epoch
      AND head.source_snapshot_digest = source.snapshot_digest
      AND source.eligibility = 'eligible'
) current_head ON TRUE
ORDER BY requested.request_ordinal
"#;

const SEMANTIC_GRAPH_COVERAGE_SQL: &str = r#"
WITH requested_reader(pubkey) AS (VALUES ($2::bytea)),
authorized_reader AS MATERIALIZED (
    SELECT requested_reader.pubkey
    FROM requested_reader
    LEFT JOIN users actor
      ON actor.community_id = $1 AND actor.pubkey = requested_reader.pubkey
    WHERE (
      (actor.agent_owner_pubkey IS NULL AND EXISTS (
        SELECT 1 FROM relay_members member
        WHERE member.community_id = $1
          AND member.pubkey = encode(requested_reader.pubkey, 'hex')
      ))
      OR (
        actor.agent_owner_pubkey IS NOT NULL
        AND EXISTS (
          SELECT 1 FROM relay_members owner_member
          WHERE owner_member.community_id = $1
            AND owner_member.pubkey = encode(actor.agent_owner_pubkey, 'hex')
        )
        AND NOT EXISTS (
          SELECT 1 FROM community_bans owner_ban
          WHERE owner_ban.community_id = $1
            AND owner_ban.pubkey = actor.agent_owner_pubkey
            AND owner_ban.banned
            AND (owner_ban.ban_expires_at IS NULL
                 OR owner_ban.ban_expires_at > clock_timestamp())
        )
        AND NOT EXISTS (
          SELECT 1 FROM users owner_actor
          WHERE owner_actor.community_id = $1
            AND owner_actor.pubkey = actor.agent_owner_pubkey
            AND owner_actor.agent_owner_pubkey IS NOT NULL
        )
      )
    )
    AND NOT EXISTS (
      SELECT 1 FROM community_bans actor_ban
      WHERE actor_ban.community_id = $1
        AND actor_ban.pubkey = requested_reader.pubkey
        AND actor_ban.banned
        AND (actor_ban.ban_expires_at IS NULL
             OR actor_ban.ban_expires_at > clock_timestamp())
    )
),
authorized_project AS MATERIALIZED (
    SELECT community.id AS community_id,
           community.semantic_active_generation_id AS generation_id
    FROM communities community CROSS JOIN authorized_reader
    JOIN project_view_maintenance maintenance
      ON maintenance.community_id = community.id
    JOIN project_view_state view_state
      ON view_state.community_id = community.id
    JOIN project_document_state document_state
      ON document_state.community_id = community.id
    JOIN project_context_edge_state context_state
      ON context_state.community_id = community.id
    WHERE community.id = $1
      AND community.archived_at IS NULL
      AND community.project_view_schema_version = 3
      AND community.project_view_enabled
      AND community.project_document_enabled
      AND community.meeting_community_read_enabled
      AND community.project_context_edge_enabled
      AND community.semantic_index_enabled
      AND community.semantic_graph_query_enabled
      AND maintenance.state = 'normal'
      AND view_state.schema_version = 3
      AND document_state.schema_version = 1
      AND context_state.schema_version = 2
      AND view_state.projection_pubkey = $3
      AND document_state.projection_pubkey = $3
      AND context_state.projection_pubkey = $3
      AND community.semantic_active_generation_id = $4
),
active_generation AS MATERIALIZED (
    SELECT generation.*
    FROM authorized_project project
    JOIN semantic_index_generations generation
      ON generation.community_id = project.community_id
     AND generation.generation_id = project.generation_id
    WHERE generation.lifecycle = 'active'
      AND generation.model_contract_digest = $5
      AND generation.extractor_version = $6
      AND generation.model = $7
      AND generation.dimensions = $8
      AND generation.distance_metric = 'cosine'
),
raw_graph_roles AS MATERIALIZED (
    SELECT coordinate.community_id,
      CASE coordinate.coordinate_type
        WHEN 'project_view_object' THEN 'project_view'
        WHEN 'document' THEN 'project_document'
        WHEN 'meeting' THEN 'meeting'
      END AS source_family,
      CASE coordinate.coordinate_type
        WHEN 'project_view_object' THEN coordinate.coordinate_subtype
        WHEN 'document' THEN 'document'
        WHEN 'meeting' THEN 'meeting'
      END AS source_subtype,
      coordinate.coordinate_id AS source_id,
      TRUE AS is_coordinate, FALSE AS is_context_document
    FROM project_context_edge_coordinates coordinate
    JOIN project_context_edges edge
      ON edge.community_id = coordinate.community_id
     AND edge.edge_key = coordinate.edge_key AND edge.state = 'active'
    WHERE coordinate.community_id = $1
    UNION ALL
    SELECT binding.community_id, 'project_document', 'document',
      binding.context_document_id, FALSE, TRUE
    FROM project_context_document_bindings binding
    JOIN project_context_edges edge
      ON edge.community_id = binding.community_id
     AND edge.edge_key = binding.edge_key AND edge.state = 'active'
    WHERE binding.community_id = $1 AND binding.state = 'active'
),
graph_roles AS MATERIALIZED (
    SELECT community_id, source_family, source_subtype, source_id,
      bool_or(is_coordinate) AS is_coordinate,
      bool_or(is_context_document) AS is_context_document
    FROM raw_graph_roles
    WHERE source_family IS NOT NULL AND source_subtype IS NOT NULL
    GROUP BY community_id, source_family, source_subtype, source_id
),
explicit_initial_sources(source_family, source_subtype, source_id)
AS MATERIALIZED (SELECT * FROM unnest($10::text[], $11::text[], $12::uuid[])),
canonical_sources AS MATERIALIZED (
    SELECT object.community_id, 'project_view'::text AS source_family,
      object.object_type AS source_subtype, object.object_id AS source_id,
      CASE
        WHEN object.object_type = 'role'
             AND object.body->'active' IS DISTINCT FROM 'true'::jsonb
          THEN 'terminal'
        WHEN object.body->>'status' IN (
          'completed', 'cancelled', 'satisfied', 'withdrawn',
          'resolved', 'closed', 'inactive'
        ) THEN 'terminal'
        ELSE 'active'
      END AS lifecycle_class
    FROM project_view_objects object
    WHERE object.community_id = $1
      AND object.schema_version = 3 AND object.deleted_at IS NULL
    UNION ALL
    SELECT document.community_id, 'project_document', 'document',
      document.document_id, 'active'
    FROM project_documents document
    WHERE document.community_id = $1 AND document.state = 'active'
    UNION ALL
    SELECT session.community_id, 'meeting', 'meeting', session.session_id,
      CASE
        WHEN session.status = 'ended' THEN 'terminal'
        WHEN runtime.runtime_phase = 'finalizing_actions' THEN 'finalizing'
        ELSE 'active'
      END
    FROM meeting_sessions session
    JOIN channels channel
      ON channel.community_id = session.community_id
     AND channel.id = session.session_id
     AND channel.room_kind = 'meeting' AND channel.deleted_at IS NULL
    LEFT JOIN meeting_v2_bootstrap_state runtime
      ON runtime.community_id = session.community_id
     AND runtime.session_id = session.session_id
    WHERE session.community_id = $1
),
authorized_sources AS MATERIALIZED (
    SELECT canonical.community_id, canonical.source_family,
      canonical.source_subtype, canonical.source_id,
      source.invalidation_epoch, source.snapshot_digest,
      source.eligibility, source.coverage_state, source.ineligibility_reason
    FROM active_generation generation
    JOIN canonical_sources canonical
      ON canonical.community_id = generation.community_id
    JOIN graph_roles role
      ON role.community_id = canonical.community_id
     AND role.source_family = canonical.source_family
     AND role.source_subtype = canonical.source_subtype
     AND role.source_id = canonical.source_id
    LEFT JOIN semantic_sources source
      ON source.community_id = canonical.community_id
     AND source.source_family = canonical.source_family
     AND source.source_subtype = canonical.source_subtype
     AND source.source_id = canonical.source_id
    LEFT JOIN explicit_initial_sources initial
      ON initial.source_family = canonical.source_family
     AND initial.source_subtype = canonical.source_subtype
     AND initial.source_id = canonical.source_id
    WHERE role.is_context_document
       OR (role.is_coordinate AND (
          $9 = 'all_current'
          OR ($9 = 'non_terminal'
              AND canonical.lifecycle_class IN ('active', 'finalizing'))
          OR ($9 = 'terminal_only' AND canonical.lifecycle_class = 'terminal')
          OR initial.source_id IS NOT NULL
       ))
),
observed AS MATERIALIZED (
    SELECT source.source_family, source.source_subtype, source.source_id,
      source.eligibility, source.coverage_state, source.ineligibility_reason,
      unit.summary_coverage,
      embedding.embedding, job.state AS job_state
    FROM authorized_sources source CROSS JOIN active_generation generation
    LEFT JOIN semantic_source_generation_heads head
      ON head.community_id = source.community_id
     AND head.generation_id = generation.generation_id
     AND head.source_family = source.source_family
     AND head.source_subtype = source.source_subtype
     AND head.source_id = source.source_id
     AND head.source_invalidation_epoch = source.invalidation_epoch
     AND head.source_snapshot_digest = source.snapshot_digest
    LEFT JOIN semantic_unit_sets unit_set
      ON unit_set.community_id = head.community_id
     AND unit_set.unit_set_id = head.unit_set_id
     AND unit_set.source_family = source.source_family
     AND unit_set.source_subtype = source.source_subtype
     AND unit_set.source_id = source.source_id
     AND unit_set.source_invalidation_epoch = source.invalidation_epoch
     AND unit_set.source_snapshot_digest = source.snapshot_digest
     AND unit_set.state = 'active'
     AND unit_set.extractor_version = generation.extractor_version
    LEFT JOIN semantic_units unit
      ON unit.community_id = unit_set.community_id
     AND unit.unit_set_id = unit_set.unit_set_id
     AND unit.unit_kind = 'overview' AND unit.unit_key = 'overview'
    LEFT JOIN semantic_embeddings embedding
      ON embedding.community_id = unit.community_id
     AND embedding.unit_set_id = unit.unit_set_id
     AND embedding.unit_key = unit.unit_key
     AND embedding.generation_id = generation.generation_id
     AND embedding.model_contract_digest = generation.model_contract_digest
     AND embedding.dimensions = generation.dimensions
     AND embedding.response_model = generation.model
     AND vector_dims(embedding.embedding) = generation.dimensions
    LEFT JOIN semantic_index_jobs job
      ON job.community_id = source.community_id
     AND job.generation_id = generation.generation_id
     AND job.source_family = source.source_family
     AND job.source_subtype = source.source_subtype
     AND job.source_id = source.source_id
     AND job.desired_invalidation_epoch = source.invalidation_epoch
),
classified AS MATERIALIZED (
    SELECT *, CASE
      WHEN eligibility = 'eligible'
        AND embedding IS NOT NULL AND vector_norm(embedding) > 0 THEN 'current'
      WHEN eligibility = 'eligible'
        AND embedding IS NOT NULL AND vector_norm(embedding) = 0
        THEN 'non_queryable_zero_vector'
      WHEN job_state = 'poison'
        OR (job_state = 'succeeded' AND embedding IS NULL)
        OR ineligibility_reason = 'invalid_canonical_state' THEN 'failed'
      WHEN job_state IN ('pending', 'claimed', 'retry') THEN 'building'
      WHEN coverage_state = 'unsupported'
        OR ineligibility_reason = 'source_capability_unavailable' THEN 'unsupported'
      ELSE 'missing'
    END AS coverage_class
    FROM observed
)
SELECT coverage_class, count(*)::bigint AS source_count,
  count(*) FILTER (
    WHERE coverage_class = 'current' AND summary_coverage = 'title_only'
  )::bigint AS title_only_count
FROM classified
GROUP BY coverage_class
ORDER BY CASE coverage_class
  WHEN 'current' THEN 0
  WHEN 'non_queryable_zero_vector' THEN 1
  WHEN 'failed' THEN 2
  WHEN 'building' THEN 3
  WHEN 'unsupported' THEN 4
  ELSE 5
END
"#;

const CURRENT_CONTEXT_OVERVIEWS_SQL: &str = r#"
WITH requested_reader(pubkey) AS (
    VALUES ($2::bytea)
),
authorized_reader AS MATERIALIZED (
    SELECT requested_reader.pubkey
    FROM requested_reader
    LEFT JOIN users actor
      ON actor.community_id = $1 AND actor.pubkey = requested_reader.pubkey
    WHERE (
        (actor.agent_owner_pubkey IS NULL AND EXISTS (
            SELECT 1 FROM relay_members member
            WHERE member.community_id = $1
              AND member.pubkey = encode(requested_reader.pubkey, 'hex')
        ))
        OR (
            actor.agent_owner_pubkey IS NOT NULL
            AND EXISTS (
                SELECT 1 FROM relay_members owner_member
                WHERE owner_member.community_id = $1
                  AND owner_member.pubkey = encode(actor.agent_owner_pubkey, 'hex')
            )
            AND NOT EXISTS (
                SELECT 1 FROM community_bans owner_ban
                WHERE owner_ban.community_id = $1
                  AND owner_ban.pubkey = actor.agent_owner_pubkey
                  AND owner_ban.banned
                  AND (owner_ban.ban_expires_at IS NULL
                       OR owner_ban.ban_expires_at > clock_timestamp())
            )
            AND NOT EXISTS (
                SELECT 1 FROM users owner_actor
                WHERE owner_actor.community_id = $1
                  AND owner_actor.pubkey = actor.agent_owner_pubkey
                  AND owner_actor.agent_owner_pubkey IS NOT NULL
            )
        )
    )
    AND NOT EXISTS (
        SELECT 1 FROM community_bans actor_ban
        WHERE actor_ban.community_id = $1
          AND actor_ban.pubkey = requested_reader.pubkey
          AND actor_ban.banned
          AND (actor_ban.ban_expires_at IS NULL
               OR actor_ban.ban_expires_at > clock_timestamp())
    )
),
authorized_project AS MATERIALIZED (
    SELECT community.id AS community_id,
           community.semantic_active_generation_id AS generation_id
    FROM communities community
    CROSS JOIN authorized_reader
    JOIN project_view_maintenance maintenance
      ON maintenance.community_id = community.id
    JOIN project_view_state view_state
      ON view_state.community_id = community.id
    JOIN project_document_state document_state
      ON document_state.community_id = community.id
    JOIN project_context_edge_state context_state
      ON context_state.community_id = community.id
    WHERE community.id = $1
      AND community.archived_at IS NULL
      AND community.project_view_schema_version = 3
      AND community.project_view_enabled
      AND community.project_document_enabled
      AND community.meeting_community_read_enabled
      AND community.project_context_edge_enabled
      AND community.semantic_index_enabled
      AND community.semantic_graph_query_enabled
      AND maintenance.state = 'normal'
      AND view_state.schema_version = 3
      AND document_state.schema_version = 1
      AND context_state.schema_version = 2
      AND view_state.projection_pubkey = $3
      AND document_state.projection_pubkey = $3
      AND context_state.projection_pubkey = $3
      AND community.semantic_active_generation_id = $4
),
active_generation AS MATERIALIZED (
    SELECT generation.*
    FROM authorized_project project
    JOIN semantic_index_generations generation
      ON generation.community_id = project.community_id
     AND generation.generation_id = project.generation_id
    WHERE generation.lifecycle = 'active'
      AND generation.model_contract_digest = $5
      AND generation.extractor_version = $6
      AND generation.model = $7
      AND generation.dimensions = $8
      AND generation.distance_metric = 'cosine'
),
requested_sources(source_family, source_subtype, source_id) AS MATERIALIZED (
    SELECT * FROM unnest($9::text[], $10::text[], $11::uuid[])
)
SELECT source.community_id, source.source_family, source.source_subtype, source.source_id,
       source.invalidation_epoch, source.snapshot_digest, source.source_basis,
       unit_set.unit_set_id, unit.unit_key, unit.semantic_text_digest,
       unit.summary_coverage, unit.semantic_text
FROM active_generation generation
JOIN semantic_source_generation_heads head
  ON head.community_id = generation.community_id
 AND head.generation_id = generation.generation_id
JOIN requested_sources requested
  ON requested.source_family = head.source_family
 AND requested.source_subtype = head.source_subtype
 AND requested.source_id = head.source_id
JOIN semantic_sources source
  ON source.community_id = head.community_id
 AND source.source_family = head.source_family
 AND source.source_subtype = head.source_subtype
 AND source.source_id = head.source_id
 AND source.invalidation_epoch = head.source_invalidation_epoch
 AND source.snapshot_digest = head.source_snapshot_digest
JOIN semantic_unit_sets unit_set
  ON unit_set.community_id = head.community_id
 AND unit_set.unit_set_id = head.unit_set_id
 AND unit_set.source_family = head.source_family
 AND unit_set.source_subtype = head.source_subtype
 AND unit_set.source_id = head.source_id
 AND unit_set.source_invalidation_epoch = head.source_invalidation_epoch
 AND unit_set.source_snapshot_digest = head.source_snapshot_digest
 AND unit_set.state = 'active'
 AND unit_set.extractor_version = generation.extractor_version
JOIN semantic_units unit
  ON unit.community_id = unit_set.community_id
 AND unit.unit_set_id = unit_set.unit_set_id
 AND unit.unit_kind = 'overview'
 AND unit.unit_key = 'overview'
JOIN semantic_embeddings embedding
  ON embedding.community_id = unit.community_id
 AND embedding.unit_set_id = unit.unit_set_id
 AND embedding.unit_key = unit.unit_key
 AND embedding.generation_id = generation.generation_id
 AND embedding.model_contract_digest = generation.model_contract_digest
 AND embedding.dimensions = generation.dimensions
 AND embedding.response_model = generation.model
 AND vector_dims(embedding.embedding) = generation.dimensions
 AND vector_norm(embedding.embedding) > 0
WHERE source.eligibility = 'eligible'
ORDER BY source.source_family, source.source_subtype, source.source_id
"#;

const EXACT_SOURCE_SCORES_SQL: &str = r#"
WITH requested_reader(pubkey) AS (
    VALUES ($2::bytea)
),
authorized_reader AS MATERIALIZED (
    SELECT requested_reader.pubkey
    FROM requested_reader
    LEFT JOIN users actor
      ON actor.community_id = $1 AND actor.pubkey = requested_reader.pubkey
    WHERE (
        (actor.agent_owner_pubkey IS NULL AND EXISTS (
            SELECT 1 FROM relay_members member
            WHERE member.community_id = $1
              AND member.pubkey = encode(requested_reader.pubkey, 'hex')
        ))
        OR (
            actor.agent_owner_pubkey IS NOT NULL
            AND EXISTS (
                SELECT 1 FROM relay_members owner_member
                WHERE owner_member.community_id = $1
                  AND owner_member.pubkey = encode(actor.agent_owner_pubkey, 'hex')
            )
            AND NOT EXISTS (
                SELECT 1 FROM community_bans owner_ban
                WHERE owner_ban.community_id = $1
                  AND owner_ban.pubkey = actor.agent_owner_pubkey
                  AND owner_ban.banned
                  AND (owner_ban.ban_expires_at IS NULL
                       OR owner_ban.ban_expires_at > clock_timestamp())
            )
            AND NOT EXISTS (
                SELECT 1 FROM users owner_actor
                WHERE owner_actor.community_id = $1
                  AND owner_actor.pubkey = actor.agent_owner_pubkey
                  AND owner_actor.agent_owner_pubkey IS NOT NULL
            )
        )
    )
    AND NOT EXISTS (
        SELECT 1 FROM community_bans actor_ban
        WHERE actor_ban.community_id = $1
          AND actor_ban.pubkey = requested_reader.pubkey
          AND actor_ban.banned
          AND (actor_ban.ban_expires_at IS NULL
               OR actor_ban.ban_expires_at > clock_timestamp())
    )
),
authorized_project AS MATERIALIZED (
    SELECT community.id AS community_id,
           community.semantic_active_generation_id AS generation_id
    FROM communities community
    CROSS JOIN authorized_reader
    JOIN project_view_maintenance maintenance
      ON maintenance.community_id = community.id
    JOIN project_view_state view_state
      ON view_state.community_id = community.id
    JOIN project_document_state document_state
      ON document_state.community_id = community.id
    JOIN project_context_edge_state context_state
      ON context_state.community_id = community.id
    WHERE community.id = $1
      AND community.archived_at IS NULL
      AND community.project_view_schema_version = 3
      AND community.project_view_enabled
      AND community.project_document_enabled
      AND community.meeting_community_read_enabled
      AND community.project_context_edge_enabled
      AND community.semantic_index_enabled
      AND community.semantic_graph_query_enabled
      AND maintenance.state = 'normal'
      AND view_state.schema_version = 3
      AND document_state.schema_version = 1
      AND context_state.schema_version = 2
      AND view_state.projection_pubkey = $3
      AND document_state.projection_pubkey = $3
      AND context_state.projection_pubkey = $3
      AND community.semantic_active_generation_id = $4
),
active_generation AS MATERIALIZED (
    SELECT generation.*
    FROM authorized_project project
    JOIN semantic_index_generations generation
      ON generation.community_id = project.community_id
     AND generation.generation_id = project.generation_id
    WHERE generation.lifecycle = 'active'
      AND generation.model_contract_digest = $5
      AND generation.extractor_version = $6
      AND generation.model = $7
      AND generation.dimensions = $8
      AND generation.distance_metric = 'cosine'
),
raw_graph_roles AS MATERIALIZED (
    SELECT coordinate.community_id,
           CASE coordinate.coordinate_type
             WHEN 'project_view_object' THEN 'project_view'
             WHEN 'document' THEN 'project_document'
             WHEN 'meeting' THEN 'meeting'
           END AS source_family,
           CASE coordinate.coordinate_type
             WHEN 'project_view_object' THEN coordinate.coordinate_subtype
             WHEN 'document' THEN 'document'
             WHEN 'meeting' THEN 'meeting'
           END AS source_subtype,
           coordinate.coordinate_id AS source_id,
           TRUE AS is_coordinate,
           encode(coordinate.edge_key, 'hex') AS coordinate_edge_key,
           NULL::jsonb AS binding
    FROM project_context_edge_coordinates coordinate
    JOIN project_context_edges edge
      ON edge.community_id = coordinate.community_id
     AND edge.edge_key = coordinate.edge_key
     AND edge.state = 'active'
    WHERE coordinate.community_id = $1
    UNION ALL
    SELECT binding.community_id, 'project_document', 'document',
           binding.context_document_id, FALSE, NULL::text,
           jsonb_build_object(
             'edge_key', encode(binding.edge_key, 'hex'),
             'edge_last_context_revision', edge.last_context_revision,
             'edge_source_change_id', encode(edge.current_source_change_id, 'hex'),
             'binding_context_revision', binding.binding_context_revision,
             'binding_source_change_id', encode(binding.current_source_change_id, 'hex'),
             'binding_projection_event_id', encode(binding.current_projection_event_id, 'hex')
           )
    FROM project_context_document_bindings binding
    JOIN project_context_edges edge
      ON edge.community_id = binding.community_id
     AND edge.edge_key = binding.edge_key
     AND edge.state = 'active'
    WHERE binding.community_id = $1 AND binding.state = 'active'
),
graph_roles AS MATERIALIZED (
    SELECT community_id, source_family, source_subtype, source_id,
           bool_or(is_coordinate) AS is_coordinate,
           COALESCE(
             jsonb_agg(to_jsonb(coordinate_edge_key) ORDER BY coordinate_edge_key)
               FILTER (WHERE coordinate_edge_key IS NOT NULL),
             '[]'::jsonb
           ) AS coordinate_incident_edge_keys,
           COALESCE(
             jsonb_agg(binding ORDER BY binding->>'edge_key')
               FILTER (WHERE binding IS NOT NULL),
             '[]'::jsonb
           ) AS bindings
    FROM raw_graph_roles
    WHERE source_family IS NOT NULL AND source_subtype IS NOT NULL
    GROUP BY community_id, source_family, source_subtype, source_id
),
explicit_initial_sources(source_family, source_subtype, source_id)
AS MATERIALIZED (
    SELECT * FROM unnest($10::text[], $11::text[], $12::uuid[])
),
requested_candidates(source_family, source_subtype, source_id)
AS MATERIALIZED (
    SELECT * FROM unnest($16::text[], $17::text[], $18::uuid[])
),
eligible AS MATERIALIZED (
    SELECT source.community_id, source.source_family, source.source_subtype, source.source_id,
           source.invalidation_epoch, source.snapshot_digest,
           source.source_basis, source.lifecycle_class, source.source_status,
           unit_set.unit_set_id, unit.unit_key, unit.semantic_text_digest,
           unit.summary_coverage, embedding.embedding,
           role.is_coordinate,
           role.coordinate_incident_edge_keys,
           (role.is_coordinate AND (
               $9 = 'all_current'
               OR ($9 = 'non_terminal'
                   AND source.lifecycle_class IN ('active', 'finalizing'))
               OR ($9 = 'terminal_only'
                   AND source.lifecycle_class = 'terminal')
               OR initial.source_id IS NOT NULL
           )) AS coordinate_entry_eligible,
           role.bindings
    FROM active_generation generation
    JOIN semantic_source_generation_heads head
      ON head.community_id = generation.community_id
     AND head.generation_id = generation.generation_id
    JOIN semantic_sources source
      ON source.community_id = head.community_id
     AND source.source_family = head.source_family
     AND source.source_subtype = head.source_subtype
     AND source.source_id = head.source_id
     AND source.invalidation_epoch = head.source_invalidation_epoch
     AND source.snapshot_digest = head.source_snapshot_digest
    JOIN semantic_unit_sets unit_set
      ON unit_set.community_id = head.community_id
     AND unit_set.unit_set_id = head.unit_set_id
     AND unit_set.source_family = head.source_family
     AND unit_set.source_subtype = head.source_subtype
     AND unit_set.source_id = head.source_id
     AND unit_set.source_invalidation_epoch = head.source_invalidation_epoch
     AND unit_set.source_snapshot_digest = head.source_snapshot_digest
     AND unit_set.state = 'active'
     AND unit_set.extractor_version = generation.extractor_version
    JOIN semantic_units unit
      ON unit.community_id = unit_set.community_id
     AND unit.unit_set_id = unit_set.unit_set_id
     AND unit.unit_kind = 'overview'
     AND unit.unit_key = 'overview'
    JOIN semantic_embeddings embedding
      ON embedding.community_id = unit.community_id
     AND embedding.unit_set_id = unit.unit_set_id
     AND embedding.unit_key = unit.unit_key
     AND embedding.generation_id = generation.generation_id
     AND embedding.model_contract_digest = generation.model_contract_digest
     AND embedding.dimensions = generation.dimensions
     AND embedding.response_model = generation.model
     AND vector_dims(embedding.embedding) = generation.dimensions
     AND vector_norm(embedding.embedding) > 0
    JOIN graph_roles role
      ON role.community_id = source.community_id
     AND role.source_family = source.source_family
     AND role.source_subtype = source.source_subtype
     AND role.source_id = source.source_id
    LEFT JOIN explicit_initial_sources initial
      ON initial.source_family = source.source_family
     AND initial.source_subtype = source.source_subtype
     AND initial.source_id = source.source_id
    WHERE source.eligibility = 'eligible'
      AND (
        (role.is_coordinate AND (
          $9 = 'all_current'
          OR ($9 = 'non_terminal'
              AND source.lifecycle_class IN ('active', 'finalizing'))
          OR ($9 = 'terminal_only' AND source.lifecycle_class = 'terminal')
          OR initial.source_id IS NOT NULL
        ))
        OR jsonb_array_length(role.bindings) > 0
      )
      AND (
        NOT $15
        OR EXISTS (
          SELECT 1 FROM requested_candidates candidate
          WHERE candidate.source_family = source.source_family
            AND candidate.source_subtype = source.source_subtype
            AND candidate.source_id = source.source_id
        )
      )
),
query_vectors(channel_id, query_vector) AS MATERIALIZED (
    SELECT * FROM unnest($13::bytea[], $14::vector[])
),
distances AS (
    SELECT eligible.*, query_vectors.channel_id,
           eligible.embedding <=> query_vectors.query_vector AS distance
    FROM eligible CROSS JOIN query_vectors
),
finite_distances AS MATERIALIZED (
    SELECT * FROM distances
    WHERE distance > '-Infinity'::double precision
      AND distance < 'Infinity'::double precision
),
ranked AS (
    SELECT finite_distances.*,
           floor((
             (greatest(-1.0, least(1.0, 1.0 - distance)) + 1.0)
             / 2.0
           ) * 1000000.0 + 0.5)::bigint AS semantic_score,
           row_number() OVER (
             PARTITION BY channel_id
             ORDER BY distance ASC, source_family ASC,
                      source_subtype ASC, source_id ASC, unit_key ASC
           ) AS channel_rank
    FROM finite_distances
)
SELECT community_id, channel_id, source_family, source_subtype, source_id,
       invalidation_epoch, snapshot_digest, source_basis,
       lifecycle_class, source_status, unit_set_id, unit_key,
       semantic_text_digest, summary_coverage, is_coordinate,
       coordinate_entry_eligible, coordinate_incident_edge_keys,
       bindings, semantic_score, channel_rank
FROM ranked
WHERE $19::bigint IS NULL OR channel_rank <= $19
ORDER BY channel_id, channel_rank, source_family, source_subtype, source_id, unit_key
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SemanticGraphQueryEgressRecheck {
    Ready { context_state_set_digest: Digest32 },
    ContextChanged,
    Unavailable,
}

fn validate_egress_expectations(
    expected_ticket: &SemanticGraphQueryTicket,
    reader_pubkey: &[u8],
    expected_contexts: &[SemanticContextEgressExpectation],
) -> Result<Vec<SemanticContextEgressExpectation>> {
    validate_pubkey(reader_pubkey)?;
    if expected_contexts.len() > MAX_CONTEXT_COORDINATES {
        return Err(DbError::InvalidData(
            "semantic egress context count exceeds the server bound".to_string(),
        ));
    }
    let mut expected_contexts = expected_contexts.to_vec();
    expected_contexts.sort_by_key(|context| semantic_source_sort_key(context.source()));
    for context in &expected_contexts {
        validate_source_inputs(
            expected_ticket.community_id,
            std::slice::from_ref(context.source()),
            1,
            "egress context source",
        )?;
        if let SemanticContextEgressExpectation::Accepted(context) = context {
            validate_head_for_source(&context.semantic_head, &context.source)?;
        }
    }
    if expected_contexts
        .windows(2)
        .any(|pair| pair[0].source() == pair[1].source())
    {
        return Err(DbError::InvalidData(
            "semantic egress context sources must be unique".to_string(),
        ));
    }
    Ok(expected_contexts)
}

async fn recheck_semantic_graph_query_egress_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    expected_ticket: &SemanticGraphQueryTicket,
    reader_pubkey: &[u8],
    expected_projection_pubkey: &PublicKey,
    expected_contexts: &[SemanticContextEgressExpectation],
) -> Result<SemanticGraphQueryEgressRecheck> {
    if !lock_semantic_graph_query_community_row(tx, expected_ticket.community_id).await? {
        return Ok(SemanticGraphQueryEgressRecheck::Unavailable);
    }
    let locked_generation: Option<Uuid> = sqlx::query_scalar(LOCK_EGRESS_GENERATION_SQL)
        .bind(expected_ticket.community_id.as_uuid())
        .bind(expected_ticket.generation.generation_id)
        .fetch_optional(&mut **tx)
        .await?;
    let locked_context_state: Option<Uuid> = sqlx::query_scalar(LOCK_EGRESS_CONTEXT_STATE_SQL)
        .bind(expected_ticket.community_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await?;
    if locked_generation.is_none() || locked_context_state.is_none() {
        return Ok(SemanticGraphQueryEgressRecheck::Unavailable);
    }
    let observed_ticket = match load_authorized_ticket_in_tx(
        tx,
        expected_ticket.community_id,
        reader_pubkey,
        expected_projection_pubkey,
    )
    .await
    {
        Ok(ticket) => ticket,
        Err(DbError::AccessDenied(_)) => {
            return Ok(SemanticGraphQueryEgressRecheck::Unavailable);
        }
        Err(error) => return Err(error),
    };
    if !same_generation_contract(&observed_ticket, expected_ticket)
        || observed_ticket.project_context_revision != expected_ticket.project_context_revision
    {
        return Ok(SemanticGraphQueryEgressRecheck::Unavailable);
    }

    if !expected_contexts.is_empty() {
        let sources: Vec<SemanticSourceIdentity> = expected_contexts
            .iter()
            .map(|context| context.source().clone())
            .collect();
        let lock_sources: Vec<SemanticSourceIdentity> = expected_contexts
            .iter()
            .filter(|context| match context {
                SemanticContextEgressExpectation::Accepted(_) => true,
                SemanticContextEgressExpectation::Omitted { evidence, .. } => {
                    evidence.source_invalidation_epoch.is_some()
                }
            })
            .map(|context| context.source().clone())
            .collect();
        if !lock_semantic_sources_for_egress(tx, expected_ticket.community_id, &lock_sources)
            .await?
        {
            return Ok(SemanticGraphQueryEgressRecheck::ContextChanged);
        }
        let observed =
            observe_context_egress_expectations_in_tx(tx, &observed_ticket, &sources).await?;
        if observed != expected_contexts {
            return Ok(SemanticGraphQueryEgressRecheck::ContextChanged);
        }
    }

    Ok(SemanticGraphQueryEgressRecheck::Ready {
        context_state_set_digest: context_state_set_digest(expected_contexts)?,
    })
}

async fn lock_semantic_graph_query_community_row(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> Result<bool> {
    Ok(
        sqlx::query_scalar::<_, Uuid>(LOCK_FINAL_CONFIRMATION_COMMUNITY_SQL)
            .bind(community_id.as_uuid())
            .fetch_optional(&mut **tx)
            .await?
            .is_some(),
    )
}

async fn lock_semantic_sources_for_egress(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    sources: &[SemanticSourceIdentity],
) -> Result<bool> {
    let requested = source_key_arrays(sources);
    let rows = sqlx::query(
        "WITH requested(source_family,source_subtype,source_id) AS MATERIALIZED (\
             SELECT * FROM unnest($2::text[],$3::text[],$4::uuid[])\
         ) \
         SELECT source.community_id,source.source_family,source.source_subtype,source.source_id \
         FROM semantic_sources source JOIN requested \
           ON requested.source_family=source.source_family \
          AND requested.source_subtype=source.source_subtype \
          AND requested.source_id=source.source_id \
         WHERE source.community_id=$1 \
         ORDER BY source.source_family,source.source_subtype,source.source_id \
         FOR SHARE OF source",
    )
    .bind(community_id.as_uuid())
    .bind(&requested.families)
    .bind(&requested.subtypes)
    .bind(&requested.ids)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != sources.len() {
        return Ok(false);
    }
    for (row, expected) in rows.iter().zip(sources) {
        let observed =
            source_identity_from_row(row, "source_family", "source_subtype", "source_id")?;
        if &observed != expected {
            return Err(DbError::InvalidData(
                "semantic egress context lock order is inconsistent".to_string(),
            ));
        }
    }
    Ok(true)
}

async fn observe_context_egress_expectations_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    ticket: &SemanticGraphQueryTicket,
    sources: &[SemanticSourceIdentity],
) -> Result<Vec<SemanticContextEgressExpectation>> {
    let states =
        load_current_semantic_source_states_in_tx(tx, ticket, sources, MAX_CONTEXT_COORDINATES)
            .await?;
    let mut observations = Vec::with_capacity(sources.len());
    for (source, state) in sources.iter().zip(states) {
        if source != &state.source {
            return Err(DbError::InvalidData(
                "semantic egress context source-state order is inconsistent".to_string(),
            ));
        }
        let canonical = match observe_semantic_source_in_connection(tx, source).await {
            Ok(observation) => Some(observation),
            Err(DbError::NotFound(_)) => None,
            Err(error) => return Err(error),
        };
        observations.push(context_egress_expectation(source, &state, canonical)?);
    }
    Ok(observations)
}

fn context_egress_expectation(
    source: &SemanticSourceIdentity,
    state: &CurrentSemanticSourceState,
    canonical: Option<CanonicalSemanticSourceObservation>,
) -> Result<SemanticContextEgressExpectation> {
    let Some(canonical) = canonical else {
        return Ok(SemanticContextEgressExpectation::Omitted {
            reason: SemanticContextOmissionReason::SourceNotFound,
            evidence: omitted_context_evidence(source, state, None),
        });
    };
    if !matches!(canonical.eligibility, SemanticEligibility::Eligible) {
        return Ok(SemanticContextEgressExpectation::Omitted {
            reason: SemanticContextOmissionReason::SourceIneligible,
            evidence: omitted_context_evidence(source, state, Some(&canonical)),
        });
    }
    let reason = match state.availability {
        CurrentSemanticAvailabilityClass::Missing => {
            Some(SemanticContextOmissionReason::SemanticHeadMissing)
        }
        CurrentSemanticAvailabilityClass::Building => {
            Some(SemanticContextOmissionReason::SemanticHeadBuilding)
        }
        CurrentSemanticAvailabilityClass::Failed => {
            Some(SemanticContextOmissionReason::SemanticHeadFailed)
        }
        CurrentSemanticAvailabilityClass::Unsupported => {
            Some(SemanticContextOmissionReason::SourceIneligible)
        }
        CurrentSemanticAvailabilityClass::Current => None,
    };
    if let Some(reason) = reason {
        return Ok(SemanticContextEgressExpectation::Omitted {
            reason,
            evidence: omitted_context_evidence(source, state, Some(&canonical)),
        });
    }
    let semantic_head = state.head.clone().ok_or_else(|| {
        DbError::InvalidData("current semantic egress context lacks its head".to_string())
    })?;
    validate_canonical_against_head(&canonical, &semantic_head)?;
    Ok(SemanticContextEgressExpectation::Accepted(
        SemanticContextHeadExpectation {
            source: source.clone(),
            semantic_head,
        },
    ))
}

fn context_state_set_digest(contexts: &[SemanticContextEgressExpectation]) -> Result<Digest32> {
    let mut hasher = Sha256::new();
    let domain = b"buzz.semantic-graph-query-egress-context-state-set";
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain);
    hasher.update((contexts.len() as u64).to_be_bytes());
    for context in contexts {
        let source = context.source();
        let (family, subtype) = semantic_source_db_key(source.kind);
        for value in [family.as_bytes(), subtype.as_bytes()] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value);
        }
        hasher.update(source.source_id.as_bytes());
        match context {
            SemanticContextEgressExpectation::Accepted(context) => {
                hasher.update([0]);
                hasher.update(context.semantic_head.invalidation_epoch.to_be_bytes());
                hasher.update(context.semantic_head.snapshot_digest.as_bytes());
                hasher.update(context.semantic_head.unit_set_id.as_bytes());
                hasher.update((context.semantic_head.unit_key.len() as u64).to_be_bytes());
                hasher.update(context.semantic_head.unit_key.as_bytes());
                hasher.update(context.semantic_head.semantic_text_digest.as_bytes());
                hasher.update([match context.semantic_head.summary_coverage {
                    SemanticCoverage::TitleOnly => 0,
                    SemanticCoverage::TitleAndSummary => 1,
                }]);
            }
            SemanticContextEgressExpectation::Omitted { reason, evidence } => {
                hasher.update([1, context_omission_reason_rank(*reason)]);
                append_optional_u64(&mut hasher, evidence.source_invalidation_epoch);
                append_optional_digest(&mut hasher, evidence.source_snapshot_digest);
                let basis = evidence
                    .source_basis
                    .as_ref()
                    .map(serde_json::to_vec)
                    .transpose()?;
                append_optional_bytes(&mut hasher, basis.as_deref());
            }
        }
    }
    Ok(Digest32::from_bytes(hasher.finalize().into()))
}

const fn context_omission_reason_rank(reason: SemanticContextOmissionReason) -> u8 {
    match reason {
        SemanticContextOmissionReason::SourceNotFound => 0,
        SemanticContextOmissionReason::SourceIneligible => 1,
        SemanticContextOmissionReason::SemanticHeadMissing => 2,
        SemanticContextOmissionReason::SemanticHeadBuilding => 3,
        SemanticContextOmissionReason::SemanticHeadFailed => 4,
    }
}

fn append_optional_u64(hasher: &mut Sha256, value: Option<u64>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.to_be_bytes());
        }
        None => hasher.update([0]),
    }
}

fn append_optional_digest(hasher: &mut Sha256, value: Option<Digest32>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.as_bytes());
        }
        None => hasher.update([0]),
    }
}

fn append_optional_bytes(hasher: &mut Sha256, value: Option<&[u8]>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value);
        }
        None => hasher.update([0]),
    }
}

async fn load_authorized_ticket_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    reader_pubkey: &[u8],
    expected_projection_pubkey: &PublicKey,
) -> Result<SemanticGraphQueryTicket> {
    let row = sqlx::query(AUTHORIZED_TICKET_SQL)
        .bind(community_id.as_uuid())
        .bind(reader_pubkey)
        .bind(expected_projection_pubkey.as_bytes())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(unavailable)?;

    // These strict parity checks run only after the coarse authorization CTE,
    // so an unauthorized caller cannot use their cost or result as an oracle.
    if !Db::project_view_v3_structural_ready_in_tx(tx, community_id, expected_projection_pubkey)
        .await?
        || !crate::project_document::document_projection_parity(
            tx,
            community_id,
            expected_projection_pubkey,
            None,
            None,
        )
        .await?
        || !crate::project_context::context_projection_parity(
            tx,
            community_id,
            expected_projection_pubkey,
        )
        .await?
    {
        return Err(unavailable());
    }
    sqlx::query("SELECT project_document_validate_community($1)")
        .bind(community_id.as_uuid())
        .execute(&mut **tx)
        .await?;
    sqlx::query("SELECT project_context_validate_community($1)")
        .bind(community_id.as_uuid())
        .execute(&mut **tx)
        .await?;

    let generation = semantic_generation_from_row(&row)?;
    if generation.lifecycle != "active" || generation.community_id != community_id {
        return Err(unavailable());
    }
    let query_fences = QueryCompatibilityFences::for_source_contract(&generation.model_contract)
        .map_err(|error| {
            DbError::InvalidData(format!(
                "active semantic generation is not query compatible: {error}"
            ))
        })?;
    if query_fences.source_generation_contract_digest != generation.model_contract_digest {
        return Err(DbError::InvalidData(
            "active semantic generation/query source fence mismatch".to_string(),
        ));
    }
    Ok(SemanticGraphQueryTicket {
        community_id,
        generation,
        query_fences,
        project_context_revision: nonnegative_u64(
            row.try_get("context_revision")?,
            "project_context_revision",
        )?,
        projection_generation: positive_u64(
            row.try_get("projection_generation")?,
            "project_context_projection_generation",
        )?,
        observed_at: row.try_get("observed_at")?,
    })
}

fn same_generation_contract(
    observed: &SemanticGraphQueryTicket,
    expected: &SemanticGraphQueryTicket,
) -> bool {
    observed.community_id == expected.community_id
        && observed.generation == expected.generation
        && observed.query_fences == expected.query_fences
}

fn same_release_snapshot(
    observed: &SemanticGraphQueryTicket,
    expected: &SemanticGraphQueryTicket,
) -> bool {
    same_generation_contract(observed, expected)
        && observed.projection_generation == expected.projection_generation
        && observed.project_context_revision == expected.project_context_revision
}

fn validate_pubkey(pubkey: &[u8]) -> Result<()> {
    if pubkey.len() == 32 {
        Ok(())
    } else {
        Err(unavailable())
    }
}

fn validate_timeouts(timeouts: SemanticGraphReadTimeouts) -> Result<()> {
    if timeouts.statement.is_zero()
        || timeouts.lock.is_zero()
        || timeouts.idle_in_transaction.is_zero()
    {
        return Err(DbError::InvalidData(
            "semantic graph read timeouts must be positive".to_string(),
        ));
    }
    Ok(())
}

async fn set_local_timeout(
    tx: &mut Transaction<'_, Postgres>,
    setting: &'static str,
    duration: Duration,
) -> Result<()> {
    let millis = duration.as_millis().min(u128::from(u64::MAX));
    sqlx::query("SELECT set_config($1, $2, true)")
        .bind(setting)
        .bind(format!("{millis}ms"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn unavailable() -> DbError {
    DbError::AccessDenied("semantic graph query unavailable".to_string())
}

// Final confirmations first wait for the shared Community advisory lock under
// READ COMMITTED, then take row locks in this stable order: Community,
// expected generation, Project Context state, canonical semantic source
// identities, and fleet assertion. The lock-wait statement's snapshot is not
// reused by later checks.
const FINAL_CONFIRMATION_ISOLATION_SQL: &str = "SET TRANSACTION ISOLATION LEVEL READ COMMITTED";
const LOCK_FINAL_CONFIRMATION_COMMUNITY_SQL: &str =
    "SELECT id FROM communities WHERE id=$1 FOR SHARE";
const LOCK_EGRESS_GENERATION_SQL: &str = "SELECT generation_id FROM semantic_index_generations \
     WHERE community_id=$1 AND generation_id=$2 FOR SHARE";
const LOCK_EGRESS_CONTEXT_STATE_SQL: &str = "SELECT community_id FROM project_context_edge_state \
     WHERE community_id=$1 FOR SHARE";

const AUTHORIZED_TICKET_SQL: &str = r#"
WITH requested_reader(pubkey) AS (
    VALUES ($2::bytea)
),
authorized_reader AS MATERIALIZED (
    SELECT requested_reader.pubkey
    FROM requested_reader
    LEFT JOIN users actor
      ON actor.community_id = $1 AND actor.pubkey = requested_reader.pubkey
    WHERE (
        (
            actor.agent_owner_pubkey IS NULL
            AND EXISTS (
                SELECT 1 FROM relay_members direct_member
                WHERE direct_member.community_id = $1
                  AND direct_member.pubkey = encode(requested_reader.pubkey, 'hex')
            )
        )
        OR (
            actor.agent_owner_pubkey IS NOT NULL
            AND EXISTS (
                SELECT 1 FROM relay_members owner_member
                WHERE owner_member.community_id = $1
                  AND owner_member.pubkey = encode(actor.agent_owner_pubkey, 'hex')
            )
            AND NOT EXISTS (
                SELECT 1 FROM community_bans owner_ban
                WHERE owner_ban.community_id = $1
                  AND owner_ban.pubkey = actor.agent_owner_pubkey
                  AND owner_ban.banned
                  AND (owner_ban.ban_expires_at IS NULL
                       OR owner_ban.ban_expires_at > clock_timestamp())
            )
            AND NOT EXISTS (
                SELECT 1 FROM users owner_actor
                WHERE owner_actor.community_id = $1
                  AND owner_actor.pubkey = actor.agent_owner_pubkey
                  AND owner_actor.agent_owner_pubkey IS NOT NULL
            )
        )
    )
    AND NOT EXISTS (
        SELECT 1 FROM community_bans actor_ban
        WHERE actor_ban.community_id = $1
          AND actor_ban.pubkey = requested_reader.pubkey
          AND actor_ban.banned
          AND (actor_ban.ban_expires_at IS NULL
               OR actor_ban.ban_expires_at > clock_timestamp())
    )
),
authorized_project AS MATERIALIZED (
    SELECT community.id AS community_id,
           community.semantic_active_generation_id AS generation_id,
           context_state.context_revision,
           context_state.projection_generation
    FROM communities community
    CROSS JOIN authorized_reader
    JOIN project_view_maintenance maintenance
      ON maintenance.community_id = community.id
    JOIN project_view_state view_state
      ON view_state.community_id = community.id
    JOIN project_document_state document_state
      ON document_state.community_id = community.id
    JOIN project_context_edge_state context_state
      ON context_state.community_id = community.id
    WHERE community.id = $1
      AND community.archived_at IS NULL
      AND community.project_view_schema_version = 3
      AND community.project_view_enabled
      AND community.project_document_enabled
      AND community.meeting_community_read_enabled
      AND community.project_context_edge_enabled
      AND community.semantic_index_enabled
      AND community.semantic_graph_query_enabled
      AND maintenance.state = 'normal'
      AND view_state.schema_version = 3
      AND document_state.schema_version = 1
      AND context_state.schema_version = 2
      AND view_state.projection_pubkey = $3
      AND document_state.projection_pubkey = $3
      AND context_state.projection_pubkey = $3
      AND community.semantic_active_generation_id IS NOT NULL
)
SELECT generation.*, project.context_revision, project.projection_generation,
       clock_timestamp() AS observed_at
FROM authorized_project project
JOIN semantic_index_generations generation
  ON generation.community_id = project.community_id
 AND generation.generation_id = project.generation_id
WHERE generation.lifecycle = 'active'
"#;

#[cfg(test)]
mod tests {
    use buzz_core::CommunityId;
    use buzz_project_context::{EdgeKey, ProjectContextCoordinate};
    use buzz_project_view::ProjectViewObjectType;
    use buzz_semantic::{
        DeterministicFakeEncoder, Digest32, EmbeddingVector, IneligibilityReason,
        ProjectDocumentSourceBasis, SemanticCoverage, SemanticEligibility, SemanticEncoder,
        SemanticLifecycleClass, SemanticModelContract, SemanticSourceBasis, SemanticSourceIdentity,
        SemanticSourceKind,
    };
    use buzz_semantic_query::{
        ContextDocumentBindingObservation, ProjectContextBindingProvenance,
        ProjectContextEdgeProvenance, QueryCompatibilityFences, RelationRankCursor, Score,
        SemanticEdgeObservation, TargetRankCursor,
    };
    use chrono::Utc;
    use pgvector::Vector;
    use sqlx::Row;
    use uuid::Uuid;

    use super::{
        bindings_from_json, canonical_query_coordinates, compare_ranked_relations,
        compare_ranked_targets, context_state_set_digest, current_availability_from_db,
        edge_keys_from_json, partition_exact_recall, same_release_snapshot,
        semantic_hyperedge_identity_bytes, semantic_source_identity_for_coordinate,
        slice_ranked_relations, slice_ranked_targets, source_eligibility_from_db,
        validate_query_vectors, SemanticContextEgressExpectation, SemanticContextOmissionReason,
        SemanticCurrentContextOverview, SemanticCurrentHead, SemanticExactQueryVector,
        SemanticExactRecallExhaustion, SemanticExactSourceScore, SemanticGraphQueryTicket,
        SemanticGraphStructuralRoles, SemanticOmittedContextEvidence, SemanticRankedRelationOption,
        SemanticRankedTargetOption, SemanticTraversalSliceExhaustion, AUTHORIZED_TICKET_SQL,
        COMPLETE_HYPEREDGE_SQL, CURRENT_COORDINATE_MEMBERSHIPS_SQL,
        CURRENT_SEMANTIC_SOURCE_STATES_SQL, EXACT_SOURCE_SCORES_SQL,
        FINAL_CONFIRMATION_ISOLATION_SQL, INCIDENT_RELATION_REFS_SQL,
        LOCK_EGRESS_CONTEXT_STATE_SQL, LOCK_EGRESS_GENERATION_SQL,
        LOCK_FINAL_CONFIRMATION_COMMUNITY_SQL, SEMANTIC_GRAPH_COVERAGE_SQL,
    };
    use crate::semantic::SemanticGenerationRecord;
    use crate::{Db, DbConfig};

    fn ticket(contract: SemanticModelContract) -> SemanticGraphQueryTicket {
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        let query_fences =
            QueryCompatibilityFences::for_source_contract(&contract).expect("query fences");
        SemanticGraphQueryTicket {
            community_id,
            generation: SemanticGenerationRecord {
                community_id,
                generation_id: Uuid::new_v4(),
                lifecycle: "active".to_string(),
                extractor_version: "overview-v1".to_string(),
                model_contract: contract,
                model_contract_digest: query_fences.source_generation_contract_digest,
                rebuild_completed_at: Some(Utc::now()),
                created_at: Utc::now(),
            },
            query_fences,
            projection_generation: 1,
            project_context_revision: 1,
            observed_at: Utc::now(),
        }
    }

    #[test]
    fn coordinate_release_snapshot_binds_generation_projection_and_context_revision() {
        let expected = ticket(SemanticModelContract::volcengine_overview_v1());
        assert!(same_release_snapshot(&expected, &expected));

        let mut projection_changed = expected.clone();
        projection_changed.projection_generation += 1;
        assert!(!same_release_snapshot(&projection_changed, &expected));

        let mut context_changed = expected.clone();
        context_changed.project_context_revision += 1;
        assert!(!same_release_snapshot(&context_changed, &expected));

        let mut generation_changed = expected.clone();
        generation_changed.generation.generation_id = Uuid::new_v4();
        assert!(!same_release_snapshot(&generation_changed, &expected));
    }

    fn exact_score(channel_id: Digest32, rank: u32, source_id: Uuid) -> SemanticExactSourceScore {
        let source_change_id = Digest32::from_bytes([7; 32]);
        SemanticExactSourceScore {
            channel_id,
            source: SemanticSourceIdentity {
                community_id: Uuid::from_u128(1),
                kind: SemanticSourceKind::ProjectDocument,
                source_id,
            },
            head: SemanticCurrentHead {
                invalidation_epoch: 3,
                snapshot_digest: Digest32::from_bytes([8; 32]),
                source_basis: SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                    document_revision: 2,
                    source_change_id,
                }),
                unit_set_id: Uuid::from_u128(20 + u128::from(rank)),
                unit_key: "overview".to_string(),
                semantic_text_digest: Digest32::from_bytes([9; 32]),
                summary_coverage: SemanticCoverage::TitleAndSummary,
            },
            lifecycle: SemanticLifecycleClass::Active,
            source_status: Some("active".to_string()),
            roles: SemanticGraphStructuralRoles {
                coordinate: false,
                coordinate_entry_eligible: false,
                coordinate_incident_edge_keys: Vec::new(),
                context_document_bindings: Vec::new(),
            },
            score: Score::new(500_000).expect("score"),
            channel_rank: rank,
        }
    }

    #[test]
    fn current_context_overview_debug_redacts_provider_text_and_source_identity() {
        let source_id = Uuid::new_v4();
        let score = exact_score(Digest32::from_bytes([9; 32]), 1, source_id);
        let overview = SemanticCurrentContextOverview {
            source: score.source,
            head: score.head,
            semantic_text: "CONFIDENTIAL-OVERVIEW-中文".to_owned(),
        };
        let debug = format!("{overview:?}");

        assert!(!debug.contains("CONFIDENTIAL-OVERVIEW"));
        assert!(!debug.contains(&source_id.to_string()));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("semantic_text_bytes"));
    }

    fn relation_option(
        edge_byte: u8,
        document_id: Uuid,
        score: u32,
    ) -> SemanticRankedRelationOption {
        let edge_key = EdgeKey::from_hex(&hex::encode([edge_byte; 32])).expect("edge key");
        SemanticRankedRelationOption {
            edge_key,
            edge_provenance: ProjectContextEdgeProvenance {
                last_context_revision: 3,
                source_change_id: Digest32::from_bytes([edge_byte; 32]),
            },
            document_id,
            binding_provenance: ProjectContextBindingProvenance {
                binding_context_revision: 4,
                source_change_id: Digest32::from_bytes([5; 32]),
                projection_event_id: Digest32::from_bytes([6; 32]),
            },
            channel_scores: vec![exact_score(Digest32::from_bytes([1; 32]), 1, document_id)],
            local_coherence: None,
            document_score: Score::new(score).expect("document score"),
        }
    }

    fn target_option(
        coordinate: ProjectContextCoordinate,
        score: u32,
    ) -> SemanticRankedTargetOption {
        let source_id = match &coordinate {
            ProjectContextCoordinate::ProjectViewObject { object_id, .. } => *object_id,
            ProjectContextCoordinate::Document { document_id } => *document_id,
            ProjectContextCoordinate::Meeting { meeting_id } => *meeting_id,
        };
        let mut exact = exact_score(Digest32::from_bytes([1; 32]), 1, source_id);
        exact.source.kind = match &coordinate {
            ProjectContextCoordinate::ProjectViewObject { .. } => {
                SemanticSourceKind::ProjectView(buzz_semantic::ProjectViewSemanticType::Work)
            }
            ProjectContextCoordinate::Document { .. } => SemanticSourceKind::ProjectDocument,
            ProjectContextCoordinate::Meeting { .. } => SemanticSourceKind::Meeting,
        };
        let pair_source = exact.source.clone();
        SemanticRankedTargetOption {
            coordinate,
            channel_scores: vec![exact.clone()],
            relation_document_coherence: super::SemanticCurrentSourcePairDistance {
                left: SemanticSourceIdentity {
                    community_id: pair_source.community_id,
                    kind: SemanticSourceKind::ProjectDocument,
                    source_id: Uuid::from_u128(300),
                },
                right: pair_source,
                left_head: exact.head.clone(),
                right_head: exact.head,
                score: Score::new(700_000).expect("coherence"),
            },
            target_score: Score::new(score).expect("target score"),
            transition_score: Score::new(score).expect("transition score"),
        }
    }

    #[test]
    fn query_vectors_require_nonzero_values_unique_channels_and_three_fences() {
        let encoder = DeterministicFakeEncoder::new(3).expect("fake encoder");
        let ticket = ticket(encoder.contract().clone());
        let vector = SemanticExactQueryVector {
            channel_id: Digest32::from_bytes([1; 32]),
            query_fences: ticket.query_fences,
            embedding: EmbeddingVector::new(vec![1.0, 0.0, 0.0], &ticket.generation.model_contract)
                .expect("embedding"),
        };
        assert!(validate_query_vectors(&ticket, std::slice::from_ref(&vector)).is_ok());

        let duplicate = vec![vector.clone(), vector.clone()];
        assert!(validate_query_vectors(&ticket, &duplicate).is_err());

        let mut wrong_fence = vector.clone();
        wrong_fence.query_fences.query_contract_digest = Digest32::from_bytes([9; 32]);
        assert!(validate_query_vectors(&ticket, &[wrong_fence]).is_err());

        assert!(
            EmbeddingVector::new(vec![0.0, 0.0, 0.0], &ticket.generation.model_contract).is_err()
        );
    }

    #[test]
    fn exact_sql_materializes_roles_before_distance_and_quantizes_in_db() {
        let roles = EXACT_SOURCE_SCORES_SQL.find("graph_roles AS MATERIALIZED");
        let eligible = EXACT_SOURCE_SCORES_SQL.find("eligible AS MATERIALIZED");
        let distance = EXACT_SOURCE_SCORES_SQL.find("eligible.embedding <=>");
        assert!(roles < eligible && eligible < distance);
        assert!(EXACT_SOURCE_SCORES_SQL.contains("embedding.response_model = generation.model"));
        assert!(EXACT_SOURCE_SCORES_SQL.contains("vector_norm(embedding.embedding) > 0"));
        assert!(EXACT_SOURCE_SCORES_SQL.contains("semantic_graph_query_enabled"));
        assert!(EXACT_SOURCE_SCORES_SQL.contains("::bigint AS semantic_score"));
        assert!(EXACT_SOURCE_SCORES_SQL.contains("GROUP BY community_id, source_family"));
        assert!(EXACT_SOURCE_SCORES_SQL.contains("coordinate_incident_edge_keys"));
    }

    #[test]
    fn recall_k_plus_one_is_internal_and_accounts_for_empty_channels() {
        let truncated_channel = Digest32::from_bytes([1; 32]);
        let exhausted_channel = Digest32::from_bytes([2; 32]);
        let empty_channel = Digest32::from_bytes([3; 32]);
        let batch = partition_exact_recall(
            &[truncated_channel, exhausted_channel, empty_channel],
            vec![
                exact_score(truncated_channel, 1, Uuid::from_u128(101)),
                exact_score(truncated_channel, 2, Uuid::from_u128(102)),
                exact_score(exhausted_channel, 1, Uuid::from_u128(103)),
            ],
            1,
        )
        .expect("partition K+1 recall");
        assert_eq!(batch.scores.len(), 2);
        assert!(batch.scores.iter().all(|score| score.channel_rank == 1));
        assert_eq!(batch.channels.len(), 3);
        assert_eq!(
            batch.channels[0].exhaustion,
            SemanticExactRecallExhaustion::Truncated
        );
        assert_eq!(batch.channels[0].returned_count, 1);
        assert_eq!(
            batch.channels[1].exhaustion,
            SemanticExactRecallExhaustion::Exhausted
        );
        assert_eq!(batch.channels[1].returned_count, 1);
        assert_eq!(batch.channels[2].returned_count, 0);
        assert!(partition_exact_recall(
            &[truncated_channel],
            vec![exact_score(truncated_channel, 3, Uuid::from_u128(104))],
            1,
        )
        .is_err());
    }

    #[test]
    fn coordinate_mapping_and_input_canonicalization_are_closed() {
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        let work_id = Uuid::new_v4();
        let document_id = Uuid::new_v4();
        let work = ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: work_id,
        };
        let document = ProjectContextCoordinate::Document { document_id };
        let source = semantic_source_identity_for_coordinate(community_id, &work)
            .expect("map Work Coordinate");
        assert_eq!(
            source.kind,
            SemanticSourceKind::ProjectView(buzz_semantic::ProjectViewSemanticType::Work)
        );
        assert_eq!(source.source_id, work_id);

        let canonical = canonical_query_coordinates(
            community_id,
            &[document.clone(), work.clone()],
            2,
            "test Coordinate",
        )
        .expect("canonical inputs");
        assert_eq!(canonical, vec![work.clone(), document]);
        assert!(canonical_query_coordinates(
            community_id,
            &[work.clone(), work],
            2,
            "test Coordinate",
        )
        .is_err());
        assert!(semantic_source_identity_for_coordinate(
            community_id,
            &ProjectContextCoordinate::ProjectViewObject {
                object_type: ProjectViewObjectType::ProjectProfile,
                object_id: Uuid::new_v4(),
            },
        )
        .is_err());
    }

    #[test]
    fn hydration_support_queries_are_snapshot_bound_and_bodyless() {
        assert!(CURRENT_COORDINATE_MEMBERSHIPS_SQL.contains("state.context_revision = $5"));
        assert!(CURRENT_COORDINATE_MEMBERSHIPS_SQL.contains("edge.state = 'active'"));
        for marker in [
            "head.source_invalidation_epoch = source.invalidation_epoch",
            "head.source_snapshot_digest = source.snapshot_digest",
            "embedding.response_model = generation.model",
            "vector_norm(embedding.embedding) > 0",
            "job.desired_invalidation_epoch = source.invalidation_epoch",
        ] {
            assert!(CURRENT_SEMANTIC_SOURCE_STATES_SQL.contains(marker));
        }
        assert!(!CURRENT_SEMANTIC_SOURCE_STATES_SQL.contains("content_markdown"));
        assert!(!CURRENT_SEMANTIC_SOURCE_STATES_SQL.contains("project_view_projection"));
        assert!(current_availability_from_db("unknown").is_err());
    }

    #[test]
    fn traversal_sql_is_revision_bound_complete_and_never_key_prefix_scores() {
        for marker in [
            "state.context_revision = $3",
            "edge.state = 'active'",
            "binding.state = 'active'",
            "ORDER BY binding.context_document_id",
        ] {
            assert!(COMPLETE_HYPEREDGE_SQL.contains(marker));
        }
        for marker in [
            "state.context_revision = $5",
            "coordinate.coordinate_subtype IS NOT DISTINCT FROM $3",
            "edge.state = 'active'",
            "binding.state = 'active'",
            "ORDER BY edge.edge_key, binding.context_document_id",
            "LIMIT $6",
        ] {
            assert!(INCIDENT_RELATION_REFS_SQL.contains(marker));
        }
        let candidate_set = EXACT_SOURCE_SCORES_SQL
            .find("requested_candidates")
            .expect("candidate set");
        let distance = EXACT_SOURCE_SCORES_SQL
            .find("eligible.embedding <=>")
            .expect("exact distance");
        let rank = EXACT_SOURCE_SCORES_SQL
            .find("row_number() OVER")
            .expect("rank after distance");
        assert!(candidate_set < distance && distance < rank);
    }

    #[test]
    fn complete_hyperedge_identity_includes_all_coordinates_and_bindings() {
        let community_id = Uuid::new_v4();
        let coordinates = vec![
            ProjectContextCoordinate::ProjectViewObject {
                object_type: ProjectViewObjectType::Work,
                object_id: Uuid::new_v4(),
            },
            ProjectContextCoordinate::Document {
                document_id: Uuid::new_v4(),
            },
        ];
        let edge_key = EdgeKey::derive(community_id, &coordinates).expect("edge key");
        let edge = SemanticEdgeObservation {
            edge_key,
            complete_coordinates: coordinates,
            provenance: ProjectContextEdgeProvenance {
                last_context_revision: 7,
                source_change_id: Digest32::from_bytes([1; 32]),
            },
            current_context_document_bindings: vec![ContextDocumentBindingObservation {
                document_id: Uuid::new_v4(),
                provenance: ProjectContextBindingProvenance {
                    binding_context_revision: 8,
                    source_change_id: Digest32::from_bytes([2; 32]),
                    projection_event_id: Digest32::from_bytes([3; 32]),
                },
            }],
        };
        let serialized = serde_json::to_vec(&edge).expect("edge identity JSON");
        assert_eq!(
            semantic_hyperedge_identity_bytes(&edge).expect("identity bytes"),
            serialized.len()
        );
        assert!(serialized
            .windows("complete_coordinates".len())
            .any(|window| window == b"complete_coordinates"));
        assert!(serialized
            .windows("current_context_document_bindings".len())
            .any(|window| window == b"current_context_document_bindings"));
    }

    #[test]
    fn traversal_keysets_are_score_first_stable_and_observe_limit_plus_one() {
        let first_id = Uuid::from_u128(1);
        let second_id = Uuid::from_u128(2);
        let third_id = Uuid::from_u128(3);
        let mut relations = vec![
            relation_option(2, third_id, 700_000),
            relation_option(1, second_id, 700_000),
            relation_option(1, first_id, 800_000),
        ];
        relations.sort_by(compare_ranked_relations);
        assert_eq!(relations[0].document_id, first_id);
        assert_eq!(relations[1].document_id, second_id);
        let (first_page, exhaustion) =
            slice_ranked_relations(relations.clone(), None, 2).expect("relation slice");
        assert_eq!(first_page.len(), 2);
        assert_eq!(exhaustion, SemanticTraversalSliceExhaustion::Truncated);
        let cursor = RelationRankCursor {
            document_score: first_page[1].document_score,
            edge_key: first_page[1].edge_key,
            document_id: first_page[1].document_id,
        };
        let (second_page, exhaustion) =
            slice_ranked_relations(relations, Some(&cursor), 2).expect("relation continuation");
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].document_id, third_id);
        assert_eq!(exhaustion, SemanticTraversalSliceExhaustion::Exhausted);

        let first_coordinate = ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: Uuid::from_u128(11),
        };
        let second_coordinate = ProjectContextCoordinate::Document {
            document_id: Uuid::from_u128(12),
        };
        let third_coordinate = ProjectContextCoordinate::Meeting {
            meeting_id: Uuid::from_u128(13),
        };
        let mut targets = vec![
            target_option(third_coordinate.clone(), 600_000),
            target_option(second_coordinate, 700_000),
            target_option(first_coordinate, 700_000),
        ];
        targets.sort_by(compare_ranked_targets);
        let (first_page, exhaustion) =
            slice_ranked_targets(targets.clone(), None, 2).expect("target slice");
        assert_eq!(first_page.len(), 2);
        assert_eq!(exhaustion, SemanticTraversalSliceExhaustion::Truncated);
        let cursor = TargetRankCursor {
            transition_score: first_page[1].transition_score,
            target_coordinate: first_page[1].coordinate.clone(),
        };
        let (second_page, exhaustion) =
            slice_ranked_targets(targets, Some(&cursor), 2).expect("target continuation");
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].coordinate, third_coordinate);
        assert_eq!(exhaustion, SemanticTraversalSliceExhaustion::Exhausted);
    }

    #[test]
    fn traversal_omission_and_postflight_contracts_are_closed() {
        assert_eq!(
            source_eligibility_from_db(Some("ineligible"), Some("tombstone")).expect("tombstone"),
            Some(SemanticEligibility::Ineligible(
                IneligibilityReason::Tombstone
            ))
        );
        assert!(source_eligibility_from_db(Some("eligible"), Some("deleted")).is_err());
        for marker in [
            "community_bans actor_ban",
            "community_bans owner_ban",
            "community.project_view_enabled",
            "community.project_document_enabled",
            "community.meeting_community_read_enabled",
            "community.project_context_edge_enabled",
            "community.semantic_index_enabled",
            "community.semantic_graph_query_enabled",
            "context_state.schema_version = 2",
        ] {
            assert!(AUTHORIZED_TICKET_SQL.contains(marker));
        }
    }

    #[test]
    fn omitted_context_expectations_detect_reason_epoch_and_snapshot_churn() {
        let source = SemanticSourceIdentity {
            community_id: Uuid::new_v4(),
            kind: SemanticSourceKind::ProjectDocument,
            source_id: Uuid::new_v4(),
        };
        let basis = SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
            document_revision: 4,
            source_change_id: Digest32::from_bytes([4; 32]),
        });
        let expected = SemanticContextEgressExpectation::Omitted {
            reason: SemanticContextOmissionReason::SemanticHeadBuilding,
            evidence: SemanticOmittedContextEvidence {
                source: source.clone(),
                source_invalidation_epoch: Some(7),
                source_basis: Some(basis.clone()),
                source_snapshot_digest: Some(Digest32::from_bytes([5; 32])),
            },
        };
        let mut changed_epoch = expected.clone();
        let SemanticContextEgressExpectation::Omitted { evidence, .. } = &mut changed_epoch else {
            panic!("omitted expectation");
        };
        evidence.source_invalidation_epoch = Some(8);
        let changed_reason = SemanticContextEgressExpectation::Omitted {
            reason: SemanticContextOmissionReason::SemanticHeadFailed,
            evidence: SemanticOmittedContextEvidence {
                source,
                source_invalidation_epoch: Some(7),
                source_basis: Some(basis),
                source_snapshot_digest: Some(Digest32::from_bytes([5; 32])),
            },
        };
        assert_ne!(expected, changed_epoch);
        assert_ne!(expected, changed_reason);
        assert_ne!(
            context_state_set_digest(std::slice::from_ref(&expected)).expect("expected digest"),
            context_state_set_digest(std::slice::from_ref(&changed_epoch)).expect("changed digest")
        );
        assert_ne!(
            context_state_set_digest(std::slice::from_ref(&expected)).expect("expected digest"),
            context_state_set_digest(std::slice::from_ref(&changed_reason))
                .expect("changed digest")
        );
    }

    #[test]
    fn egress_locks_generation_and_graph_revision_before_reservation() {
        assert_eq!(
            FINAL_CONFIRMATION_ISOLATION_SQL,
            "SET TRANSACTION ISOLATION LEVEL READ COMMITTED"
        );
        assert!(!FINAL_CONFIRMATION_ISOLATION_SQL.contains("REPEATABLE READ"));
        assert!(LOCK_FINAL_CONFIRMATION_COMMUNITY_SQL.contains("FOR SHARE"));
        assert!(LOCK_EGRESS_GENERATION_SQL.contains("FOR SHARE"));
        assert!(LOCK_EGRESS_GENERATION_SQL.contains("generation_id=$2"));
        assert!(LOCK_EGRESS_CONTEXT_STATE_SQL.contains("FOR SHARE"));
        assert!(LOCK_EGRESS_CONTEXT_STATE_SQL.contains("community_id=$1"));
    }

    #[test]
    fn coordinate_incident_edge_parser_sorts_and_rejects_duplicates() {
        let keys = edge_keys_from_json(
            serde_json::json!(["22".repeat(32), "11".repeat(32)]),
            "test Edge",
        )
        .expect("edge keys");
        assert_eq!(keys[0].to_hex(), "11".repeat(32));
        assert!(edge_keys_from_json(
            serde_json::json!(["11".repeat(32), "11".repeat(32)]),
            "test Edge",
        )
        .is_err());
    }

    #[test]
    fn coverage_priority_is_closed_and_current_epoch_bound() {
        for marker in [
            "non_queryable_zero_vector",
            "job_state = 'poison'",
            "job_state IN ('pending', 'claimed', 'retry')",
            "coverage_state = 'unsupported'",
            "ELSE 'missing'",
        ] {
            assert!(SEMANTIC_GRAPH_COVERAGE_SQL.contains(marker));
        }
        assert!(SEMANTIC_GRAPH_COVERAGE_SQL
            .contains("job.desired_invalidation_epoch = source.invalidation_epoch"));
        assert!(SEMANTIC_GRAPH_COVERAGE_SQL
            .contains("head.source_invalidation_epoch = source.invalidation_epoch"));
    }

    #[test]
    fn binding_metadata_parser_is_closed_and_deterministic() {
        let bindings = bindings_from_json(serde_json::json!([{
            "edge_key": "11".repeat(32),
            "edge_last_context_revision": 7,
            "edge_source_change_id": "22".repeat(32),
            "binding_context_revision": 8,
            "binding_source_change_id": "33".repeat(32),
            "binding_projection_event_id": "44".repeat(32)
        }]))
        .expect("binding metadata");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].edge_last_context_revision, 7);
        assert!(bindings_from_json(serde_json::json!([{"edge_key": "bad"}])).is_err());
    }

    #[tokio::test]
    async fn final_confirmation_lock_orders_writer_first_and_permit_first_revocations() {
        let Ok(database_url) = std::env::var("BUZZ_TEST_SEMANTIC_DATABASE_URL") else {
            return;
        };
        let db = Db::new(&DbConfig {
            database_url,
            ..DbConfig::default()
        })
        .await
        .expect("semantic final-confirmation test database");
        db.migrate().await.expect("semantic migrations");
        let reader = [41_u8; 32];
        let actor = [42_u8; 32];

        // Writer-first: the canonical writer holds the exclusive Community
        // lock and commits a ban before the final confirmation can acquire its
        // shared lock. READ COMMITTED must then observe the committed ban.
        let writer_first = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id,host) VALUES ($1,$2)")
            .bind(writer_first.as_uuid())
            .bind(format!(
                "semantic-release-writer-{}.invalid",
                writer_first.as_uuid()
            ))
            .execute(&db.pool)
            .await
            .expect("writer-first Community");
        let mut writer_tx = db.pool.begin().await.expect("writer-first transaction");
        crate::relay_members::acquire_membership_write_lock(&mut writer_tx, writer_first)
            .await
            .expect("exclusive membership lock");
        sqlx::query(
            "INSERT INTO community_bans(community_id,pubkey,banned,actor_pubkey) \
             VALUES ($1,$2,TRUE,$3)",
        )
        .bind(writer_first.as_uuid())
        .bind(reader.as_slice())
        .bind(actor.as_slice())
        .execute(&mut *writer_tx)
        .await
        .expect("stage writer-first ban");

        let reader_db = db.clone();
        let mut writer_first_confirmation = tokio::spawn(async move {
            let mut tx = reader_db
                .begin_semantic_graph_final_confirmation(writer_first)
                .await
                .expect("writer-first final confirmation");
            let banned: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM community_bans \
                 WHERE community_id=$1 AND pubkey=$2 AND banned)",
            )
            .bind(writer_first.as_uuid())
            .bind(reader.as_slice())
            .fetch_one(&mut *tx)
            .await
            .expect("observe writer-first ban");
            tx.commit().await.expect("commit writer-first confirmation");
            banned
        });
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(150),
                &mut writer_first_confirmation,
            )
            .await
            .is_err(),
            "shared final confirmation must wait for the canonical writer"
        );
        writer_tx.commit().await.expect("commit writer-first ban");
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), writer_first_confirmation,)
                .await
                .expect("writer-first confirmation timeout")
                .expect("writer-first confirmation task"),
            "a revocation committed before shared-lock acquisition must be visible"
        );

        // Permit-first: a final confirmation that already owns the shared
        // Community lock linearizes before the canonical ban writer. The ban
        // cannot commit until the confirmation transaction releases its lock.
        let permit_first = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id,host) VALUES ($1,$2)")
            .bind(permit_first.as_uuid())
            .bind(format!(
                "semantic-release-permit-{}.invalid",
                permit_first.as_uuid()
            ))
            .execute(&db.pool)
            .await
            .expect("permit-first Community");
        let mut permit_tx = db
            .begin_semantic_graph_final_confirmation(permit_first)
            .await
            .expect("permit-first final confirmation");
        let banned_before: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM community_bans \
             WHERE community_id=$1 AND pubkey=$2 AND banned)",
        )
        .bind(permit_first.as_uuid())
        .bind(reader.as_slice())
        .fetch_one(&mut *permit_tx)
        .await
        .expect("observe pre-permit ban state");
        assert!(!banned_before);

        let writer_db = db.clone();
        let mut permit_first_writer = tokio::spawn(async move {
            crate::moderation::ban_member_with_revocation(
                &writer_db.pool,
                permit_first,
                &reader,
                &actor,
                Some("semantic release fence test"),
                None,
                &[43_u8; 32],
            )
            .await
        });
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(150),
                &mut permit_first_writer,
            )
            .await
            .is_err(),
            "canonical revocation must wait behind a granted release lock"
        );
        permit_tx
            .commit()
            .await
            .expect("commit permit-first confirmation");
        tokio::time::timeout(std::time::Duration::from_secs(5), permit_first_writer)
            .await
            .expect("permit-first writer timeout")
            .expect("permit-first writer task")
            .expect("commit permit-first ban");
    }

    #[tokio::test]
    async fn pgvector_exact_order_and_score_match_bruteforce_reference() {
        let Ok(database_url) = std::env::var("BUZZ_TEST_SEMANTIC_DATABASE_URL") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("semantic query test database");
        let ids = vec![Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        let values = [
            vec![1.0_f32, 0.0, 0.0],
            vec![0.0_f32, 1.0, 0.0],
            vec![-1.0_f32, 0.0, 0.0],
        ];
        let vectors: Vec<Vector> = values.iter().cloned().map(Vector::from).collect();
        let query = Vector::from(vec![1.0_f32, 0.0, 0.0]);
        let rows = sqlx::query(
            "WITH candidates(id, embedding) AS ( \
               SELECT * FROM unnest($1::uuid[], $2::vector[]) \
             ), distances AS ( \
               SELECT id, embedding <=> $3::vector AS distance FROM candidates \
             ) \
             SELECT id, distance, floor(( \
               (greatest(-1.0, least(1.0, 1.0 - distance)) + 1.0) / 2.0 \
             ) * 1000000.0 + 0.5)::bigint AS semantic_score \
             FROM distances ORDER BY distance, id",
        )
        .bind(&ids)
        .bind(&vectors)
        .bind(query)
        .fetch_all(&pool)
        .await
        .expect("exact pgvector reference");

        let mut expected: Vec<(Uuid, f64, Score)> = ids
            .iter()
            .copied()
            .zip(values.iter())
            .map(|(id, value)| {
                let distance = 1.0 - f64::from(value[0]);
                (
                    id,
                    distance,
                    Score::from_cosine_distance(distance).expect("reference score"),
                )
            })
            .collect();
        expected.sort_by(|left, right| {
            left.1
                .total_cmp(&right.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        assert_eq!(rows.len(), expected.len());
        for (row, (expected_id, expected_distance, expected_score)) in rows.iter().zip(expected) {
            assert_eq!(row.try_get::<Uuid, _>("id").expect("id"), expected_id);
            assert!(
                (row.try_get::<f64, _>("distance").expect("distance") - expected_distance).abs()
                    < 1e-12
            );
            assert_eq!(
                row.try_get::<i64, _>("semantic_score")
                    .expect("semantic score"),
                i64::from(expected_score.raw())
            );
        }
    }
}
