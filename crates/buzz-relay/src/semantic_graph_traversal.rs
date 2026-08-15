//! Bounded Hyperedge traversal for one semantic-graph root session.
//!
//! The search consumes only the current writer-DB repeatable-read transaction
//! retained by the root stage. Complete Hyperedge identity is always handled
//! atomically; lifecycle and semantic readiness affect continuation targets,
//! never the Coordinate set returned for an Edge.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;

use buzz_db::semantic_query::{
    semantic_source_identity_for_coordinate, SemanticCanonicalHydrationBatch, SemanticCurrentHead,
    SemanticEdgeTargetRankOutcome, SemanticEdgeTargetRankRequest, SemanticExactSourceScore,
    SemanticGraphQueryTicket, SemanticGraphQueryVectorBundle, SemanticGraphReadTx,
    SemanticHydratedCurrentSource, SemanticHyperedgeExpectation, SemanticHyperedgeReadOutcome,
    SemanticIncidentRelationRankOutcome, SemanticIncidentRelationRankRequest,
    SemanticRankedRelationOption, SemanticRankedTargetOption, SemanticRelationOptionOmission,
    SemanticTargetOptionOmission, SemanticTraversalConditionedChannel,
    SemanticTraversalQueryChannels, SemanticTraversalSliceExhaustion,
    SemanticTraversalSourceOmissionReason,
};
use buzz_project_context::{EdgeKey, ProjectContextCoordinate};
use buzz_semantic::{Digest32, SemanticSourceIdentity, SemanticSourceKind};
use buzz_semantic_query::{
    derive_path_id, document_score, environment_gain, first_wave_slice, harmonic_score,
    highest_precedence_stop, path_score, target_coordinate_score, AnchorGain,
    BoundedSuccessorAccumulator, BranchStopReason, CompletionReason, ConditionedEvidence,
    ContextDocumentBindingObservation, ExhaustedDimension, FrontierPathState, LifecycleFilter,
    RelationRankCursor, RootStructuralEntrypoint, Score, ScoreExplanation, SeedOutcome,
    SemanticContinuedCoordinate, SemanticEdgeObservation, SemanticGraphQueryBudget,
    SemanticGraphQueryCoverage, SemanticGraphQueryError, SemanticHyperedgeHop, SemanticPath,
    SemanticProvenance, SemanticRelationDocument, SemanticRoot, SemanticScoreRole,
    SemanticSourcePreview, TargetRankCursor, TruncationSample, MAX_TRUNCATION_SAMPLES,
    RELATION_FLOOR, TARGET_FLOOR, TRANSITION_FLOOR,
};
use tokio::time::Instant;

use crate::semantic_graph_observability::{
    record_db_distance_rows, record_query_error, record_snapshot_transaction, stage_timer,
    SemanticGraphDistanceStage, SemanticGraphMetricStage,
};
use crate::semantic_graph_query::{
    QueryChannelBinding, SemanticGraphRootQueryError, SemanticGraphRootQuerySession,
    SemanticGraphSelectedRoot,
};

/// Completed retrieval forest before response packing and Stage D postflight.
pub(crate) struct SemanticGraphTraversalOutcome {
    pub(crate) request_id: uuid::Uuid,
    pub(crate) project_id: uuid::Uuid,
    pub(crate) ticket: SemanticGraphQueryTicket,
    pub(crate) input_observations: buzz_semantic_query::SemanticGraphQueryInputObservations,
    pub(crate) roots: Vec<SemanticRoot>,
    pub(crate) paths: Vec<SemanticPath>,
    pub(crate) coverage: SemanticGraphQueryCoverage,
    pub(crate) completion_reason: CompletionReason,
    pub(crate) exhausted_dimensions: Vec<ExhaustedDimension>,
    pub(crate) absolute_deadline: Instant,
}

impl std::fmt::Debug for SemanticGraphTraversalOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticGraphTraversalOutcome")
            .field("root_count", &self.roots.len())
            .field("path_count", &self.paths.len())
            .field("completion_reason", &self.completion_reason)
            .finish_non_exhaustive()
    }
}

/// Traverse every selected root entrypoint inside its retained Stage C RR
/// snapshot and commit the read transaction after the forest is sealed.
pub(crate) async fn complete_semantic_graph_traversal(
    session: SemanticGraphRootQuerySession,
) -> Result<SemanticGraphTraversalOutcome, SemanticGraphRootQueryError> {
    let _timer = stage_timer(SemanticGraphMetricStage::Traversal);
    let result = complete_semantic_graph_traversal_inner(session).await;
    if let Err(error) = &result {
        record_query_error(SemanticGraphMetricStage::Traversal, error.metric_code());
    }
    result
}

async fn complete_semantic_graph_traversal_inner(
    mut session: SemanticGraphRootQuerySession,
) -> Result<SemanticGraphTraversalOutcome, SemanticGraphRootQueryError> {
    let channels = TraversalChannels::from_bindings(&session.channels)?;
    let roots = session.outcome.roots.clone();
    let query = session.query.clone();
    let mut coverage = session.outcome.coverage.clone();
    let mut exhausted_dimensions = session.outcome.exhausted_dimensions.clone();
    let search = {
        let mut backend = DbTraversalBackend {
            read: &mut session.read,
        };
        TraversalEngine::new(
            &mut backend,
            &session.outcome.ticket,
            &session.query_vectors,
            &channels,
            query.lifecycle_filter,
            query.budget,
            session.work_deadline,
        )?
        .search(&roots)
        .await?
    };

    // Traversal deliberately stops before the snapshot-close reserve begins.
    // Committing against the later deadline preserves a valid
    // `wall_time_exhausted` partial result instead of converting the expected
    // stop into a hard 504 merely because the traversal deadline elapsed.
    run_db_before(session.snapshot_close_deadline, session.read.commit())
        .await?
        .ok_or(SemanticGraphRootQueryError::QueryDeadlineExceeded)?;
    record_snapshot_transaction(session.snapshot_started_at.elapsed());
    apply_search_coverage(&mut coverage, &search);
    exhausted_dimensions.extend(search.exhausted_dimensions.iter().copied());
    exhausted_dimensions.sort_unstable();
    exhausted_dimensions.dedup();
    let completion_reason = if search.wall_time_exhausted {
        exhausted_dimensions.clear();
        CompletionReason::WallTimeExhausted
    } else if exhausted_dimensions.is_empty() {
        CompletionReason::FrontierExhausted
    } else {
        CompletionReason::BudgetExhausted
    };
    coverage.validate()?;

    Ok(SemanticGraphTraversalOutcome {
        request_id: query.request_id,
        project_id: query.project_id,
        ticket: session.outcome.ticket.clone(),
        input_observations: session.outcome.input_observations.clone(),
        roots: search.roots,
        paths: search.paths,
        coverage,
        completion_reason,
        exhausted_dimensions,
        absolute_deadline: session.absolute_deadline,
    })
}

#[async_trait::async_trait]
trait TraversalBackend {
    async fn load_hyperedge(
        &mut self,
        expectation: &SemanticHyperedgeExpectation,
    ) -> Result<SemanticHyperedgeReadOutcome, buzz_db::DbError>;

    async fn rank_relations(
        &mut self,
        request: SemanticIncidentRelationRankRequest<'_>,
    ) -> Result<SemanticIncidentRelationRankOutcome, buzz_db::DbError>;

    async fn rank_targets(
        &mut self,
        request: SemanticEdgeTargetRankRequest<'_>,
    ) -> Result<SemanticEdgeTargetRankOutcome, buzz_db::DbError>;

    async fn hydrate(
        &mut self,
        scores: &[SemanticExactSourceScore],
    ) -> Result<SemanticCanonicalHydrationBatch, buzz_db::DbError>;
}

struct DbTraversalBackend<'a> {
    read: &'a mut SemanticGraphReadTx,
}

#[async_trait::async_trait]
impl TraversalBackend for DbTraversalBackend<'_> {
    async fn load_hyperedge(
        &mut self,
        expectation: &SemanticHyperedgeExpectation,
    ) -> Result<SemanticHyperedgeReadOutcome, buzz_db::DbError> {
        self.read.load_complete_hyperedge(expectation).await
    }

    async fn rank_relations(
        &mut self,
        request: SemanticIncidentRelationRankRequest<'_>,
    ) -> Result<SemanticIncidentRelationRankOutcome, buzz_db::DbError> {
        self.read
            .rank_incident_relation_options_exact(request)
            .await
    }

    async fn rank_targets(
        &mut self,
        request: SemanticEdgeTargetRankRequest<'_>,
    ) -> Result<SemanticEdgeTargetRankOutcome, buzz_db::DbError> {
        self.read.rank_edge_target_options_exact(request).await
    }

    async fn hydrate(
        &mut self,
        scores: &[SemanticExactSourceScore],
    ) -> Result<SemanticCanonicalHydrationBatch, buzz_db::DbError> {
        self.read.hydrate_current_exact_sources(scores).await
    }
}

#[derive(Clone)]
struct TraversalChannels {
    problem_channel_id: Digest32,
    conditioned: Vec<SemanticTraversalConditionedChannel>,
}

impl TraversalChannels {
    fn from_bindings(
        bindings: &[QueryChannelBinding],
    ) -> Result<Self, SemanticGraphRootQueryError> {
        let problem = bindings
            .iter()
            .filter(|binding| binding.context_coordinate.is_none())
            .collect::<Vec<_>>();
        if problem.len() != 1 {
            return Err(invalid_state(
                "traversal channel binding must contain exactly one Q0",
            ));
        }
        let conditioned = bindings
            .iter()
            .filter_map(|binding| {
                binding
                    .context_coordinate
                    .clone()
                    .map(|context_coordinate| SemanticTraversalConditionedChannel {
                        channel_id: binding.channel_id,
                        context_coordinate,
                    })
            })
            .collect();
        Ok(Self {
            problem_channel_id: problem[0].channel_id,
            conditioned,
        })
    }

    fn borrow<'a>(
        &'a self,
        query_vectors: &'a SemanticGraphQueryVectorBundle,
    ) -> SemanticTraversalQueryChannels<'a> {
        SemanticTraversalQueryChannels {
            query_vectors,
            problem_channel_id: self.problem_channel_id,
            conditioned: &self.conditioned,
        }
    }
}

struct TraversalEngine<'a, B> {
    backend: &'a mut B,
    ticket: &'a SemanticGraphQueryTicket,
    query_vectors: &'a SemanticGraphQueryVectorBundle,
    channels: &'a TraversalChannels,
    lifecycle_filter: LifecycleFilter,
    budget: SemanticGraphQueryBudget,
    work_deadline: Instant,
    materialization: MaterializationState,
    hydrated: HashMap<SemanticSourceIdentity, SemanticHydratedCurrentSource>,
    edge_cache: HashMap<EdgeKey, SemanticEdgeObservation>,
    relation_omissions: HashSet<RelationOmissionKey>,
    target_omissions: HashSet<TargetOmissionKey>,
    oversized_edges: HashSet<EdgeKey>,
    exhaustion: ExhaustionState,
    wall_time_exhausted: bool,
}

impl<'a, B: TraversalBackend> TraversalEngine<'a, B> {
    fn new(
        backend: &'a mut B,
        ticket: &'a SemanticGraphQueryTicket,
        query_vectors: &'a SemanticGraphQueryVectorBundle,
        channels: &'a TraversalChannels,
        lifecycle_filter: LifecycleFilter,
        budget: SemanticGraphQueryBudget,
        work_deadline: Instant,
    ) -> Result<Self, SemanticGraphRootQueryError> {
        if query_vectors.is_empty() {
            return Err(invalid_state("traversal requires the Q0 query vector"));
        }
        Ok(Self {
            backend,
            ticket,
            query_vectors,
            channels,
            lifecycle_filter,
            budget,
            work_deadline,
            materialization: MaterializationState::default(),
            hydrated: HashMap::new(),
            edge_cache: HashMap::new(),
            relation_omissions: HashSet::new(),
            target_omissions: HashSet::new(),
            oversized_edges: HashSet::new(),
            exhaustion: ExhaustionState::default(),
            wall_time_exhausted: false,
        })
    }

    async fn search(
        mut self,
        roots: &[SemanticGraphSelectedRoot],
    ) -> Result<SearchOutput, SemanticGraphRootQueryError> {
        let mut seeds = build_seed_work(roots, self.budget.beam_width)?;
        seeds.sort_by(|left, right| left.seed_order.cmp(&right.seed_order));
        let mut deferred = Vec::new();
        let mut queue = Vec::new();
        let mut stopped = Vec::new();
        let mut zero_hop = HashMap::new();
        let seed_count = seeds.len();

        for (ordinal, mut work) in seeds.into_iter().enumerate() {
            let remaining_seeds = seed_count.saturating_sub(ordinal);
            let mut quantum =
                Quantum::first_wave(&self.materialization, &self.budget, remaining_seeds);
            match self.advance(&mut work, &mut quantum).await? {
                AdvanceOutcome::Sealed(successors) => {
                    let produced_successor = !successors.is_empty();
                    publish_successors(
                        successors,
                        work.seed_id,
                        work.seed_order.clone(),
                        &self.budget,
                        &mut queue,
                        &mut stopped,
                        &mut self.exhaustion,
                    )?;
                    if !produced_successor {
                        zero_hop.insert(work.seed_id, work.stop_reason());
                    }
                }
                AdvanceOutcome::Deferred => deferred.push(work),
                AdvanceOutcome::GlobalStop(reason) => {
                    finish_cutoff_work(
                        work,
                        reason,
                        &self.budget,
                        &mut self.exhaustion,
                        &mut stopped,
                        &mut zero_hop,
                    )?;
                }
            }
        }

        queue.extend(deferred);
        while !queue.is_empty() && !self.wall_time_exhausted && !self.exhaustion.global_stop {
            queue.sort_by(compare_work);
            let mut work = queue.remove(0);
            let mut quantum = Quantum::global_step();
            match self.advance(&mut work, &mut quantum).await? {
                AdvanceOutcome::Sealed(successors) => {
                    let produced_successor = !successors.is_empty();
                    publish_successors(
                        successors,
                        work.seed_id,
                        work.seed_order.clone(),
                        &self.budget,
                        &mut queue,
                        &mut stopped,
                        &mut self.exhaustion,
                    )?;
                    if !produced_successor {
                        if work.path.hops.is_empty() {
                            zero_hop.insert(work.seed_id, work.stop_reason());
                        } else {
                            stopped.push(StoppedPath::from_work(work)?);
                        }
                    }
                }
                AdvanceOutcome::Deferred => queue.push(work),
                AdvanceOutcome::GlobalStop(reason) => {
                    finish_cutoff_work(
                        work,
                        reason,
                        &self.budget,
                        &mut self.exhaustion,
                        &mut stopped,
                        &mut zero_hop,
                    )?;
                }
            }
        }

        let cutoff_reason = if self.wall_time_exhausted {
            Some(BranchStopReason::WallTimeExhausted)
        } else if self.exhaustion.global_stop {
            Some(BranchStopReason::GlobalBudgetExhausted)
        } else {
            None
        };
        if let Some(reason) = cutoff_reason {
            for work in queue {
                finish_cutoff_work(
                    work,
                    reason,
                    &self.budget,
                    &mut self.exhaustion,
                    &mut stopped,
                    &mut zero_hop,
                )?;
            }
        }

        self.finish(roots, stopped, zero_hop)
    }

