//! Internal environment-conditioned semantic graph root orchestration.
//!
//! This module deliberately stops before Hyperedge traversal and before any
//! HTTP/Event representation. It owns the short ticket/egress checks, the one
//! bounded Provider batch, the writer-DB repeatable-read root snapshot, and
//! deterministic root ranking. Source/query text and vectors never cross this
//! internal boundary or enter logs and metrics.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use buzz_core::CommunityId;
use buzz_db::semantic_query::{
    semantic_source_identity_for_coordinate, SemanticCanonicalHydrationBatch,
    SemanticCanonicalSourceSnapshot, SemanticContextCoordinateObservation,
    SemanticContextCoordinateObservationBatch, SemanticContextEgressExpectation,
    SemanticContextOmissionReason, SemanticCurrentSourcePair, SemanticExactRecallBatch,
    SemanticExactRecallExhaustion, SemanticExactSourceScore, SemanticGraphEmbeddingCoverageClass,
    SemanticGraphQueryTicket, SemanticGraphQueryVectorBundle, SemanticGraphReadTimeouts,
    SemanticGraphReadTx, SemanticInitialCoordinateObservation,
    SemanticInitialCoordinateObservationBatch, SemanticInitialHeadState,
    SemanticInitialOmissionReason,
};
use buzz_project_context::ProjectContextCoordinate;
use buzz_project_view::ProjectViewObjectType;
use buzz_semantic::{
    Digest32, ProjectViewSemanticType, SemanticSourceIdentity, SemanticSourceKind,
};
use buzz_semantic_query::{
    build_query_encoder_inputs, candidate_score, derive_root_id, environment_gain,
    root_diversity_priority, AcceptedContextCoordinateObservation,
    AcceptedInitialCoordinateObservation, AnchorGain, AutomaticRootLane, CanonicalSourceProvenance,
    ConditionedContextOverview, ConditionedEvidence, CurrentGraphMembershipObservation,
    DegradedModeCounts, EmbeddingCoverageCounts, EncodedSemanticQuery, ExhaustedDimension,
    OmittedContextChannelCounts, OmittedContextCoordinateObservation,
    OmittedContextCoordinateReason, OmittedForResponseBudgetCounts,
    OmittedInitialCoordinateObservation, OmittedInitialCoordinateReason,
    ProjectContextBindingProvenance, ProjectContextEdgeProvenance, ProviderEncodedSemanticInput,
    ProviderEncodedSemanticInputBundle, RootDiscoveryChannel, RootStructuralEntrypoint, Score,
    ScoreExplanation, SelectedAutomaticRoot, SemanticComputationRoute, SemanticGraphQuery,
    SemanticGraphQueryCoverage, SemanticGraphQueryError, SemanticGraphQueryInputObservations,
    SemanticHeadProvenance, SemanticHeadState, SemanticProvenance, SemanticQueryChannelKind,
    SemanticQueryEncoderInput, SemanticScoreRole, SemanticSourcePreview,
    TruncationCountsByDimension, BASE_ENTRY_FLOOR, RELATION_FLOOR, RESPONSE_TAIL_RESERVE_MS,
    SEMANTIC_COMPUTATION_ROUTES, SNAPSHOT_CLOSE_RESERVE_MS,
};
use std::sync::Arc;

use tokio::sync::OwnedSemaphorePermit;
use tokio::time::Instant;

use crate::semantic_graph_observability::{
    record_db_distance_rows, record_generation_retry, record_provider_failure, stage_timer,
    SemanticGraphDistanceStage, SemanticGraphMetricStage, SemanticGraphProviderFailure,
    SemanticGraphQueryMetricError,
};
use crate::semantic_provider::TrackedProviderFailure;
use crate::semantic_query_runtime::{
    encode_once, execute_provider_egress, propagate_relay_shutdown, provider_retry_backoff,
    provider_retry_decision, record_vector_reuse, ProviderEgressObservation, ProviderEgressPlan,
    ProviderRetryDecision, ProviderRetryRoute, SemanticDeadlineWindow, SemanticDeadlineWindows,
    SemanticEncodeOnceFailure, SemanticExecutionContext, SemanticOperationAttemptClass,
    SemanticProviderEgressFailure, SemanticStageAbort, SemanticVectorReuseOutcome,
};
use crate::AppState;

/// Start the internal root stage of one authenticated semantic graph query.
///
/// `reader_pubkey` must come from the already-verified request credential and
/// `community_id` from host resolution. The payload Project is checked against
/// that host boundary before any source observation or Provider egress.
pub(crate) async fn begin_semantic_graph_root_query(
    state: &AppState,
    community_id: CommunityId,
    reader_pubkey: &[u8],
    query: SemanticGraphQuery,
) -> Result<SemanticGraphRootQuerySession, SemanticGraphRootQueryError> {
    let _timer = stage_timer(SemanticGraphMetricStage::Root);
    let result =
        begin_semantic_graph_root_query_inner(state, community_id, reader_pubkey, query).await;
    if let Err(error) = &result {
        crate::semantic_graph_observability::record_query_error(
            SemanticGraphMetricStage::Root,
            error.metric_code(),
        );
    }
    result
}

async fn begin_semantic_graph_root_query_inner(
    state: &AppState,
    community_id: CommunityId,
    reader_pubkey: &[u8],
    query: SemanticGraphQuery,
) -> Result<SemanticGraphRootQuerySession, SemanticGraphRootQueryError> {
    let query = query.validate_and_canonicalize()?;
    if query.project_id != *community_id.as_uuid() {
        return Err(SemanticGraphRootQueryError::InvalidProject);
    }
    if !state.config.semantic_graph_query_http_available {
        return Err(SemanticGraphRootQueryError::QueryDisabled);
    }
    let started_at = Instant::now();
    let deadlines = QueryDeadlines::new(started_at, query.budget.max_wall_time_ms)?;
    let context = SemanticExecutionContext::new(
        SemanticOperationAttemptClass::CompletePath,
        SemanticDeadlineWindows::new(
            deadlines.work.into_std(),
            deadlines.work.into_std(),
            deadlines.snapshot_close.into_std(),
            deadlines.absolute.into_std(),
        )
        .map_err(|_| invalid_state("query deadline windows cannot be frozen"))?,
    );
    let process_permit = state
        .semantic_graph_query_semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| SemanticGraphRootQueryError::ProcessBusy)?;
    let provider = match state.semantic_provider() {
        Ok(Some(provider)) => provider,
        Ok(None) | Err(_) => {
            record_provider_failure(SemanticGraphProviderFailure::Unavailable);
            return Err(SemanticGraphRootQueryError::QueryEncoderUnavailable);
        }
    };

    // R4 items 5 and 6: the churn restart reuses the failed attempt's
    // exact-compatible encoded vectors when the fresh observation rebuilds
    // byte-identical inputs under the same generation. The stash lives only
    // inside this request (plan §4.6): nothing is persisted and nothing is
    // shared across requests, pods, or processes.
    let mut reusable = None;
    let mut last_churn = None;
    for attempt in 0..=1_u8 {
        match root_query_attempt(RootQueryAttemptPlan {
            state,
            provider,
            community_id,
            reader_pubkey,
            query: &query,
            deadlines,
            context: &context,
            reusable: &mut reusable,
        })
        .await
        {
            Ok(stage) => {
                return Ok(SemanticGraphRootQuerySession {
                    read: stage.read,
                    query: query.clone(),
                    outcome: stage.outcome,
                    query_vectors: stage.query_vectors,
                    channels: stage.channels,
                    snapshot_close_deadline: deadlines.snapshot_close,
                    absolute_deadline: deadlines.absolute,
                    snapshot_started_at: stage.snapshot_started_at,
                    context,
                    shutdown: Arc::clone(&state.shutting_down),
                    _process_permit: process_permit,
                    _traversal_permit: stage.traversal_permit,
                });
            }
            Err(error @ SemanticGraphRootQueryError::SemanticGenerationChanged)
            | Err(error @ SemanticGraphRootQueryError::ContextSourceChanged)
                if attempt == 0 =>
            {
                record_generation_retry();
                last_churn = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_churn.unwrap_or(SemanticGraphRootQueryError::ContextSourceChanged))
}

/// One root-attempt plan: everything a fresh operation attempt needs.
struct RootQueryAttemptPlan<'a> {
    state: &'a AppState,
    provider: &'a crate::semantic_provider::VolcengineSemanticProvider,
    community_id: CommunityId,
    reader_pubkey: &'a [u8],
    query: &'a SemanticGraphQuery,
    deadlines: QueryDeadlines,
    context: &'a SemanticExecutionContext,
    reusable: &'a mut Option<ReusableQueryVectors>,
}

