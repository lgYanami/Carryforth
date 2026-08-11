use std::collections::{BTreeMap, BTreeSet};

use buzz_project_context::{EdgeKey, ProjectContextCoordinate};
use buzz_project_view::ProjectViewObjectType;
use buzz_semantic::{
    Digest32, ProjectViewSemanticType, SemanticCoverage, SemanticLifecycleClass,
    SemanticSourceBasis, SemanticSourceIdentity, SemanticSourceKind,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    budget_profile_digest, candidate_score, document_score, environment_gain, harmonic_score,
    path_score, query_contract_digest, ranking_contract_digest, target_coordinate_score,
    AnchorGain, ConditionedEvidence, LifecycleFilter, PathScoreExplanation, QueryContractResult,
    Score, SemanticGraphQuery, SemanticGraphQueryError, MAX_TRUNCATION_SAMPLES,
};

/// Exact query/index/graph observations that fence one returned snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticGraphQueryObservations {
    /// Active semantic generation observed in Stage C.
    pub semantic_generation_id: Uuid,
    /// Complete Foundation source-generation contract digest.
    pub source_generation_contract_digest: Digest32,
    /// Comparable vector-space fence.
    pub embedding_space_fence: Digest32,
    /// Query template/serializer/input-limit digest.
    pub query_contract_digest: Digest32,
    /// Fixed-point ranking algorithm digest.
    pub ranking_contract_digest: Digest32,
    /// Budget defaults, caps, counters, and packing digest.
    pub budget_profile_digest: Digest32,
    /// Active overview extractor contract.
    pub extractor_version: String,
    /// Project Context catalog revision in the Stage C snapshot.
    pub project_context_revision: u64,
    /// Writer-DB transaction observation time.
    pub snapshot_observed_at: DateTime<Utc>,
}

/// Current graph membership evidence for one caller-supplied Coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentGraphMembershipObservation {
    /// Project Context revision at which membership was observed.
    pub context_revision: u64,
    /// Canonically sorted current incident Edge identities.
    pub incident_edge_keys: Vec<EdgeKey>,
}

/// Current queryable semantic-head provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticHeadProvenance {
    /// Active generation that owns the embedding.
    pub generation_id: Uuid,
    /// Stable unit key, currently `overview`.
    pub unit_key: String,
    /// Current canonical source snapshot represented by the unit.
    pub snapshot_digest: Digest32,
}

/// Closed semantic state of an accepted explicit initial Coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "provenance", rename_all = "snake_case")]
pub enum SemanticHeadState {
    /// Exact current-generation head is queryable.
    Current(SemanticHeadProvenance),
    /// No exact current-generation semantic head exists.
    Missing,
    /// Exact current-generation work is pending or claimed.
    Building,
    /// Exact current-generation work failed.
    Failed,
    /// The current source cannot be represented by this generation.
    Unsupported,
}

/// Accepted current in-graph explicit initial Coordinate observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedInitialCoordinateObservation {
    /// Accepted Coordinate.
    pub coordinate: ProjectContextCoordinate,
    /// Current graph membership.
    pub graph_membership: CurrentGraphMembershipObservation,
    /// Typed canonical source basis.
    pub source_basis: SemanticSourceBasis,
    /// Current semantic availability; missing embedding does not remove root.
    pub semantic_state: SemanticHeadState,
}

/// Closed reason an in-graph initial Coordinate could not become a root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OmittedInitialCoordinateReason {
    /// Canonical source no longer exists.
    SourceNotFound,
    /// Canonical source was hard deleted.
    SourceDeleted,
    /// Canonical source is a bodyless tombstone.
    SourceTombstoned,
    /// Canonical source is otherwise ineligible.
    SourceIneligible,
}

/// In-graph initial Coordinate omitted for a source-local availability reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OmittedInitialCoordinateObservation {
    /// Omitted Coordinate.
    pub coordinate: ProjectContextCoordinate,
    /// Current graph membership that remains visible.
    pub graph_membership: CurrentGraphMembershipObservation,
    /// Closed source-local omission reason.
    pub reason: OmittedInitialCoordinateReason,
}

/// Accepted current context lens and exact Qi provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedContextCoordinateObservation {
    /// Context Coordinate.
    pub coordinate: ProjectContextCoordinate,
    /// Typed current canonical basis.
    pub source_basis: SemanticSourceBasis,
    /// Actual current lifecycle, independent from result filtering.
    pub lifecycle: SemanticLifecycleClass,
    /// Exact current semantic head used to build Qi.
    pub semantic_head: SemanticHeadProvenance,
}

/// Closed reason a context Coordinate did not produce Qi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OmittedContextCoordinateReason {
    /// Canonical source does not exist.
    SourceNotFound,
    /// Canonical source is ineligible.
    SourceIneligible,
    /// No current-generation head exists.
    SemanticHeadMissing,
    /// Current-generation head is being built.
    SemanticHeadBuilding,
    /// Current-generation head failed.
    SemanticHeadFailed,
    /// Canonical problem-plus-overview exceeds Provider input limits.
    ConditionedInputUnsupported,
}

/// Context Coordinate omitted before Provider egress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OmittedContextCoordinateObservation {
    /// Omitted context Coordinate.
    pub coordinate: ProjectContextCoordinate,
    /// Closed omission reason; authorization failures are whole-request errors.
    pub reason: OmittedContextCoordinateReason,
}

/// Closed observations of every caller-supplied initial and context Coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticGraphQueryInputObservations {
    /// Current in-graph explicit roots.
    pub accepted_initial_coordinates: Vec<AcceptedInitialCoordinateObservation>,
    /// Current Coordinates that are not members of any active Edge.
    pub initial_not_in_graph: Vec<ProjectContextCoordinate>,
    /// In-graph Coordinates whose canonical source cannot be used.
    pub omitted_initial_coordinates: Vec<OmittedInitialCoordinateObservation>,
    /// Current context lenses that produced Qi.
    pub accepted_context_coordinates: Vec<AcceptedContextCoordinateObservation>,
    /// Context lenses omitted for closed source-local reasons.
    pub omitted_context_coordinates: Vec<OmittedContextCoordinateObservation>,
}

/// Source-owned preview copied into the bounded result.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticSourcePreview {
    /// Current canonical title or name.
    pub title: String,
    /// Complete source-owned summary when it fits the response budget.
    pub summary: Option<String>,
    /// Why an existing summary was omitted.
    pub summary_omitted_reason: Option<SummaryOmittedReason>,
}

impl std::fmt::Debug for SemanticSourcePreview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticSourcePreview")
            .field("title", &"<redacted>")
            .field("title_bytes", &self.title.len())
            .field("summary", &self.summary.as_ref().map(|_| "<redacted>"))
            .field("summary_bytes", &self.summary.as_ref().map(String::len))
            .field("summary_omitted_reason", &self.summary_omitted_reason)
            .finish()
    }
}

/// Closed reason a canonical summary was not copied into the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryOmittedReason {
    /// The complete summary did not fit the response budget.
    ResponseBudget,
}

/// Canonical source currentness attached to every hydrated semantic item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalSourceProvenance {
    /// Typed source basis rather than an invented common revision.
    pub source_basis: SemanticSourceBasis,
    /// Current source invalidation epoch.
    pub source_invalidation_epoch: u64,
    /// Exact canonical source snapshot digest.
    pub source_snapshot_digest: Digest32,
    /// Whether overview semantic text includes a summary.
    pub summary_coverage: SemanticCoverage,
}

/// Current-generation unit and embedding provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticProvenance {
    /// Active generation identity.
    pub generation_id: Uuid,
    /// Stable unit key.
    pub unit_key: String,
    /// Source snapshot represented by the unit.
    pub source_snapshot_digest: Digest32,
    /// Complete source generation contract digest.
    pub source_generation_contract_digest: Digest32,
    /// Comparable model-space fence.
    pub embedding_space_fence: Digest32,
}

/// Exact current Edge row provenance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextEdgeProvenance {
    /// Last Project Context revision that changed this Edge.
    pub last_context_revision: u64,
    /// Canonical source change identity that produced the current Edge row.
    pub source_change_id: Digest32,
}

/// Exact current Context Document binding provenance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextBindingProvenance {
    /// Project Context revision that produced this binding row.
    pub binding_context_revision: u64,
    /// Canonical source change identity that produced the binding.
    pub source_change_id: Digest32,
    /// Current relay-signed binding projection Event identity.
    pub projection_event_id: Digest32,
}