    async fn advance(
        &mut self,
        work: &mut ExpansionWork,
        quantum: &mut Quantum,
    ) -> Result<AdvanceOutcome, SemanticGraphRootQueryError> {
        loop {
            if Instant::now() >= self.work_deadline {
                self.wall_time_exhausted = true;
                return Ok(AdvanceOutcome::GlobalStop(
                    BranchStopReason::WallTimeExhausted,
                ));
            }
            if self.exhaustion.global_stop {
                return Ok(AdvanceOutcome::GlobalStop(
                    BranchStopReason::GlobalBudgetExhausted,
                ));
            }

            if let Some(mut target) = work.pending_targets.pop_front() {
                match self.advance_targets(work, &mut target, quantum).await? {
                    UnitAdvance::Done => continue,
                    UnitAdvance::Deferred => {
                        work.pending_targets.push_front(target);
                        return Ok(AdvanceOutcome::Deferred);
                    }
                    UnitAdvance::GlobalStop(reason) => {
                        work.pending_targets.push_front(target);
                        return Ok(AdvanceOutcome::GlobalStop(reason));
                    }
                }
            }

            if let Some(relation) = work.pending_relations.pop_front() {
                match self.advance_relation(work, relation, quantum).await? {
                    RelationAdvance::Ready(target) => {
                        work.pending_targets.push_back(*target);
                        continue;
                    }
                    RelationAdvance::Skipped => continue,
                    RelationAdvance::GlobalStop(reason) => {
                        return Ok(AdvanceOutcome::GlobalStop(reason));
                    }
                }
            }

            if !work.incident_exhausted {
                match self.advance_incident(work, quantum).await? {
                    UnitAdvance::Done => continue,
                    UnitAdvance::Deferred => return Ok(AdvanceOutcome::Deferred),
                    UnitAdvance::GlobalStop(reason) => {
                        return Ok(AdvanceOutcome::GlobalStop(reason));
                    }
                }
            }

            let accumulator = work.accumulator.take().ok_or_else(|| {
                invalid_state("sealed traversal work lost its successor accumulator")
            })?;
            let successors = accumulator.into_successors();
            let suppressed = work
                .qualifying_successors_seen
                .saturating_sub(successors.len());
            if suppressed > 0 {
                self.exhaustion
                    .record(ExhaustedDimension::BeamWidth, suppressed as u64, work)?;
            }
            return Ok(AdvanceOutcome::Sealed(successors));
        }
    }

    async fn advance_incident(
        &mut self,
        work: &mut ExpansionWork,
        quantum: &mut Quantum,
    ) -> Result<UnitAdvance, SemanticGraphRootQueryError> {
        let coordinate = work
            .path
            .current_coordinate
            .clone()
            .ok_or_else(|| invalid_state("Coordinate incident work lacks a Coordinate"))?;
        if !work.incident_started {
            match admit_coordinate_expansion(
                &mut self.materialization.expanded_coordinates,
                usize::from(self.budget.max_expanded_coordinates),
                &mut quantum.expanded_coordinates,
            ) {
                CoordinateExpansionAdmission::GlobalExhausted => {
                    self.record_global_exhaustion(
                        ExhaustedDimension::ExpandedCoordinates,
                        1,
                        work,
                    )?;
                    return Ok(UnitAdvance::GlobalStop(
                        BranchStopReason::GlobalBudgetExhausted,
                    ));
                }
                CoordinateExpansionAdmission::Deferred => return Ok(UnitAdvance::Deferred),
                CoordinateExpansionAdmission::Admitted => work.incident_started = true,
            }
        }

        let global_remaining = usize::from(self.budget.max_relation_options_materialized)
            .saturating_sub(self.materialization.relations.len());
        if quantum.relation_options == 0 && global_remaining > 0 {
            return Ok(UnitAdvance::Deferred);
        }
        let requested = quantum
            .relation_options
            .min(global_remaining)
            .max(1)
            .min(usize::from(u16::MAX));
        let limit = u16::try_from(requested)
            .map_err(|_| invalid_state("relation traversal slice exceeds u16"))?;
        let request = SemanticIncidentRelationRankRequest {
            entered_from: &coordinate,
            channels: self.channels.borrow(self.query_vectors),
            after: work.incident_after.as_ref(),
            limit,
        };
        let Some(outcome) =
            run_db_before(self.work_deadline, self.backend.rank_relations(request)).await?
        else {
            self.wall_time_exhausted = true;
            return Ok(UnitAdvance::GlobalStop(BranchStopReason::WallTimeExhausted));
        };
        let batch = match outcome {
            SemanticIncidentRelationRankOutcome::Ranked(batch) => *batch,
            SemanticIncidentRelationRankOutcome::OptionSetTooLarge { observed_at_least } => {
                self.record_global_exhaustion(
                    ExhaustedDimension::RelationOptionsMaterialized,
                    observed_at_least as u64,
                    work,
                )?;
                return Ok(UnitAdvance::GlobalStop(
                    BranchStopReason::GlobalBudgetExhausted,
                ));
            }
        };
        validate_snapshot(self.ticket, &batch.snapshot)?;
        record_db_distance_rows(
            SemanticGraphDistanceStage::Relation,
            batch
                .options
                .iter()
                .map(|option| option.channel_scores.len())
                .sum(),
        );
        self.observe_relation_omissions(&batch.omitted);
        work.stop.below_relevance |= batch.below_relation_floor > 0;

        let mut last_cursor = work.incident_after.clone();
        for option in batch.options {
            let key = RelationMaterializationKey {
                entered_from: Some(coordinate.clone()),
                edge_key: option.edge_key,
                document_id: option.document_id,
            };
            if !self.materialization.relations.contains(&key) {
                if self.materialization.relations.len()
                    >= usize::from(self.budget.max_relation_options_materialized)
                {
                    self.record_global_exhaustion(
                        ExhaustedDimension::RelationOptionsMaterialized,
                        1,
                        work,
                    )?;
                    return Ok(UnitAdvance::GlobalStop(
                        BranchStopReason::GlobalBudgetExhausted,
                    ));
                }
                if quantum.relation_options == 0 {
                    work.incident_after = last_cursor;
                    return Ok(UnitAdvance::Deferred);
                }
                self.materialization.relations.insert(key);
                quantum.relation_options -= 1;
            }
            last_cursor = Some(RelationRankCursor {
                document_score: option.document_score,
                edge_key: option.edge_key,
                document_id: option.document_id,
            });
            work.pending_relations.push_back(option);
        }
        work.incident_after = last_cursor;
        match batch.exhaustion {
            SemanticTraversalSliceExhaustion::Exhausted => work.incident_exhausted = true,
            SemanticTraversalSliceExhaustion::Truncated => {
                if self.materialization.relations.len()
                    >= usize::from(self.budget.max_relation_options_materialized)
                {
                    self.record_global_exhaustion(
                        ExhaustedDimension::RelationOptionsMaterialized,
                        1,
                        work,
                    )?;
                    return Ok(UnitAdvance::GlobalStop(
                        BranchStopReason::GlobalBudgetExhausted,
                    ));
                }
                if quantum.relation_options == 0 {
                    return Ok(UnitAdvance::Deferred);
                }
            }
        }
        Ok(UnitAdvance::Done)
    }

    async fn advance_relation(
        &mut self,
        work: &mut ExpansionWork,
        relation: SemanticRankedRelationOption,
        _quantum: &mut Quantum,
    ) -> Result<RelationAdvance, SemanticGraphRootQueryError> {
        if work.path.visited_edges.contains(&relation.edge_key) {
            work.stop.cycle_or_duplicate = true;
            return Ok(RelationAdvance::Skipped);
        }
        let parts = semantic_score_parts(
            &relation.channel_scores,
            self.channels,
            self.ticket.community_id,
        )?;
        let expected_document_source =
            project_document_source(self.ticket.community_id, relation.document_id);
        if parts.representative.source != expected_document_source {
            return Err(invalid_state(
                "ranked relation score belongs to a different Document",
            ));
        }
        validate_relation_coherence(
            work,
            &parts.representative,
            relation.local_coherence.as_ref(),
            self.ticket.community_id,
        )?;
        let observed_coherence = relation.local_coherence.as_ref().map(|value| value.score);
        let recomputed = document_score(
            parts.problem_score,
            parts.environment_gain,
            observed_coherence,
        );
        if recomputed != relation.document_score || recomputed < RELATION_FLOOR {
            return Err(invalid_state(
                "ranked relation score does not recompute above its floor",
            ));
        }
        let Some(hydrated) = self.hydrate_source(&parts.representative).await? else {
            self.wall_time_exhausted = true;
            return Ok(RelationAdvance::GlobalStop(
                BranchStopReason::WallTimeExhausted,
            ));
        };
        let document = SemanticRelationDocument {
            document_id: relation.document_id,
            binding_provenance: relation.binding_provenance.clone(),
            preview: preview(&hydrated),
            canonical_provenance: canonical_provenance(&hydrated),
            semantic_provenance: semantic_provenance(self.ticket, &parts.representative),
            document_score: relation.document_score,
            score_explanation: relation_score_explanation(&parts, &relation),
        };
        document.score_explanation.validate()?;
        let required_binding = ContextDocumentBindingObservation {
            document_id: relation.document_id,
            provenance: relation.binding_provenance,
        };
        Ok(RelationAdvance::Ready(Box::new(TargetWork {
            entered_from: work.path.current_coordinate.clone(),
            relation_admitted: true,
            expectation: SemanticHyperedgeExpectation {
                edge_key: relation.edge_key,
                edge_provenance: relation.edge_provenance,
                required_binding: Some(required_binding),
            },
            edge: None,
            document,
            document_head: parts.representative.head,
            after: None,
            exhausted: false,
        })))
    }

    async fn advance_targets(
        &mut self,
        work: &mut ExpansionWork,
        target: &mut TargetWork,
        quantum: &mut Quantum,
    ) -> Result<UnitAdvance, SemanticGraphRootQueryError> {
        if !target.relation_admitted {
            let key = RelationMaterializationKey {
                entered_from: target.entered_from.clone(),
                edge_key: target.expectation.edge_key,
                document_id: target.document.document_id,
            };
            match admit_new_key(
                &mut self.materialization.relations,
                key,
                usize::from(self.budget.max_relation_options_materialized),
                &mut quantum.relation_options,
            ) {
                Admission::Reused | Admission::Admitted => target.relation_admitted = true,
                Admission::Deferred => return Ok(UnitAdvance::Deferred),
                Admission::GlobalExhausted => {
                    self.record_global_exhaustion(
                        ExhaustedDimension::RelationOptionsMaterialized,
                        1,
                        work,
                    )?;
                    return Ok(UnitAdvance::GlobalStop(
                        BranchStopReason::GlobalBudgetExhausted,
                    ));
                }
            }
        }

        if target.edge.is_none() {
            if let Some(cached) = self.edge_cache.get(&target.expectation.edge_key) {
                validate_cached_edge(cached, &target.expectation)?;
                target.edge = Some(cached.clone());
            } else {
                if self.materialization.edges.len()
                    >= usize::from(self.budget.max_incident_edges_materialized)
                {
                    self.record_global_exhaustion(
                        ExhaustedDimension::IncidentEdgesMaterialized,
                        1,
                        work,
                    )?;
                    return Ok(UnitAdvance::GlobalStop(
                        BranchStopReason::GlobalBudgetExhausted,
                    ));
                }
                if quantum.incident_edges == 0 {
                    return Ok(UnitAdvance::Deferred);
                }
                let Some(outcome) = run_db_before(
                    self.work_deadline,
                    self.backend.load_hyperedge(&target.expectation),
                )
                .await?
                else {
                    self.wall_time_exhausted = true;
                    return Ok(UnitAdvance::GlobalStop(BranchStopReason::WallTimeExhausted));
                };
                match outcome {
                    SemanticHyperedgeReadOutcome::Current(edge) => {
                        validate_cached_edge(&edge, &target.expectation)?;
                        let edge = *edge;
                        self.materialization.edges.insert(edge.edge_key);
                        quantum.incident_edges -= 1;
                        self.edge_cache.insert(edge.edge_key, edge.clone());
                        target.edge = Some(edge);
                    }
                    SemanticHyperedgeReadOutcome::Changed => {
                        return Err(invalid_state(
                            "Hyperedge changed inside one repeatable-read snapshot",
                        ));
                    }
                    SemanticHyperedgeReadOutcome::HyperedgeTooLarge { .. } => {
                        work.stop.hyperedge_too_large = true;
                        self.oversized_edges.insert(target.expectation.edge_key);
                        target.exhausted = true;
                        return Ok(UnitAdvance::Done);
                    }
                }
            }
        }
        if target.exhausted {
            return Ok(UnitAdvance::Done);
        }

        let global_remaining = usize::from(self.budget.max_target_options_materialized)
            .saturating_sub(self.materialization.targets.len());
        if quantum.target_options == 0 && global_remaining > 0 {
            return Ok(UnitAdvance::Deferred);
        }
        let requested = quantum
            .target_options
            .min(global_remaining)
            .max(1)
            .min(usize::from(u16::MAX));
        let limit = u16::try_from(requested)
            .map_err(|_| invalid_state("target traversal slice exceeds u16"))?;
        let edge = target
            .edge
            .clone()
            .ok_or_else(|| invalid_state("target traversal lost its complete Hyperedge"))?;
        let request = SemanticEdgeTargetRankRequest {
            hyperedge: &edge,
            relation_document_id: target.document.document_id,
            relation_document_head: &target.document_head,
            document_score: target.document.document_score,
            lifecycle_filter: self.lifecycle_filter,
            channels: self.channels.borrow(self.query_vectors),
            after: target.after.as_ref(),
            limit,
        };
        let Some(outcome) =
            run_db_before(self.work_deadline, self.backend.rank_targets(request)).await?
        else {
            self.wall_time_exhausted = true;
            return Ok(UnitAdvance::GlobalStop(BranchStopReason::WallTimeExhausted));
        };
        let batch = match outcome {
            SemanticEdgeTargetRankOutcome::Ranked(batch) => *batch,
            SemanticEdgeTargetRankOutcome::HyperedgeChanged => {
                return Err(invalid_state(
                    "target Hyperedge changed inside one repeatable-read snapshot",
                ));
            }
            SemanticEdgeTargetRankOutcome::HyperedgeTooLarge { .. } => {
                work.stop.hyperedge_too_large = true;
                self.oversized_edges.insert(target.expectation.edge_key);
                target.exhausted = true;
                return Ok(UnitAdvance::Done);
            }
            SemanticEdgeTargetRankOutcome::OptionSetTooLarge { observed_at_least } => {
                self.record_global_exhaustion(
                    ExhaustedDimension::TargetOptionsMaterialized,
                    observed_at_least as u64,
                    work,
                )?;
                return Ok(UnitAdvance::GlobalStop(
                    BranchStopReason::GlobalBudgetExhausted,
                ));
            }
        };
        validate_snapshot(self.ticket, &batch.snapshot)?;
        if batch.edge != edge {
            return Err(invalid_state(
                "target rank returned a different complete Hyperedge",
            ));
        }
        record_db_distance_rows(
            SemanticGraphDistanceStage::Target,
            batch
                .options
                .iter()
                .map(|option| option.channel_scores.len())
                .sum(),
        );
        self.observe_target_omissions(edge.edge_key, target.document.document_id, &batch.omitted);
        work.stop.below_relevance |=
            batch.below_target_floor > 0 || batch.below_transition_floor > 0;

        let mut admitted = Vec::new();
        let mut last_cursor = target.after.clone();
        for option in batch.options {
            let key = TargetMaterializationKey {
                entered_from: target.entered_from.clone(),
                edge_key: edge.edge_key,
                document_id: target.document.document_id,
                coordinate: option.coordinate.clone(),
            };
            if !self.materialization.targets.contains(&key) {
                if self.materialization.targets.len()
                    >= usize::from(self.budget.max_target_options_materialized)
                {
                    self.record_global_exhaustion(
                        ExhaustedDimension::TargetOptionsMaterialized,
                        1,
                        work,
                    )?;
                    return Ok(UnitAdvance::GlobalStop(
                        BranchStopReason::GlobalBudgetExhausted,
                    ));
                }
                if quantum.target_options == 0 {
                    target.after = last_cursor;
                    return Ok(UnitAdvance::Deferred);
                }
                self.materialization.targets.insert(key);
                quantum.target_options -= 1;
            }
            let (cursor, option) = classify_ranked_target(&work.path, edge.edge_key, option);
            last_cursor = Some(cursor);
            if let Some(option) = option {
                admitted.push(option);
            } else {
                work.stop.cycle_or_duplicate = true;
            }
        }
        target.after = last_cursor;
        for option in admitted {
            let successor = self
                .build_successor(work, target, edge.clone(), option)
                .await?;
            let Some(successor) = successor else {
                self.wall_time_exhausted = true;
                return Ok(UnitAdvance::GlobalStop(BranchStopReason::WallTimeExhausted));
            };
            work.qualifying_successors_seen += 1;
            work.accumulator
                .as_mut()
                .ok_or_else(|| invalid_state("active work lost its successor accumulator"))?
                .admit(successor)?;
        }
        match batch.exhaustion {
            SemanticTraversalSliceExhaustion::Exhausted => target.exhausted = true,
            SemanticTraversalSliceExhaustion::Truncated => {
                if self.materialization.targets.len()
                    >= usize::from(self.budget.max_target_options_materialized)
                {
                    self.record_global_exhaustion(
                        ExhaustedDimension::TargetOptionsMaterialized,
                        1,
                        work,
                    )?;
                    return Ok(UnitAdvance::GlobalStop(
                        BranchStopReason::GlobalBudgetExhausted,
                    ));
                }
                if quantum.target_options == 0 {
                    return Ok(UnitAdvance::Deferred);
                }
            }
        }
        Ok(UnitAdvance::Done)
    }