async fn root_query_attempt(
    plan: RootQueryAttemptPlan<'_>,
) -> Result<StageCRootBuild, SemanticGraphRootQueryError> {
    let RootQueryAttemptPlan {
        state,
        provider,
        community_id,
        reader_pubkey,
        query,
        deadlines,
        context,
        reusable,
    } = plan;
    // Unreachable while the churn loop above bounds root attempts at the
    // compiled cap; kept so the counting ledger owns the restart dimension
    // from R4 on.
    context
        .ledger()
        .begin_operation_attempt()
        .map_err(|_| SemanticGraphRootQueryError::QueryProviderBusy)?;
    propagate_relay_shutdown(state, context);
    if let Err(abort) = context.admit_stage() {
        return Err(match abort {
            SemanticStageAbort::Deadline(_) | SemanticStageAbort::Cancelled(_) => {
                SemanticGraphRootQueryError::QueryDeadlineExceeded
            }
        });
    }
    let mut prior_vectors = reusable.take();
    loop {
        // Every physical Provider attempt assembles its own fresh plan
        // (plan §4.3): fresh authorized ticket, fresh context observation,
        // and — whenever it actually encodes — a fresh reservation with its
        // egress confirmation. No ticket, observation, reservation, permit,
        // or routing decision is carried across attempts.
        let bootstrap_ticket = run_root_stage(
            context,
            state.db.semantic_graph_query_ticket(
                community_id,
                reader_pubkey,
                &state.relay_keypair.public_key(),
            ),
        )
        .await?
        .map_err(map_ticket_error)?;
        let (ticket, stage_a_context) = observe_context_snapshot(
            state,
            &bootstrap_ticket,
            reader_pubkey,
            &query.context_coordinates,
            context,
        )
        .await?;
        if provider.source_contract() != &ticket.generation.model_contract {
            record_provider_failure(SemanticGraphProviderFailure::Unavailable);
            return Err(SemanticGraphRootQueryError::QueryEncoderUnavailable);
        }
        let input_build =
            build_query_encoder_inputs(query, &conditioned_overviews(&stage_a_context))?;
        let unsupported_conditioned = input_build
            .omitted_contexts
            .iter()
            .map(|omitted| omitted.context_coordinate.clone())
            .collect::<HashSet<_>>();
        let channels = query_channel_bindings(&input_build.inputs);
        let common_inputs = input_build.semantic_input_bundle()?;
        let context_expectations = stage_a_context
            .observations
            .iter()
            .map(SemanticContextEgressExpectation::from_observation)
            .collect::<Vec<_>>();

        // R4 items 5 and 6: exactly one reuse decision per attempt. The
        // churned attempt's stash is offered once; an exact identity match
        // re-binds its vectors under the fresh ticket with no Provider
        // egress at all, while anything else falls through to a full encode.
        let mut reused_vectors = None;
        if let Some(stash) = prior_vectors.take() {
            if stash.identity == QueryVectorReuseIdentity::of(&input_build.inputs, &ticket) {
                match rebind_reusable_query_vectors(&ticket, stash.encoded) {
                    Ok(vectors) => {
                        record_vector_reuse(
                            SemanticOperationAttemptClass::CompletePath,
                            SemanticVectorReuseOutcome::Reused,
                        );
                        reused_vectors = Some(vectors);
                    }
                    Err(_) => {
                        record_vector_reuse(
                            SemanticOperationAttemptClass::CompletePath,
                            SemanticVectorReuseOutcome::ReuseRejected,
                        );
                    }
                }
            } else {
                record_vector_reuse(
                    SemanticOperationAttemptClass::CompletePath,
                    SemanticVectorReuseOutcome::Reencoded,
                );
            }
        }

        let query_vectors = match reused_vectors {
            Some(vectors) => vectors,
            None => {
                // Shared R2 Provider egress executor: reservation,
                // deadline-aware wait, routing trust, and final no-wait
                // confirmation in one zero-policy sequence. Its neutral
                // outcome maps back onto this surface's frozen public
                // errors; `ProviderUnavailable` keeps its pre-R2 ticket
                // re-read.
                if let Err(failure) = execute_provider_egress(ProviderEgressPlan {
                    state,
                    context,
                    ticket: &ticket,
                    reader_pubkey,
                    expected_contexts: &context_expectations,
                    observation: ProviderEgressObservation::CompletePathQuery,
                })
                .await
                {
                    return Err(match failure {
                        SemanticProviderEgressFailure::ProviderUnavailable => {
                            classify_ticket_failure(state, &ticket, reader_pubkey, deadlines.work)
                                .await
                        }
                        failure => map_provider_egress_failure(failure),
                    });
                }

                metrics::histogram!("buzz_semantic_graph_query_provider_input_bytes").record(
                    common_inputs
                        .inputs()
                        .iter()
                        .map(|input| input.exact_utf8_text().len() as f64)
                        .sum::<f64>(),
                );
                let _provider_timer = stage_timer(SemanticGraphMetricStage::Provider);
                // R4 items 1-3: the closed runtime policy owns the retry
                // decision. A sanctioned retry backoffs inside the work
                // window and restarts this loop — which re-reads the ticket,
                // re-observes the context, and takes a fresh reservation —
                // so no stale plan fragment survives. A declined or
                // exhausted retry returns the last typed failure through
                // the frozen single-attempt projection.
                let encoded = match encode_once(
                    context,
                    ProviderEgressObservation::CompletePathQuery,
                    encode_complete_path_inputs(
                        provider,
                        SEMANTIC_COMPUTATION_ROUTES.bounded_complete_path,
                        &input_build.inputs,
                        &common_inputs,
                    ),
                )
                .await
                {
                    Ok(encoded) => encoded,
                    Err(SemanticEncodeOnceFailure::DeadlineExceeded)
                    | Err(SemanticEncodeOnceFailure::Cancelled(_)) => {
                        return Err(SemanticGraphRootQueryError::QueryDeadlineExceeded);
                    }
                    Err(SemanticEncodeOnceFailure::Provider(tracked)) => {
                        match provider_retry_decision(
                            ProviderRetryRoute::R4,
                            tracked.failure,
                            context,
                        ) {
                            ProviderRetryDecision::Terminal => {
                                let TrackedProviderFailure { error, .. } = tracked;
                                record_provider_failure(provider_failure_class(&error));
                                return Err(map_encoder_error(error));
                            }
                            ProviderRetryDecision::Retry { backoff } => {
                                if provider_retry_backoff(context, backoff).await.is_err() {
                                    return Err(SemanticGraphRootQueryError::QueryDeadlineExceeded);
                                }
                                continue;
                            }
                        }
                    }
                };
                drop(_provider_timer);
                let (query_vectors, stash) = match encoded {
                    CompletePathEncodedInputs::Compatibility(encoded) => {
                        let values = encoded
                            .into_iter()
                            .map(EncodedSemanticQuery::into_provider_encoded)
                            .collect::<Vec<_>>();
                        (
                            bind_compatibility_query_vectors(
                                &ticket,
                                &input_build.inputs,
                                &values,
                            )?,
                            ReusableEncodedInputs::Compatibility(values),
                        )
                    }
                    CompletePathEncodedInputs::Common(encoded) => (
                        SemanticGraphQueryVectorBundle::bind(&ticket, encoded.clone())?,
                        ReusableEncodedInputs::Common(encoded),
                    ),
                };
                // Offer this attempt's exact-compatible vectors to a possible
                // churn restart of the same request (plan §4.6).
                *reusable = Some(ReusableQueryVectors {
                    identity: QueryVectorReuseIdentity::of(&input_build.inputs, &ticket),
                    encoded: stash,
                });
                query_vectors
            }
        };
        drop(input_build);

        let traversal_permit = state
            .semantic_graph_traversal_semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                metrics::counter!(
                    "buzz_semantic_graph_traversal_admission_total",
                    "outcome" => "busy"
                )
                .increment(1);
                SemanticGraphRootQueryError::TraversalBusy
            })?;
        metrics::counter!("buzz_semantic_graph_traversal_admission_total", "outcome" => "admitted")
            .increment(1);
        metrics::histogram!("buzz_semantic_graph_traversal_admission_wait_seconds").record(0.0);
        metrics::gauge!("buzz_semantic_graph_traversal_limit")
            .set(state.config.semantic_graph_traversal_max_in_flight as f64);
        let traversal_permit = SemanticGraphTraversalPermit::new(traversal_permit);
        let read = begin_generation_bound_read(state, &ticket, reader_pubkey, context).await?;
        return build_stage_c_roots(
            StageCReadAdmission {
                read,
                traversal_permit,
            },
            query,
            &stage_a_context,
            channels,
            query_vectors,
            &unsupported_conditioned,
            context,
        )
        .await;
    }
}

/// Request-local identity of one encoded query-input batch (R4 item 6).
///
/// The ordered exact text digests of the encoder inputs plus the semantic
/// generation they were encoded under: equal identities mean a rebuilt batch
/// would send byte-identical Provider inputs under the same generation, so
/// the already encoded vectors may be re-bound instead of re-sent. The
/// identity is content-free — digests only, never query text.
#[derive(PartialEq)]
struct QueryVectorReuseIdentity {
    input_digests: Vec<Digest32>,
    generation_id: uuid::Uuid,
}

impl QueryVectorReuseIdentity {
    fn of(inputs: &[SemanticQueryEncoderInput], ticket: &SemanticGraphQueryTicket) -> Self {
        Self {
            input_digests: inputs.iter().map(|input| input.text_digest()).collect(),
            generation_id: ticket.generation.generation_id,
        }
    }
}

/// Pre-bind Provider-encoded values stashed for one churn restart.
enum ReusableEncodedInputs {
    /// Compatibility route values in input order.
    Compatibility(Vec<ProviderEncodedSemanticInput>),
    /// Migrated common-route bundle.
    Common(ProviderEncodedSemanticInputBundle),
}

/// R4 items 5 and 6: request-local vector reuse stash (plan §4.6).
///
/// Only pre-bind encoded values and their content-free identity are kept —
/// nothing crosses requests, pods, or processes and nothing is persisted.
/// The rebind under a fresh ticket revalidates every fence, so a generation
/// or contract movement rejects the reuse instead of reusing stale vectors.
struct ReusableQueryVectors {
    identity: QueryVectorReuseIdentity,
    encoded: ReusableEncodedInputs,
}

/// Re-bind stashed Provider-encoded values under one fresh ticket.
///
/// The matching reuse identity already proved the rebuilt inputs equal the
/// stashed batch byte-for-byte and in order, so only the fresh-ticket fence
/// validation runs here; its failure rejects the reuse and the caller
/// re-encodes.
fn rebind_reusable_query_vectors(
    ticket: &SemanticGraphQueryTicket,
    encoded: ReusableEncodedInputs,
) -> Result<SemanticGraphQueryVectorBundle, SemanticGraphRootQueryError> {
    match encoded {
        ReusableEncodedInputs::Common(bundle) => {
            SemanticGraphQueryVectorBundle::bind(ticket, bundle)
                .map_err(SemanticGraphRootQueryError::Database)
        }
        ReusableEncodedInputs::Compatibility(values) => {
            let vectors = values
                .into_iter()
                .map(|value| buzz_db::semantic_query::SemanticExactQueryVector::new(ticket, value))
                .collect::<Result<Vec<_>, _>>()
                .map_err(SemanticGraphRootQueryError::Database)?;
            SemanticGraphQueryVectorBundle::from_compatibility_vectors(ticket, vectors)
                .map_err(SemanticGraphRootQueryError::Database)
        }
    }
}

async fn classify_ticket_failure(
    state: &AppState,
    expected: &SemanticGraphQueryTicket,
    reader_pubkey: &[u8],
    work_deadline: Instant,
) -> SemanticGraphRootQueryError {
    match run_before_work_deadline(
        work_deadline,
        state.db.semantic_graph_query_ticket(
            expected.community_id,
            reader_pubkey,
            &state.relay_keypair.public_key(),
        ),
    )
    .await
    {
        Ok(Ok(fresh)) if !same_generation(&fresh, expected) => {
            SemanticGraphRootQueryError::SemanticGenerationChanged
        }
        Ok(Ok(fresh)) if fresh.project_context_revision != expected.project_context_revision => {
            SemanticGraphRootQueryError::ContextSourceChanged
        }
        Err(error) => error,
        Ok(Ok(_)) | Ok(Err(buzz_db::DbError::AccessDenied(_))) => {
            SemanticGraphRootQueryError::AuthorizationChanged
        }
        Ok(Err(error)) => SemanticGraphRootQueryError::Database(error),
    }
}

/// Map one neutral shared-executor outcome onto the frozen complete-path
/// public error.
///
/// The caller intercepts `ProviderUnavailable` first and re-reads the ticket
/// through [`classify_ticket_failure`], exactly as in the pre-R2 inline
/// sequence; this arm only mirrors that classifier's terminal fallback.
/// `AttemptLedgerExhausted` has no pre-R2 counterpart — it is unreachable
/// under the R2 zero-policy caps and maps onto the admission failure until
/// R4 owns real retry decisions. R3 adds `Cancelled`: a cancelled,
/// disconnected, or shutting-down request keeps the frozen deadline error.
fn map_provider_egress_failure(
    failure: SemanticProviderEgressFailure,
) -> SemanticGraphRootQueryError {
    match failure {
        SemanticProviderEgressFailure::DeadlineExceeded
        | SemanticProviderEgressFailure::Cancelled(_) => {
            SemanticGraphRootQueryError::QueryDeadlineExceeded
        }
        SemanticProviderEgressFailure::Database(error) => {
            SemanticGraphRootQueryError::Database(error)
        }
        SemanticProviderEgressFailure::AdmissionBusy
        | SemanticProviderEgressFailure::AttemptLedgerExhausted(_) => {
            SemanticGraphRootQueryError::QueryProviderBusy
        }
        SemanticProviderEgressFailure::ContextChanged => {
            SemanticGraphRootQueryError::ContextSourceChanged
        }
        SemanticProviderEgressFailure::FleetUnavailable => {
            SemanticGraphRootQueryError::QueryFleetUnavailable
        }
        SemanticProviderEgressFailure::ProviderUnavailable => {
            SemanticGraphRootQueryError::AuthorizationChanged
        }
        SemanticProviderEgressFailure::ReservationContractViolated => {
            invalid_state("query Provider reservation generation does not match its ticket")
        }
        SemanticProviderEgressFailure::PermitContractViolated => {
            invalid_state("query egress permit does not match its Provider reservation")
        }
        SemanticProviderEgressFailure::LatestStartUnrepresentable => {
            invalid_state("query Provider deadline cannot be represented")
        }
    }
}