/// One current Context Document membership on a returned complete Hyperedge.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextDocumentBindingObservation {
    /// Project Document identity.
    pub document_id: Uuid,
    /// Exact current binding provenance.
    pub provenance: ProjectContextBindingProvenance,
}

/// Orthogonal discovery evidence for one root source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "channel", rename_all = "snake_case", deny_unknown_fields)]
pub enum RootDiscoveryChannel {
    /// Caller supplied the Coordinate as an explicit initial root.
    ExplicitInitial,
    /// Source was discovered by problem-only Q0.
    ProblemNeutral,
    /// Source was discovered by one context-conditioned Qi.
    ContextConditioned {
        /// Context Coordinate that produced the discovery branch.
        context_coordinate: ProjectContextCoordinate,
    },
}

/// One role-specific graph entrypoint through which a root may expand.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case", deny_unknown_fields)]
pub enum RootStructuralEntrypoint {
    /// Coordinate incident-Edge expansion.
    Coordinate {
        /// Exact Coordinate represented by this source role.
        coordinate: ProjectContextCoordinate,
    },
    /// Context Document expansion through its one current binding.
    ContextDocument {
        /// Bound exact Hyperedge.
        edge_key: EdgeKey,
        /// Context Document identity.
        document_id: Uuid,
        /// Exact Edge row provenance.
        edge_provenance: ProjectContextEdgeProvenance,
        /// Exact binding row provenance.
        binding_provenance: ProjectContextBindingProvenance,
    },
}

/// Role used to recompute one structured semantic score explanation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticScoreRole {
    /// Automatic or embedded explicit Coordinate candidate.
    Candidate,
    /// Context Document selected directly as a root.
    RelationRoot,
    /// Relation Document reached from a Coordinate.
    RelationDocument,
    /// Target Coordinate reached through a relation Document.
    TargetCoordinate,
    /// Complete relation-document/target transition.
    Transition,
}

/// Closed score penalty kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScorePenaltyKind {
    /// Length penalty applied once per complete hop.
    Hop,
}

/// One exact fixed-point score penalty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScorePenalty {
    /// Closed penalty kind.
    pub kind: ScorePenaltyKind,
    /// Fixed-point amount subtracted.
    pub amount: Score,
}

/// Structured source/transition score explanation sufficient for exact
/// fixed-point recomputation without raw vectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoreExplanation {
    /// Role selecting the frozen recomputation formula.
    pub score_role: SemanticScoreRole,
    /// Q0 normalized similarity.
    pub problem_score: Score,
    /// All independently attributable Qi evidence.
    pub conditioned_evidence: Vec<ConditionedEvidence>,
    /// Strongest deduplicated weighted gain.
    pub highest_gain: Score,
    /// Second-strongest deduplicated weighted gain.
    pub second_highest_gain: Score,
    /// Frozen bounded environment gain.
    pub environment_gain: Score,
    /// Closed explicit-initial anchor contribution.
    pub anchor_gain: AnchorGain,
    /// Optional local or relation-document coherence.
    pub local_coherence: Option<Score>,
    /// Relation Document score when applicable.
    pub document_score: Option<Score>,
    /// Target Coordinate score when applicable.
    pub target_coordinate_score: Option<Score>,
    /// Harmonic transition score when applicable.
    pub transition_score: Option<Score>,
    /// Closed penalties applied by this formula.
    pub penalties: Vec<ScorePenalty>,
    /// Final fixed-point result.
    pub final_score: Score,
}

impl ScoreExplanation {
    /// Verify the final score against the role-selected fixed-point formula.
    pub fn validate(&self) -> QueryContractResult<()> {
        if self.conditioned_evidence.iter().any(|evidence| {
            *evidence
                != ConditionedEvidence::new(
                    evidence.context_coordinate.clone(),
                    self.problem_score,
                    evidence.conditioned_score,
                )
        }) {
            return Err(SemanticGraphQueryError::InvalidState(
                "conditioned score evidence is internally inconsistent".to_owned(),
            ));
        }
        let environment = environment_gain(&self.conditioned_evidence);
        if self.conditioned_evidence != environment.conditioned_evidence
            || self.highest_gain != environment.highest_gain
            || self.second_highest_gain != environment.second_highest_gain
            || self.environment_gain != environment.environment_gain
            || !self.penalties.is_empty()
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "environment score evidence or penalties are inconsistent".to_owned(),
            ));
        }
        let expected = match self.score_role {
            SemanticScoreRole::Candidate | SemanticScoreRole::RelationRoot => {
                candidate_score(self.problem_score, self.environment_gain, self.anchor_gain)
            }
            SemanticScoreRole::RelationDocument => document_score(
                self.problem_score,
                self.environment_gain,
                self.local_coherence,
            ),
            SemanticScoreRole::TargetCoordinate => target_coordinate_score(
                self.problem_score,
                self.environment_gain,
                self.local_coherence.ok_or_else(|| {
                    SemanticGraphQueryError::InvalidState(
                        "target score explanation lacks relation coherence".to_owned(),
                    )
                })?,
            ),
            SemanticScoreRole::Transition => harmonic_score(
                self.document_score.ok_or_else(|| {
                    SemanticGraphQueryError::InvalidState(
                        "transition explanation lacks document score".to_owned(),
                    )
                })?,
                self.target_coordinate_score.ok_or_else(|| {
                    SemanticGraphQueryError::InvalidState(
                        "transition explanation lacks target score".to_owned(),
                    )
                })?,
            ),
        };
        if expected != self.final_score {
            return Err(SemanticGraphQueryError::InvalidState(
                "score explanation does not recompute final score".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Outcome of expanding one root structural entrypoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedOutcome {
    /// Entrypoint whose expansion is accounted for.
    pub structural_entrypoint: RootStructuralEntrypoint,
    /// Search-produced paths before max-path and response packing.
    pub produced_path_count: u32,
    /// Exact zero-hop stop reason when no path was produced.
    pub zero_hop_stop_reason: Option<BranchStopReason>,
}

/// One deduplicated semantic root source with every eligible graph entrypoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticRoot {
    /// Deterministic project/source/entrypoint identity.
    pub root_id: Digest32,
    /// All retained discovery evidence.
    pub discovery_channels: Vec<RootDiscoveryChannel>,
    /// All role-specific eligible structural entrypoints.
    pub structural_entrypoints: Vec<RootStructuralEntrypoint>,
    /// Canonical source identity, deduplicated across structural roles.
    pub source: SemanticSourceIdentity,
    /// Current source-owned preview.
    pub preview: SemanticSourcePreview,
    /// Actual source lifecycle.
    pub lifecycle: SemanticLifecycleClass,
    /// Optional source-native status.
    pub source_status: Option<String>,
    /// Current canonical source provenance.
    pub canonical_provenance: CanonicalSourceProvenance,
    /// Current semantic provenance, absent for embedding-less explicit roots.
    pub semantic_provenance: Option<SemanticProvenance>,
    /// Root score, absent for embedding-less explicit roots.
    pub semantic_score: Option<Score>,
    /// Exact score explanation when a semantic score exists.
    pub score_explanation: Option<ScoreExplanation>,
    /// One outcome per structural entrypoint.
    pub seed_outcomes: Vec<SeedOutcome>,
}

/// Complete canonical Edge identity and current Context Document membership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticEdgeObservation {
    /// Deterministic Edge identity.
    pub edge_key: EdgeKey,
    /// Complete canonical Coordinate set; never lifecycle-filtered.
    pub complete_coordinates: Vec<ProjectContextCoordinate>,
    /// Exact current Edge row provenance.
    pub provenance: ProjectContextEdgeProvenance,
    /// All current bindings sorted by Document UUID.
    pub current_context_document_bindings: Vec<ContextDocumentBindingObservation>,
}

/// Selected relation Document materialized in one hop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticRelationDocument {
    /// Project Document identity.
    pub document_id: Uuid,
    /// Exact selected binding provenance.
    pub binding_provenance: ProjectContextBindingProvenance,
    /// Current source-owned preview.
    pub preview: SemanticSourcePreview,
    /// Current canonical source provenance.
    pub canonical_provenance: CanonicalSourceProvenance,
    /// Current semantic provenance.
    pub semantic_provenance: SemanticProvenance,
    /// Relation Document or relation-root score.
    pub document_score: Score,
    /// Exact score explanation.
    pub score_explanation: ScoreExplanation,
}