    async fn build_successor(
        &mut self,
        work: &ExpansionWork,
        target: &TargetWork,
        edge: SemanticEdgeObservation,
        option: SemanticRankedTargetOption,
    ) -> Result<Option<FrontierPathState>, SemanticGraphRootQueryError> {
        let parts = semantic_score_parts(
            &option.channel_scores,
            self.channels,
            self.ticket.community_id,
        )?;
        let expected_target =
            semantic_source_identity_for_coordinate(self.ticket.community_id, &option.coordinate)?;
        if parts.representative.source != expected_target {
            return Err(invalid_state(
                "ranked target score belongs to a different Coordinate",
            ));
        }
        validate_target_coherence(
            target,
            &parts.representative,
            &option.relation_document_coherence,
            self.ticket.community_id,
        )?;
        let recomputed_target = target_coordinate_score(
            parts.problem_score,
            parts.environment_gain,
            option.relation_document_coherence.score,
        );
        let recomputed_transition =
            harmonic_score(target.document.document_score, recomputed_target);
        if recomputed_target != option.target_score
            || recomputed_transition != option.transition_score
            || option.target_score < TARGET_FLOOR
            || option.transition_score < TRANSITION_FLOOR
        {
            return Err(invalid_state(
                "ranked target or transition score does not recompute above its floor",
            ));
        }
        let Some(hydrated) = self.hydrate_source(&parts.representative).await? else {
            return Ok(None);
        };
        let explanation = target_score_explanation(&parts, &option);
        explanation.validate()?;
        let continued = SemanticContinuedCoordinate {
            coordinate: option.coordinate,
            preview: preview(&hydrated),
            lifecycle: hydrated.canonical.lifecycle,
            canonical_provenance: canonical_provenance(&hydrated),
            semantic_provenance: semantic_provenance(self.ticket, &parts.representative),
            target_score: option.target_score,
            score_explanation: explanation,
        };
        let hop = SemanticHyperedgeHop {
            ordinal: 0,
            entered_from_coordinate: target.entered_from.clone(),
            edge,
            selected_relation_document: target.document.clone(),
            continued_to_coordinate: continued,
            transition_score: option.transition_score,
        };
        work.path.append_hop(hop).map(Some).map_err(Into::into)
    }

    async fn hydrate_source(
        &mut self,
        score: &SemanticExactSourceScore,
    ) -> Result<Option<SemanticHydratedCurrentSource>, SemanticGraphRootQueryError> {
        if let Some(cached) = self.hydrated.get(&score.source) {
            if cached.semantic_head != score.head {
                return Err(invalid_state(
                    "traversal hydration cache disagrees with exact source head",
                ));
            }
            return Ok(Some(cached.clone()));
        }
        let Some(batch) = run_db_before(
            self.work_deadline,
            self.backend.hydrate(std::slice::from_ref(score)),
        )
        .await?
        else {
            return Ok(None);
        };
        validate_snapshot(self.ticket, &batch.snapshot)?;
        let hydrated = batch
            .sources
            .into_iter()
            .next()
            .filter(|hydrated| {
                hydrated.canonical.source == score.source && hydrated.semantic_head == score.head
            })
            .ok_or_else(|| invalid_state("traversal hydration result is incomplete"))?;
        self.hydrated.insert(score.source.clone(), hydrated.clone());
        Ok(Some(hydrated))
    }

    fn observe_relation_omissions(&mut self, omissions: &[SemanticRelationOptionOmission]) {
        for omission in omissions {
            let key = RelationOmissionKey {
                edge_key: omission.edge_key,
                document_id: omission.document_id,
                reason: omission_reason_rank(omission.reason),
            };
            self.relation_omissions.insert(key);
        }
    }

    fn observe_target_omissions(
        &mut self,
        edge_key: EdgeKey,
        document_id: uuid::Uuid,
        omissions: &[SemanticTargetOptionOmission],
    ) {
        for omission in omissions {
            let key = TargetOmissionKey {
                edge_key,
                document_id,
                coordinate: omission.coordinate.clone(),
                reason: omission_reason_rank(omission.reason),
            };
            self.target_omissions.insert(key);
        }
    }

    fn record_global_exhaustion(
        &mut self,
        dimension: ExhaustedDimension,
        count: u64,
        work: &ExpansionWork,
    ) -> Result<(), SemanticGraphRootQueryError> {
        self.exhaustion.global_stop = true;
        self.exhaustion.record(dimension, count, work)
    }

    fn finish(
        mut self,
        selected_roots: &[SemanticGraphSelectedRoot],
        stopped: Vec<StoppedPath>,
        zero_hop: HashMap<SeedId, BranchStopReason>,
    ) -> Result<SearchOutput, SemanticGraphRootQueryError> {
        let candidates = stopped
            .into_iter()
            .map(StoppedPath::into_semantic_path)
            .collect::<Result<Vec<_>, _>>()?;
        let retained = retain_ranked_paths(
            candidates,
            usize::from(self.budget.max_paths),
            &mut self.exhaustion,
        )?;

        let roots = selected_roots
            .iter()
            .enumerate()
            .map(|(root_index, root)| {
                let seed_outcomes = root
                    .structural_entrypoints
                    .iter()
                    .enumerate()
                    .map(|(entrypoint_index, entrypoint)| {
                        let seed_id = SeedId {
                            root_index,
                            entrypoint_index,
                        };
                        let produced_path_count = retained
                            .produced_by_seed
                            .get(&seed_id)
                            .copied()
                            .unwrap_or_default();
                        SeedOutcome {
                            structural_entrypoint: entrypoint.clone(),
                            produced_path_count,
                            zero_hop_stop_reason: (produced_path_count == 0).then(|| {
                                zero_hop
                                    .get(&seed_id)
                                    .copied()
                                    .unwrap_or(BranchStopReason::FrontierExhausted)
                            }),
                        }
                    })
                    .collect();
                SemanticRoot {
                    root_id: root.root_id,
                    discovery_channels: root.discovery_channels.clone(),
                    structural_entrypoints: root.structural_entrypoints.clone(),
                    source: root.source.clone(),
                    preview: root.preview.clone(),
                    lifecycle: root.lifecycle,
                    source_status: root.source_status.clone(),
                    canonical_provenance: root.canonical_provenance.clone(),
                    semantic_provenance: root.semantic_provenance.clone(),
                    semantic_score: root.semantic_score,
                    score_explanation: root.score_explanation.clone(),
                    seed_outcomes,
                }
            })
            .collect();

        let mut exhausted_dimensions = self.exhaustion.dimensions.into_iter().collect::<Vec<_>>();
        exhausted_dimensions.sort_unstable();
        let relation_embedding_missing = self
            .relation_omissions
            .iter()
            .filter(|omission| is_embedding_omission_rank(omission.reason))
            .count() as u64;
        let target_embedding_missing = self
            .target_omissions
            .iter()
            .filter(|omission| is_embedding_omission_rank(omission.reason))
            .count() as u64;

        Ok(SearchOutput {
            roots,
            paths: retained.paths,
            expanded_coordinates: self.materialization.expanded_coordinates as u64,
            incident_edges_materialized: self.materialization.edges.len() as u64,
            relation_options_materialized: self.materialization.relations.len() as u64,
            target_options_materialized: self.materialization.targets.len() as u64,
            paths_generated: retained.paths_generated as u64,
            paths_retained: retained.paths_retained as u64,
            relation_embedding_missing,
            target_embedding_missing,
            hyperedge_too_large: self.oversized_edges.len() as u64,
            truncation_counts: self.exhaustion.counts,
            truncation_samples: canonicalize_truncation_samples(self.exhaustion.samples),
            exhausted_dimensions,
            wall_time_exhausted: self.wall_time_exhausted,
        })
    }
}

fn classify_ranked_target(
    path: &FrontierPathState,
    edge_key: EdgeKey,
    option: SemanticRankedTargetOption,
) -> (TargetRankCursor, Option<SemanticRankedTargetOption>) {
    let cursor = TargetRankCursor {
        transition_score: option.transition_score,
        target_coordinate: option.coordinate.clone(),
    };
    let admitted = (!path.visited_coordinates.contains(&option.coordinate)
        && !path.visited_edges.contains(&edge_key))
    .then_some(option);
    (cursor, admitted)
}

enum UnitAdvance {
    Done,
    Deferred,
    GlobalStop(BranchStopReason),
}

enum RelationAdvance {
    Ready(Box<TargetWork>),
    Skipped,
    GlobalStop(BranchStopReason),
}