/// Stage C session handed to the later Hyperedge traversal phase.
///
/// Keeping the transaction and process permit in this value prevents Phase 4
/// from accidentally traversing a newer graph snapshot or escaping the local
/// concurrency bound.
pub(crate) struct SemanticGraphRootQuerySession {
    pub(crate) read: SemanticGraphReadTx,
    pub(crate) query: SemanticGraphQuery,
    pub(crate) outcome: SemanticGraphRootQueryOutcome,
    pub(crate) query_vectors: SemanticGraphQueryVectorBundle,
    pub(crate) channels: Vec<QueryChannelBinding>,
    /// Later deadline reserved exclusively for closing the read-only snapshot.
    pub(crate) snapshot_close_deadline: Instant,
    pub(crate) absolute_deadline: Instant,
    pub(crate) snapshot_started_at: std::time::Instant,
    /// Reliability context of the logical request, carried into traversal
    /// and the later release/sign fence so every phase arbitrates the same
    /// latch and cancellation token.
    pub(crate) context: SemanticExecutionContext,
    /// Host-owned shutdown flag consulted between traversal units.
    pub(crate) shutdown: Arc<std::sync::atomic::AtomicBool>,
    _process_permit: OwnedSemaphorePermit,
    _traversal_permit: SemanticGraphTraversalPermit,
}

struct SemanticGraphTraversalPermit {
    _permit: OwnedSemaphorePermit,
}

impl SemanticGraphTraversalPermit {
    fn new(permit: OwnedSemaphorePermit) -> Self {
        metrics::gauge!("buzz_semantic_graph_traversal_in_flight").increment(1.0);
        Self { _permit: permit }
    }
}

impl Drop for SemanticGraphTraversalPermit {
    fn drop(&mut self) {
        metrics::gauge!("buzz_semantic_graph_traversal_in_flight").decrement(1.0);
    }
}

impl std::fmt::Debug for SemanticGraphRootQuerySession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticGraphRootQuerySession")
            .field("root_count", &self.outcome.roots.len())
            .field("query_vector_count", &self.query_vectors.len())
            .finish_non_exhaustive()
    }
}

/// Root-only result before traversal, response packing, postflight, or signing.
pub(crate) struct SemanticGraphRootQueryOutcome {
    pub(crate) ticket: SemanticGraphQueryTicket,
    pub(crate) input_observations: SemanticGraphQueryInputObservations,
    pub(crate) roots: Vec<SemanticGraphSelectedRoot>,
    pub(crate) coverage: SemanticGraphQueryCoverage,
    pub(crate) exhausted_dimensions: Vec<ExhaustedDimension>,
}

impl std::fmt::Debug for SemanticGraphRootQueryOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticGraphRootQueryOutcome")
            .field("root_count", &self.roots.len())
            .field("exhausted_dimensions", &self.exhausted_dimensions)
            .finish_non_exhaustive()
    }
}

/// One selected source root with all role-specific eligible entrypoints.
/// Phase 4 turns this into the public `SemanticRoot` after every seed has an
/// honest traversal outcome.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SemanticGraphSelectedRoot {
    pub(crate) root_id: Digest32,
    pub(crate) source: SemanticSourceIdentity,
    pub(crate) discovery_channels: Vec<RootDiscoveryChannel>,
    pub(crate) structural_entrypoints: Vec<RootStructuralEntrypoint>,
    pub(crate) preview: SemanticSourcePreview,
    pub(crate) lifecycle: buzz_semantic::SemanticLifecycleClass,
    pub(crate) source_status: Option<String>,
    pub(crate) canonical_provenance: CanonicalSourceProvenance,
    pub(crate) semantic_provenance: Option<SemanticProvenance>,
    /// Exact internal head retained for Stage 4 pair-currentness requests.
    pub(crate) semantic_head: Option<buzz_db::semantic_query::SemanticCurrentHead>,
    pub(crate) semantic_score: Option<Score>,
    pub(crate) score_explanation: Option<ScoreExplanation>,
    pub(crate) automatic_lane: Option<AutomaticRootLane>,
}

impl std::fmt::Debug for SemanticGraphSelectedRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SemanticGraphSelectedRoot")
            .field("source_kind", &self.source.kind)
            .field("discovery_channel_count", &self.discovery_channels.len())
            .field(
                "structural_entrypoint_count",
                &self.structural_entrypoints.len(),
            )
            .field("automatic_lane", &self.automatic_lane)
            .finish_non_exhaustive()
    }
}