/// Continued target Coordinate materialized in one hop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticContinuedCoordinate {
    /// Target Coordinate identity.
    pub coordinate: ProjectContextCoordinate,
    /// Current source-owned preview.
    pub preview: SemanticSourcePreview,
    /// Actual current source lifecycle used by the target selector.
    pub lifecycle: SemanticLifecycleClass,
    /// Current canonical source provenance.
    pub canonical_provenance: CanonicalSourceProvenance,
    /// Current semantic provenance.
    pub semantic_provenance: SemanticProvenance,
    /// Target score conditioned by the selected relation Document.
    pub target_score: Score,
    /// Exact score explanation.
    pub score_explanation: ScoreExplanation,
}

/// One complete, atomic Hyperedge traversal hop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticHyperedgeHop {
    /// One-based path-local hop ordinal.
    pub ordinal: u16,
    /// Entered Coordinate, absent only for a relation-Document root seed.
    pub entered_from_coordinate: Option<ProjectContextCoordinate>,
    /// Complete Hyperedge identity and all current bindings.
    pub edge: SemanticEdgeObservation,
    /// Selected Context Document relation material.
    pub selected_relation_document: SemanticRelationDocument,
    /// Continued target Coordinate.
    pub continued_to_coordinate: SemanticContinuedCoordinate,
    /// Zero-absorbing harmonic transition score.
    pub transition_score: Score,
}

/// Closed reason a returned leaf or truncated prefix stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchStopReason {
    /// All current authorized outgoing options were inspected.
    FrontierExhausted,
    /// Scorable options existed but none passed fixed floors.
    BelowRelevanceThreshold,
    /// Qualifying options all repeated a path Coordinate or Edge.
    CycleOrDuplicate,
    /// Hop cap was reached without inspecting the next frontier.
    MaxHopsReached,
    /// A complete Hyperedge identity exceeds its fixed contract limit.
    HyperedgeTooLarge,
    /// A global logical materialization budget stopped expansion.
    GlobalBudgetExhausted,
    /// The shared work deadline stopped expansion.
    WallTimeExhausted,
}

impl BranchStopReason {
    /// Closed precedence used when multiple stop conditions are observed.
    pub const fn precedence(self) -> u8 {
        match self {
            Self::WallTimeExhausted => 0,
            Self::GlobalBudgetExhausted => 1,
            Self::HyperedgeTooLarge => 2,
            Self::MaxHopsReached => 3,
            Self::CycleOrDuplicate => 4,
            Self::BelowRelevanceThreshold => 5,
            Self::FrontierExhausted => 6,
        }
    }
}

/// One stopped retrieval-forest path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticPath {
    /// Deterministic root plus complete ordered-hop provenance identity.
    pub path_id: Digest32,
    /// Owning root identity.
    pub root_id: Digest32,
    /// Complete ordered hops; intermediate prefixes are not emitted.
    pub hops: Vec<SemanticHyperedgeHop>,
    /// Coordinate reached by the final hop.
    pub terminal_coordinate: ProjectContextCoordinate,
    /// Final length-neutral path score.
    pub path_score: Score,
    /// Exact fixed-point recomputation components.
    pub path_score_explanation: PathScoreExplanation,
    /// Why this returned leaf or truncated prefix stopped.
    pub branch_stop_reason: BranchStopReason,
}

/// Closed overall successful completion reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionReason {
    /// Current bounded frontier was fully processed.
    FrontierExhausted,
    /// Eligible logical work was suppressed by at least one budget dimension.
    BudgetExhausted,
    /// Work deadline was reached with response-tail time remaining.
    WallTimeExhausted,
}

/// Closed budget dimensions in their canonical reporting order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExhaustedDimension {
    /// Per-channel recall suppressed a K+1 source.
    RecallPerChannel,
    /// Automatic root selection suppressed a qualifying source.
    SemanticRoots,
    /// Hop cap deliberately left a frontier uninspected.
    HopsPerPath,
    /// Per-state successor accumulator observed B+1.
    BeamWidth,
    /// A provenance-distinct Coordinate expansion was suppressed.
    ExpandedCoordinates,
    /// A new complete Edge materialization was suppressed.
    IncidentEdgesMaterialized,
    /// A new relation option was suppressed.
    RelationOptionsMaterialized,
    /// A new target option was suppressed.
    TargetOptionsMaterialized,
    /// A returnable N+1 path was suppressed.
    Paths,
    /// Result packing omitted bounded content.
    ResponseBytes,
}

/// Mutually exclusive semantic embedding coverage counts.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingCoverageCounts {
    /// Exact current-generation queryable heads.
    pub current: u64,
    /// No exact current head or current-epoch job observation.
    pub missing: u64,
    /// Current-epoch pending, claimed, or retry jobs.
    pub building: u64,
    /// Current-epoch poison or succeeded-without-complete-head observations.
    pub failed: u64,
    /// Closed source/generation incompatibility observations.
    pub unsupported: u64,
    /// Otherwise matching embeddings whose norm is zero.
    pub non_queryable_zero_vector: u64,
}

/// Fixed omission counts for requested context branches.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OmittedContextChannelCounts {
    /// Missing canonical source.
    pub source_not_found: u64,
    /// Ineligible canonical source.
    pub source_ineligible: u64,
    /// Missing exact current semantic head.
    pub semantic_head_missing: u64,
    /// Current semantic head is being built.
    pub semantic_head_building: u64,
    /// Current semantic head failed.
    pub semantic_head_failed: u64,
    /// Canonical Qi input exceeds Provider limits.
    pub conditioned_input_unsupported: u64,
}

/// Items omitted only while fitting the final response byte budget.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OmittedForResponseBudgetCounts {
    /// Automatic roots omitted from serialization.
    pub automatic_roots: u64,
    /// Whole paths omitted from serialization.
    pub paths: u64,
    /// Whole summaries omitted while retaining their source shell.
    pub summaries: u64,
}

/// Fixed truncation counts by every logical budget dimension.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TruncationCountsByDimension {
    /// Suppressed recall sources.
    pub recall_per_channel: u64,
    /// Suppressed automatic roots.
    pub semantic_roots: u64,
    /// Paths whose next frontier was intentionally uninspected.
    pub hops_per_path: u64,
    /// Suppressed per-state successors.
    pub beam_width: u64,
    /// Suppressed Coordinate expansions.
    pub expanded_coordinates: u64,
    /// Suppressed complete Edge materializations.
    pub incident_edges_materialized: u64,
    /// Suppressed relation options.
    pub relation_options_materialized: u64,
    /// Suppressed target options.
    pub target_options_materialized: u64,
    /// Suppressed returnable paths.
    pub paths: u64,
    /// Byte-budget omissions.
    pub response_bytes: u64,
}

/// One bounded diagnostic sample of budget truncation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TruncationSample {
    /// Owning root identity.
    pub root_id: Digest32,
    /// Optional owning path identity.
    pub path_id: Option<Digest32>,
    /// Structural seed involved in truncation.
    pub structural_entrypoint: RootStructuralEntrypoint,
    /// Exhausted logical budget dimension.
    pub dimension: ExhaustedDimension,
}

/// Fixed degraded-mode counters orthogonal to completion reason.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DegradedModeCounts {
    /// Incident relation Documents lacking usable current embedding.
    pub relation_embedding_missing: u64,
    /// Edge target Coordinates lacking usable current embedding.
    pub target_embedding_missing: u64,
    /// Authorized graph sources not fully covered by the active generation.
    pub index_coverage_partial: u64,
    /// Existing summaries omitted for response budget.
    pub summary_omitted_for_response_budget: u64,
    /// Complete Hyperedge identities exceeding the fixed 64 KiB bound.
    pub hyperedge_too_large: u64,
}