enum Admission {
    Reused,
    Admitted,
    Deferred,
    GlobalExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoordinateExpansionAdmission {
    Admitted,
    Deferred,
    GlobalExhausted,
}

fn admit_coordinate_expansion(
    expanded: &mut usize,
    cap: usize,
    quantum: &mut usize,
) -> CoordinateExpansionAdmission {
    if *expanded >= cap {
        CoordinateExpansionAdmission::GlobalExhausted
    } else if *quantum == 0 {
        CoordinateExpansionAdmission::Deferred
    } else {
        *expanded += 1;
        *quantum -= 1;
        CoordinateExpansionAdmission::Admitted
    }
}

fn admit_new_key<T: Eq + std::hash::Hash>(
    set: &mut HashSet<T>,
    key: T,
    cap: usize,
    quantum: &mut usize,
) -> Admission {
    if set.contains(&key) {
        Admission::Reused
    } else if set.len() >= cap {
        Admission::GlobalExhausted
    } else if *quantum == 0 {
        Admission::Deferred
    } else {
        set.insert(key);
        *quantum -= 1;
        Admission::Admitted
    }
}

#[derive(Clone)]
struct SemanticScoreParts {
    representative: SemanticExactSourceScore,
    problem_score: Score,
    conditioned_evidence: Vec<ConditionedEvidence>,
    highest_gain: Score,
    second_highest_gain: Score,
    environment_gain: Score,
}

fn semantic_score_parts(
    rows: &[SemanticExactSourceScore],
    channels: &TraversalChannels,
    community_id: buzz_core::CommunityId,
) -> Result<SemanticScoreParts, SemanticGraphRootQueryError> {
    if rows.len() != 1 + channels.conditioned.len() {
        return Err(invalid_state(
            "traversal source score matrix has an incomplete channel set",
        ));
    }
    let representative = rows
        .iter()
        .find(|row| row.channel_id == channels.problem_channel_id)
        .cloned()
        .ok_or_else(|| invalid_state("traversal source score matrix lacks Q0"))?;
    if representative.source.community_id != *community_id.as_uuid() {
        return Err(invalid_state(
            "traversal source score escaped its Community",
        ));
    }
    let mut seen = HashSet::new();
    for row in rows {
        if !seen.insert(row.channel_id)
            || row.source != representative.source
            || row.head != representative.head
            || row.lifecycle != representative.lifecycle
            || row.source_status != representative.source_status
            || row.roles != representative.roles
        {
            return Err(invalid_state(
                "traversal source score matrix has conflicting rows",
            ));
        }
    }
    let conditioned_evidence = channels
        .conditioned
        .iter()
        .map(|channel| {
            let score = rows
                .iter()
                .find(|row| row.channel_id == channel.channel_id)
                .map(|row| row.score)
                .ok_or_else(|| invalid_state("traversal source score matrix lacks one Qi"))?;
            Ok(ConditionedEvidence::new(
                channel.context_coordinate.clone(),
                representative.score,
                score,
            ))
        })
        .collect::<Result<Vec<_>, SemanticGraphRootQueryError>>()?;
    let environment = environment_gain(&conditioned_evidence);
    Ok(SemanticScoreParts {
        representative,
        problem_score: rows
            .iter()
            .find(|row| row.channel_id == channels.problem_channel_id)
            .map(|row| row.score)
            .ok_or_else(|| invalid_state("traversal source score matrix lost Q0"))?,
        conditioned_evidence: environment.conditioned_evidence,
        highest_gain: environment.highest_gain,
        second_highest_gain: environment.second_highest_gain,
        environment_gain: environment.environment_gain,
    })
}

fn project_document_source(
    community_id: buzz_core::CommunityId,
    document_id: uuid::Uuid,
) -> SemanticSourceIdentity {
    SemanticSourceIdentity {
        community_id: *community_id.as_uuid(),
        kind: SemanticSourceKind::ProjectDocument,
        source_id: document_id,
    }
}

fn validate_relation_coherence(
    work: &ExpansionWork,
    document: &SemanticExactSourceScore,
    coherence: Option<&buzz_db::semantic_query::SemanticCurrentSourcePairDistance>,
    community_id: buzz_core::CommunityId,
) -> Result<(), SemanticGraphRootQueryError> {
    let entered = work
        .path
        .current_coordinate
        .as_ref()
        .ok_or_else(|| invalid_state("Coordinate relation rank lost its entered Coordinate"))?;
    let entered_source = semantic_source_identity_for_coordinate(community_id, entered)?;
    match coherence {
        Some(coherence)
            if coherence.left == document.source
                && coherence.left_head == document.head
                && coherence.right == entered_source =>
        {
            Ok(())
        }
        None if work.path.root_score.is_none() && work.path.hops.is_empty() => Ok(()),
        Some(_) | None => Err(invalid_state(
            "ranked relation coherence has conflicting source provenance",
        )),
    }
}

fn validate_target_coherence(
    target: &TargetWork,
    coordinate: &SemanticExactSourceScore,
    coherence: &buzz_db::semantic_query::SemanticCurrentSourcePairDistance,
    community_id: buzz_core::CommunityId,
) -> Result<(), SemanticGraphRootQueryError> {
    let relation_source = project_document_source(community_id, target.document.document_id);
    if coherence.left != relation_source
        || coherence.left_head != target.document_head
        || coherence.right != coordinate.source
        || coherence.right_head != coordinate.head
    {
        return Err(invalid_state(
            "ranked target coherence has conflicting source provenance",
        ));
    }
    Ok(())
}

fn relation_score_explanation(
    parts: &SemanticScoreParts,
    relation: &SemanticRankedRelationOption,
) -> ScoreExplanation {
    ScoreExplanation {
        score_role: SemanticScoreRole::RelationDocument,
        problem_score: parts.problem_score,
        conditioned_evidence: parts.conditioned_evidence.clone(),
        highest_gain: parts.highest_gain,
        second_highest_gain: parts.second_highest_gain,
        environment_gain: parts.environment_gain,
        anchor_gain: AnchorGain::None,
        local_coherence: relation.local_coherence.as_ref().map(|value| value.score),
        document_score: Some(relation.document_score),
        target_coordinate_score: None,
        transition_score: None,
        penalties: Vec::new(),
        final_score: relation.document_score,
    }
}

fn target_score_explanation(
    parts: &SemanticScoreParts,
    target: &SemanticRankedTargetOption,
) -> ScoreExplanation {
    ScoreExplanation {
        score_role: SemanticScoreRole::TargetCoordinate,
        problem_score: parts.problem_score,
        conditioned_evidence: parts.conditioned_evidence.clone(),
        highest_gain: parts.highest_gain,
        second_highest_gain: parts.second_highest_gain,
        environment_gain: parts.environment_gain,
        anchor_gain: AnchorGain::None,
        local_coherence: Some(target.relation_document_coherence.score),
        document_score: None,
        target_coordinate_score: Some(target.target_score),
        transition_score: None,
        penalties: Vec::new(),
        final_score: target.target_score,
    }
}

fn preview(hydrated: &SemanticHydratedCurrentSource) -> SemanticSourcePreview {
    SemanticSourcePreview {
        title: hydrated.canonical.title.clone(),
        summary: hydrated.canonical.summary.clone(),
        summary_omitted_reason: None,
    }
}

fn canonical_provenance(
    hydrated: &SemanticHydratedCurrentSource,
) -> buzz_semantic_query::CanonicalSourceProvenance {
    buzz_semantic_query::CanonicalSourceProvenance {
        source_basis: hydrated.canonical.source_basis.clone(),
        source_invalidation_epoch: hydrated.canonical.source_invalidation_epoch,
        source_snapshot_digest: hydrated.canonical.source_snapshot_digest,
        summary_coverage: hydrated.semantic_head.summary_coverage,
    }
}

fn semantic_provenance(
    ticket: &SemanticGraphQueryTicket,
    score: &SemanticExactSourceScore,
) -> SemanticProvenance {
    SemanticProvenance {
        generation_id: ticket.generation.generation_id,
        unit_key: score.head.unit_key.clone(),
        source_snapshot_digest: score.head.snapshot_digest,
        source_generation_contract_digest: ticket.query_fences.source_generation_contract_digest,
        embedding_space_fence: ticket.query_fences.embedding_space_fence,
    }
}

fn validate_snapshot(
    ticket: &SemanticGraphQueryTicket,
    snapshot: &buzz_db::semantic_query::SemanticGraphSnapshotBinding,
) -> Result<(), SemanticGraphRootQueryError> {
    if snapshot.community_id != ticket.community_id
        || snapshot.generation_id != ticket.generation.generation_id
        || snapshot.query_fences != ticket.query_fences
        || snapshot.extractor_version != ticket.generation.extractor_version
        || snapshot.project_context_revision != ticket.project_context_revision
        || snapshot.observed_at != ticket.observed_at
    {
        return Err(invalid_state(
            "traversal DB result escaped its repeatable-read snapshot",
        ));
    }
    Ok(())
}

fn validate_cached_edge(
    edge: &SemanticEdgeObservation,
    expectation: &SemanticHyperedgeExpectation,
) -> Result<(), SemanticGraphRootQueryError> {
    if edge.edge_key != expectation.edge_key
        || edge.provenance != expectation.edge_provenance
        || expectation
            .required_binding
            .as_ref()
            .is_some_and(|binding| !edge.current_context_document_bindings.contains(binding))
    {
        return Err(invalid_state(
            "cached Hyperedge does not satisfy its exact expectation",
        ));
    }
    Ok(())
}

fn omission_reason_rank(reason: SemanticTraversalSourceOmissionReason) -> u8 {
    match reason {
        SemanticTraversalSourceOmissionReason::SourceNotFound => 0,
        SemanticTraversalSourceOmissionReason::SourceTombstoned => 1,
        SemanticTraversalSourceOmissionReason::SourceDeleted => 2,
        SemanticTraversalSourceOmissionReason::SourceIneligible => 3,
        SemanticTraversalSourceOmissionReason::LifecycleFiltered => 4,
        SemanticTraversalSourceOmissionReason::SemanticHeadMissing => 5,
        SemanticTraversalSourceOmissionReason::SemanticHeadBuilding => 6,
        SemanticTraversalSourceOmissionReason::SemanticHeadFailed => 7,
        SemanticTraversalSourceOmissionReason::SemanticHeadUnsupported => 8,
        SemanticTraversalSourceOmissionReason::SourceNotReadable => 9,
    }
}

fn is_embedding_omission_rank(rank: u8) -> bool {
    (5..=8).contains(&rank)
}

async fn run_db_before<F, T>(
    deadline: Instant,
    future: F,
) -> Result<Option<T>, SemanticGraphRootQueryError>
where
    F: Future<Output = Result<T, buzz_db::DbError>>,
{
    if Instant::now() >= deadline {
        return Ok(None);
    }
    match tokio::time::timeout_at(deadline, future).await {
        Ok(result) => result
            .map(Some)
            .map_err(SemanticGraphRootQueryError::Database),
        Err(_) => Ok(None),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SeedId {
    root_index: usize,
    entrypoint_index: usize,
}

struct ExpansionWork {
    seed_id: SeedId,
    seed_order: Vec<u8>,
    path: FrontierPathState,
    incident_started: bool,
    incident_exhausted: bool,
    incident_after: Option<RelationRankCursor>,
    pending_relations: VecDeque<SemanticRankedRelationOption>,
    pending_targets: VecDeque<TargetWork>,
    accumulator: Option<BoundedSuccessorAccumulator>,
    qualifying_successors_seen: usize,
    stop: StopObservations,
}

impl ExpansionWork {
    fn coordinate(
        seed_id: SeedId,
        seed_order: Vec<u8>,
        path: FrontierPathState,
        beam_width: u16,
    ) -> Result<Self, SemanticGraphRootQueryError> {
        Ok(Self {
            seed_id,
            seed_order,
            path,
            incident_started: false,
            incident_exhausted: false,
            incident_after: None,
            pending_relations: VecDeque::new(),
            pending_targets: VecDeque::new(),
            accumulator: Some(BoundedSuccessorAccumulator::new(beam_width)?),
            qualifying_successors_seen: 0,
            stop: StopObservations::default(),
        })
    }

    fn relation_root(
        seed_id: SeedId,
        seed_order: Vec<u8>,
        path: FrontierPathState,
        target: TargetWork,
        beam_width: u16,
    ) -> Result<Self, SemanticGraphRootQueryError> {
        let mut pending_targets = VecDeque::new();
        pending_targets.push_back(target);
        Ok(Self {
            seed_id,
            seed_order,
            path,
            incident_started: true,
            incident_exhausted: true,
            incident_after: None,
            pending_relations: VecDeque::new(),
            pending_targets,
            accumulator: Some(BoundedSuccessorAccumulator::new(beam_width)?),
            qualifying_successors_seen: 0,
            stop: StopObservations::default(),
        })
    }

    fn stop_reason(&self) -> BranchStopReason {
        self.stop.reason()
    }
}

#[derive(Clone)]
struct TargetWork {
    entered_from: Option<ProjectContextCoordinate>,
    relation_admitted: bool,
    expectation: SemanticHyperedgeExpectation,
    edge: Option<SemanticEdgeObservation>,
    document: SemanticRelationDocument,
    document_head: SemanticCurrentHead,
    after: Option<TargetRankCursor>,
    exhausted: bool,
}

#[derive(Debug, Default, Clone)]
struct StopObservations {
    below_relevance: bool,
    cycle_or_duplicate: bool,
    hyperedge_too_large: bool,
}

impl StopObservations {
    fn reason(&self) -> BranchStopReason {
        let mut reasons = Vec::new();
        if self.hyperedge_too_large {
            reasons.push(BranchStopReason::HyperedgeTooLarge);
        }
        if self.cycle_or_duplicate {
            reasons.push(BranchStopReason::CycleOrDuplicate);
        }
        if self.below_relevance {
            reasons.push(BranchStopReason::BelowRelevanceThreshold);
        }
        reasons.push(BranchStopReason::FrontierExhausted);
        highest_precedence_stop(&reasons).unwrap_or(BranchStopReason::FrontierExhausted)
    }
}

enum AdvanceOutcome {
    Sealed(Vec<FrontierPathState>),
    Deferred,
    GlobalStop(BranchStopReason),
}

#[derive(Debug, Clone, Copy)]
struct Quantum {
    expanded_coordinates: usize,
    incident_edges: usize,
    relation_options: usize,
    target_options: usize,
}

impl Quantum {
    fn first_wave(
        materialization: &MaterializationState,
        budget: &SemanticGraphQueryBudget,
        remaining_seeds: usize,
    ) -> Self {
        Self {
            expanded_coordinates: first_wave_slice(
                usize::from(budget.max_expanded_coordinates)
                    .saturating_sub(materialization.expanded_coordinates),
                remaining_seeds,
            ),
            incident_edges: first_wave_slice(
                usize::from(budget.max_incident_edges_materialized)
                    .saturating_sub(materialization.edges.len()),
                remaining_seeds,
            ),
            relation_options: first_wave_slice(
                usize::from(budget.max_relation_options_materialized)
                    .saturating_sub(materialization.relations.len()),
                remaining_seeds,
            ),
            target_options: first_wave_slice(
                usize::from(budget.max_target_options_materialized)
                    .saturating_sub(materialization.targets.len()),
                remaining_seeds,
            ),
        }
    }

    fn global_step() -> Self {
        Self {
            expanded_coordinates: 1,
            incident_edges: 1,
            relation_options: 1,
            target_options: 1,
        }
    }
}

#[derive(Debug, Default)]
struct MaterializationState {
    expanded_coordinates: usize,
    edges: HashSet<EdgeKey>,
    relations: HashSet<RelationMaterializationKey>,
    targets: HashSet<TargetMaterializationKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RelationMaterializationKey {
    entered_from: Option<ProjectContextCoordinate>,
    edge_key: EdgeKey,
    document_id: uuid::Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TargetMaterializationKey {
    entered_from: Option<ProjectContextCoordinate>,
    edge_key: EdgeKey,
    document_id: uuid::Uuid,
    coordinate: ProjectContextCoordinate,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RelationOmissionKey {
    edge_key: EdgeKey,
    document_id: uuid::Uuid,
    reason: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TargetOmissionKey {
    edge_key: EdgeKey,
    document_id: uuid::Uuid,
    coordinate: ProjectContextCoordinate,
    reason: u8,
}

#[derive(Debug, Default)]
struct ExhaustionState {
    dimensions: HashSet<ExhaustedDimension>,
    counts: buzz_semantic_query::TruncationCountsByDimension,
    samples: Vec<TruncationSample>,
    global_stop: bool,
}

impl ExhaustionState {
    fn record(
        &mut self,
        dimension: ExhaustedDimension,
        count: u64,
        work: &ExpansionWork,
    ) -> Result<(), SemanticGraphRootQueryError> {
        self.dimensions.insert(dimension);
        let target = match dimension {
            ExhaustedDimension::RecallPerChannel => &mut self.counts.recall_per_channel,
            ExhaustedDimension::SemanticRoots => &mut self.counts.semantic_roots,
            ExhaustedDimension::HopsPerPath => &mut self.counts.hops_per_path,
            ExhaustedDimension::BeamWidth => &mut self.counts.beam_width,
            ExhaustedDimension::ExpandedCoordinates => &mut self.counts.expanded_coordinates,
            ExhaustedDimension::IncidentEdgesMaterialized => {
                &mut self.counts.incident_edges_materialized
            }
            ExhaustedDimension::RelationOptionsMaterialized => {
                &mut self.counts.relation_options_materialized
            }
            ExhaustedDimension::TargetOptionsMaterialized => {
                &mut self.counts.target_options_materialized
            }
            ExhaustedDimension::Paths => &mut self.counts.paths,
            ExhaustedDimension::ResponseBytes => &mut self.counts.response_bytes,
        };
        *target = target.saturating_add(count);
        if self.samples.len() < MAX_TRUNCATION_SAMPLES {
            self.samples.push(TruncationSample {
                root_id: work.path.root_id,
                path_id: if work.path.hops.is_empty() {
                    None
                } else {
                    Some(derive_path_id(work.path.root_id, &work.path.hops)?)
                },
                structural_entrypoint: work.path.structural_entrypoint.clone(),
                dimension,
            });
        }
        Ok(())
    }

    fn record_state(
        &mut self,
        dimension: ExhaustedDimension,
        count: u64,
        state: &FrontierPathState,
    ) -> Result<(), SemanticGraphRootQueryError> {
        self.add_count(dimension, count);
        if self.samples.len() < MAX_TRUNCATION_SAMPLES {
            self.samples.push(TruncationSample {
                root_id: state.root_id,
                path_id: if state.hops.is_empty() {
                    None
                } else {
                    Some(derive_path_id(state.root_id, &state.hops)?)
                },
                structural_entrypoint: state.structural_entrypoint.clone(),
                dimension,
            });
        }
        Ok(())
    }

    fn record_path_limit(
        &mut self,
        count: u64,
        suppressed: &[RankedStoppedPath],
    ) -> Result<(), SemanticGraphRootQueryError> {
        self.add_count(ExhaustedDimension::Paths, count);
        for candidate in suppressed {
            if self.samples.len() >= MAX_TRUNCATION_SAMPLES {
                break;
            }
            self.samples.push(TruncationSample {
                root_id: candidate.path.root_id,
                path_id: Some(candidate.path.path_id),
                structural_entrypoint: candidate.structural_entrypoint.clone(),
                dimension: ExhaustedDimension::Paths,
            });
        }
        Ok(())
    }

    fn add_count(&mut self, dimension: ExhaustedDimension, count: u64) {
        self.dimensions.insert(dimension);
        let target = truncation_count_mut(&mut self.counts, dimension);
        *target = target.saturating_add(count);
    }
}

fn truncation_count_mut(
    counts: &mut buzz_semantic_query::TruncationCountsByDimension,
    dimension: ExhaustedDimension,
) -> &mut u64 {
    match dimension {
        ExhaustedDimension::RecallPerChannel => &mut counts.recall_per_channel,
        ExhaustedDimension::SemanticRoots => &mut counts.semantic_roots,
        ExhaustedDimension::HopsPerPath => &mut counts.hops_per_path,
        ExhaustedDimension::BeamWidth => &mut counts.beam_width,
        ExhaustedDimension::ExpandedCoordinates => &mut counts.expanded_coordinates,
        ExhaustedDimension::IncidentEdgesMaterialized => &mut counts.incident_edges_materialized,
        ExhaustedDimension::RelationOptionsMaterialized => {
            &mut counts.relation_options_materialized
        }
        ExhaustedDimension::TargetOptionsMaterialized => &mut counts.target_options_materialized,
        ExhaustedDimension::Paths => &mut counts.paths,
        ExhaustedDimension::ResponseBytes => &mut counts.response_bytes,
    }
}

struct StoppedPath {
    seed_id: SeedId,
    state: FrontierPathState,
    reason: BranchStopReason,
}

impl StoppedPath {
    fn from_work(work: ExpansionWork) -> Result<Self, SemanticGraphRootQueryError> {
        if work.path.hops.is_empty() {
            return Err(invalid_state("zero-hop state cannot become a SemanticPath"));
        }
        let reason = work.stop_reason();
        Ok(Self {
            seed_id: work.seed_id,
            state: work.path,
            reason,
        })
    }

    fn from_state(
        seed_id: SeedId,
        state: FrontierPathState,
        reason: BranchStopReason,
    ) -> Result<Self, SemanticGraphRootQueryError> {
        if state.hops.is_empty() {
            return Err(invalid_state("zero-hop state cannot become a SemanticPath"));
        }
        Ok(Self {
            seed_id,
            state,
            reason,
        })
    }

    fn into_semantic_path(self) -> Result<RankedStoppedPath, SemanticGraphRootQueryError> {
        let FrontierPathState {
            root_id,
            structural_entrypoint,
            hops,
            root_score,
            path_score: observed_path_score,
            ..
        } = self.state;
        let terminal_coordinate = hops
            .last()
            .map(|hop| hop.continued_to_coordinate.coordinate.clone())
            .ok_or_else(|| invalid_state("stopped traversal path has no hop"))?;
        let transitions = hops
            .iter()
            .map(|hop| hop.transition_score)
            .collect::<Vec<_>>();
        let path_score_explanation = path_score(root_score, &transitions).map_err(|error| {
            SemanticGraphRootQueryError::Contract(SemanticGraphQueryError::InvalidScore(
                error.to_string(),
            ))
        })?;
        let final_score = path_score_explanation
            .final_score
            .ok_or_else(|| invalid_state("non-empty traversal path has no score"))?;
        if observed_path_score != Some(final_score) {
            return Err(invalid_state(
                "stopped traversal path score disagrees with its prefix",
            ));
        }
        let path_id = derive_path_id(root_id, &hops)?;
        Ok(RankedStoppedPath {
            seed_id: self.seed_id,
            structural_entrypoint,
            path: SemanticPath {
                path_id,
                root_id,
                hops,
                terminal_coordinate,
                path_score: final_score,
                path_score_explanation,
                branch_stop_reason: self.reason,
            },
        })
    }
}

struct RankedStoppedPath {
    seed_id: SeedId,
    structural_entrypoint: RootStructuralEntrypoint,
    path: SemanticPath,
}

struct SearchOutput {
    roots: Vec<SemanticRoot>,
    paths: Vec<SemanticPath>,
    expanded_coordinates: u64,
    incident_edges_materialized: u64,
    relation_options_materialized: u64,
    target_options_materialized: u64,
    paths_generated: u64,
    paths_retained: u64,
    relation_embedding_missing: u64,
    target_embedding_missing: u64,
    hyperedge_too_large: u64,
    truncation_counts: buzz_semantic_query::TruncationCountsByDimension,
    truncation_samples: Vec<TruncationSample>,
    exhausted_dimensions: Vec<ExhaustedDimension>,
    wall_time_exhausted: bool,
}

fn publish_successors(
    successors: Vec<FrontierPathState>,
    seed_id: SeedId,
    seed_order: Vec<u8>,
    budget: &SemanticGraphQueryBudget,
    queue: &mut Vec<ExpansionWork>,
    stopped: &mut Vec<StoppedPath>,
    exhaustion: &mut ExhaustionState,
) -> Result<(), SemanticGraphRootQueryError> {
    for successor in successors {
        if successor.hops.len() >= usize::from(budget.max_hops_per_path) {
            exhaustion.record_state(ExhaustedDimension::HopsPerPath, 1, &successor)?;
            stopped.push(StoppedPath::from_state(
                seed_id,
                successor,
                BranchStopReason::MaxHopsReached,
            )?);
        } else {
            queue.push(ExpansionWork::coordinate(
                seed_id,
                seed_order.clone(),
                successor,
                budget.beam_width,
            )?);
        }
    }
    Ok(())
}

fn finish_cutoff_work(
    mut work: ExpansionWork,
    reason: BranchStopReason,
    budget: &SemanticGraphQueryBudget,
    exhaustion: &mut ExhaustionState,
    stopped: &mut Vec<StoppedPath>,
    zero_hop: &mut HashMap<SeedId, BranchStopReason>,
) -> Result<(), SemanticGraphRootQueryError> {
    let successors = work
        .accumulator
        .take()
        .ok_or_else(|| invalid_state("cutoff traversal work lost its successor accumulator"))?
        .into_successors();
    let beam_suppressed = work
        .qualifying_successors_seen
        .saturating_sub(successors.len());
    if beam_suppressed > 0 {
        exhaustion.record(ExhaustedDimension::BeamWidth, beam_suppressed as u64, &work)?;
    }
    if successors.is_empty() {
        if work.path.hops.is_empty() {
            zero_hop.insert(work.seed_id, reason);
        } else {
            stopped.push(StoppedPath::from_state(work.seed_id, work.path, reason)?);
        }
    } else {
        for successor in successors {
            if successor.hops.len() >= usize::from(budget.max_hops_per_path) {
                exhaustion.record_state(ExhaustedDimension::HopsPerPath, 1, &successor)?;
            }
            stopped.push(StoppedPath::from_state(work.seed_id, successor, reason)?);
        }
    }
    Ok(())
}

fn compare_semantic_paths(left: &RankedStoppedPath, right: &RankedStoppedPath) -> Ordering {
    right
        .path
        .path_score
        .cmp(&left.path.path_score)
        .then_with(|| left.path.path_id.cmp(&right.path.path_id))
}

struct RetainedPaths {
    paths: Vec<SemanticPath>,
    paths_generated: usize,
    paths_retained: usize,
    produced_by_seed: HashMap<SeedId, u32>,
}

fn retain_ranked_paths(
    mut candidates: Vec<RankedStoppedPath>,
    retained_limit: usize,
    exhaustion: &mut ExhaustionState,
) -> Result<RetainedPaths, SemanticGraphRootQueryError> {
    candidates.sort_by(compare_semantic_paths);
    candidates.dedup_by(|left, right| left.path.path_id == right.path.path_id);

    // Generated accounting describes every provenance-distinct stopped path
    // produced by search. Diversity and max_paths are later retention policies
    // and must not rewrite per-seed search outcomes.
    let paths_generated = candidates.len();
    let produced_by_seed = candidates
        .iter()
        .fold(HashMap::new(), |mut counts, candidate| {
            let count = counts.entry(candidate.seed_id).or_insert(0_u32);
            *count = count.saturating_add(1);
            counts
        });

    // Endpoint diversity is not a budget dimension. Preserve the best two
    // full-provenance paths to one terminal Coordinate.
    let mut endpoints = HashMap::<ProjectContextCoordinate, u8>::new();
    candidates.retain(|candidate| {
        let count = endpoints
            .entry(candidate.path.terminal_coordinate.clone())
            .or_default();
        if *count >= 2 {
            false
        } else {
            *count += 1;
            true
        }
    });

    if candidates.len() > retained_limit {
        let suppressed = candidates.len() - retained_limit;
        exhaustion.record_path_limit(suppressed as u64, &candidates[retained_limit..])?;
        candidates.truncate(retained_limit);
    }
    let paths_retained = candidates.len();
    Ok(RetainedPaths {
        paths: candidates
            .into_iter()
            .map(|candidate| candidate.path)
            .collect(),
        paths_generated,
        paths_retained,
        produced_by_seed,
    })
}

fn canonicalize_truncation_samples(mut samples: Vec<TruncationSample>) -> Vec<TruncationSample> {
    samples.sort_by(|left, right| {
        left.dimension
            .cmp(&right.dimension)
            .then_with(|| left.root_id.cmp(&right.root_id))
            .then_with(|| left.path_id.cmp(&right.path_id))
            .then_with(|| {
                entrypoint_key(&left.structural_entrypoint)
                    .cmp(&entrypoint_key(&right.structural_entrypoint))
            })
    });
    samples.dedup_by(|left, right| {
        left.dimension == right.dimension
            && left.root_id == right.root_id
            && left.path_id == right.path_id
            && left.structural_entrypoint == right.structural_entrypoint
    });
    samples.truncate(MAX_TRUNCATION_SAMPLES);
    samples
}

fn entrypoint_key(entrypoint: &RootStructuralEntrypoint) -> Vec<u8> {
    let mut key = Vec::new();
    append_entrypoint_key(&mut key, entrypoint);
    key
}

fn apply_search_coverage(coverage: &mut SemanticGraphQueryCoverage, search: &SearchOutput) {
    coverage.expanded_coordinates = search.expanded_coordinates;
    coverage.incident_edges_materialized = search.incident_edges_materialized;
    coverage.relation_options_materialized = search.relation_options_materialized;
    coverage.target_options_materialized = search.target_options_materialized;
    coverage.paths_generated = search.paths_generated;
    coverage.paths_retained = search.paths_retained;
    coverage.degraded_mode_counts.relation_embedding_missing = search.relation_embedding_missing;
    coverage.degraded_mode_counts.target_embedding_missing = search.target_embedding_missing;
    coverage.degraded_mode_counts.hyperedge_too_large = search.hyperedge_too_large;
    add_truncation_counts(
        &mut coverage.truncation_counts_by_dimension,
        &search.truncation_counts,
    );
    coverage
        .truncation_samples
        .extend(search.truncation_samples.iter().cloned());
    coverage.truncation_samples =
        canonicalize_truncation_samples(std::mem::take(&mut coverage.truncation_samples));
}

fn add_truncation_counts(
    target: &mut buzz_semantic_query::TruncationCountsByDimension,
    added: &buzz_semantic_query::TruncationCountsByDimension,
) {
    target.recall_per_channel = target
        .recall_per_channel
        .saturating_add(added.recall_per_channel);
    target.semantic_roots = target.semantic_roots.saturating_add(added.semantic_roots);
    target.hops_per_path = target.hops_per_path.saturating_add(added.hops_per_path);
    target.beam_width = target.beam_width.saturating_add(added.beam_width);
    target.expanded_coordinates = target
        .expanded_coordinates
        .saturating_add(added.expanded_coordinates);
    target.incident_edges_materialized = target
        .incident_edges_materialized
        .saturating_add(added.incident_edges_materialized);
    target.relation_options_materialized = target
        .relation_options_materialized
        .saturating_add(added.relation_options_materialized);
    target.target_options_materialized = target
        .target_options_materialized
        .saturating_add(added.target_options_materialized);
    target.paths = target.paths.saturating_add(added.paths);
    target.response_bytes = target.response_bytes.saturating_add(added.response_bytes);
}

fn build_seed_work(
    roots: &[SemanticGraphSelectedRoot],
    beam_width: u16,
) -> Result<Vec<ExpansionWork>, SemanticGraphRootQueryError> {
    let mut seeds = Vec::new();
    for (root_index, root) in roots.iter().enumerate() {
        for (entrypoint_index, entrypoint) in root.structural_entrypoints.iter().enumerate() {
            let seed_id = SeedId {
                root_index,
                entrypoint_index,
            };
            let seed_order = seed_order_key(&root.source, entrypoint);
            let path =
                FrontierPathState::seed(root.root_id, entrypoint.clone(), root.semantic_score);
            match entrypoint {
                RootStructuralEntrypoint::Coordinate { .. } => seeds.push(
                    ExpansionWork::coordinate(seed_id, seed_order, path, beam_width)?,
                ),
                RootStructuralEntrypoint::ContextDocument {
                    edge_key,
                    document_id,
                    edge_provenance,
                    binding_provenance,
                } => {
                    if root.source.kind != SemanticSourceKind::ProjectDocument
                        || root.source.source_id != *document_id
                    {
                        return Err(invalid_state(
                            "relation root source does not match its Document entrypoint",
                        ));
                    }
                    let semantic_head = root.semantic_head.clone().ok_or_else(|| {
                        invalid_state("relation root lacks an exact semantic head")
                    })?;
                    let semantic_provenance = root
                        .semantic_provenance
                        .clone()
                        .ok_or_else(|| invalid_state("relation root lacks semantic provenance"))?;
                    let document_score = root
                        .semantic_score
                        .ok_or_else(|| invalid_state("relation root lacks its semantic score"))?;
                    if document_score < RELATION_FLOOR {
                        return Err(invalid_state("relation root is below the relation floor"));
                    }
                    let mut explanation = root.score_explanation.clone().ok_or_else(|| {
                        invalid_state("relation root lacks its score explanation")
                    })?;
                    explanation.score_role = SemanticScoreRole::RelationRoot;
                    explanation.validate()?;
                    let binding = ContextDocumentBindingObservation {
                        document_id: *document_id,
                        provenance: binding_provenance.clone(),
                    };
                    let target = TargetWork {
                        entered_from: None,
                        relation_admitted: false,
                        expectation: SemanticHyperedgeExpectation {
                            edge_key: *edge_key,
                            edge_provenance: edge_provenance.clone(),
                            required_binding: Some(binding.clone()),
                        },
                        edge: None,
                        document: SemanticRelationDocument {
                            document_id: *document_id,
                            binding_provenance: binding.provenance,
                            preview: root.preview.clone(),
                            canonical_provenance: root.canonical_provenance.clone(),
                            semantic_provenance,
                            document_score,
                            score_explanation: explanation,
                        },
                        document_head: semantic_head,
                        after: None,
                        exhausted: false,
                    };
                    seeds.push(ExpansionWork::relation_root(
                        seed_id, seed_order, path, target, beam_width,
                    )?);
                }
            }
        }
    }
    Ok(seeds)
}

fn seed_order_key(
    source: &SemanticSourceIdentity,
    entrypoint: &RootStructuralEntrypoint,
) -> Vec<u8> {
    let mut key = Vec::with_capacity(128);
    key.extend_from_slice(source.community_id.as_bytes());
    match source.kind {
        SemanticSourceKind::ProjectView(kind) => {
            key.extend_from_slice(&[0, project_view_kind_rank(kind)]);
        }
        SemanticSourceKind::ProjectDocument => key.extend_from_slice(&[1, 0]),
        SemanticSourceKind::Meeting => key.extend_from_slice(&[2, 0]),
    }
    key.extend_from_slice(source.source_id.as_bytes());
    append_entrypoint_key(&mut key, entrypoint);
    key
}

const fn project_view_kind_rank(kind: buzz_semantic::ProjectViewSemanticType) -> u8 {
    use buzz_semantic::ProjectViewSemanticType;
    match kind {
        ProjectViewSemanticType::ProjectProfile => 0,
        ProjectViewSemanticType::Goal => 1,
        ProjectViewSemanticType::Role => 2,
        ProjectViewSemanticType::Plan => 3,
        ProjectViewSemanticType::Stage => 4,
        ProjectViewSemanticType::Requirement => 5,
        ProjectViewSemanticType::Issue => 6,
        ProjectViewSemanticType::Work => 7,
        ProjectViewSemanticType::Resource => 8,
    }
}

fn append_entrypoint_key(output: &mut Vec<u8>, entrypoint: &RootStructuralEntrypoint) {
    match entrypoint {
        RootStructuralEntrypoint::Coordinate { coordinate } => {
            output.push(0);
            append_coordinate_key(output, coordinate);
        }
        RootStructuralEntrypoint::ContextDocument {
            edge_key,
            document_id,
            ..
        } => {
            output.push(1);
            output.extend_from_slice(edge_key.as_bytes());
            output.extend_from_slice(document_id.as_bytes());
        }
    }
}

fn append_coordinate_key(output: &mut Vec<u8>, coordinate: &ProjectContextCoordinate) {
    match coordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        } => {
            output.push(0);
            output.extend_from_slice(object_type.as_str().as_bytes());
            output.push(0);
            output.extend_from_slice(object_id.as_bytes());
        }
        ProjectContextCoordinate::Document { document_id } => {
            output.push(1);
            output.extend_from_slice(document_id.as_bytes());
        }
        ProjectContextCoordinate::Meeting { meeting_id } => {
            output.push(2);
            output.extend_from_slice(meeting_id.as_bytes());
        }
    }
}

fn compare_work(left: &ExpansionWork, right: &ExpansionWork) -> Ordering {
    right
        .path
        .scheduling_priority()
        .cmp(&left.path.scheduling_priority())
        .then_with(|| left.seed_order.cmp(&right.seed_order))
        .then_with(|| work_continuation_key(left).cmp(&work_continuation_key(right)))
}

fn work_continuation_key(work: &ExpansionWork) -> Vec<u8> {
    let mut key = Vec::new();
    for hop in &work.path.hops {
        key.extend_from_slice(hop.edge.edge_key.as_bytes());
        key.extend_from_slice(hop.selected_relation_document.document_id.as_bytes());
        append_coordinate_key(&mut key, &hop.continued_to_coordinate.coordinate);
    }
    if let Some(cursor) = &work.incident_after {
        key.push(0);
        key.extend_from_slice(&cursor.document_score.raw().to_be_bytes());
        key.extend_from_slice(cursor.edge_key.as_bytes());
        key.extend_from_slice(cursor.document_id.as_bytes());
    }
    if let Some(target) = work.pending_targets.front() {
        key.push(1);
        key.extend_from_slice(target.expectation.edge_key.as_bytes());
        key.extend_from_slice(target.document.document_id.as_bytes());
        if let Some(cursor) = &target.after {
            key.extend_from_slice(&cursor.transition_score.raw().to_be_bytes());
            append_coordinate_key(&mut key, &cursor.target_coordinate);
        }
    }
    key
}

fn invalid_state(reason: &'static str) -> SemanticGraphRootQueryError {
    SemanticGraphRootQueryError::Contract(SemanticGraphQueryError::InvalidState(reason.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_core::CommunityId;
    use buzz_db::semantic_query::SemanticExactQueryVector;
    use buzz_project_context::canonicalize_coordinates;
    use buzz_project_view::ProjectViewObjectType;
    use buzz_semantic::{
        DeterministicFakeEncoder, ProjectDocumentSourceBasis, ProjectViewSemanticType,
        ProjectViewSourceBasis, SemanticCoverage, SemanticEncoder, SemanticLifecycleClass,
        SemanticSourceBasis,
    };
    use buzz_semantic_query::{
        budget_profile_digest, build_problem_query_encoder_input, candidate_score, derive_root_id,
        query_contract_digest, ranking_contract_digest, CanonicalSourceProvenance,
        DegradedModeCounts, EmbeddingCoverageCounts, OmittedContextChannelCounts,
        OmittedForResponseBudgetCounts, ProjectContextEdgeProvenance, ProviderEncodedSemanticInput,
        ProviderEncodedSemanticInputBundle, RootDiscoveryChannel, SemanticGraphQuery,
        SemanticGraphQueryCoverage, SemanticGraphQueryInputObservations,
        SemanticGraphQueryObservations, SemanticQueryInputBundle, TruncationCountsByDimension,
    };
    use chrono::Utc;
    use nostr::Keys;

    use crate::semantic_graph_response::{
        pack_semantic_graph_response, sign_packed_semantic_graph_response,
        validate_completed_semantic_graph_forest, SemanticGraphResponsePackingInput,
    };

    #[tokio::test]
    async fn snapshot_close_reserve_remains_usable_after_traversal_deadline() {
        let traversal_deadline = Instant::now();
        let snapshot_close_deadline = traversal_deadline + std::time::Duration::from_secs(1);
        assert!(Instant::now() >= traversal_deadline);

        let closed = run_db_before(snapshot_close_deadline, async {
            Ok::<_, buzz_db::DbError>(())
        })
        .await
        .expect("snapshot close is not a database error");

        assert_eq!(closed, Some(()));
    }

    fn digest(value: u8) -> Digest32 {
        Digest32::from_bytes([value; 32])
    }

    fn score(value: u32) -> Score {
        Score::new(value).expect("score fixture")
    }

    fn edge(value: u8) -> EdgeKey {
        EdgeKey::from_hex(&hex::encode([value; 32])).expect("edge fixture")
    }

    fn work(value: u128) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: uuid::Uuid::from_u128(value),
        }
    }

    fn fixture_uuid(seed: u64) -> uuid::Uuid {
        uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}"))
            .expect("UUIDv4 fixture")
    }

    fn linear_work(seed: u64) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: fixture_uuid(seed),
        }
    }

    fn view_source(value: u128) -> SemanticSourceIdentity {
        SemanticSourceIdentity {
            community_id: uuid::Uuid::from_u128(1),
            kind: SemanticSourceKind::ProjectView(ProjectViewSemanticType::Work),
            source_id: uuid::Uuid::from_u128(value),
        }
    }

    fn view_head(value: u8) -> SemanticCurrentHead {
        SemanticCurrentHead {
            invalidation_epoch: 1,
            snapshot_digest: digest(value),
            source_basis: SemanticSourceBasis::ProjectView(ProjectViewSourceBasis {
                schema_version: 3,
                object_revision: 1,
                source_change_id: digest(value.wrapping_add(1)),
            }),
            unit_set_id: uuid::Uuid::from_u128(100 + u128::from(value)),
            unit_key: "overview".to_owned(),
            semantic_text_digest: digest(value.wrapping_add(2)),
            summary_coverage: SemanticCoverage::TitleAndSummary,
        }
    }

    fn target_option(coordinate: ProjectContextCoordinate) -> SemanticRankedTargetOption {
        let source_id = match &coordinate {
            ProjectContextCoordinate::ProjectViewObject { object_id, .. } => *object_id,
            ProjectContextCoordinate::Document { document_id } => *document_id,
            ProjectContextCoordinate::Meeting { meeting_id } => *meeting_id,
        };
        let source = view_source(source_id.as_u128());
        let head = view_head(source.source_id.as_u128() as u8);
        let document = SemanticSourceIdentity {
            community_id: source.community_id,
            kind: SemanticSourceKind::ProjectDocument,
            source_id: uuid::Uuid::from_u128(99),
        };
        SemanticRankedTargetOption {
            coordinate,
            channel_scores: vec![SemanticExactSourceScore {
                channel_id: digest(1),
                source: source.clone(),
                head: head.clone(),
                lifecycle: SemanticLifecycleClass::Active,
                source_status: None,
                roles: buzz_db::semantic_query::SemanticGraphStructuralRoles {
                    coordinate: true,
                    coordinate_entry_eligible: true,
                    coordinate_incident_edge_keys: vec![edge(7)],
                    context_document_bindings: Vec::new(),
                },
                score: score(900_000),
                channel_rank: 1,
            }],
            relation_document_coherence:
                buzz_db::semantic_query::SemanticCurrentSourcePairDistance {
                    left: document,
                    right: source,
                    left_head: view_head(99),
                    right_head: head,
                    score: score(900_000),
                },
            target_score: score(900_000),
            transition_score: score(900_000),
        }
    }

    fn binding(value: u8, document_id: uuid::Uuid) -> ContextDocumentBindingObservation {
        ContextDocumentBindingObservation {
            document_id,
            provenance: buzz_semantic_query::ProjectContextBindingProvenance {
                binding_context_revision: u64::from(value),
                source_change_id: digest(value),
                projection_event_id: digest(value.wrapping_add(1)),
            },
        }
    }

    fn one_hop_successor(target_value: u128) -> FrontierPathState {
        let entered = work(1);
        let target = work(target_value);
        let complete_coordinates = vec![entered.clone(), work(2), work(3)];
        let edge_key = edge(7);
        let document_id = uuid::Uuid::from_u128(99);
        let selected_binding = binding(1, document_id);
        let problem_score = score(900_000);
        let coherence = score(900_000);
        let relation_score = document_score(problem_score, Score::ZERO, Some(coherence));
        let target_score = target_coordinate_score(problem_score, Score::ZERO, coherence);
        let transition_score = harmonic_score(relation_score, target_score);
        let document_head = SemanticCurrentHead {
            source_basis: SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                document_revision: 1,
                source_change_id: digest(40),
            }),
            ..view_head(40)
        };
        let target_head = view_head(target_value as u8);
        let provenance = |head: &SemanticCurrentHead| CanonicalSourceProvenance {
            source_basis: head.source_basis.clone(),
            source_invalidation_epoch: head.invalidation_epoch,
            source_snapshot_digest: head.snapshot_digest,
            summary_coverage: head.summary_coverage,
        };
        let semantic = |head: &SemanticCurrentHead, value: u8| SemanticProvenance {
            generation_id: uuid::Uuid::from_u128(2),
            unit_key: "overview".to_owned(),
            source_snapshot_digest: head.snapshot_digest,
            source_generation_contract_digest: digest(value),
            embedding_space_fence: digest(value.wrapping_add(1)),
        };
        let relation_explanation = ScoreExplanation {
            score_role: SemanticScoreRole::RelationDocument,
            problem_score,
            conditioned_evidence: Vec::new(),
            highest_gain: Score::ZERO,
            second_highest_gain: Score::ZERO,
            environment_gain: Score::ZERO,
            anchor_gain: AnchorGain::None,
            local_coherence: Some(coherence),
            document_score: Some(relation_score),
            target_coordinate_score: None,
            transition_score: None,
            penalties: Vec::new(),
            final_score: relation_score,
        };
        let target_explanation = ScoreExplanation {
            score_role: SemanticScoreRole::TargetCoordinate,
            problem_score,
            conditioned_evidence: Vec::new(),
            highest_gain: Score::ZERO,
            second_highest_gain: Score::ZERO,
            environment_gain: Score::ZERO,
            anchor_gain: AnchorGain::None,
            local_coherence: Some(coherence),
            document_score: None,
            target_coordinate_score: Some(target_score),
            transition_score: None,
            penalties: Vec::new(),
            final_score: target_score,
        };
        let hop = SemanticHyperedgeHop {
            ordinal: 0,
            entered_from_coordinate: Some(entered.clone()),
            edge: SemanticEdgeObservation {
                edge_key,
                complete_coordinates,
                provenance: ProjectContextEdgeProvenance {
                    last_context_revision: 1,
                    source_change_id: digest(50),
                },
                current_context_document_bindings: vec![selected_binding.clone()],
            },
            selected_relation_document: SemanticRelationDocument {
                document_id,
                binding_provenance: selected_binding.provenance,
                preview: SemanticSourcePreview {
                    title: "relation".to_owned(),
                    summary: None,
                    summary_omitted_reason: None,
                },
                canonical_provenance: provenance(&document_head),
                semantic_provenance: semantic(&document_head, 60),
                document_score: relation_score,
                score_explanation: relation_explanation,
            },
            continued_to_coordinate: SemanticContinuedCoordinate {
                coordinate: target,
                preview: SemanticSourcePreview {
                    title: "target".to_owned(),
                    summary: None,
                    summary_omitted_reason: None,
                },
                lifecycle: SemanticLifecycleClass::Active,
                canonical_provenance: provenance(&target_head),
                semantic_provenance: semantic(&target_head, target_value as u8),
                target_score,
                score_explanation: target_explanation,
            },
            transition_score,
        };
        FrontierPathState::seed(
            digest(1),
            RootStructuralEntrypoint::Coordinate {
                coordinate: entered,
            },
            Some(problem_score),
        )
        .append_hop(hop)
        .expect("one-hop successor")
    }