/// Content-free closed failures. Public HTTP mapping and virtual Events belong
/// to Phase 5 and must not stringify nested provider/DB payloads into logs.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SemanticGraphRootQueryError {
    #[error("semantic graph query deployment master is disabled")]
    QueryDisabled,
    #[error("semantic graph query project does not match the host Community")]
    InvalidProject,
    #[error("semantic graph query process admission is busy")]
    ProcessBusy,
    #[error("semantic graph Stage C traversal admission is busy")]
    TraversalBusy,
    #[error("semantic graph query Provider is unavailable")]
    QueryEncoderUnavailable,
    #[error("semantic graph query Provider admission is busy")]
    QueryProviderBusy,
    #[error("semantic graph query HTTP fleet assertion is unavailable")]
    QueryFleetUnavailable,
    #[error("semantic graph query generation changed")]
    SemanticGenerationChanged,
    #[error("semantic graph query context source changed")]
    ContextSourceChanged,
    #[error("semantic graph query authorization or capability changed")]
    AuthorizationChanged,
    #[error("semantic graph query deadline exceeded")]
    QueryDeadlineExceeded,
    #[error("semantic graph query database operation failed")]
    Database(#[source] buzz_db::DbError),
    #[error("semantic graph query contract operation failed")]
    Contract(#[source] SemanticGraphQueryError),
}

impl From<buzz_db::DbError> for SemanticGraphRootQueryError {
    fn from(error: buzz_db::DbError) -> Self {
        Self::Database(error)
    }
}

impl From<SemanticGraphQueryError> for SemanticGraphRootQueryError {
    fn from(error: SemanticGraphQueryError) -> Self {
        Self::Contract(error)
    }
}

impl SemanticGraphRootQueryError {
    pub(crate) fn metric_code(&self) -> SemanticGraphQueryMetricError {
        match self {
            Self::QueryDisabled => SemanticGraphQueryMetricError::QueryDisabled,
            Self::InvalidProject => SemanticGraphQueryMetricError::InvalidProject,
            Self::ProcessBusy => SemanticGraphQueryMetricError::ProcessBusy,
            Self::TraversalBusy => SemanticGraphQueryMetricError::TraversalBusy,
            Self::QueryEncoderUnavailable => SemanticGraphQueryMetricError::ProviderUnavailable,
            Self::QueryProviderBusy => SemanticGraphQueryMetricError::ProviderBusy,
            Self::QueryFleetUnavailable => SemanticGraphQueryMetricError::Readiness,
            Self::SemanticGenerationChanged => {
                SemanticGraphQueryMetricError::SemanticGenerationChanged
            }
            Self::ContextSourceChanged => SemanticGraphQueryMetricError::ContextSourceChanged,
            Self::AuthorizationChanged => SemanticGraphQueryMetricError::AuthorizationChanged,
            Self::QueryDeadlineExceeded => SemanticGraphQueryMetricError::DeadlineExceeded,
            Self::Database(_) => SemanticGraphQueryMetricError::Database,
            Self::Contract(_) => SemanticGraphQueryMetricError::Contract,
        }
    }
}

#[derive(Debug, Clone)]
struct CandidateScores {
    source: SemanticSourceIdentity,
    problem_score: Score,
    conditioned_evidence: Vec<ConditionedEvidence>,
    highest_gain: Score,
    second_highest_gain: Score,
    environment_gain: Score,
    anchor_gain: AnchorGain,
    candidate_score: Score,
    discovered_problem_neutral: bool,
    discovery_channels: Vec<RootDiscoveryChannel>,
    structural_entrypoints: Vec<RootStructuralEntrypoint>,
    representative: SemanticExactSourceScore,
}

impl CandidateScores {
    fn explanation(&self) -> ScoreExplanation {
        ScoreExplanation {
            score_role: SemanticScoreRole::Candidate,
            problem_score: self.problem_score,
            conditioned_evidence: self.conditioned_evidence.clone(),
            highest_gain: self.highest_gain,
            second_highest_gain: self.second_highest_gain,
            environment_gain: self.environment_gain,
            anchor_gain: self.anchor_gain,
            local_coherence: None,
            document_score: None,
            target_coordinate_score: None,
            transition_score: None,
            penalties: Vec::new(),
            final_score: self.candidate_score,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SourcePairKey {
    left: SemanticSourceIdentity,
    right: SemanticSourceIdentity,
}

impl SourcePairKey {
    fn new(left: &SemanticSourceIdentity, right: &SemanticSourceIdentity) -> Self {
        if compare_sources(left, right).is_gt() {
            Self {
                left: right.clone(),
                right: left.clone(),
            }
        } else {
            Self {
                left: left.clone(),
                right: right.clone(),
            }
        }
    }
}

#[derive(Debug, Clone)]
struct RootSelectionState {
    selected: Vec<SelectedAutomaticRoot>,
    pair_scores: HashMap<SourcePairKey, Score>,
}

#[derive(Debug, Clone)]
pub(crate) struct QueryChannelBinding {
    pub(crate) channel_id: Digest32,
    pub(crate) context_coordinate: Option<ProjectContextCoordinate>,
}

#[derive(Debug, Clone)]
struct ExplicitRootMaterial {
    coordinate: ProjectContextCoordinate,
    source: SemanticSourceIdentity,
    incident_edge_keys: Vec<buzz_project_context::EdgeKey>,
    canonical: SemanticCanonicalSourceSnapshot,
    semantic_state: SemanticInitialHeadState,
}

struct StageCRootBuild {
    read: SemanticGraphReadTx,
    traversal_permit: SemanticGraphTraversalPermit,
    outcome: SemanticGraphRootQueryOutcome,
    query_vectors: SemanticGraphQueryVectorBundle,
    channels: Vec<QueryChannelBinding>,
    snapshot_started_at: std::time::Instant,
}

struct StageCReadAdmission {
    read: SemanticGraphReadTx,
    traversal_permit: SemanticGraphTraversalPermit,
}

fn prepare_candidate_scores(
    matrix: &[SemanticExactSourceScore],
    recall: &SemanticExactRecallBatch,
    channels: &[QueryChannelBinding],
    explicit_roots: &[ExplicitRootMaterial],
) -> Result<Vec<CandidateScores>, SemanticGraphRootQueryError> {
    let problem_channel = channels
        .first()
        .filter(|channel| channel.context_coordinate.is_none())
        .ok_or_else(|| invalid_state("query score matrix lacks Q0 channel"))?;
    if channels
        .iter()
        .skip(1)
        .any(|channel| channel.context_coordinate.is_none())
    {
        return Err(invalid_state("query score matrix repeats Q0 channel"));
    }

    let mut discovered: HashMap<SemanticSourceIdentity, Vec<RootDiscoveryChannel>> = HashMap::new();
    for hit in &recall.scores {
        let channel = channels
            .iter()
            .find(|channel| channel.channel_id == hit.channel_id)
            .ok_or_else(|| invalid_state("recall contains an unknown query channel"))?;
        let evidence = match channel.context_coordinate.as_ref() {
            None => RootDiscoveryChannel::ProblemNeutral,
            Some(context_coordinate) => RootDiscoveryChannel::ContextConditioned {
                context_coordinate: context_coordinate.clone(),
            },
        };
        discovered
            .entry(hit.source.clone())
            .or_default()
            .push(evidence);
    }

    let mut grouped: HashMap<SemanticSourceIdentity, Vec<&SemanticExactSourceScore>> =
        HashMap::new();
    for score in matrix {
        grouped.entry(score.source.clone()).or_default().push(score);
    }
    let explicit_sources: HashSet<SemanticSourceIdentity> = explicit_roots
        .iter()
        .map(|root| root.source.clone())
        .collect();
    let explicit_edges: HashSet<buzz_project_context::EdgeKey> = explicit_roots
        .iter()
        .flat_map(|root| root.incident_edge_keys.iter().copied())
        .collect();

    let mut candidates = Vec::with_capacity(grouped.len());
    for (source, rows) in grouped {
        if rows.len() != channels.len() {
            return Err(invalid_state(
                "candidate score matrix is incomplete for one source",
            ));
        }
        let score_channel_ids = rows
            .iter()
            .map(|row| row.channel_id)
            .collect::<HashSet<_>>();
        if score_channel_ids.len() != channels.len() {
            return Err(invalid_state(
                "candidate score matrix duplicates a query channel",
            ));
        }
        let representative = rows
            .first()
            .copied()
            .ok_or_else(|| invalid_state("candidate score group is empty"))?;
        if rows.iter().any(|row| {
            row.head != representative.head
                || row.lifecycle != representative.lifecycle
                || row.source_status != representative.source_status
                || row.roles != representative.roles
        }) {
            return Err(invalid_state(
                "candidate score matrix disagrees on current source provenance",
            ));
        }
        let problem_score = rows
            .iter()
            .find(|row| row.channel_id == problem_channel.channel_id)
            .map(|row| row.score)
            .ok_or_else(|| invalid_state("candidate score matrix lacks Q0 value"))?;
        let mut conditioned_evidence = Vec::with_capacity(channels.len().saturating_sub(1));
        for channel in channels.iter().skip(1) {
            let conditioned_score = rows
                .iter()
                .find(|row| row.channel_id == channel.channel_id)
                .map(|row| row.score)
                .ok_or_else(|| invalid_state("candidate score matrix lacks Qi value"))?;
            let context_coordinate = channel
                .context_coordinate
                .clone()
                .ok_or_else(|| invalid_state("conditioned channel lacks Coordinate"))?;
            conditioned_evidence.push(ConditionedEvidence::new(
                context_coordinate,
                problem_score,
                conditioned_score,
            ));
        }
        let environment = environment_gain(&conditioned_evidence);
        let anchor_gain = if explicit_sources.contains(&source) {
            AnchorGain::ExplicitInitial
        } else if representative
            .roles
            .coordinate_incident_edge_keys
            .iter()
            .any(|edge| explicit_edges.contains(edge))
            || representative
                .roles
                .context_document_bindings
                .iter()
                .any(|binding| explicit_edges.contains(&binding.edge_key))
        {
            AnchorGain::SameHyperedge
        } else {
            AnchorGain::None
        };
        let score = candidate_score(problem_score, environment.environment_gain, anchor_gain);
        let mut discovery_channels = discovered.remove(&source).unwrap_or_default();
        if explicit_sources.contains(&source) {
            discovery_channels.push(RootDiscoveryChannel::ExplicitInitial);
        }
        canonicalize_discovery_channels(&mut discovery_channels);

        let mut structural_entrypoints = Vec::new();
        if representative.roles.coordinate_entry_eligible
            && (explicit_sources.contains(&source) || problem_score >= BASE_ENTRY_FLOOR)
        {
            structural_entrypoints.push(RootStructuralEntrypoint::Coordinate {
                coordinate: coordinate_for_source(&source),
            });
        }
        if problem_score >= BASE_ENTRY_FLOOR && score >= RELATION_FLOOR {
            if !representative.roles.context_document_bindings.is_empty()
                && source.kind != SemanticSourceKind::ProjectDocument
            {
                return Err(invalid_state(
                    "non-Document source has a Context Document structural role",
                ));
            }
            structural_entrypoints.extend(
                representative
                    .roles
                    .context_document_bindings
                    .iter()
                    .map(|binding| context_document_entrypoint(source.source_id, binding)),
            );
        }
        canonicalize_entrypoints(&mut structural_entrypoints);
        let discovered_problem_neutral =
            discovery_channels.contains(&RootDiscoveryChannel::ProblemNeutral);
        candidates.push(CandidateScores {
            source,
            problem_score,
            conditioned_evidence: environment.conditioned_evidence,
            highest_gain: environment.highest_gain,
            second_highest_gain: environment.second_highest_gain,
            environment_gain: environment.environment_gain,
            anchor_gain,
            candidate_score: score,
            discovered_problem_neutral,
            discovery_channels,
            structural_entrypoints,
            representative: representative.clone(),
        });
    }
    if !discovered.is_empty() {
        return Err(invalid_state(
            "recalled source is absent from the complete score matrix",
        ));
    }
    candidates.sort_by(|left, right| compare_sources(&left.source, &right.source));
    Ok(candidates)
}

fn context_document_entrypoint(
    document_id: uuid::Uuid,
    binding: &buzz_db::semantic_query::SemanticContextDocumentBinding,
) -> RootStructuralEntrypoint {
    RootStructuralEntrypoint::ContextDocument {
        edge_key: binding.edge_key,
        document_id,
        edge_provenance: ProjectContextEdgeProvenance {
            last_context_revision: binding.edge_last_context_revision,
            source_change_id: binding.edge_source_change_id,
        },
        binding_provenance: ProjectContextBindingProvenance {
            binding_context_revision: binding.binding_context_revision,
            source_change_id: binding.binding_source_change_id,
            projection_event_id: binding.binding_projection_event_id,
        },
    }
}

impl RootSelectionState {
    fn new() -> Self {
        Self {
            selected: Vec::new(),
            pair_scores: HashMap::new(),
        }
    }

    fn redundancy(&self, source: &SemanticSourceIdentity) -> Score {
        self.selected
            .iter()
            .filter_map(|selected| {
                self.pair_scores
                    .get(&SourcePairKey::new(source, &selected.source))
                    .copied()
            })
            .max()
            .unwrap_or(Score::ZERO)
    }
}

fn next_automatic_root(
    candidates: &[CandidateScores],
    state: &RootSelectionState,
    lane: AutomaticRootLane,
) -> Option<SelectedAutomaticRoot> {
    candidates
        .iter()
        .filter(|candidate| {
            candidate.problem_score >= BASE_ENTRY_FLOOR
                && !candidate.structural_entrypoints.is_empty()
                && !state
                    .selected
                    .iter()
                    .any(|selected| selected.source == candidate.source)
                && (lane == AutomaticRootLane::Mixed || candidate.discovered_problem_neutral)
        })
        .map(|candidate| {
            let relevance_score = match lane {
                AutomaticRootLane::ProblemNeutral => candidate.problem_score,
                AutomaticRootLane::Mixed => candidate.candidate_score,
            };
            SelectedAutomaticRoot {
                source: candidate.source.clone(),
                lane,
                relevance_score,
                selection_priority: root_diversity_priority(
                    relevance_score,
                    state.redundancy(&candidate.source),
                ),
            }
        })
        .max_by(|left, right| {
            left.selection_priority
                .cmp(&right.selection_priority)
                .then_with(|| left.relevance_score.cmp(&right.relevance_score))
                .then_with(|| compare_sources(&right.source, &left.source))
        })
}

fn strongest_problem_neutral(candidates: &[CandidateScores]) -> Option<SelectedAutomaticRoot> {
    candidates
        .iter()
        .filter(|candidate| {
            candidate.problem_score >= BASE_ENTRY_FLOOR
                && candidate.discovered_problem_neutral
                && !candidate.structural_entrypoints.is_empty()
        })
        .max_by(|left, right| {
            left.problem_score
                .cmp(&right.problem_score)
                .then_with(|| compare_sources(&right.source, &left.source))
        })
        .map(|candidate| SelectedAutomaticRoot {
            source: candidate.source.clone(),
            lane: AutomaticRootLane::ProblemNeutral,
            relevance_score: candidate.problem_score,
            selection_priority: candidate.problem_score,
        })
}

async fn select_automatic_roots_incremental(
    read: &mut SemanticGraphReadTx,
    candidates: &[CandidateScores],
    maximum: u16,
    context: &SemanticExecutionContext,
) -> Result<Vec<SelectedAutomaticRoot>, SemanticGraphRootQueryError> {
    let qualifying = candidates
        .iter()
        .filter(|candidate| {
            candidate.problem_score >= BASE_ENTRY_FLOOR
                && !candidate.structural_entrypoints.is_empty()
                && !candidate
                    .discovery_channels
                    .contains(&RootDiscoveryChannel::ExplicitInitial)
        })
        .cloned()
        .collect::<Vec<_>>();
    if maximum == 0 || qualifying.is_empty() {
        return Ok(Vec::new());
    }

    let limit = usize::from(maximum);
    let neutral_reserved = limit.div_ceil(2);
    let mut state = RootSelectionState::new();
    let mut redundancy_loaded_for = HashSet::new();
    if let Some(pinned) = strongest_problem_neutral(&qualifying) {
        redundancy_loaded_for.insert(pinned.source.clone());
        state.selected.push(pinned);
        load_redundancy_for_selected(
            read,
            &qualifying,
            state.selected.last().map(|selected| &selected.source),
            &mut state.pair_scores,
            context,
        )
        .await?;
    }

    while state.selected.len() < neutral_reserved && state.selected.len() < limit {
        let Some(next) =
            next_automatic_root(&qualifying, &state, AutomaticRootLane::ProblemNeutral)
        else {
            break;
        };
        let source = next.source.clone();
        state.selected.push(next);
        if redundancy_loaded_for.insert(source.clone()) {
            load_redundancy_for_selected(
                read,
                &qualifying,
                Some(&source),
                &mut state.pair_scores,
                context,
            )
            .await?;
        }
    }

    while state.selected.len() < limit {
        let Some(next) = next_automatic_root(&qualifying, &state, AutomaticRootLane::Mixed) else {
            break;
        };
        let source = next.source.clone();
        state.selected.push(next);
        if redundancy_loaded_for.insert(source.clone()) {
            load_redundancy_for_selected(
                read,
                &qualifying,
                Some(&source),
                &mut state.pair_scores,
                context,
            )
            .await?;
        }
    }
    Ok(state.selected)
}

async fn load_redundancy_for_selected(
    read: &mut SemanticGraphReadTx,
    candidates: &[CandidateScores],
    selected: Option<&SemanticSourceIdentity>,
    pair_scores: &mut HashMap<SourcePairKey, Score>,
    context: &SemanticExecutionContext,
) -> Result<(), SemanticGraphRootQueryError> {
    let Some(selected) = selected else {
        return Ok(());
    };
    let pairs = candidates
        .iter()
        .filter(|candidate| candidate.source != *selected)
        .filter(|candidate| {
            !pair_scores.contains_key(&SourcePairKey::new(&candidate.source, selected))
        })
        .map(|candidate| SemanticCurrentSourcePair {
            left: candidate.source.clone(),
            right: selected.clone(),
        })
        .collect::<Vec<_>>();
    if pairs.is_empty() {
        return Ok(());
    }
    let expected = pairs
        .iter()
        .map(|pair| SourcePairKey::new(&pair.left, &pair.right))
        .collect::<HashSet<_>>();
    let observed = run_root_stage(context, read.score_current_source_pairs_exact(&pairs)).await??;
    record_db_distance_rows(SemanticGraphDistanceStage::RootRedundancy, observed.len());
    if observed.len() != expected.len() {
        return Err(invalid_state("root redundancy score result is incomplete"));
    }
    for pair in observed {
        let key = SourcePairKey::new(&pair.left, &pair.right);
        if !expected.contains(&key) || pair_scores.insert(key, pair.score).is_some() {
            return Err(invalid_state(
                "root redundancy score result is duplicated or unexpected",
            ));
        }
    }
    Ok(())
}

fn compare_sources(left: &SemanticSourceIdentity, right: &SemanticSourceIdentity) -> Ordering {
    left.community_id
        .as_bytes()
        .cmp(right.community_id.as_bytes())
        .then_with(|| source_kind_rank(left.kind).cmp(&source_kind_rank(right.kind)))
        .then_with(|| left.source_id.as_bytes().cmp(right.source_id.as_bytes()))
}

const fn source_kind_rank(kind: SemanticSourceKind) -> (u8, u8) {
    match kind {
        SemanticSourceKind::ProjectView(subtype) => (0, project_view_kind_rank(subtype)),
        SemanticSourceKind::ProjectDocument => (1, 0),
        SemanticSourceKind::Meeting => (2, 0),
    }
}

const fn project_view_kind_rank(kind: ProjectViewSemanticType) -> u8 {
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

fn coordinate_for_source(source: &SemanticSourceIdentity) -> ProjectContextCoordinate {
    match source.kind {
        SemanticSourceKind::ProjectView(kind) => ProjectContextCoordinate::ProjectViewObject {
            object_type: project_view_object_type(kind),
            object_id: source.source_id,
        },
        SemanticSourceKind::ProjectDocument => ProjectContextCoordinate::Document {
            document_id: source.source_id,
        },
        SemanticSourceKind::Meeting => ProjectContextCoordinate::Meeting {
            meeting_id: source.source_id,
        },
    }
}

const fn project_view_object_type(kind: ProjectViewSemanticType) -> ProjectViewObjectType {
    match kind {
        ProjectViewSemanticType::ProjectProfile => ProjectViewObjectType::ProjectProfile,
        ProjectViewSemanticType::Goal => ProjectViewObjectType::Goal,
        ProjectViewSemanticType::Role => ProjectViewObjectType::Role,
        ProjectViewSemanticType::Plan => ProjectViewObjectType::Plan,
        ProjectViewSemanticType::Stage => ProjectViewObjectType::Stage,
        ProjectViewSemanticType::Requirement => ProjectViewObjectType::Requirement,
        ProjectViewSemanticType::Issue => ProjectViewObjectType::Issue,
        ProjectViewSemanticType::Work => ProjectViewObjectType::Work,
        ProjectViewSemanticType::Resource => ProjectViewObjectType::Resource,
    }
}

fn canonicalize_discovery_channels(channels: &mut Vec<RootDiscoveryChannel>) {
    channels.sort_by_key(discovery_channel_key);
    channels.dedup();
}

fn discovery_channel_key(channel: &RootDiscoveryChannel) -> (u8, Option<ProjectContextCoordinate>) {
    match channel {
        RootDiscoveryChannel::ExplicitInitial => (0, None),
        RootDiscoveryChannel::ProblemNeutral => (1, None),
        RootDiscoveryChannel::ContextConditioned { context_coordinate } => {
            (2, Some(context_coordinate.clone()))
        }
    }
}

fn canonicalize_entrypoints(entrypoints: &mut Vec<RootStructuralEntrypoint>) {
    entrypoints.sort_by(compare_entrypoints);
    entrypoints.dedup();
}

fn compare_entrypoints(
    left: &RootStructuralEntrypoint,
    right: &RootStructuralEntrypoint,
) -> Ordering {
    match (left, right) {
        (
            RootStructuralEntrypoint::Coordinate { coordinate: left },
            RootStructuralEntrypoint::Coordinate { coordinate: right },
        ) => left.cmp(right),
        (RootStructuralEntrypoint::Coordinate { .. }, _) => Ordering::Less,
        (_, RootStructuralEntrypoint::Coordinate { .. }) => Ordering::Greater,
        (
            RootStructuralEntrypoint::ContextDocument {
                edge_key: left_edge,
                document_id: left_document,
                ..
            },
            RootStructuralEntrypoint::ContextDocument {
                edge_key: right_edge,
                document_id: right_document,
                ..
            },
        ) => left_edge
            .as_bytes()
            .cmp(right_edge.as_bytes())
            .then_with(|| left_document.as_bytes().cmp(right_document.as_bytes())),
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

fn canonical_provenance(score: &SemanticExactSourceScore) -> CanonicalSourceProvenance {
    CanonicalSourceProvenance {
        source_basis: score.head.source_basis.clone(),
        source_invalidation_epoch: score.head.invalidation_epoch,
        source_snapshot_digest: score.head.snapshot_digest,
        summary_coverage: score.head.summary_coverage,
    }
}

fn coverage_from_db(
    observed: &buzz_db::semantic_query::SemanticGraphCoverage,
) -> (EmbeddingCoverageCounts, DegradedModeCounts) {
    let get = |class| observed.embedding.get(&class).copied().unwrap_or(0);
    let embedding = EmbeddingCoverageCounts {
        current: get(SemanticGraphEmbeddingCoverageClass::Current),
        missing: get(SemanticGraphEmbeddingCoverageClass::Missing),
        building: get(SemanticGraphEmbeddingCoverageClass::Building),
        failed: get(SemanticGraphEmbeddingCoverageClass::Failed),
        unsupported: get(SemanticGraphEmbeddingCoverageClass::Unsupported),
        non_queryable_zero_vector: get(SemanticGraphEmbeddingCoverageClass::NonQueryableZeroVector),
    };
    let degraded = DegradedModeCounts {
        index_coverage_partial: observed
            .authorized_graph_sources
            .saturating_sub(observed.current_indexed_graph_sources),
        ..DegradedModeCounts::default()
    };
    (embedding, degraded)
}

fn explicit_root_materials(
    community_id: CommunityId,
    batch: &SemanticInitialCoordinateObservationBatch,
) -> Result<Vec<ExplicitRootMaterial>, SemanticGraphRootQueryError> {
    batch
        .observations
        .iter()
        .filter_map(|observation| match observation {
            SemanticInitialCoordinateObservation::Accepted {
                coordinate,
                graph_membership,
                canonical,
                semantic_state,
            } => Some((coordinate, graph_membership, canonical, semantic_state)),
            SemanticInitialCoordinateObservation::NotInGraph { .. }
            | SemanticInitialCoordinateObservation::Omitted { .. } => None,
        })
        .map(
            |(coordinate, graph_membership, canonical, semantic_state)| {
                Ok(ExplicitRootMaterial {
                    coordinate: coordinate.clone(),
                    source: semantic_source_identity_for_coordinate(community_id, coordinate)?,
                    incident_edge_keys: graph_membership.incident_edge_keys.clone(),
                    canonical: canonical.as_ref().clone(),
                    semantic_state: semantic_state.clone(),
                })
            },
        )
        .collect()
}

fn build_input_observations(
    generation_id: uuid::Uuid,
    initial: &SemanticInitialCoordinateObservationBatch,
    context: &SemanticContextCoordinateObservationBatch,
    unsupported_conditioned: &HashSet<ProjectContextCoordinate>,
) -> SemanticGraphQueryInputObservations {
    let mut accepted_initial_coordinates = Vec::new();
    let mut initial_not_in_graph = Vec::new();
    let mut omitted_initial_coordinates = Vec::new();
    for observation in &initial.observations {
        match observation {
            SemanticInitialCoordinateObservation::Accepted {
                coordinate,
                graph_membership,
                canonical,
                semantic_state,
            } => accepted_initial_coordinates.push(AcceptedInitialCoordinateObservation {
                coordinate: coordinate.clone(),
                graph_membership: CurrentGraphMembershipObservation {
                    context_revision: graph_membership.project_context_revision,
                    incident_edge_keys: graph_membership.incident_edge_keys.clone(),
                },
                source_basis: canonical.source_basis.clone(),
                semantic_state: match semantic_state {
                    SemanticInitialHeadState::Current(head) => {
                        SemanticHeadState::Current(SemanticHeadProvenance {
                            generation_id,
                            unit_key: head.unit_key.clone(),
                            snapshot_digest: head.snapshot_digest,
                        })
                    }
                    SemanticInitialHeadState::Missing => SemanticHeadState::Missing,
                    SemanticInitialHeadState::Building => SemanticHeadState::Building,
                    SemanticInitialHeadState::Failed => SemanticHeadState::Failed,
                    SemanticInitialHeadState::Unsupported => SemanticHeadState::Unsupported,
                },
            }),
            SemanticInitialCoordinateObservation::NotInGraph { coordinate, .. } => {
                initial_not_in_graph.push(coordinate.clone());
            }
            SemanticInitialCoordinateObservation::Omitted {
                coordinate,
                graph_membership,
                reason,
            } => omitted_initial_coordinates.push(OmittedInitialCoordinateObservation {
                coordinate: coordinate.clone(),
                graph_membership: CurrentGraphMembershipObservation {
                    context_revision: graph_membership.project_context_revision,
                    incident_edge_keys: graph_membership.incident_edge_keys.clone(),
                },
                reason: match reason {
                    SemanticInitialOmissionReason::SourceNotFound => {
                        OmittedInitialCoordinateReason::SourceNotFound
                    }
                    SemanticInitialOmissionReason::SourceDeleted => {
                        OmittedInitialCoordinateReason::SourceDeleted
                    }
                    SemanticInitialOmissionReason::SourceTombstoned => {
                        OmittedInitialCoordinateReason::SourceTombstoned
                    }
                    SemanticInitialOmissionReason::SourceIneligible => {
                        OmittedInitialCoordinateReason::SourceIneligible
                    }
                },
            }),
        }
    }

    let mut accepted_context_coordinates = Vec::new();
    let mut omitted_context_coordinates = Vec::new();
    for observation in &context.observations {
        match observation {
            SemanticContextCoordinateObservation::Accepted(accepted)
                if unsupported_conditioned.contains(&accepted.coordinate) =>
            {
                omitted_context_coordinates.push(OmittedContextCoordinateObservation {
                    coordinate: accepted.coordinate.clone(),
                    reason: OmittedContextCoordinateReason::ConditionedInputUnsupported,
                });
            }
            SemanticContextCoordinateObservation::Accepted(accepted) => {
                accepted_context_coordinates.push(AcceptedContextCoordinateObservation {
                    coordinate: accepted.coordinate.clone(),
                    source_basis: accepted.canonical.source_basis.clone(),
                    lifecycle: accepted.canonical.lifecycle,
                    semantic_head: SemanticHeadProvenance {
                        generation_id,
                        unit_key: accepted.semantic_head.unit_key.clone(),
                        snapshot_digest: accepted.semantic_head.snapshot_digest,
                    },
                });
            }
            SemanticContextCoordinateObservation::Omitted {
                coordinate, reason, ..
            } => {
                omitted_context_coordinates.push(OmittedContextCoordinateObservation {
                    coordinate: coordinate.clone(),
                    reason: context_omission_reason(*reason),
                });
            }
        }
    }
    SemanticGraphQueryInputObservations {
        accepted_initial_coordinates,
        initial_not_in_graph,
        omitted_initial_coordinates,
        accepted_context_coordinates,
        omitted_context_coordinates,
    }
}

fn context_omission_reason(
    reason: SemanticContextOmissionReason,
) -> OmittedContextCoordinateReason {
    match reason {
        SemanticContextOmissionReason::SourceNotFound => {
            OmittedContextCoordinateReason::SourceNotFound
        }
        SemanticContextOmissionReason::SourceIneligible => {
            OmittedContextCoordinateReason::SourceIneligible
        }
        SemanticContextOmissionReason::SemanticHeadMissing => {
            OmittedContextCoordinateReason::SemanticHeadMissing
        }
        SemanticContextOmissionReason::SemanticHeadBuilding => {
            OmittedContextCoordinateReason::SemanticHeadBuilding
        }
        SemanticContextOmissionReason::SemanticHeadFailed => {
            OmittedContextCoordinateReason::SemanticHeadFailed
        }
    }
}

fn omitted_context_counts(
    observations: &SemanticGraphQueryInputObservations,
) -> OmittedContextChannelCounts {
    let mut counts = OmittedContextChannelCounts::default();
    for observation in &observations.omitted_context_coordinates {
        match observation.reason {
            OmittedContextCoordinateReason::SourceNotFound => counts.source_not_found += 1,
            OmittedContextCoordinateReason::SourceIneligible => counts.source_ineligible += 1,
            OmittedContextCoordinateReason::SemanticHeadMissing => {
                counts.semantic_head_missing += 1;
            }
            OmittedContextCoordinateReason::SemanticHeadBuilding => {
                counts.semantic_head_building += 1;
            }
            OmittedContextCoordinateReason::SemanticHeadFailed => {
                counts.semantic_head_failed += 1;
            }
            OmittedContextCoordinateReason::ConditionedInputUnsupported => {
                counts.conditioned_input_unsupported += 1;
            }
        }
    }
    counts
}

fn conditioned_overviews(
    context: &SemanticContextCoordinateObservationBatch,
) -> Vec<ConditionedContextOverview> {
    context
        .observations
        .iter()
        .filter_map(|observation| match observation {
            SemanticContextCoordinateObservation::Accepted(accepted) => {
                Some(ConditionedContextOverview {
                    coordinate: accepted.coordinate.clone(),
                    current_overview_semantic_text: accepted.semantic_text.clone(),
                })
            }
            SemanticContextCoordinateObservation::Omitted { .. } => None,
        })
        .collect()
}

fn context_observations_still_match(
    expected: &SemanticContextCoordinateObservationBatch,
    observed: &SemanticContextCoordinateObservationBatch,
) -> bool {
    expected.snapshot.community_id == observed.snapshot.community_id
        && expected.snapshot.generation_id == observed.snapshot.generation_id
        && expected.observations == observed.observations
}

fn build_selected_roots(
    project_id: uuid::Uuid,
    ticket: &SemanticGraphQueryTicket,
    explicit: &[ExplicitRootMaterial],
    candidates: &[CandidateScores],
    selected_automatic: &[SelectedAutomaticRoot],
    automatic_hydration: &SemanticCanonicalHydrationBatch,
) -> Result<Vec<SemanticGraphSelectedRoot>, SemanticGraphRootQueryError> {
    let mut roots = Vec::with_capacity(explicit.len() + selected_automatic.len());
    let explicit_sources = explicit
        .iter()
        .map(|root| root.source.clone())
        .collect::<HashSet<_>>();

    let mut explicit = explicit.to_vec();
    explicit.sort_by(|left, right| compare_sources(&left.source, &right.source));
    for material in explicit {
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.source == material.source);
        let mut discovery_channels = candidate
            .map(|candidate| candidate.discovery_channels.clone())
            .unwrap_or_default();
        discovery_channels.push(RootDiscoveryChannel::ExplicitInitial);
        canonicalize_discovery_channels(&mut discovery_channels);
        let mut structural_entrypoints = candidate
            .map(|candidate| candidate.structural_entrypoints.clone())
            .unwrap_or_default();
        structural_entrypoints.push(RootStructuralEntrypoint::Coordinate {
            coordinate: material.coordinate.clone(),
        });
        canonicalize_entrypoints(&mut structural_entrypoints);
        let semantic = match (&material.semantic_state, candidate) {
            (SemanticInitialHeadState::Current(expected), Some(candidate))
                if candidate.representative.head == *expected =>
            {
                Some(candidate)
            }
            (SemanticInitialHeadState::Current(_), _) => {
                return Err(invalid_state(
                    "current explicit root is absent from the exact score matrix",
                ));
            }
            (
                SemanticInitialHeadState::Missing
                | SemanticInitialHeadState::Building
                | SemanticInitialHeadState::Failed
                | SemanticInitialHeadState::Unsupported,
                _,
            ) => None,
        };
        let canonical_provenance = canonical_provenance_from_snapshot(
            &material.canonical,
            semantic.map(|candidate| candidate.representative.head.summary_coverage),
        );
        let semantic_provenance =
            semantic.map(|candidate| semantic_provenance(ticket, &candidate.representative));
        let semantic_score = semantic.map(|candidate| candidate.candidate_score);
        let score_explanation = semantic.map(CandidateScores::explanation);
        let root_id = derive_root_id(project_id, &material.source, &structural_entrypoints)?;
        roots.push(SemanticGraphSelectedRoot {
            root_id,
            source: material.source,
            discovery_channels,
            structural_entrypoints,
            preview: preview_from_snapshot(&material.canonical),
            lifecycle: material.canonical.lifecycle,
            source_status: material.canonical.source_status.clone(),
            canonical_provenance,
            semantic_provenance,
            semantic_head: semantic.map(|candidate| candidate.representative.head.clone()),
            semantic_score,
            score_explanation,
            automatic_lane: None,
        });
    }

    for selected in selected_automatic {
        if explicit_sources.contains(&selected.source) {
            return Err(invalid_state(
                "automatic root selection repeated an explicit source",
            ));
        }
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.source == selected.source)
            .ok_or_else(|| invalid_state("selected automatic root lacks score state"))?;
        let hydrated = automatic_hydration
            .sources
            .iter()
            .find(|hydrated| hydrated.canonical.source == selected.source)
            .ok_or_else(|| invalid_state("selected automatic root lacks canonical hydration"))?;
        if hydrated.semantic_head != candidate.representative.head {
            return Err(invalid_state(
                "automatic root hydration disagrees with exact score head",
            ));
        }
        let root_id = derive_root_id(
            project_id,
            &candidate.source,
            &candidate.structural_entrypoints,
        )?;
        roots.push(SemanticGraphSelectedRoot {
            root_id,
            source: candidate.source.clone(),
            discovery_channels: candidate.discovery_channels.clone(),
            structural_entrypoints: candidate.structural_entrypoints.clone(),
            preview: preview_from_snapshot(&hydrated.canonical),
            lifecycle: hydrated.canonical.lifecycle,
            source_status: hydrated.canonical.source_status.clone(),
            canonical_provenance: canonical_provenance(&candidate.representative),
            semantic_provenance: Some(semantic_provenance(ticket, &candidate.representative)),
            semantic_head: Some(candidate.representative.head.clone()),
            semantic_score: Some(candidate.candidate_score),
            score_explanation: Some(candidate.explanation()),
            automatic_lane: Some(selected.lane),
        });
    }
    Ok(roots)
}

fn preview_from_snapshot(snapshot: &SemanticCanonicalSourceSnapshot) -> SemanticSourcePreview {
    SemanticSourcePreview {
        title: snapshot.title.clone(),
        summary: snapshot.summary.clone(),
        summary_omitted_reason: None,
    }
}

fn canonical_provenance_from_snapshot(
    snapshot: &SemanticCanonicalSourceSnapshot,
    semantic_coverage: Option<buzz_semantic::SemanticCoverage>,
) -> CanonicalSourceProvenance {
    let summary_coverage = semantic_coverage.unwrap_or_else(|| {
        if snapshot.summary.is_some() {
            buzz_semantic::SemanticCoverage::TitleAndSummary
        } else {
            buzz_semantic::SemanticCoverage::TitleOnly
        }
    });
    CanonicalSourceProvenance {
        source_basis: snapshot.source_basis.clone(),
        source_invalidation_epoch: snapshot.source_invalidation_epoch,
        source_snapshot_digest: snapshot.source_snapshot_digest,
        summary_coverage,
    }
}

async fn build_stage_c_roots(
    admission: StageCReadAdmission,
    query: &SemanticGraphQuery,
    expected_context: &SemanticContextCoordinateObservationBatch,
    channels: Vec<QueryChannelBinding>,
    query_vectors: SemanticGraphQueryVectorBundle,
    unsupported_conditioned: &HashSet<ProjectContextCoordinate>,
    context: &SemanticExecutionContext,
) -> Result<StageCRootBuild, SemanticGraphRootQueryError> {
    let StageCReadAdmission {
        mut read,
        traversal_permit,
    } = admission;
    let snapshot_started_at = std::time::Instant::now();
    let observed_context = run_root_stage(
        context,
        read.observe_context_coordinates(&query.context_coordinates),
    )
    .await??;
    if !context_observations_still_match(expected_context, &observed_context) {
        let _ = read.rollback().await;
        return Err(SemanticGraphRootQueryError::ContextSourceChanged);
    }

    let initial = run_root_stage(
        context,
        read.observe_initial_coordinates(&query.initial_coordinates),
    )
    .await??;
    let explicit = explicit_root_materials(read.ticket().community_id, &initial)?;
    let explicit_sources = explicit
        .iter()
        .map(|root| root.source.clone())
        .collect::<Vec<_>>();

    let recall = run_root_stage(
        context,
        read.recall_current_graph_sources_exact(
            query.lifecycle_filter,
            &explicit_sources,
            &query_vectors,
            query.budget.max_recall_per_channel,
        ),
    )
    .await??;
    record_db_distance_rows(SemanticGraphDistanceStage::RootRecall, recall.scores.len());
    let mut candidate_sources = recall
        .scores
        .iter()
        .map(|score| score.source.clone())
        .chain(explicit_sources.iter().cloned())
        .collect::<Vec<_>>();
    candidate_sources.sort_by(compare_sources);
    candidate_sources.dedup();
    let matrix = run_root_stage(
        context,
        read.score_candidate_matrix_exact(
            query.lifecycle_filter,
            &explicit_sources,
            &query_vectors,
            &candidate_sources,
        ),
    )
    .await??;
    record_db_distance_rows(SemanticGraphDistanceStage::RootMatrix, matrix.len());
    let candidates = prepare_candidate_scores(&matrix, &recall, &channels, &explicit)?;
    let selected_automatic = select_automatic_roots_incremental(
        &mut read,
        &candidates,
        query.budget.max_semantic_roots,
        context,
    )
    .await?;
    let selected_scores = selected_automatic
        .iter()
        .map(|selected| {
            candidates
                .iter()
                .find(|candidate| candidate.source == selected.source)
                .map(|candidate| candidate.representative.clone())
                .ok_or_else(|| invalid_state("selected root lost its exact score"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let automatic_hydration = run_root_stage(
        context,
        read.hydrate_current_exact_sources(&selected_scores),
    )
    .await??;
    let db_coverage = run_root_stage(
        context,
        read.semantic_graph_coverage(query.lifecycle_filter, &explicit_sources),
    )
    .await??;

    let input_observations = build_input_observations(
        read.ticket().generation.generation_id,
        &initial,
        &observed_context,
        unsupported_conditioned,
    );
    let roots = build_selected_roots(
        query.project_id,
        read.ticket(),
        &explicit,
        &candidates,
        &selected_automatic,
        &automatic_hydration,
    )?;
    let (embedding_coverage, degraded_mode_counts) = coverage_from_db(&db_coverage);

    let explicit_set = explicit_sources.into_iter().collect::<HashSet<_>>();
    let qualifying = candidates
        .iter()
        .filter(|candidate| {
            !explicit_set.contains(&candidate.source)
                && candidate.problem_score >= BASE_ENTRY_FLOOR
                && !candidate.structural_entrypoints.is_empty()
        })
        .collect::<Vec<_>>();
    let neutral_candidates_considered = qualifying
        .iter()
        .filter(|candidate| candidate.discovered_problem_neutral)
        .count() as u64;
    let conditioned_candidates_considered = qualifying
        .iter()
        .filter(|candidate| {
            candidate
                .discovery_channels
                .iter()
                .any(|channel| matches!(channel, RootDiscoveryChannel::ContextConditioned { .. }))
        })
        .count() as u64;
    let suppressed_roots = qualifying.len().saturating_sub(selected_automatic.len()) as u64;
    let truncated_recall_channels = recall
        .channels
        .iter()
        .filter(|channel| channel.exhaustion == SemanticExactRecallExhaustion::Truncated)
        .count() as u64;
    let mut exhausted_dimensions = Vec::new();
    if truncated_recall_channels > 0 {
        exhausted_dimensions.push(ExhaustedDimension::RecallPerChannel);
    }
    if suppressed_roots > 0 {
        exhausted_dimensions.push(ExhaustedDimension::SemanticRoots);
    }
    let coverage = SemanticGraphQueryCoverage {
        authorized_graph_sources: db_coverage.authorized_graph_sources,
        current_indexed_graph_sources: db_coverage.current_indexed_graph_sources,
        title_only_sources: db_coverage.title_only_sources,
        embedding_coverage,
        query_channels_requested: 1 + query.context_coordinates.len() as u64,
        query_channels_executed: query_vectors.len() as u64,
        omitted_context_channel_counts_by_reason: omitted_context_counts(&input_observations),
        neutral_candidates_considered,
        conditioned_candidates_considered,
        roots_selected: roots.len() as u64,
        roots_returned: 0,
        expanded_coordinates: 0,
        incident_edges_materialized: 0,
        relation_options_materialized: 0,
        target_options_materialized: 0,
        paths_generated: 0,
        paths_retained: 0,
        paths_returned: 0,
        omitted_for_response_budget: OmittedForResponseBudgetCounts::default(),
        truncation_counts_by_dimension: TruncationCountsByDimension {
            recall_per_channel: truncated_recall_channels,
            semantic_roots: suppressed_roots,
            ..TruncationCountsByDimension::default()
        },
        truncation_samples: Vec::new(),
        degraded_mode_counts,
    };
    coverage.validate()?;
    let ticket = read.ticket().clone();
    Ok(StageCRootBuild {
        read,
        traversal_permit,
        outcome: SemanticGraphRootQueryOutcome {
            ticket,
            input_observations,
            roots,
            coverage,
            exhausted_dimensions,
        },
        query_vectors,
        channels,
        snapshot_started_at,
    })
}

fn query_read_timeouts(remaining: Duration) -> SemanticGraphReadTimeouts {
    let positive = remaining.max(Duration::from_millis(1));
    SemanticGraphReadTimeouts {
        statement: positive.min(Duration::from_secs(5)),
        lock: positive.min(Duration::from_millis(250)),
        idle_in_transaction: positive.min(Duration::from_secs(10)),
    }
}

async fn observe_context_snapshot(
    state: &AppState,
    ticket: &SemanticGraphQueryTicket,
    reader_pubkey: &[u8],
    coordinates: &[ProjectContextCoordinate],
    context: &SemanticExecutionContext,
) -> Result<
    (
        SemanticGraphQueryTicket,
        SemanticContextCoordinateObservationBatch,
    ),
    SemanticGraphRootQueryError,
> {
    let mut read = begin_generation_bound_read(state, ticket, reader_pubkey, context).await?;
    let observation =
        run_root_stage(context, read.observe_context_coordinates(coordinates)).await??;
    let observed_ticket = read.ticket().clone();
    run_root_stage(context, read.commit()).await??;
    Ok((observed_ticket, observation))
}

async fn begin_generation_bound_read(
    state: &AppState,
    ticket: &SemanticGraphQueryTicket,
    reader_pubkey: &[u8],
    context: &SemanticExecutionContext,
) -> Result<SemanticGraphReadTx, SemanticGraphRootQueryError> {
    let work_deadline = context.windows().window(SemanticDeadlineWindow::Work);
    let remaining = work_deadline
        .checked_duration_since(std::time::Instant::now())
        .ok_or(SemanticGraphRootQueryError::QueryDeadlineExceeded)?;
    let opened = run_root_stage(
        context,
        state.db.begin_semantic_graph_read(
            ticket,
            reader_pubkey,
            state.relay_keypair.public_key(),
            query_read_timeouts(remaining),
        ),
    )
    .await?;
    match opened {
        Ok(read) => Ok(read),
        Err(buzz_db::DbError::AccessDenied(_)) => {
            let fresh = run_root_stage(
                context,
                state.db.semantic_graph_query_ticket(
                    ticket.community_id,
                    reader_pubkey,
                    &state.relay_keypair.public_key(),
                ),
            )
            .await?;
            match fresh {
                Ok(fresh) if !same_generation(&fresh, ticket) => {
                    Err(SemanticGraphRootQueryError::SemanticGenerationChanged)
                }
                Ok(_) | Err(_) => Err(SemanticGraphRootQueryError::AuthorizationChanged),
            }
        }
        Err(error) => Err(SemanticGraphRootQueryError::Database(error)),
    }
}

fn same_generation(left: &SemanticGraphQueryTicket, right: &SemanticGraphQueryTicket) -> bool {
    left.community_id == right.community_id
        && left.generation == right.generation
        && left.query_fences == right.query_fences
}

fn query_channel_bindings(inputs: &[SemanticQueryEncoderInput]) -> Vec<QueryChannelBinding> {
    inputs
        .iter()
        .map(|input| QueryChannelBinding {
            channel_id: input.channel_id(),
            context_coordinate: match input.channel_kind() {
                SemanticQueryChannelKind::Problem => None,
                SemanticQueryChannelKind::ConditionedContext { context_coordinate } => {
                    Some(context_coordinate.clone())
                }
            },
        })
        .collect()
}

/// Encode the complete-path input batch with transport handoff certainty.
///
/// The tracked failure keeps the frozen public `SemanticGraphQueryError`
/// value while carrying the R4 transport-class observation the closed retry
/// policy needs; coordinators collapse it back onto the frozen error when
/// their retry budget is declined or exhausted.
async fn encode_complete_path_inputs(
    provider: &crate::semantic_provider::VolcengineSemanticProvider,
    route: SemanticComputationRoute,
    inputs: &[SemanticQueryEncoderInput],
    common_inputs: &buzz_semantic_query::SemanticQueryInputBundle,
) -> Result<CompletePathEncodedInputs, TrackedProviderFailure<SemanticGraphQueryError>> {
    match route {
        SemanticComputationRoute::Legacy => provider
            .encode_queries_tracked(inputs)
            .await
            .map(CompletePathEncodedInputs::Compatibility),
        SemanticComputationRoute::Migrated => provider
            .encode_semantic_inputs_tracked(common_inputs)
            .await
            .map(CompletePathEncodedInputs::Common),
    }
}

enum CompletePathEncodedInputs {
    Compatibility(Vec<EncodedSemanticQuery>),
    Common(ProviderEncodedSemanticInputBundle),
}

fn bind_compatibility_query_vectors(
    ticket: &SemanticGraphQueryTicket,
    inputs: &[SemanticQueryEncoderInput],
    encoded: &[ProviderEncodedSemanticInput],
) -> Result<SemanticGraphQueryVectorBundle, SemanticGraphRootQueryError> {
    if encoded.len() != inputs.len() {
        return Err(invalid_state(
            "query Provider result count does not match the input batch",
        ));
    }
    let vectors = inputs
        .iter()
        .zip(encoded)
        .map(|(input, encoded)| {
            if encoded.request_id() != input.request_id()
                || encoded.channel_id() != input.channel_id()
                || encoded.response_model() != ticket.generation.model_contract.model
            {
                return Err(invalid_state(
                    "query Provider result does not preserve its request/channel/model binding",
                ));
            }
            Ok(buzz_db::semantic_query::SemanticExactQueryVector::new(
                ticket,
                encoded.clone(),
            )?)
        })
        .collect::<Result<Vec<_>, SemanticGraphRootQueryError>>()?;
    SemanticGraphQueryVectorBundle::from_compatibility_vectors(ticket, vectors)
        .map_err(SemanticGraphRootQueryError::Database)
}

fn map_encoder_error(error: SemanticGraphQueryError) -> SemanticGraphRootQueryError {
    match error {
        SemanticGraphQueryError::ProviderRateLimited { .. } => {
            SemanticGraphRootQueryError::QueryProviderBusy
        }
        SemanticGraphQueryError::ProviderTransport
        | SemanticGraphQueryError::ProviderRetryable { .. }
        | SemanticGraphQueryError::ProviderRejected { .. }
        | SemanticGraphQueryError::ProviderResponse => {
            SemanticGraphRootQueryError::QueryEncoderUnavailable
        }
        other => SemanticGraphRootQueryError::Contract(other),
    }
}

fn provider_failure_class(error: &SemanticGraphQueryError) -> SemanticGraphProviderFailure {
    match error {
        SemanticGraphQueryError::ProviderRateLimited { .. } => {
            SemanticGraphProviderFailure::RateLimited
        }
        SemanticGraphQueryError::ProviderTransport => SemanticGraphProviderFailure::Transport,
        SemanticGraphQueryError::ProviderRetryable { .. } => {
            SemanticGraphProviderFailure::Retryable
        }
        SemanticGraphQueryError::ProviderRejected { .. } => SemanticGraphProviderFailure::Rejected,
        SemanticGraphQueryError::ProviderResponse => SemanticGraphProviderFailure::InvalidResponse,
        _ => SemanticGraphProviderFailure::InvalidResponse,
    }
}

fn map_ticket_error(error: buzz_db::DbError) -> SemanticGraphRootQueryError {
    match error {
        buzz_db::DbError::AccessDenied(_) => SemanticGraphRootQueryError::AuthorizationChanged,
        other => SemanticGraphRootQueryError::Database(other),
    }
}

#[derive(Debug, Clone, Copy)]
struct QueryDeadlines {
    work: Instant,
    snapshot_close: Instant,
    absolute: Instant,
}

impl QueryDeadlines {
    fn new(
        started_at: Instant,
        max_wall_time_ms: u32,
    ) -> Result<Self, SemanticGraphRootQueryError> {
        let total = Duration::from_millis(u64::from(max_wall_time_ms));
        let response_tail = Duration::from_millis(u64::from(RESPONSE_TAIL_RESERVE_MS));
        let snapshot_close_reserve = Duration::from_millis(u64::from(SNAPSHOT_CLOSE_RESERVE_MS));
        let Some(snapshot_close_budget) = total.checked_sub(response_tail) else {
            return Err(SemanticGraphRootQueryError::QueryDeadlineExceeded);
        };
        let Some(work_budget) = snapshot_close_budget.checked_sub(snapshot_close_reserve) else {
            return Err(SemanticGraphRootQueryError::QueryDeadlineExceeded);
        };
        if work_budget.is_zero() {
            return Err(SemanticGraphRootQueryError::QueryDeadlineExceeded);
        }
        Ok(Self {
            work: started_at + work_budget,
            snapshot_close: started_at + snapshot_close_budget,
            absolute: started_at + total,
        })
    }
}

/// Run one root-stage database step inside the request's shared context.
///
/// Same work-window bound as the pre-R3 inline timeout, plus the context's
/// cancellation race: a cancelled, disconnected, or shutting-down request
/// stops starting new root or Stage C steps and observes the same frozen
/// deadline error either way.
async fn run_root_stage<F, T>(
    context: &SemanticExecutionContext,
    future: F,
) -> Result<T, SemanticGraphRootQueryError>
where
    F: std::future::Future<Output = T>,
{
    context
        .run_stage(SemanticDeadlineWindow::Work, future)
        .await
        .map_err(|abort| match abort {
            SemanticStageAbort::Deadline(_) | SemanticStageAbort::Cancelled(_) => {
                SemanticGraphRootQueryError::QueryDeadlineExceeded
            }
        })
}

/// Deadline-only wrapper kept for the terminal ticket re-classification in
/// [`classify_ticket_failure`]: that path already failed its Provider egress
/// and must keep its verbatim pre-R3 error mapping.
async fn run_before_work_deadline<F, T>(
    work_deadline: Instant,
    future: F,
) -> Result<T, SemanticGraphRootQueryError>
where
    F: std::future::Future<Output = T>,
{
    tokio::time::timeout_at(work_deadline, future)
        .await
        .map_err(|_| SemanticGraphRootQueryError::QueryDeadlineExceeded)
}

fn invalid_state(reason: &'static str) -> SemanticGraphRootQueryError {
    SemanticGraphRootQueryError::Contract(SemanticGraphQueryError::InvalidState(reason.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egress_failures_keep_the_frozen_complete_path_public_errors() {
        use SemanticGraphRootQueryError as Query;
        assert!(matches!(
            map_provider_egress_failure(SemanticProviderEgressFailure::DeadlineExceeded),
            Query::QueryDeadlineExceeded
        ));
        assert!(matches!(
            map_provider_egress_failure(SemanticProviderEgressFailure::Database(
                buzz_db::DbError::AccessDenied("denied".to_owned())
            )),
            Query::Database(_)
        ));
        assert!(matches!(
            map_provider_egress_failure(SemanticProviderEgressFailure::AdmissionBusy),
            Query::QueryProviderBusy
        ));
        assert!(matches!(
            map_provider_egress_failure(SemanticProviderEgressFailure::ContextChanged),
            Query::ContextSourceChanged
        ));
        assert!(matches!(
            map_provider_egress_failure(SemanticProviderEgressFailure::FleetUnavailable),
            Query::QueryFleetUnavailable
        ));
        assert!(matches!(
            map_provider_egress_failure(SemanticProviderEgressFailure::ProviderUnavailable),
            Query::AuthorizationChanged
        ));
        assert!(matches!(
            map_provider_egress_failure(SemanticProviderEgressFailure::ReservationContractViolated),
            Query::Contract(SemanticGraphQueryError::InvalidState(_))
        ));
        assert!(matches!(
            map_provider_egress_failure(SemanticProviderEgressFailure::PermitContractViolated),
            Query::Contract(SemanticGraphQueryError::InvalidState(_))
        ));
        assert!(matches!(
            map_provider_egress_failure(SemanticProviderEgressFailure::LatestStartUnrepresentable),
            Query::Contract(SemanticGraphQueryError::InvalidState(_))
        ));
        assert!(matches!(
            map_provider_egress_failure(SemanticProviderEgressFailure::AttemptLedgerExhausted(
                crate::semantic_query_runtime::SemanticAttemptExhausted::OperationAttempts,
            )),
            Query::QueryProviderBusy
        ));
    }

    #[test]
    fn egress_contract_violation_reasons_are_verbatim() {
        match map_provider_egress_failure(
            SemanticProviderEgressFailure::ReservationContractViolated,
        ) {
            SemanticGraphRootQueryError::Contract(error) => match error {
                SemanticGraphQueryError::InvalidState(reason) => assert_eq!(
                    reason,
                    "query Provider reservation generation does not match its ticket"
                ),
                other => panic!("unexpected contract error: {other:?}"),
            },
            other => panic!("unexpected public error: {other:?}"),
        }
        assert!(matches!(
            map_provider_egress_failure(SemanticProviderEgressFailure::Cancelled(
                crate::semantic_query_runtime::SemanticCancellationSource::ServerShutdown,
            )),
            SemanticGraphRootQueryError::QueryDeadlineExceeded
        ));
        match map_provider_egress_failure(SemanticProviderEgressFailure::PermitContractViolated) {
            SemanticGraphRootQueryError::Contract(error) => match error {
                SemanticGraphQueryError::InvalidState(reason) => assert_eq!(
                    reason,
                    "query egress permit does not match its Provider reservation"
                ),
                other => panic!("unexpected contract error: {other:?}"),
            },
            other => panic!("unexpected public error: {other:?}"),
        }
    }

    fn score(value: u32) -> Score {
        Score::new(value).expect("score fixture")
    }

    fn source(value: u128) -> SemanticSourceIdentity {
        SemanticSourceIdentity {
            community_id: uuid::Uuid::from_u128(1),
            kind: SemanticSourceKind::ProjectView(ProjectViewSemanticType::Work),
            source_id: uuid::Uuid::from_u128(value),
        }
    }

    fn selection_candidate(
        value: u128,
        problem: u32,
        mixed: u32,
        neutral: bool,
    ) -> CandidateScores {
        let source = source(value);
        let head = buzz_db::semantic_query::SemanticCurrentHead {
            invalidation_epoch: 1,
            snapshot_digest: Digest32::from_bytes([value as u8; 32]),
            source_basis: buzz_semantic::SemanticSourceBasis::ProjectView(
                buzz_semantic::ProjectViewSourceBasis {
                    schema_version: 3,
                    object_revision: 1,
                    source_change_id: Digest32::from_bytes([1; 32]),
                },
            ),
            unit_set_id: uuid::Uuid::from_u128(value + 100),
            unit_key: "overview".to_owned(),
            semantic_text_digest: Digest32::from_bytes([2; 32]),
            summary_coverage: buzz_semantic::SemanticCoverage::TitleOnly,
        };
        let representative = SemanticExactSourceScore {
            channel_id: Digest32::from_bytes([0; 32]),
            source: source.clone(),
            head,
            lifecycle: buzz_semantic::SemanticLifecycleClass::Active,
            source_status: None,
            roles: buzz_db::semantic_query::SemanticGraphStructuralRoles {
                coordinate: true,
                coordinate_entry_eligible: true,
                coordinate_incident_edge_keys: Vec::new(),
                context_document_bindings: Vec::new(),
            },
            score: score(problem),
            channel_rank: 1,
        };
        CandidateScores {
            source: source.clone(),
            problem_score: score(problem),
            conditioned_evidence: Vec::new(),
            highest_gain: Score::ZERO,
            second_highest_gain: Score::ZERO,
            environment_gain: Score::ZERO,
            anchor_gain: AnchorGain::None,
            candidate_score: score(mixed),
            discovered_problem_neutral: neutral,
            discovery_channels: Vec::new(),
            structural_entrypoints: vec![RootStructuralEntrypoint::Coordinate {
                coordinate: coordinate_for_source(&source),
            }],
            representative,
        }
    }

    fn score_row(
        source: SemanticSourceIdentity,
        channel_id: Digest32,
        value: u32,
        roles: buzz_db::semantic_query::SemanticGraphStructuralRoles,
    ) -> SemanticExactSourceScore {
        let mut row =
            selection_candidate(source.source_id.as_u128(), value, value, true).representative;
        row.source = source;
        row.channel_id = channel_id;
        row.score = score(value);
        row.roles = roles;
        row
    }

    #[test]
    fn incremental_selection_pins_neutral_then_uses_exact_redundancy() {
        let candidates = vec![
            selection_candidate(1, 900_000, 900_000, true),
            selection_candidate(2, 850_000, 850_000, true),
            selection_candidate(3, 800_000, 980_000, false),
        ];
        let mut state = RootSelectionState::new();
        state
            .selected
            .push(strongest_problem_neutral(&candidates).expect("pinned"));
        state
            .pair_scores
            .insert(SourcePairKey::new(&source(1), &source(2)), score(990_000));
        state
            .pair_scores
            .insert(SourcePairKey::new(&source(1), &source(3)), score(100_000));

        let next =
            next_automatic_root(&candidates, &state, AutomaticRootLane::Mixed).expect("mixed root");
        assert_eq!(state.selected[0].source, source(1));
        assert_eq!(next.source, source(3));
    }

    #[test]
    fn conditioned_gain_reorders_mixed_roots_without_displacing_neutral_pin() {
        let q0 = Digest32::from_bytes([10; 32]);
        let qi = Digest32::from_bytes([11; 32]);
        let context = ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Work,
            object_id: uuid::Uuid::from_u128(50),
        };
        let roles = buzz_db::semantic_query::SemanticGraphStructuralRoles {
            coordinate: true,
            coordinate_entry_eligible: true,
            coordinate_incident_edge_keys: Vec::new(),
            context_document_bindings: Vec::new(),
        };
        let first = source(1);
        let second = source(2);
        let matrix = vec![
            score_row(first.clone(), q0, 720_000, roles.clone()),
            score_row(first.clone(), qi, 720_000, roles.clone()),
            score_row(second.clone(), q0, 700_000, roles.clone()),
            score_row(second.clone(), qi, 1_000_000, roles),
        ];
        let recall = SemanticExactRecallBatch {
            scores: vec![matrix[0].clone(), matrix[1].clone(), matrix[3].clone()],
            channels: vec![
                buzz_db::semantic_query::SemanticExactRecallChannelObservation {
                    channel_id: q0,
                    returned_count: 1,
                    exhaustion: SemanticExactRecallExhaustion::Exhausted,
                },
                buzz_db::semantic_query::SemanticExactRecallChannelObservation {
                    channel_id: qi,
                    returned_count: 2,
                    exhaustion: SemanticExactRecallExhaustion::Exhausted,
                },
            ],
        };
        let channels = vec![
            QueryChannelBinding {
                channel_id: q0,
                context_coordinate: None,
            },
            QueryChannelBinding {
                channel_id: qi,
                context_coordinate: Some(context),
            },
        ];
        let candidates =
            prepare_candidate_scores(&matrix, &recall, &channels, &[]).expect("candidates");
        let first_candidate = candidates
            .iter()
            .find(|candidate| candidate.source == first)
            .expect("first");
        let second_candidate = candidates
            .iter()
            .find(|candidate| candidate.source == second)
            .expect("second");
        assert!(first_candidate.problem_score > second_candidate.problem_score);
        assert!(second_candidate.candidate_score > first_candidate.candidate_score);

        let mut state = RootSelectionState::new();
        state
            .selected
            .push(strongest_problem_neutral(&candidates).expect("neutral pin"));
        assert_eq!(state.selected[0].source, first);
        assert_eq!(
            next_automatic_root(&candidates, &state, AutomaticRootLane::Mixed)
                .expect("mixed")
                .source,
            second
        );
    }

    #[test]
    fn source_and_entrypoint_order_is_input_permutation_independent() {
        let coordinate = RootStructuralEntrypoint::Coordinate {
            coordinate: coordinate_for_source(&source(9)),
        };
        let document = RootStructuralEntrypoint::ContextDocument {
            edge_key: buzz_project_context::EdgeKey::from_hex(&"07".repeat(32)).expect("edge key"),
            document_id: uuid::Uuid::from_u128(8),
            edge_provenance: ProjectContextEdgeProvenance {
                last_context_revision: 1,
                source_change_id: Digest32::from_bytes([1; 32]),
            },
            binding_provenance: ProjectContextBindingProvenance {
                binding_context_revision: 1,
                source_change_id: Digest32::from_bytes([2; 32]),
                projection_event_id: Digest32::from_bytes([3; 32]),
            },
        };
        let mut left = vec![document.clone(), coordinate.clone()];
        let mut right = vec![coordinate, document];
        canonicalize_entrypoints(&mut left);
        canonicalize_entrypoints(&mut right);
        assert_eq!(left, right);
    }

    #[test]
    fn deadline_reserves_snapshot_close_and_response_tails() {
        let started = Instant::now();
        let deadlines =
            QueryDeadlines::new(started, buzz_semantic_query::MAX_WALL_TIME_MS).expect("deadline");
        assert_eq!(
            deadlines.work.duration_since(started),
            Duration::from_secs(174)
        );
        assert_eq!(
            deadlines.snapshot_close.duration_since(started),
            Duration::from_secs(179)
        );
        assert_eq!(
            deadlines.absolute.duration_since(started),
            Duration::from_secs(180)
        );
        assert!(QueryDeadlines::new(
            started,
            RESPONSE_TAIL_RESERVE_MS + SNAPSHOT_CLOSE_RESERVE_MS
        )
        .is_err());
    }
}