/// Bounded, permission-scoped coverage and work accounting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticGraphQueryCoverage {
    /// Unique authorized graph source identities with an eligible entrypoint.
    pub authorized_graph_sources: u64,
    /// Authorized sources with exact current queryable heads.
    pub current_indexed_graph_sources: u64,
    /// Current indexed sources whose overview is title-only.
    pub title_only_sources: u64,
    /// Mutually exclusive active-generation embedding coverage.
    pub embedding_coverage: EmbeddingCoverageCounts,
    /// Q0 plus all requested context Coordinates.
    pub query_channels_requested: u64,
    /// Q0 plus every Qi actually sent to the Provider.
    pub query_channels_executed: u64,
    /// Fixed counts for omitted Qi branches.
    pub omitted_context_channel_counts_by_reason: OmittedContextChannelCounts,
    /// Qualifying problem-neutral candidates inspected by root selection.
    pub neutral_candidates_considered: u64,
    /// Qualifying conditioned candidates inspected by root selection.
    pub conditioned_candidates_considered: u64,
    /// Roots selected before response packing.
    pub roots_selected: u64,
    /// Roots serialized after response packing.
    pub roots_returned: u64,
    /// Provenance-distinct Coordinate incident expansions begun.
    pub expanded_coordinates: u64,
    /// Globally unique complete Edge identities materialized.
    pub incident_edges_materialized: u64,
    /// Unique relation options materialized.
    pub relation_options_materialized: u64,
    /// Unique target options materialized.
    pub target_options_materialized: u64,
    /// Search-produced stopped paths before top-N retention.
    pub paths_generated: u64,
    /// Paths retained after max-path top-N.
    pub paths_retained: u64,
    /// Paths serialized after response packing.
    pub paths_returned: u64,
    /// Byte-budget omissions.
    pub omitted_for_response_budget: OmittedForResponseBudgetCounts,
    /// Complete truncation counts by dimension.
    pub truncation_counts_by_dimension: TruncationCountsByDimension,
    /// Canonically sorted diagnostic samples, capped at 32.
    pub truncation_samples: Vec<TruncationSample>,
    /// Degraded modes that do not redefine completion.
    pub degraded_mode_counts: DegradedModeCounts,
}

impl SemanticGraphQueryCoverage {
    /// Validate mutually exclusive coverage and generated/retained/returned
    /// monotonicity invariants.
    pub fn validate(&self) -> QueryContractResult<()> {
        let classified = self
            .embedding_coverage
            .current
            .saturating_add(self.embedding_coverage.missing)
            .saturating_add(self.embedding_coverage.building)
            .saturating_add(self.embedding_coverage.failed)
            .saturating_add(self.embedding_coverage.unsupported)
            .saturating_add(self.embedding_coverage.non_queryable_zero_vector);
        if classified != self.authorized_graph_sources
            || self.embedding_coverage.current != self.current_indexed_graph_sources
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "embedding coverage does not partition authorized graph sources".to_owned(),
            ));
        }
        if self.title_only_sources > self.current_indexed_graph_sources
            || self.paths_generated < self.paths_retained
            || self.paths_retained < self.paths_returned
            || self.roots_selected < self.roots_returned
            || self.query_channels_requested < self.query_channels_executed
            || self.truncation_samples.len() > MAX_TRUNCATION_SAMPLES
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "semantic query coverage counters violate monotonic bounds".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Complete unversioned closed semantic graph query result.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticGraphQueryResult {
    /// Request correlation identity.
    pub request_id: Uuid,
    /// Host-derived Project identity.
    pub project_id: Uuid,
    /// Authenticated exact-request binding digest.
    pub request_binding_digest: Digest32,
    /// Generation, graph, ranking, and budget observations.
    pub observations: SemanticGraphQueryObservations,
    /// Closed observations for every caller-supplied Coordinate.
    pub input_observations: SemanticGraphQueryInputObservations,
    /// Bounded roots retained by response packing.
    pub roots: Vec<SemanticRoot>,
    /// Bounded stopped retrieval paths retained by response packing.
    pub paths: Vec<SemanticPath>,
    /// Permission-scoped coverage and truncation accounting.
    pub coverage: SemanticGraphQueryCoverage,
    /// Overall successful completion reason.
    pub completion_reason: CompletionReason,
    /// Canonically sorted actually exhausted budget dimensions.
    pub exhausted_dimensions: Vec<ExhaustedDimension>,
}

impl std::fmt::Debug for SemanticGraphQueryResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticGraphQueryResult")
            .field("root_count", &self.roots.len())
            .field("path_count", &self.paths.len())
            .field("completion_reason", &self.completion_reason)
            .field("exhausted_dimensions", &self.exhausted_dimensions)
            .finish_non_exhaustive()
    }
}