    #[derive(Clone)]
    struct LinearStep {
        entered: ProjectContextCoordinate,
        target: ProjectContextCoordinate,
        edge: SemanticEdgeObservation,
        document_id: uuid::Uuid,
        binding: ContextDocumentBindingObservation,
        entered_head: SemanticCurrentHead,
        document_head: SemanticCurrentHead,
        target_head: SemanticCurrentHead,
    }

    struct LinearTraversalBackend {
        ticket: SemanticGraphQueryTicket,
        problem_channel_id: Digest32,
        steps: Vec<LinearStep>,
    }

    impl LinearTraversalBackend {
        fn snapshot(&self) -> buzz_db::semantic_query::SemanticGraphSnapshotBinding {
            buzz_db::semantic_query::SemanticGraphSnapshotBinding {
                community_id: self.ticket.community_id,
                generation_id: self.ticket.generation.generation_id,
                query_fences: self.ticket.query_fences,
                extractor_version: self.ticket.generation.extractor_version.clone(),
                project_context_revision: self.ticket.project_context_revision,
                observed_at: self.ticket.observed_at,
            }
        }

        fn step_for_entered(&self, entered: &ProjectContextCoordinate) -> &LinearStep {
            self.steps
                .iter()
                .find(|step| &step.entered == entered)
                .expect("synthetic traversal entered Coordinate")
        }