impl SemanticGraphQueryResult {
    /// Validate cross-object identities, score recomputation, path integrity,
    /// coverage accounting, and completion semantics before signing.
    pub fn validate(&self) -> QueryContractResult<()> {
        self.coverage.validate()?;
        if self
            .exhausted_dimensions
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "exhausted dimensions must be strictly canonical".to_owned(),
            ));
        }
        match self.completion_reason {
            CompletionReason::BudgetExhausted if self.exhausted_dimensions.is_empty() => {
                return Err(SemanticGraphQueryError::InvalidState(
                    "budget exhaustion requires an exhausted dimension".to_owned(),
                ));
            }
            CompletionReason::FrontierExhausted | CompletionReason::WallTimeExhausted
                if !self.exhausted_dimensions.is_empty() =>
            {
                return Err(SemanticGraphQueryError::InvalidState(
                    "non-budget completion must not report exhausted dimensions".to_owned(),
                ));
            }
            _ => {}
        }

        let mut roots_by_id = BTreeMap::new();
        for root in &self.roots {
            root.validate(self.project_id, &self.observations)?;
            if roots_by_id.insert(root.root_id, root).is_some() {
                return Err(SemanticGraphQueryError::InvalidState(
                    "duplicate returned root id".to_owned(),
                ));
            }
        }
        let mut path_ids = BTreeSet::new();
        for path in &self.paths {
            let root = roots_by_id.get(&path.root_id).ok_or_else(|| {
                SemanticGraphQueryError::InvalidState("path references a missing root".to_owned())
            })?;
            path.validate(self.project_id, root, &self.observations)?;
            if !path_ids.insert(path.path_id) {
                return Err(SemanticGraphQueryError::InvalidState(
                    "duplicate path id".to_owned(),
                ));
            }
        }
        if self.coverage.roots_returned != self.roots.len() as u64
            || self.coverage.paths_returned != self.paths.len() as u64
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "returned root/path coverage does not match payload".to_owned(),
            ));
        }
        Ok(())
    }

    /// Validate this result against the exact canonical caller request.
    ///
    /// This is the request-aware half of result verification. It binds the
    /// compiled query/ranking/budget contracts, requires the input
    /// observations to partition the caller's Coordinate arrays exactly, and
    /// prevents a correctly signed result from claiming channels, roots,
    /// paths, lifecycle eligibility, or work beyond the caller's budget.
    pub fn validate_for_request(&self, request: &SemanticGraphQuery) -> QueryContractResult<()> {
        let request = request.clone().validate_and_canonicalize()?;
        self.validate()?;

        if self.request_id != request.request_id || self.project_id != request.project_id {
            return Err(SemanticGraphQueryError::InvalidState(
                "semantic result does not belong to the canonical request".to_owned(),
            ));
        }
        if self.observations.query_contract_digest != query_contract_digest()
            || self.observations.ranking_contract_digest != ranking_contract_digest()?
            || self.observations.budget_profile_digest != budget_profile_digest()?
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "semantic result compiled contract digests do not match this verifier".to_owned(),
            ));
        }

        self.validate_input_observations_for_request(&request)?;
        self.validate_query_channels_for_request(&request)?;
        self.validate_root_evidence_for_request(&request)?;
        self.validate_budget_for_request(&request)
    }

    fn validate_input_observations_for_request(
        &self,
        request: &SemanticGraphQuery,
    ) -> QueryContractResult<()> {
        let input = &self.input_observations;
        validate_coordinate_order(
            input
                .accepted_initial_coordinates
                .iter()
                .map(|observation| &observation.coordinate),
            "accepted initial Coordinate observations",
        )?;
        validate_coordinate_order(
            input.initial_not_in_graph.iter(),
            "not-in-graph initial Coordinate observations",
        )?;
        validate_coordinate_order(
            input
                .omitted_initial_coordinates
                .iter()
                .map(|observation| &observation.coordinate),
            "omitted initial Coordinate observations",
        )?;
        validate_coordinate_order(
            input
                .accepted_context_coordinates
                .iter()
                .map(|observation| &observation.coordinate),
            "accepted context Coordinate observations",
        )?;
        validate_coordinate_order(
            input
                .omitted_context_coordinates
                .iter()
                .map(|observation| &observation.coordinate),
            "omitted context Coordinate observations",
        )?;

        let mut observed_initial = BTreeSet::new();
        for observation in &input.accepted_initial_coordinates {
            require_unique_coordinate(&mut observed_initial, &observation.coordinate, "initial")?;
            if !basis_matches_coordinate(&observation.source_basis, &observation.coordinate) {
                return Err(SemanticGraphQueryError::InvalidState(
                    "accepted initial source basis does not match its Coordinate".to_owned(),
                ));
            }
            validate_graph_membership(
                &observation.graph_membership,
                self.observations.project_context_revision,
            )?;
            if let SemanticHeadState::Current(head) = &observation.semantic_state {
                validate_input_head(head, self.observations.semantic_generation_id)?;
            }
        }
        for coordinate in &input.initial_not_in_graph {
            require_unique_coordinate(&mut observed_initial, coordinate, "initial")?;
        }
        for observation in &input.omitted_initial_coordinates {
            require_unique_coordinate(&mut observed_initial, &observation.coordinate, "initial")?;
            validate_graph_membership(
                &observation.graph_membership,
                self.observations.project_context_revision,
            )?;
        }
        let expected_initial = request
            .initial_coordinates
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if observed_initial != expected_initial {
            return Err(SemanticGraphQueryError::InvalidState(
                "initial Coordinate observations do not exactly partition the request".to_owned(),
            ));
        }

        let mut observed_context = BTreeSet::new();
        for observation in &input.accepted_context_coordinates {
            require_unique_coordinate(&mut observed_context, &observation.coordinate, "context")?;
            if !basis_matches_coordinate(&observation.source_basis, &observation.coordinate) {
                return Err(SemanticGraphQueryError::InvalidState(
                    "accepted context source basis does not match its Coordinate".to_owned(),
                ));
            }
            validate_input_head(
                &observation.semantic_head,
                self.observations.semantic_generation_id,
            )?;
            if matches!(
                observation.lifecycle,
                SemanticLifecycleClass::Tombstone | SemanticLifecycleClass::Deleted
            ) {
                return Err(SemanticGraphQueryError::InvalidState(
                    "ineligible lifecycle cannot be an accepted context Coordinate".to_owned(),
                ));
            }
        }
        for observation in &input.omitted_context_coordinates {
            require_unique_coordinate(&mut observed_context, &observation.coordinate, "context")?;
        }
        let expected_context = request
            .context_coordinates
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if observed_context != expected_context {
            return Err(SemanticGraphQueryError::InvalidState(
                "context Coordinate observations do not exactly partition the request".to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_query_channels_for_request(
        &self,
        request: &SemanticGraphQuery,
    ) -> QueryContractResult<()> {
        let requested = 1_u64
            .checked_add(request.context_coordinates.len() as u64)
            .ok_or(SemanticGraphQueryError::Serialization)?;
        let executed = 1_u64
            .checked_add(self.input_observations.accepted_context_coordinates.len() as u64)
            .ok_or(SemanticGraphQueryError::Serialization)?;
        if self.coverage.query_channels_requested != requested
            || self.coverage.query_channels_executed != executed
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "semantic query channel counts do not match the request observations".to_owned(),
            ));
        }

        let mut omitted = OmittedContextChannelCounts::default();
        for observation in &self.input_observations.omitted_context_coordinates {
            match observation.reason {
                OmittedContextCoordinateReason::SourceNotFound => omitted.source_not_found += 1,
                OmittedContextCoordinateReason::SourceIneligible => {
                    omitted.source_ineligible += 1;
                }
                OmittedContextCoordinateReason::SemanticHeadMissing => {
                    omitted.semantic_head_missing += 1;
                }
                OmittedContextCoordinateReason::SemanticHeadBuilding => {
                    omitted.semantic_head_building += 1;
                }
                OmittedContextCoordinateReason::SemanticHeadFailed => {
                    omitted.semantic_head_failed += 1;
                }
                OmittedContextCoordinateReason::ConditionedInputUnsupported => {
                    omitted.conditioned_input_unsupported += 1;
                }
            }
        }
        if self.coverage.omitted_context_channel_counts_by_reason != omitted {
            return Err(SemanticGraphQueryError::InvalidState(
                "omitted context channel counts do not match input observations".to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_root_evidence_for_request(
        &self,
        request: &SemanticGraphQuery,
    ) -> QueryContractResult<()> {
        let accepted_initial = self
            .input_observations
            .accepted_initial_coordinates
            .iter()
            .map(|observation| observation.coordinate.clone())
            .collect::<BTreeSet<_>>();
        let accepted_context = self
            .input_observations
            .accepted_context_coordinates
            .iter()
            .map(|observation| observation.coordinate.clone())
            .collect::<BTreeSet<_>>();

        for root in &self.roots {
            if matches!(
                root.lifecycle,
                SemanticLifecycleClass::Tombstone | SemanticLifecycleClass::Deleted
            ) {
                return Err(SemanticGraphQueryError::InvalidState(
                    "ineligible lifecycle cannot be returned as a semantic root".to_owned(),
                ));
            }
            if root.discovery_channels.is_empty()
                || root
                    .discovery_channels
                    .windows(2)
                    .any(|pair| compare_discovery_channels(&pair[0], &pair[1]).is_ge())
            {
                return Err(SemanticGraphQueryError::InvalidState(
                    "root discovery channels must be non-empty, unique, and canonical".to_owned(),
                ));
            }

            let is_explicit = root
                .discovery_channels
                .contains(&RootDiscoveryChannel::ExplicitInitial);
            let has_semantic_discovery = root.discovery_channels.iter().any(|channel| {
                matches!(
                    channel,
                    RootDiscoveryChannel::ProblemNeutral
                        | RootDiscoveryChannel::ContextConditioned { .. }
                )
            });
            if has_semantic_discovery && root.semantic_score.is_none() {
                return Err(SemanticGraphQueryError::InvalidState(
                    "semantic root discovery lacks score and provenance".to_owned(),
                ));
            }
            let matching_explicit_entrypoints = root
                .structural_entrypoints
                .iter()
                .filter_map(|entrypoint| match entrypoint {
                    RootStructuralEntrypoint::Coordinate { coordinate }
                        if accepted_initial.contains(coordinate) =>
                    {
                        Some(coordinate)
                    }
                    _ => None,
                })
                .count();
            if is_explicit != (matching_explicit_entrypoints == 1) {
                return Err(SemanticGraphQueryError::InvalidState(
                    "explicit root discovery does not match one accepted initial Coordinate"
                        .to_owned(),
                ));
            }

            for channel in &root.discovery_channels {
                if let RootDiscoveryChannel::ContextConditioned { context_coordinate } = channel {
                    if !accepted_context.contains(context_coordinate) {
                        return Err(SemanticGraphQueryError::InvalidState(
                            "conditioned root discovery references an unaccepted context Coordinate"
                                .to_owned(),
                        ));
                    }
                }
            }
            if !is_explicit
                && root.structural_entrypoints.iter().any(|entrypoint| {
                    matches!(entrypoint, RootStructuralEntrypoint::Coordinate { .. })
                })
                && !lifecycle_matches(request.lifecycle_filter, root.lifecycle)
            {
                return Err(SemanticGraphQueryError::InvalidState(
                    "automatic Coordinate root violates the requested lifecycle filter".to_owned(),
                ));
            }

            if root.score_explanation.as_ref().is_some_and(|explanation| {
                explanation
                    .conditioned_evidence
                    .iter()
                    .any(|evidence| !accepted_context.contains(&evidence.context_coordinate))
            }) {
                return Err(SemanticGraphQueryError::InvalidState(
                    "root score evidence references an unaccepted context Coordinate".to_owned(),
                ));
            }
        }

        for coordinate in &accepted_initial {
            let matching_roots = self
                .roots
                .iter()
                .filter(|root| {
                    root.discovery_channels
                        .contains(&RootDiscoveryChannel::ExplicitInitial)
                        && root.structural_entrypoints.iter().any(|entrypoint| {
                            matches!(
                                entrypoint,
                                RootStructuralEntrypoint::Coordinate {
                                    coordinate: root_coordinate
                                } if root_coordinate == coordinate
                            )
                        })
                })
                .collect::<Vec<_>>();
            if matching_roots.len() != 1 {
                return Err(SemanticGraphQueryError::InvalidState(
                    "accepted initial Coordinate does not have exactly one returned explicit root"
                        .to_owned(),
                ));
            }
            let observation = self
                .input_observations
                .accepted_initial_coordinates
                .iter()
                .find(|observation| &observation.coordinate == coordinate)
                .ok_or_else(|| {
                    SemanticGraphQueryError::InvalidState(
                        "explicit root lacks its accepted initial observation".to_owned(),
                    )
                })?;
            validate_explicit_root_provenance(observation, matching_roots[0])?;
        }

        for path in &self.paths {
            for hop in &path.hops {
                if !lifecycle_matches(
                    request.lifecycle_filter,
                    hop.continued_to_coordinate.lifecycle,
                ) {
                    return Err(SemanticGraphQueryError::InvalidState(
                        "continued target Coordinate violates the requested lifecycle filter"
                            .to_owned(),
                    ));
                }
                for explanation in [
                    &hop.selected_relation_document.score_explanation,
                    &hop.continued_to_coordinate.score_explanation,
                ] {
                    if explanation
                        .conditioned_evidence
                        .iter()
                        .any(|evidence| !accepted_context.contains(&evidence.context_coordinate))
                    {
                        return Err(SemanticGraphQueryError::InvalidState(
                            "path score evidence references an unaccepted context Coordinate"
                                .to_owned(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_budget_for_request(&self, request: &SemanticGraphQuery) -> QueryContractResult<()> {
        let budget = request.budget;
        let explicit_roots = self
            .roots
            .iter()
            .filter(|root| {
                root.discovery_channels
                    .contains(&RootDiscoveryChannel::ExplicitInitial)
            })
            .count() as u64;
        let automatic_roots = self.roots.len() as u64 - explicit_roots;
        let maximum_selected_roots = self.input_observations.accepted_initial_coordinates.len()
            as u64
            + u64::from(budget.max_semantic_roots);
        let maximum_conditioned_candidates =
            self.input_observations.accepted_context_coordinates.len() as u64
                * u64::from(budget.max_recall_per_channel);

        if automatic_roots > u64::from(budget.max_semantic_roots)
            || self.coverage.roots_selected > maximum_selected_roots
            || self.coverage.neutral_candidates_considered
                > u64::from(budget.max_recall_per_channel)
            || self.coverage.conditioned_candidates_considered > maximum_conditioned_candidates
            || self.coverage.expanded_coordinates > u64::from(budget.max_expanded_coordinates)
            || self.coverage.incident_edges_materialized
                > u64::from(budget.max_incident_edges_materialized)
            || self.coverage.relation_options_materialized
                > u64::from(budget.max_relation_options_materialized)
            || self.coverage.target_options_materialized
                > u64::from(budget.max_target_options_materialized)
            || self.coverage.paths_retained > u64::from(budget.max_paths)
            || self.paths.len() > usize::from(budget.max_paths)
            || self
                .paths
                .iter()
                .any(|path| path.hops.len() > usize::from(budget.max_hops_per_path))
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "semantic result exceeds a caller-requested work budget".to_owned(),
            ));
        }
        Ok(())
    }
}

fn validate_coordinate_order<'a>(
    coordinates: impl IntoIterator<Item = &'a ProjectContextCoordinate>,
    field: &'static str,
) -> QueryContractResult<()> {
    let mut previous: Option<&ProjectContextCoordinate> = None;
    for coordinate in coordinates {
        if previous.is_some_and(|previous| previous >= coordinate) {
            return Err(SemanticGraphQueryError::InvalidState(format!(
                "{field} must be strictly canonical"
            )));
        }
        previous = Some(coordinate);
    }
    Ok(())
}

fn require_unique_coordinate(
    observed: &mut BTreeSet<ProjectContextCoordinate>,
    coordinate: &ProjectContextCoordinate,
    field: &'static str,
) -> QueryContractResult<()> {
    if !observed.insert(coordinate.clone()) {
        return Err(SemanticGraphQueryError::InvalidState(format!(
            "{field} Coordinate observations overlap"
        )));
    }
    Ok(())
}

fn validate_graph_membership(
    membership: &CurrentGraphMembershipObservation,
    project_context_revision: u64,
) -> QueryContractResult<()> {
    if membership.context_revision != project_context_revision
        || membership.incident_edge_keys.is_empty()
        || membership
            .incident_edge_keys
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err(SemanticGraphQueryError::InvalidState(
            "initial graph membership is empty, non-canonical, or from another snapshot".to_owned(),
        ));
    }
    Ok(())
}

fn validate_input_head(
    head: &SemanticHeadProvenance,
    semantic_generation_id: Uuid,
) -> QueryContractResult<()> {
    if head.generation_id != semantic_generation_id || head.unit_key != "overview" {
        return Err(SemanticGraphQueryError::InvalidState(
            "input semantic head does not match the observed generation contract".to_owned(),
        ));
    }
    Ok(())
}

fn validate_explicit_root_provenance(
    observation: &AcceptedInitialCoordinateObservation,
    root: &SemanticRoot,
) -> QueryContractResult<()> {
    if root.canonical_provenance.source_basis != observation.source_basis {
        return Err(SemanticGraphQueryError::InvalidState(
            "explicit root canonical basis differs from its accepted initial observation"
                .to_owned(),
        ));
    }
    match (
        &observation.semantic_state,
        root.semantic_provenance.as_ref(),
    ) {
        (SemanticHeadState::Current(head), Some(provenance))
            if provenance.generation_id == head.generation_id
                && provenance.unit_key == head.unit_key
                && provenance.source_snapshot_digest == head.snapshot_digest =>
        {
            Ok(())
        }
        (SemanticHeadState::Current(_), _) => Err(SemanticGraphQueryError::InvalidState(
            "explicit root semantic provenance differs from its current initial head".to_owned(),
        )),
        (
            SemanticHeadState::Missing
            | SemanticHeadState::Building
            | SemanticHeadState::Failed
            | SemanticHeadState::Unsupported,
            None,
        ) => Ok(()),
        (
            SemanticHeadState::Missing
            | SemanticHeadState::Building
            | SemanticHeadState::Failed
            | SemanticHeadState::Unsupported,
            Some(_),
        ) => Err(SemanticGraphQueryError::InvalidState(
            "embedding-less explicit initial unexpectedly has semantic root provenance".to_owned(),
        )),
    }
}

fn compare_discovery_channels(
    left: &RootDiscoveryChannel,
    right: &RootDiscoveryChannel,
) -> std::cmp::Ordering {
    discovery_channel_key(left).cmp(&discovery_channel_key(right))
}

fn discovery_channel_key(
    channel: &RootDiscoveryChannel,
) -> (u8, Option<&ProjectContextCoordinate>) {
    match channel {
        RootDiscoveryChannel::ExplicitInitial => (0, None),
        RootDiscoveryChannel::ProblemNeutral => (1, None),
        RootDiscoveryChannel::ContextConditioned { context_coordinate } => {
            (2, Some(context_coordinate))
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

impl SemanticRoot {
    fn validate(
        &self,
        project_id: Uuid,
        observations: &SemanticGraphQueryObservations,
    ) -> QueryContractResult<()> {
        let entrypoint_identities = self
            .structural_entrypoints
            .iter()
            .map(root_entrypoint_identity)
            .collect::<Vec<_>>();
        if self.structural_entrypoints.is_empty()
            || self.seed_outcomes.len() != self.structural_entrypoints.len()
            || entrypoint_identities
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || self
                .structural_entrypoints
                .iter()
                .zip(&self.seed_outcomes)
                .any(|(entrypoint, outcome)| {
                    entrypoint != &outcome.structural_entrypoint
                        || (outcome.produced_path_count == 0)
                            != outcome.zero_hop_stop_reason.is_some()
                })
            || derive_root_id(project_id, &self.source, &self.structural_entrypoints)?
                != self.root_id
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "root identity or structural outcome set is inconsistent".to_owned(),
            ));
        }
        self.source
            .validate()
            .map_err(|error| SemanticGraphQueryError::InvalidState(error.to_string()))?;
        if self.source.community_id != project_id
            || !basis_matches_source_kind(&self.canonical_provenance.source_basis, self.source.kind)
            || self
                .structural_entrypoints
                .iter()
                .any(|entrypoint| !source_matches_entrypoint(&self.source, entrypoint))
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "root source does not match its Project or structural entrypoint".to_owned(),
            ));
        }
        if self.semantic_score.is_some() != self.score_explanation.is_some()
            || self.semantic_score.is_some() != self.semantic_provenance.is_some()
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "root semantic score, provenance, and explanation must agree".to_owned(),
            ));
        }
        if let (Some(score), Some(explanation)) =
            (self.semantic_score, self.score_explanation.as_ref())
        {
            explanation.validate()?;
            if score != explanation.final_score {
                return Err(SemanticGraphQueryError::InvalidState(
                    "root score differs from explanation".to_owned(),
                ));
            }
        }
        validate_semantic_provenance(
            &self.canonical_provenance,
            self.semantic_provenance.as_ref(),
            observations,
        )?;
        validate_preview(&self.preview)
    }
}

impl SemanticPath {
    fn validate(
        &self,
        project_id: Uuid,
        root: &SemanticRoot,
        observations: &SemanticGraphQueryObservations,
    ) -> QueryContractResult<()> {
        if self.hops.is_empty()
            || self
                .hops
                .iter()
                .enumerate()
                .any(|(index, hop)| usize::from(hop.ordinal) != index + 1)
            || self.hops.last().is_none_or(|hop| {
                hop.continued_to_coordinate.coordinate != self.terminal_coordinate
            })
            || derive_path_id(self.root_id, &self.hops)? != self.path_id
            || self.path_score_explanation.final_score != Some(self.path_score)
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "path identity, ordinal, terminal, or score is inconsistent".to_owned(),
            ));
        }

        let first_hop = self.hops.first().ok_or_else(|| {
            SemanticGraphQueryError::InvalidState("path has no first hop".to_owned())
        })?;
        if !root
            .structural_entrypoints
            .iter()
            .any(|entrypoint| path_starts_at_entrypoint(first_hop, entrypoint))
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "path does not start at one of its root structural entrypoints".to_owned(),
            ));
        }

        let mut visited_edges = BTreeSet::new();
        let mut visited_coordinates = BTreeSet::new();
        let mut previous_coordinate = None;
        for (index, hop) in self.hops.iter().enumerate() {
            hop.validate(project_id)?;
            hop.validate_provenance(observations)?;
            if index > 0 && hop.entered_from_coordinate.as_ref() != previous_coordinate.as_ref() {
                return Err(SemanticGraphQueryError::InvalidState(
                    "path hops are not Coordinate-contiguous".to_owned(),
                ));
            }
            if !visited_edges.insert(hop.edge.edge_key) {
                return Err(SemanticGraphQueryError::InvalidState(
                    "path repeats a Hyperedge".to_owned(),
                ));
            }
            if let Some(entered) = hop.entered_from_coordinate.as_ref() {
                if !hop.edge.complete_coordinates.contains(entered) {
                    return Err(SemanticGraphQueryError::InvalidState(
                        "entered Coordinate is not a member of the complete Hyperedge".to_owned(),
                    ));
                }
                if index == 0 && !visited_coordinates.insert(entered.clone()) {
                    return Err(SemanticGraphQueryError::InvalidState(
                        "path repeats a Coordinate".to_owned(),
                    ));
                }
            } else if index > 0 {
                return Err(SemanticGraphQueryError::InvalidState(
                    "only a relation-Document root may omit entered Coordinate".to_owned(),
                ));
            }

            let continued = &hop.continued_to_coordinate.coordinate;
            if !hop.edge.complete_coordinates.contains(continued)
                || hop.entered_from_coordinate.as_ref() == Some(continued)
                || !visited_coordinates.insert(continued.clone())
            {
                return Err(SemanticGraphQueryError::InvalidState(
                    "continued Coordinate is not a distinct unvisited Hyperedge member".to_owned(),
                ));
            }
            previous_coordinate = Some(continued.clone());
        }

        let transition_scores = self
            .hops
            .iter()
            .map(|hop| hop.transition_score)
            .collect::<Vec<_>>();
        let expected_explanation = path_score(root.semantic_score, &transition_scores)
            .map_err(|error| SemanticGraphQueryError::InvalidState(error.to_string()))?;
        if self.path_score_explanation != expected_explanation {
            return Err(SemanticGraphQueryError::InvalidState(
                "path score explanation does not match its root and ordered transitions".to_owned(),
            ));
        }
        Ok(())
    }
}

fn path_starts_at_entrypoint(
    first_hop: &SemanticHyperedgeHop,
    entrypoint: &RootStructuralEntrypoint,
) -> bool {
    match entrypoint {
        RootStructuralEntrypoint::Coordinate { coordinate } => {
            first_hop.entered_from_coordinate.as_ref() == Some(coordinate)
        }
        RootStructuralEntrypoint::ContextDocument {
            edge_key,
            document_id,
            edge_provenance,
            binding_provenance,
        } => {
            first_hop.entered_from_coordinate.is_none()
                && first_hop.edge.edge_key == *edge_key
                && first_hop.edge.provenance == *edge_provenance
                && first_hop.selected_relation_document.document_id == *document_id
                && first_hop.selected_relation_document.binding_provenance == *binding_provenance
        }
    }
}

impl SemanticHyperedgeHop {
    fn validate(&self, project_id: Uuid) -> QueryContractResult<()> {
        let expected_edge = EdgeKey::derive(project_id, &self.edge.complete_coordinates)
            .map_err(|error| SemanticGraphQueryError::InvalidState(error.to_string()))?;
        if expected_edge != self.edge.edge_key
            || self
                .edge
                .current_context_document_bindings
                .windows(2)
                .any(|pair| pair[0].document_id.as_bytes() >= pair[1].document_id.as_bytes())
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "returned Hyperedge identity or binding order is invalid".to_owned(),
            ));
        }
        if matches!(
            self.continued_to_coordinate.lifecycle,
            SemanticLifecycleClass::Tombstone | SemanticLifecycleClass::Deleted
        ) {
            return Err(SemanticGraphQueryError::InvalidState(
                "continued target Coordinate has an ineligible lifecycle".to_owned(),
            ));
        }
        if !matches!(
            self.selected_relation_document
                .canonical_provenance
                .source_basis,
            SemanticSourceBasis::ProjectDocument(_)
        ) || !basis_matches_coordinate(
            &self
                .continued_to_coordinate
                .canonical_provenance
                .source_basis,
            &self.continued_to_coordinate.coordinate,
        ) {
            return Err(SemanticGraphQueryError::InvalidState(
                "hop source basis does not match its Document or target Coordinate".to_owned(),
            ));
        }
        let binding = self
            .edge
            .current_context_document_bindings
            .iter()
            .find(|binding| binding.document_id == self.selected_relation_document.document_id);
        if binding.map(|binding| &binding.provenance)
            != Some(&self.selected_relation_document.binding_provenance)
            || self.transition_score
                != harmonic_score(
                    self.selected_relation_document.document_score,
                    self.continued_to_coordinate.target_score,
                )
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "selected binding or transition score is inconsistent".to_owned(),
            ));
        }
        let expected_document_role = if self.entered_from_coordinate.is_none() {
            SemanticScoreRole::RelationRoot
        } else {
            SemanticScoreRole::RelationDocument
        };
        if self.selected_relation_document.score_explanation.score_role != expected_document_role
            || self
                .selected_relation_document
                .score_explanation
                .final_score
                != self.selected_relation_document.document_score
            || self
                .selected_relation_document
                .score_explanation
                .document_score
                .is_some_and(|score| score != self.selected_relation_document.document_score)
            || self.continued_to_coordinate.score_explanation.score_role
                != SemanticScoreRole::TargetCoordinate
            || self.continued_to_coordinate.score_explanation.final_score
                != self.continued_to_coordinate.target_score
            || self
                .continued_to_coordinate
                .score_explanation
                .target_coordinate_score
                .is_some_and(|score| score != self.continued_to_coordinate.target_score)
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "hop score roles or duplicated score fields are inconsistent".to_owned(),
            ));
        }
        self.selected_relation_document
            .score_explanation
            .validate()?;
        self.continued_to_coordinate.score_explanation.validate()?;
        validate_preview(&self.selected_relation_document.preview)?;
        validate_preview(&self.continued_to_coordinate.preview)
    }

    fn validate_provenance(
        &self,
        observations: &SemanticGraphQueryObservations,
    ) -> QueryContractResult<()> {
        validate_semantic_provenance(
            &self.selected_relation_document.canonical_provenance,
            Some(&self.selected_relation_document.semantic_provenance),
            observations,
        )?;
        validate_semantic_provenance(
            &self.continued_to_coordinate.canonical_provenance,
            Some(&self.continued_to_coordinate.semantic_provenance),
            observations,
        )
    }
}