        fn step_for_edge(&self, edge_key: EdgeKey) -> &LinearStep {
            self.steps
                .iter()
                .find(|step| step.edge.edge_key == edge_key)
                .expect("synthetic traversal Edge")
        }

        fn document_source(&self, step: &LinearStep) -> SemanticSourceIdentity {
            SemanticSourceIdentity {
                community_id: *self.ticket.community_id.as_uuid(),
                kind: SemanticSourceKind::ProjectDocument,
                source_id: step.document_id,
            }
        }

        fn score_row(
            &self,
            source: SemanticSourceIdentity,
            head: SemanticCurrentHead,
            roles: buzz_db::semantic_query::SemanticGraphStructuralRoles,
        ) -> SemanticExactSourceScore {
            SemanticExactSourceScore {
                channel_id: self.problem_channel_id,
                source,
                head,
                lifecycle: SemanticLifecycleClass::Active,
                source_status: None,
                roles,
                score: score(900_000),
                channel_rank: 1,
            }
        }

        fn hydrated(
            &self,
            source: SemanticSourceIdentity,
            head: SemanticCurrentHead,
        ) -> SemanticHydratedCurrentSource {
            SemanticHydratedCurrentSource {
                canonical: buzz_db::semantic_query::SemanticCanonicalSourceSnapshot {
                    source,
                    source_invalidation_epoch: head.invalidation_epoch,
                    source_basis: head.source_basis.clone(),
                    source_snapshot_digest: head.snapshot_digest,
                    lifecycle: SemanticLifecycleClass::Active,
                    source_status: None,
                    title: "synthetic traversal source".to_owned(),
                    summary: Some("content-free offline fixture".to_owned()),
                },
                semantic_head: head,
            }
        }
    }

    #[async_trait::async_trait]
    impl TraversalBackend for LinearTraversalBackend {
        async fn load_hyperedge(
            &mut self,
            expectation: &SemanticHyperedgeExpectation,
        ) -> Result<SemanticHyperedgeReadOutcome, buzz_db::DbError> {
            Ok(SemanticHyperedgeReadOutcome::Current(Box::new(
                self.step_for_edge(expectation.edge_key).edge.clone(),
            )))
        }

        async fn rank_relations(
            &mut self,
            request: SemanticIncidentRelationRankRequest<'_>,
        ) -> Result<SemanticIncidentRelationRankOutcome, buzz_db::DbError> {
            let step = self.step_for_entered(request.entered_from);
            let document_source = self.document_source(step);
            let document_score = document_score(score(900_000), Score::ZERO, Some(score(850_000)));
            let entered_source =
                semantic_source_identity_for_coordinate(self.ticket.community_id, &step.entered)?;
            Ok(SemanticIncidentRelationRankOutcome::Ranked(Box::new(
                buzz_db::semantic_query::SemanticIncidentRelationRankBatch {
                    snapshot: self.snapshot(),
                    options: vec![SemanticRankedRelationOption {
                        edge_key: step.edge.edge_key,
                        edge_provenance: step.edge.provenance.clone(),
                        document_id: step.document_id,
                        binding_provenance: step.binding.provenance.clone(),
                        channel_scores: vec![self.score_row(
                            document_source.clone(),
                            step.document_head.clone(),
                            buzz_db::semantic_query::SemanticGraphStructuralRoles {
                                coordinate: false,
                                coordinate_entry_eligible: false,
                                coordinate_incident_edge_keys: Vec::new(),
                                context_document_bindings: Vec::new(),
                            },
                        )],
                        local_coherence: Some(
                            buzz_db::semantic_query::SemanticCurrentSourcePairDistance {
                                left: document_source,
                                right: entered_source,
                                left_head: step.document_head.clone(),
                                right_head: step.entered_head.clone(),
                                score: score(850_000),
                            },
                        ),
                        document_score,
                    }],
                    omitted: Vec::new(),
                    below_relation_floor: 0,
                    exhaustion: SemanticTraversalSliceExhaustion::Exhausted,
                },
            )))
        }

        async fn rank_targets(
            &mut self,
            request: SemanticEdgeTargetRankRequest<'_>,
        ) -> Result<SemanticEdgeTargetRankOutcome, buzz_db::DbError> {
            let step = self.step_for_edge(request.hyperedge.edge_key);
            let document_source = self.document_source(step);
            let target_source =
                semantic_source_identity_for_coordinate(self.ticket.community_id, &step.target)?;
            let coherence = score(850_000);
            let target_score = target_coordinate_score(score(900_000), Score::ZERO, coherence);
            let transition_score = harmonic_score(request.document_score, target_score);
            Ok(SemanticEdgeTargetRankOutcome::Ranked(Box::new(
                buzz_db::semantic_query::SemanticEdgeTargetRankBatch {
                    snapshot: self.snapshot(),
                    edge: step.edge.clone(),
                    options: vec![SemanticRankedTargetOption {
                        coordinate: step.target.clone(),
                        channel_scores: vec![self.score_row(
                            target_source.clone(),
                            step.target_head.clone(),
                            buzz_db::semantic_query::SemanticGraphStructuralRoles {
                                coordinate: true,
                                coordinate_entry_eligible: true,
                                coordinate_incident_edge_keys: vec![step.edge.edge_key],
                                context_document_bindings: Vec::new(),
                            },
                        )],
                        relation_document_coherence:
                            buzz_db::semantic_query::SemanticCurrentSourcePairDistance {
                                left: document_source,
                                right: target_source,
                                left_head: step.document_head.clone(),
                                right_head: step.target_head.clone(),
                                score: coherence,
                            },
                        target_score,
                        transition_score,
                    }],
                    omitted: Vec::new(),
                    below_target_floor: 0,
                    below_transition_floor: 0,
                    exhaustion: SemanticTraversalSliceExhaustion::Exhausted,
                },
            )))
        }

        async fn hydrate(
            &mut self,
            scores: &[SemanticExactSourceScore],
        ) -> Result<SemanticCanonicalHydrationBatch, buzz_db::DbError> {
            Ok(SemanticCanonicalHydrationBatch {
                snapshot: self.snapshot(),
                sources: scores
                    .iter()
                    .map(|score| self.hydrated(score.source.clone(), score.head.clone()))
                    .collect(),
            })
        }
    }