fn validate_semantic_provenance(
    canonical: &CanonicalSourceProvenance,
    semantic: Option<&SemanticProvenance>,
    observations: &SemanticGraphQueryObservations,
) -> QueryContractResult<()> {
    if let Some(semantic) = semantic {
        if semantic.generation_id != observations.semantic_generation_id
            || semantic.source_snapshot_digest != canonical.source_snapshot_digest
            || semantic.source_generation_contract_digest
                != observations.source_generation_contract_digest
            || semantic.embedding_space_fence != observations.embedding_space_fence
            || semantic.unit_key != "overview"
        {
            return Err(SemanticGraphQueryError::InvalidState(
                "semantic provenance does not match canonical or query observations".to_owned(),
            ));
        }
    }
    Ok(())
}

fn source_matches_entrypoint(
    source: &SemanticSourceIdentity,
    entrypoint: &RootStructuralEntrypoint,
) -> bool {
    match entrypoint {
        RootStructuralEntrypoint::Coordinate { coordinate } => {
            source_matches_coordinate(source, coordinate)
        }
        RootStructuralEntrypoint::ContextDocument { document_id, .. } => {
            source.kind == SemanticSourceKind::ProjectDocument && source.source_id == *document_id
        }
    }
}

fn source_matches_coordinate(
    source: &SemanticSourceIdentity,
    coordinate: &ProjectContextCoordinate,
) -> bool {
    match (source.kind, coordinate) {
        (
            SemanticSourceKind::ProjectView(source_type),
            ProjectContextCoordinate::ProjectViewObject {
                object_type,
                object_id,
            },
        ) => {
            source_type == semantic_project_view_type(*object_type)
                && source.source_id == *object_id
        }
        (
            SemanticSourceKind::ProjectDocument,
            ProjectContextCoordinate::Document { document_id },
        ) => source.source_id == *document_id,
        (SemanticSourceKind::Meeting, ProjectContextCoordinate::Meeting { meeting_id }) => {
            source.source_id == *meeting_id
        }
        _ => false,
    }
}

const fn basis_matches_source_kind(basis: &SemanticSourceBasis, kind: SemanticSourceKind) -> bool {
    matches!(
        (basis, kind),
        (
            SemanticSourceBasis::ProjectView(_),
            SemanticSourceKind::ProjectView(_)
        ) | (
            SemanticSourceBasis::ProjectDocument(_),
            SemanticSourceKind::ProjectDocument
        ) | (SemanticSourceBasis::Meeting(_), SemanticSourceKind::Meeting)
    )
}

const fn basis_matches_coordinate(
    basis: &SemanticSourceBasis,
    coordinate: &ProjectContextCoordinate,
) -> bool {
    matches!(
        (basis, coordinate),
        (
            SemanticSourceBasis::ProjectView(_),
            ProjectContextCoordinate::ProjectViewObject { .. }
        ) | (
            SemanticSourceBasis::ProjectDocument(_),
            ProjectContextCoordinate::Document { .. }
        ) | (
            SemanticSourceBasis::Meeting(_),
            ProjectContextCoordinate::Meeting { .. }
        )
    )
}

const fn semantic_project_view_type(object_type: ProjectViewObjectType) -> ProjectViewSemanticType {
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

fn validate_preview(preview: &SemanticSourcePreview) -> QueryContractResult<()> {
    if preview.title.trim().is_empty()
        || preview.title.as_bytes().contains(&0)
        || preview
            .summary
            .as_deref()
            .is_some_and(|summary| summary.as_bytes().contains(&0))
        || (preview.summary.is_some() && preview.summary_omitted_reason.is_some())
    {
        return Err(SemanticGraphQueryError::InvalidState(
            "source preview title/summary omission state is invalid".to_owned(),
        ));
    }
    Ok(())
}

/// Derive a deterministic root identity from Project, source, and the sorted
/// structural entrypoint identities. Revision provenance is intentionally
/// excluded.
pub fn derive_root_id(
    project_id: Uuid,
    source: &SemanticSourceIdentity,
    entrypoints: &[RootStructuralEntrypoint],
) -> QueryContractResult<Digest32> {
    let mut identities = entrypoints
        .iter()
        .map(root_entrypoint_identity)
        .collect::<Vec<_>>();
    identities.sort();
    identities.dedup();
    let canonical = postcard::to_stdvec(&(project_id, source, identities))
        .map_err(|_| SemanticGraphQueryError::Serialization)?;
    Ok(hash_domain(
        b"buzz.semantic-graph-root",
        &[canonical.as_slice()],
    ))
}

/// Derive a deterministic path identity from the root and full ordered hop
/// structural/currentness provenance, excluding previews and scores.
pub fn derive_path_id(
    root_id: Digest32,
    hops: &[SemanticHyperedgeHop],
) -> QueryContractResult<Digest32> {
    let provenance = hops
        .iter()
        .map(|hop| {
            (
                hop.ordinal,
                hop.entered_from_coordinate.clone(),
                hop.edge.edge_key,
                hop.edge.complete_coordinates.clone(),
                hop.edge.provenance.clone(),
                hop.edge.current_context_document_bindings.clone(),
                hop.selected_relation_document.document_id,
                hop.selected_relation_document.binding_provenance.clone(),
                hop.selected_relation_document.canonical_provenance.clone(),
                hop.selected_relation_document.semantic_provenance.clone(),
                hop.continued_to_coordinate.coordinate.clone(),
                hop.continued_to_coordinate.lifecycle,
                hop.continued_to_coordinate.canonical_provenance.clone(),
                hop.continued_to_coordinate.semantic_provenance.clone(),
            )
        })
        .collect::<Vec<_>>();
    let canonical = postcard::to_stdvec(&(root_id, provenance))
        .map_err(|_| SemanticGraphQueryError::Serialization)?;
    Ok(hash_domain(
        b"buzz.semantic-graph-path",
        &[canonical.as_slice()],
    ))
}

fn root_entrypoint_identity(entrypoint: &RootStructuralEntrypoint) -> Vec<u8> {
    match entrypoint {
        RootStructuralEntrypoint::Coordinate { coordinate } => {
            let mut identity = vec![0];
            identity.extend_from_slice(coordinate_identity(coordinate).as_bytes());
            identity
        }
        RootStructuralEntrypoint::ContextDocument {
            edge_key,
            document_id,
            ..
        } => {
            let mut identity = vec![1];
            identity.extend_from_slice(edge_key.as_bytes());
            identity.extend_from_slice(document_id.as_bytes());
            identity
        }
    }
}

fn coordinate_identity(coordinate: &ProjectContextCoordinate) -> String {
    match coordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        } => format!("pv:{}:{object_id}", object_type.as_str()),
        ProjectContextCoordinate::Document { document_id } => format!("document:{document_id}"),
        ProjectContextCoordinate::Meeting { meeting_id } => format!("meeting:{meeting_id}"),
    }
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
    use super::{
        BranchStopReason, EmbeddingCoverageCounts, SemanticGraphQueryCoverage,
        SemanticSourcePreview, TruncationCountsByDimension,
    };
    use crate::{DegradedModeCounts, OmittedContextChannelCounts, OmittedForResponseBudgetCounts};

    #[test]
    fn branch_stop_precedence_is_closed() {
        assert!(
            BranchStopReason::WallTimeExhausted.precedence()
                < BranchStopReason::GlobalBudgetExhausted.precedence()
        );
        assert!(
            BranchStopReason::BelowRelevanceThreshold.precedence()
                < BranchStopReason::FrontierExhausted.precedence()
        );
    }

    #[test]
    fn coverage_requires_an_exact_partition() {
        let coverage = SemanticGraphQueryCoverage {
            authorized_graph_sources: 3,
            current_indexed_graph_sources: 2,
            title_only_sources: 1,
            embedding_coverage: EmbeddingCoverageCounts {
                current: 2,
                missing: 1,
                ..EmbeddingCoverageCounts::default()
            },
            query_channels_requested: 1,
            query_channels_executed: 1,
            omitted_context_channel_counts_by_reason: OmittedContextChannelCounts::default(),
            neutral_candidates_considered: 0,
            conditioned_candidates_considered: 0,
            roots_selected: 0,
            roots_returned: 0,
            expanded_coordinates: 0,
            incident_edges_materialized: 0,
            relation_options_materialized: 0,
            target_options_materialized: 0,
            paths_generated: 0,
            paths_retained: 0,
            paths_returned: 0,
            omitted_for_response_budget: OmittedForResponseBudgetCounts::default(),
            truncation_counts_by_dimension: TruncationCountsByDimension::default(),
            truncation_samples: Vec::new(),
            degraded_mode_counts: DegradedModeCounts::default(),
        };
        assert!(coverage.validate().is_ok());
    }

    #[test]
    fn source_preview_debug_redacts_title_and_summary() {
        let preview = SemanticSourcePreview {
            title: "CONFIDENTIAL-TITLE-中文".to_owned(),
            summary: Some("CONFIDENTIAL-SUMMARY-中文".to_owned()),
            summary_omitted_reason: None,
        };
        let debug = format!("{preview:?}");

        assert!(!debug.contains("CONFIDENTIAL-TITLE"));
        assert!(!debug.contains("CONFIDENTIAL-SUMMARY"));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("title_bytes"));
        assert!(debug.contains("summary_bytes"));
    }
}