    fn document_head(value: u8) -> SemanticCurrentHead {
        SemanticCurrentHead {
            invalidation_epoch: 1,
            snapshot_digest: digest(value),
            source_basis: SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                document_revision: 1,
                source_change_id: digest(value.wrapping_add(1)),
            }),
            unit_set_id: uuid::Uuid::from_u128(500 + u128::from(value)),
            unit_key: "overview".to_owned(),
            semantic_text_digest: digest(value.wrapping_add(2)),
            summary_coverage: SemanticCoverage::TitleAndSummary,
        }
    }

    fn linear_ticket() -> (SemanticGraphQueryTicket, SemanticExactQueryVector, Digest32) {
        let encoder = DeterministicFakeEncoder::new(3).expect("fake encoder");
        let contract = encoder.contract().clone();
        let query_fences =
            buzz_semantic_query::QueryCompatibilityFences::for_source_contract(&contract)
                .expect("query fences");
        let community_id = CommunityId::from_uuid(fixture_uuid(1));
        let observed_at = Utc::now();
        let ticket = SemanticGraphQueryTicket {
            community_id,
            generation: buzz_db::semantic::SemanticGenerationRecord {
                community_id,
                generation_id: fixture_uuid(2),
                lifecycle: "active".to_owned(),
                extractor_version: "overview-v1".to_owned(),
                model_contract: contract.clone(),
                model_contract_digest: query_fences.source_generation_contract_digest,
                rebuild_completed_at: Some(observed_at),
                created_at: observed_at,
            },
            query_fences,
            projection_generation: 3,
            project_context_revision: 7,
            observed_at,
        };
        let input = build_problem_query_encoder_input(fixture_uuid(3), "linear traversal")
            .expect("problem input");
        let problem_channel_id = input.channel_id();
        let encoded = ProviderEncodedSemanticInput::new(
            input.semantic_input(),
            contract.model.clone(),
            vec![1.0, 0.0, 0.0],
            &contract,
        )
        .expect("Provider-bound input");
        let vector = SemanticExactQueryVector::new(&ticket, encoded).expect("bound query vector");
        (ticket, vector, problem_channel_id)
    }

    fn migrated_linear_bundle(ticket: &SemanticGraphQueryTicket) -> SemanticGraphQueryVectorBundle {
        let input = build_problem_query_encoder_input(fixture_uuid(3), "linear traversal")
            .expect("problem input");
        let inputs =
            SemanticQueryInputBundle::from_closed_inputs(vec![input.semantic_input().clone()])
                .expect("common input bundle");
        let provider = ProviderEncodedSemanticInputBundle::new(
            &inputs,
            ticket.generation.model_contract.model.clone(),
            vec![vec![1.0, 0.0, 0.0]],
            &ticket.generation.model_contract,
        )
        .expect("common Provider result");
        SemanticGraphQueryVectorBundle::bind(ticket, provider)
            .expect("migrated complete-path bundle")
    }

    fn linear_steps(project_id: uuid::Uuid, hop_count: u8) -> Vec<LinearStep> {
        (1..=hop_count)
            .map(|index| {
                let entered = linear_work(u64::from(index));
                let target = linear_work(u64::from(index) + 1);
                let complete_coordinates =
                    canonicalize_coordinates(vec![entered.clone(), target.clone()])
                        .expect("canonical synthetic Edge");
                let edge_key = EdgeKey::derive(project_id, &complete_coordinates)
                    .expect("synthetic Edge identity");
                let document_id = fixture_uuid(1_000 + u64::from(index));
                let binding = binding(index, document_id);
                LinearStep {
                    entered,
                    target,
                    edge: SemanticEdgeObservation {
                        edge_key,
                        complete_coordinates,
                        provenance: ProjectContextEdgeProvenance {
                            last_context_revision: u64::from(index),
                            source_change_id: digest(40_u8.wrapping_add(index)),
                        },
                        current_context_document_bindings: vec![binding.clone()],
                    },
                    document_id,
                    binding,
                    entered_head: view_head(index),
                    document_head: document_head(60_u8.wrapping_add(index)),
                    target_head: view_head(index.wrapping_add(1)),
                }
            })
            .collect()
    }

    fn linear_root(
        ticket: &SemanticGraphQueryTicket,
        problem_score: Score,
    ) -> SemanticGraphSelectedRoot {
        let coordinate = linear_work(1);
        let source = semantic_source_identity_for_coordinate(ticket.community_id, &coordinate)
            .expect("synthetic root source");
        let entrypoint = RootStructuralEntrypoint::Coordinate { coordinate };
        let root_id = derive_root_id(
            *ticket.community_id.as_uuid(),
            &source,
            std::slice::from_ref(&entrypoint),
        )
        .expect("synthetic root identity");
        let final_score = candidate_score(problem_score, Score::ZERO, AnchorGain::None);
        let explanation = ScoreExplanation {
            score_role: SemanticScoreRole::Candidate,
            problem_score,
            conditioned_evidence: Vec::new(),
            highest_gain: Score::ZERO,
            second_highest_gain: Score::ZERO,
            environment_gain: Score::ZERO,
            anchor_gain: AnchorGain::None,
            local_coherence: None,
            document_score: None,
            target_coordinate_score: None,
            transition_score: None,
            penalties: Vec::new(),
            final_score,
        };
        let head = view_head(1);
        SemanticGraphSelectedRoot {
            root_id,
            source,
            discovery_channels: vec![RootDiscoveryChannel::ProblemNeutral],
            structural_entrypoints: vec![entrypoint],
            preview: SemanticSourcePreview {
                title: "synthetic root".to_owned(),
                summary: Some("content-free offline fixture".to_owned()),
                summary_omitted_reason: None,
            },
            lifecycle: SemanticLifecycleClass::Active,
            source_status: None,
            canonical_provenance: CanonicalSourceProvenance {
                source_basis: head.source_basis.clone(),
                source_invalidation_epoch: head.invalidation_epoch,
                source_snapshot_digest: head.snapshot_digest,
                summary_coverage: head.summary_coverage,
            },
            semantic_provenance: Some(SemanticProvenance {
                generation_id: ticket.generation.generation_id,
                unit_key: "overview".to_owned(),
                source_snapshot_digest: head.snapshot_digest,
                source_generation_contract_digest: ticket
                    .query_fences
                    .source_generation_contract_digest,
                embedding_space_fence: ticket.query_fences.embedding_space_fence,
            }),
            semantic_head: Some(head),
            semantic_score: Some(final_score),
            score_explanation: Some(explanation),
            automatic_lane: None,
        }
    }

    #[tokio::test]
    async fn traversal_engine_one_through_six_hops_pack_and_sign() {
        for hop_count in 1..=6 {
            let (ticket, vector, problem_channel_id) = linear_ticket();
            let budget = SemanticGraphQueryBudget {
                max_hops_per_path: hop_count,
                max_paths: 1,
                max_response_bytes: 256 * 1024,
                ..SemanticGraphQueryBudget::default()
            };
            let root = linear_root(&ticket, score(900_000));
            let mut backend = LinearTraversalBackend {
                steps: linear_steps(*ticket.community_id.as_uuid(), hop_count),
                ticket: ticket.clone(),
                problem_channel_id,
            };
            let channels = TraversalChannels {
                problem_channel_id,
                conditioned: Vec::new(),
            };
            let query_vectors =
                SemanticGraphQueryVectorBundle::from_compatibility_vectors(&ticket, vec![vector])
                    .expect("complete-path query bundle");
            let search = TraversalEngine::new(
                &mut backend,
                &ticket,
                &query_vectors,
                &channels,
                LifecycleFilter::AllCurrent,
                budget,
                Instant::now() + std::time::Duration::from_secs(5),
            )
            .expect("synthetic traversal engine")
            .search(std::slice::from_ref(&root))
            .await
            .expect("synthetic linear traversal");
            assert_eq!(search.paths.len(), 1);
            assert_eq!(search.paths[0].hops.len(), usize::from(hop_count));
            assert_eq!(
                search.exhausted_dimensions,
                vec![ExhaustedDimension::HopsPerPath]
            );
            assert_eq!(
                search.paths[0].branch_stop_reason,
                BranchStopReason::MaxHopsReached
            );

            let query = SemanticGraphQuery {
                request_id: fixture_uuid(10 + u64::from(hop_count)),
                project_id: *ticket.community_id.as_uuid(),
                problem: "synthetic graph traversal regression".to_owned(),
                initial_coordinates: Vec::new(),
                context_coordinates: Vec::new(),
                lifecycle_filter: LifecycleFilter::AllCurrent,
                budget,
            };
            let mut coverage = SemanticGraphQueryCoverage {
                authorized_graph_sources: 1,
                current_indexed_graph_sources: 1,
                title_only_sources: 0,
                embedding_coverage: EmbeddingCoverageCounts {
                    current: 1,
                    ..EmbeddingCoverageCounts::default()
                },
                query_channels_requested: 1,
                query_channels_executed: 1,
                omitted_context_channel_counts_by_reason: OmittedContextChannelCounts::default(),
                neutral_candidates_considered: 1,
                conditioned_candidates_considered: 0,
                roots_selected: 1,
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
            apply_search_coverage(&mut coverage, &search);
            let observations = SemanticGraphQueryObservations {
                semantic_generation_id: ticket.generation.generation_id,
                source_generation_contract_digest: ticket
                    .query_fences
                    .source_generation_contract_digest,
                embedding_space_fence: ticket.query_fences.embedding_space_fence,
                query_contract_digest: query_contract_digest(),
                ranking_contract_digest: ranking_contract_digest().expect("ranking digest"),
                budget_profile_digest: budget_profile_digest().expect("budget digest"),
                extractor_version: ticket.generation.extractor_version.clone(),
                project_context_revision: ticket.project_context_revision,
                snapshot_observed_at: ticket.observed_at,
            };
            let input = SemanticGraphResponsePackingInput {
                query,
                request_binding_digest: digest(99),
                observations,
                input_observations: SemanticGraphQueryInputObservations {
                    accepted_initial_coordinates: Vec::new(),
                    initial_not_in_graph: Vec::new(),
                    omitted_initial_coordinates: Vec::new(),
                    accepted_context_coordinates: Vec::new(),
                    omitted_context_coordinates: Vec::new(),
                },
                roots: search.roots,
                paths: search.paths,
                coverage,
                completion_reason: CompletionReason::BudgetExhausted,
                exhausted_dimensions: search.exhausted_dimensions,
            };
            validate_completed_semantic_graph_forest(&input)
                .expect("TraversalEngine output validates before packing");
            let relay = Keys::generate();
            let caller = Keys::generate();
            let packed = pack_semantic_graph_response(
                input,
                &relay.public_key(),
                &caller.public_key(),
                256 * 1024,
            )
            .expect("TraversalEngine output packs");
            let signed = sign_packed_semantic_graph_response(packed, &relay)
                .expect("TraversalEngine output signs");
            assert!(!signed.event_array_bytes.is_empty());
        }
    }

    #[tokio::test]
    async fn compatibility_and_common_vector_adapters_retain_the_same_path() {
        let (ticket, vector, problem_channel_id) = linear_ticket();
        let compatibility =
            SemanticGraphQueryVectorBundle::from_compatibility_vectors(&ticket, vec![vector])
                .expect("compatibility complete-path bundle");
        let migrated = migrated_linear_bundle(&ticket);
        assert_eq!(compatibility, migrated);

        let budget = SemanticGraphQueryBudget {
            max_hops_per_path: 2,
            max_paths: 1,
            ..SemanticGraphQueryBudget::default()
        };
        let root = linear_root(&ticket, score(900_000));
        let channels = TraversalChannels {
            problem_channel_id,
            conditioned: Vec::new(),
        };
        let mut compatibility_backend = LinearTraversalBackend {
            steps: linear_steps(*ticket.community_id.as_uuid(), 2),
            ticket: ticket.clone(),
            problem_channel_id,
        };
        let compatibility_output = TraversalEngine::new(
            &mut compatibility_backend,
            &ticket,
            &compatibility,
            &channels,
            LifecycleFilter::AllCurrent,
            budget,
            Instant::now() + std::time::Duration::from_secs(5),
        )
        .expect("compatibility traversal")
        .search(std::slice::from_ref(&root))
        .await
        .expect("compatibility path");
        let mut migrated_backend = LinearTraversalBackend {
            steps: linear_steps(*ticket.community_id.as_uuid(), 2),
            ticket: ticket.clone(),
            problem_channel_id,
        };
        let migrated_output = TraversalEngine::new(
            &mut migrated_backend,
            &ticket,
            &migrated,
            &channels,
            LifecycleFilter::AllCurrent,
            budget,
            Instant::now() + std::time::Duration::from_secs(5),
        )
        .expect("migrated traversal")
        .search(std::slice::from_ref(&root))
        .await
        .expect("migrated path");

        assert_eq!(compatibility_output.roots, migrated_output.roots);
        assert_eq!(compatibility_output.paths, migrated_output.paths);
        assert_eq!(
            compatibility_output.expanded_coordinates,
            migrated_output.expanded_coordinates
        );
        assert_eq!(
            compatibility_output.incident_edges_materialized,
            migrated_output.incident_edges_materialized
        );
        assert_eq!(
            compatibility_output.relation_options_materialized,
            migrated_output.relation_options_materialized
        );
        assert_eq!(
            compatibility_output.target_options_materialized,
            migrated_output.target_options_materialized
        );
        assert_eq!(
            compatibility_output.paths_generated,
            migrated_output.paths_generated
        );
        assert_eq!(
            compatibility_output.paths_retained,
            migrated_output.paths_retained
        );
        assert_eq!(
            compatibility_output.truncation_counts,
            migrated_output.truncation_counts
        );
        assert_eq!(
            compatibility_output.exhausted_dimensions,
            migrated_output.exhausted_dimensions
        );
    }

    #[tokio::test]
    async fn traversal_stops_before_an_n_plus_one_coordinate_expansion() {
        let (ticket, vector, problem_channel_id) = linear_ticket();
        let budget = SemanticGraphQueryBudget {
            max_hops_per_path: 2,
            max_expanded_coordinates: 1,
            max_paths: 1,
            max_response_bytes: 256 * 1024,
            ..SemanticGraphQueryBudget::default()
        };
        let root = linear_root(&ticket, score(900_000));
        let mut backend = LinearTraversalBackend {
            steps: linear_steps(*ticket.community_id.as_uuid(), 2),
            ticket: ticket.clone(),
            problem_channel_id,
        };
        let channels = TraversalChannels {
            problem_channel_id,
            conditioned: Vec::new(),
        };
        let search = TraversalEngine::new(
            &mut backend,
            &ticket,
            &SemanticGraphQueryVectorBundle::from_compatibility_vectors(&ticket, vec![vector])
                .expect("complete-path query bundle"),
            &channels,
            LifecycleFilter::AllCurrent,
            budget,
            Instant::now() + std::time::Duration::from_secs(5),
        )
        .expect("synthetic traversal engine")
        .search(std::slice::from_ref(&root))
        .await
        .expect("bounded synthetic traversal");

        assert_eq!(search.expanded_coordinates, 1);
        assert_eq!(search.paths.len(), 1);
        assert_eq!(search.paths[0].hops.len(), 1);
        assert_eq!(
            search.paths[0].branch_stop_reason,
            BranchStopReason::GlobalBudgetExhausted
        );
        assert!(search
            .exhausted_dimensions
            .contains(&ExhaustedDimension::ExpandedCoordinates));
    }

    fn cutoff_work_with_beam_suppression(beam_width: u16) -> ExpansionWork {
        let seed_id = SeedId {
            root_index: 0,
            entrypoint_index: 0,
        };
        let state = FrontierPathState::seed(
            digest(1),
            RootStructuralEntrypoint::Coordinate {
                coordinate: work(1),
            },
            Some(score(900_000)),
        );
        let mut expansion =
            ExpansionWork::coordinate(seed_id, vec![1], state, beam_width).expect("expansion");
        for target in [2, 3] {
            expansion.qualifying_successors_seen += 1;
            expansion
                .accumulator
                .as_mut()
                .expect("successor accumulator")
                .admit(one_hop_successor(target))
                .expect("admit successor");
        }
        expansion
    }

    #[test]
    fn fair_first_wave_slices_each_remaining_seed() {
        let budget = SemanticGraphQueryBudget {
            max_expanded_coordinates: 5,
            max_incident_edges_materialized: 5,
            max_relation_options_materialized: 5,
            max_target_options_materialized: 5,
            ..SemanticGraphQueryBudget::default()
        };
        let mut materialized = MaterializationState::default();

        let first = Quantum::first_wave(&materialized, &budget, 3);
        assert_eq!(first.expanded_coordinates, 2);
        assert_eq!(first.relation_options, 2);
        materialized.expanded_coordinates += first.expanded_coordinates;
        materialized
            .relations
            .extend(
                (0..first.relation_options).map(|value| RelationMaterializationKey {
                    entered_from: Some(work(value as u128 + 1)),
                    edge_key: edge(value as u8 + 1),
                    document_id: uuid::Uuid::from_u128(value as u128 + 20),
                }),
            );
        let second = Quantum::first_wave(&materialized, &budget, 2);
        assert_eq!(second.expanded_coordinates, 2);
        assert_eq!(second.relation_options, 2);
        materialized.expanded_coordinates += second.expanded_coordinates;
        materialized
            .relations
            .extend((2..4).map(|value| RelationMaterializationKey {
                entered_from: Some(work(value as u128 + 1)),
                edge_key: edge(value as u8 + 1),
                document_id: uuid::Uuid::from_u128(value as u128 + 20),
            }));
        let last = Quantum::first_wave(&materialized, &budget, 1);
        assert_eq!(last.expanded_coordinates, 1);
        assert_eq!(last.relation_options, 1);
    }

    #[test]
    fn cycle_target_advances_cursor_before_next_continuation_page() {
        let entered = work(1);
        let path = FrontierPathState::seed(
            digest(1),
            RootStructuralEntrypoint::Coordinate {
                coordinate: entered.clone(),
            },
            Some(score(900_000)),
        );
        let cycle = target_option(entered);
        let (after_cycle, admitted_cycle) = classify_ranked_target(&path, edge(7), cycle);
        assert!(admitted_cycle.is_none());
        assert_eq!(after_cycle.target_coordinate, work(1));

        let next = target_option(work(2));
        let (after_next, admitted_next) = classify_ranked_target(&path, edge(7), next);
        assert_eq!(after_next.target_coordinate, work(2));
        assert_eq!(
            admitted_next.expect("non-cycle continuation").coordinate,
            work(2)
        );
    }

    #[test]
    fn relation_document_root_begins_at_bound_hyperedge() {
        let document_id = uuid::Uuid::from_u128(41);
        let selected_binding = binding(3, document_id);
        let entrypoint = RootStructuralEntrypoint::ContextDocument {
            edge_key: edge(4),
            document_id,
            edge_provenance: ProjectContextEdgeProvenance {
                last_context_revision: 5,
                source_change_id: digest(4),
            },
            binding_provenance: selected_binding.provenance.clone(),
        };
        let problem_score = score(900_000);
        let final_score = candidate_score(problem_score, Score::ZERO, AnchorGain::None);
        let document_head = SemanticCurrentHead {
            source_basis: SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                document_revision: 1,
                source_change_id: digest(5),
            }),
            ..view_head(5)
        };
        let source = SemanticSourceIdentity {
            community_id: uuid::Uuid::from_u128(1),
            kind: SemanticSourceKind::ProjectDocument,
            source_id: document_id,
        };
        let explanation = ScoreExplanation {
            score_role: SemanticScoreRole::Candidate,
            problem_score,
            conditioned_evidence: Vec::new(),
            highest_gain: Score::ZERO,
            second_highest_gain: Score::ZERO,
            environment_gain: Score::ZERO,
            anchor_gain: AnchorGain::None,
            local_coherence: None,
            document_score: None,
            target_coordinate_score: None,
            transition_score: None,
            penalties: Vec::new(),
            final_score,
        };
        let root = SemanticGraphSelectedRoot {
            root_id: digest(9),
            source,
            discovery_channels: vec![RootDiscoveryChannel::ProblemNeutral],
            structural_entrypoints: vec![entrypoint],
            preview: SemanticSourcePreview {
                title: "relation".to_owned(),
                summary: Some("summary".to_owned()),
                summary_omitted_reason: None,
            },
            lifecycle: SemanticLifecycleClass::Active,
            source_status: None,
            canonical_provenance: CanonicalSourceProvenance {
                source_basis: document_head.source_basis.clone(),
                source_invalidation_epoch: document_head.invalidation_epoch,
                source_snapshot_digest: document_head.snapshot_digest,
                summary_coverage: document_head.summary_coverage,
            },
            semantic_provenance: Some(SemanticProvenance {
                generation_id: uuid::Uuid::from_u128(2),
                unit_key: "overview".to_owned(),
                source_snapshot_digest: document_head.snapshot_digest,
                source_generation_contract_digest: digest(7),
                embedding_space_fence: digest(8),
            }),
            semantic_head: Some(document_head),
            semantic_score: Some(final_score),
            score_explanation: Some(explanation),
            automatic_lane: None,
        };

        let mut work = build_seed_work(&[root], 2).expect("relation seed");
        let seed = work.pop().expect("one seed");
        assert!(seed.incident_started);
        assert!(seed.incident_exhausted);
        assert!(seed.path.current_coordinate.is_none());
        let target = seed
            .pending_targets
            .front()
            .expect("bound edge target work");
        assert!(target.entered_from.is_none());
        assert!(!target.relation_admitted);
        assert_eq!(target.expectation.edge_key, edge(4));
        assert_eq!(
            target.expectation.required_binding.as_ref(),
            Some(&selected_binding)
        );
    }

    #[test]
    fn complete_hyperedge_keeps_every_binding_and_rejects_wrong_selected_one() {
        let first = binding(1, uuid::Uuid::from_u128(11));
        let second = binding(2, uuid::Uuid::from_u128(12));
        let provenance = ProjectContextEdgeProvenance {
            last_context_revision: 3,
            source_change_id: digest(3),
        };
        let observed = SemanticEdgeObservation {
            edge_key: edge(3),
            complete_coordinates: vec![work(1), work(2), work(3)],
            provenance: provenance.clone(),
            current_context_document_bindings: vec![first.clone(), second.clone()],
        };
        let expectation = SemanticHyperedgeExpectation {
            edge_key: edge(3),
            edge_provenance: provenance.clone(),
            required_binding: Some(second),
        };
        validate_cached_edge(&observed, &expectation).expect("complete edge");
        assert_eq!(observed.complete_coordinates.len(), 3);
        assert_eq!(observed.current_context_document_bindings.len(), 2);

        let wrong = SemanticHyperedgeExpectation {
            edge_key: edge(3),
            edge_provenance: provenance,
            required_binding: Some(binding(9, uuid::Uuid::from_u128(99))),
        };
        assert!(validate_cached_edge(&observed, &wrong).is_err());
    }

    #[test]
    fn oversized_hyperedge_has_precedence_without_becoming_budget_exhaustion() {
        let stop = StopObservations {
            below_relevance: true,
            cycle_or_duplicate: true,
            hyperedge_too_large: true,
        };
        assert_eq!(stop.reason(), BranchStopReason::HyperedgeTooLarge);
        let exhaustion = ExhaustionState::default();
        assert!(!exhaustion
            .dimensions
            .contains(&ExhaustedDimension::IncidentEdgesMaterialized));
    }

    #[test]
    fn materialization_limit_reuses_existing_key_but_rejects_new_work() {
        let mut materialized = HashSet::new();
        let mut quantum = 1;
        assert!(matches!(
            admit_new_key(&mut materialized, 1_u8, 1, &mut quantum),
            Admission::Admitted
        ));
        assert_eq!(quantum, 0);
        assert!(matches!(
            admit_new_key(&mut materialized, 1_u8, 1, &mut quantum),
            Admission::Reused
        ));
        assert!(matches!(
            admit_new_key(&mut materialized, 2_u8, 1, &mut quantum),
            Admission::GlobalExhausted
        ));
        assert_eq!(materialized, HashSet::from([1_u8]));
    }

    #[test]
    fn coordinate_expansion_checks_the_global_cap_before_a_fresh_quantum() {
        let mut expanded = 63;
        let mut quantum = 1;
        assert_eq!(
            admit_coordinate_expansion(&mut expanded, 64, &mut quantum),
            CoordinateExpansionAdmission::Admitted
        );
        assert_eq!((expanded, quantum), (64, 0));

        // A later global-step quantum must not admit the N+1 expansion after
        // another work item consumed the final global slot.
        quantum = 1;
        assert_eq!(
            admit_coordinate_expansion(&mut expanded, 64, &mut quantum),
            CoordinateExpansionAdmission::GlobalExhausted
        );
        assert_eq!((expanded, quantum), (64, 1));

        let mut below_cap = 63;
        let mut empty_quantum = 0;
        assert_eq!(
            admit_coordinate_expansion(&mut below_cap, 64, &mut empty_quantum),
            CoordinateExpansionAdmission::Deferred
        );
        assert_eq!((below_cap, empty_quantum), (63, 0));
    }

    #[test]
    fn global_budget_cutoff_is_reported_for_zero_hop_seed_and_dimension() {
        let seed_id = SeedId {
            root_index: 0,
            entrypoint_index: 0,
        };
        let state = FrontierPathState::seed(
            digest(1),
            RootStructuralEntrypoint::Coordinate {
                coordinate: work(1),
            },
            Some(score(800_000)),
        );
        let expansion =
            ExpansionWork::coordinate(seed_id, vec![1], state, 2).expect("coordinate expansion");
        let mut stopped = Vec::new();
        let mut zero_hop = HashMap::new();
        let mut cutoff_exhaustion = ExhaustionState::default();
        finish_cutoff_work(
            expansion,
            BranchStopReason::GlobalBudgetExhausted,
            &SemanticGraphQueryBudget::default(),
            &mut cutoff_exhaustion,
            &mut stopped,
            &mut zero_hop,
        )
        .expect("cutoff");
        assert!(stopped.is_empty());
        assert_eq!(
            zero_hop.get(&seed_id),
            Some(&BranchStopReason::GlobalBudgetExhausted)
        );

        let state = FrontierPathState::seed(
            digest(1),
            RootStructuralEntrypoint::Coordinate {
                coordinate: work(1),
            },
            Some(score(800_000)),
        );
        let work =
            ExpansionWork::coordinate(seed_id, vec![1], state, 2).expect("coordinate expansion");
        let mut exhaustion = ExhaustionState {
            global_stop: true,
            ..ExhaustionState::default()
        };
        exhaustion
            .record(ExhaustedDimension::TargetOptionsMaterialized, 3, &work)
            .expect("budget observation");
        assert!(exhaustion.global_stop);
        assert!(exhaustion
            .dimensions
            .contains(&ExhaustedDimension::TargetOptionsMaterialized));
        assert_eq!(exhaustion.counts.target_options_materialized, 3);
        assert_eq!(exhaustion.samples.len(), 1);
    }

    #[test]
    fn target_budget_cutoff_seals_observed_beam_suppression() {
        let expansion = cutoff_work_with_beam_suppression(1);
        let mut exhaustion = ExhaustionState {
            global_stop: true,
            ..ExhaustionState::default()
        };
        exhaustion
            .record(ExhaustedDimension::TargetOptionsMaterialized, 1, &expansion)
            .expect("target budget cutoff");
        let budget = SemanticGraphQueryBudget {
            beam_width: 1,
            max_target_options_materialized: 2,
            ..SemanticGraphQueryBudget::default()
        };
        let mut stopped = Vec::new();
        let mut zero_hop = HashMap::new();

        finish_cutoff_work(
            expansion,
            BranchStopReason::GlobalBudgetExhausted,
            &budget,
            &mut exhaustion,
            &mut stopped,
            &mut zero_hop,
        )
        .expect("seal target cutoff");

        assert_eq!(stopped.len(), 1);
        assert_eq!(stopped[0].reason, BranchStopReason::GlobalBudgetExhausted);
        assert!(zero_hop.is_empty());
        assert_eq!(exhaustion.counts.target_options_materialized, 1);
        assert_eq!(exhaustion.counts.beam_width, 1);
        assert!(exhaustion
            .dimensions
            .contains(&ExhaustedDimension::TargetOptionsMaterialized));
        assert!(exhaustion
            .dimensions
            .contains(&ExhaustedDimension::BeamWidth));
    }

    #[test]
    fn wall_time_cutoff_observes_retained_successor_hop_cap() {
        let expansion = cutoff_work_with_beam_suppression(1);
        let mut exhaustion = ExhaustionState {
            global_stop: true,
            ..ExhaustionState::default()
        };
        exhaustion
            .record(ExhaustedDimension::TargetOptionsMaterialized, 1, &expansion)
            .expect("target budget cutoff");
        let budget = SemanticGraphQueryBudget {
            max_hops_per_path: 1,
            beam_width: 1,
            max_target_options_materialized: 2,
            ..SemanticGraphQueryBudget::default()
        };
        let mut stopped = Vec::new();
        let mut zero_hop = HashMap::new();

        finish_cutoff_work(
            expansion,
            BranchStopReason::WallTimeExhausted,
            &budget,
            &mut exhaustion,
            &mut stopped,
            &mut zero_hop,
        )
        .expect("seal target cutoff at hop cap");

        assert_eq!(stopped.len(), 1);
        assert_eq!(stopped[0].state.hops.len(), 1);
        assert_eq!(stopped[0].reason, BranchStopReason::WallTimeExhausted);
        assert!(zero_hop.is_empty());
        assert_eq!(exhaustion.counts.beam_width, 1);
        assert_eq!(exhaustion.counts.hops_per_path, 1);
        assert!(exhaustion
            .dimensions
            .contains(&ExhaustedDimension::HopsPerPath));
    }

    #[test]
    fn generated_and_seed_counts_precede_diversity_and_max_path_retention() {
        let seed_id = SeedId {
            root_index: 0,
            entrypoint_index: 0,
        };
        let entrypoint = RootStructuralEntrypoint::Coordinate {
            coordinate: work(1),
        };
        let candidate = |id: u8, value: u32| {
            let explanation = path_score(None, &[score(value)]).expect("path score");
            RankedStoppedPath {
                seed_id,
                structural_entrypoint: entrypoint.clone(),
                path: SemanticPath {
                    path_id: digest(id),
                    root_id: digest(20),
                    hops: Vec::new(),
                    terminal_coordinate: work(9),
                    path_score: explanation.final_score.expect("scored path"),
                    path_score_explanation: explanation,
                    branch_stop_reason: BranchStopReason::FrontierExhausted,
                },
            }
        };
        let mut exhaustion = ExhaustionState::default();
        let retained = retain_ranked_paths(
            vec![
                candidate(1, 900_000),
                candidate(2, 800_000),
                candidate(3, 700_000),
            ],
            1,
            &mut exhaustion,
        )
        .expect("retention");

        assert_eq!(retained.paths_generated, 3);
        assert_eq!(retained.produced_by_seed.get(&seed_id), Some(&3));
        assert_eq!(retained.paths_retained, 1);
        assert_eq!(retained.paths[0].path_id, digest(1));
        // Endpoint diversity removes the third path without inventing a
        // budget exhaustion; max_paths observes exactly one N+1 path.
        assert_eq!(exhaustion.counts.paths, 1);
        assert_eq!(
            exhaustion.dimensions,
            HashSet::from([ExhaustedDimension::Paths])
        );
    }

    #[test]
    fn canonical_seed_order_is_input_permutation_independent() {
        let coordinate = RootStructuralEntrypoint::Coordinate {
            coordinate: work(2),
        };
        let left = seed_order_key(&view_source(2), &coordinate);
        let right = seed_order_key(
            &view_source(1),
            &RootStructuralEntrypoint::Coordinate {
                coordinate: work(1),
            },
        );
        let mut first = vec![left.clone(), right.clone()];
        let mut second = vec![right, left];
        first.sort();
        second.sort();
        assert_eq!(first, second);
    }
}
