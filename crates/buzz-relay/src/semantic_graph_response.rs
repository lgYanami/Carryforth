//! Deterministic byte-bounded packing for Relay-signed semantic graph results.
//!
//! This module owns no persistence, pubsub, search, or traversal work. It takes
//! a completed in-memory forest and first prepares an unsigned response whose
//! byte estimate is exact for the final `[Event]` representation. The HTTP
//! bridge must perform its security/readiness postflight before calling the
//! separate one-shot signing function.

use std::collections::{BTreeMap, BTreeSet};

use buzz_core::kind::KIND_SEMANTIC_GRAPH_QUERY_RESULT;
use buzz_semantic::Digest32;
use buzz_semantic_query::{
    CompletionReason, ExhaustedDimension, SemanticGraphQuery, SemanticGraphQueryCoverage,
    SemanticGraphQueryInputObservations, SemanticGraphQueryObservations, SemanticGraphQueryResult,
    SemanticPath, SemanticRoot, SummaryOmittedReason,
};
use nostr::{Keys, PublicKey, Timestamp};
use serde::Serialize;

use crate::semantic_graph_observability::{
    record_query_error, stage_timer, SemanticGraphMetricStage, SemanticGraphQueryMetricError,
};

const RESULT_MARKER: &str = "buzz-project-context-semantic-result";
const PLACEHOLDER_EVENT_ID: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
const PLACEHOLDER_SIGNATURE: &str = concat!(
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000"
);

/// Completed traversal material consumed by deterministic response packing.
#[derive(Clone)]
pub(crate) struct SemanticGraphResponsePackingInput {
    /// Canonical caller request whose response byte budget is authoritative.
    pub(crate) query: SemanticGraphQuery,
    /// Binding digest derived from the authenticated NIP-98 request transcript.
    pub(crate) request_binding_digest: Digest32,
    /// Generation, graph, ranking, and snapshot observations.
    pub(crate) observations: SemanticGraphQueryObservations,
    /// Closed observations for all caller-supplied Coordinates.
    pub(crate) input_observations: SemanticGraphQueryInputObservations,
    /// Completed selected roots before response packing.
    pub(crate) roots: Vec<SemanticRoot>,
    /// Completed retained paths after logical `max_paths`, before byte packing.
    pub(crate) paths: Vec<SemanticPath>,
    /// Root/traversal coverage before response packing.
    pub(crate) coverage: SemanticGraphQueryCoverage,
    /// Completion reason produced by traversal.
    pub(crate) completion_reason: CompletionReason,
    /// Canonical logical dimensions exhausted before response packing.
    pub(crate) exhausted_dimensions: Vec<ExhaustedDimension>,
}

/// One deterministically packed but unsigned semantic graph response.
///
/// This value is intentionally incapable of carrying a verifiable Event. The
/// bridge performs Stage D security/readiness postflight after packing and
/// passes it to [`sign_packed_semantic_graph_response`] only on success.
pub(crate) struct PackedSemanticGraphResponse {
    /// Exact closed result to copy into the final Event content.
    pub(crate) result: SemanticGraphQueryResult,
    /// Relay identity that was covered by the deterministic byte estimate.
    expected_relay: PublicKey,
    /// Authenticated caller bound into the exact `p` tag.
    authenticated_caller: PublicKey,
    /// Frozen timestamp covered by the deterministic byte estimate.
    created_at: Timestamp,
    /// Effective request/server response limit.
    effective_limit: usize,
    /// Exact predicted compact JSON byte length of the final one-Event array.
    pub(crate) estimated_event_array_bytes: usize,
}

impl std::fmt::Debug for PackedSemanticGraphResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PackedSemanticGraphResponse")
            .field("root_count", &self.result.roots.len())
            .field("path_count", &self.result.paths.len())
            .field(
                "estimated_event_array_bytes",
                &self.estimated_event_array_bytes,
            )
            .finish_non_exhaustive()
    }
}

/// Final one-shot signed response-only semantic graph Event.
pub(crate) struct SignedSemanticGraphResponse {
    /// Exact compact JSON bytes for the one-Event response array.
    pub(crate) event_array_bytes: Vec<u8>,
}

impl std::fmt::Debug for SignedSemanticGraphResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SignedSemanticGraphResponse")
            .field("event_array_bytes", &self.event_array_bytes.len())
            .finish_non_exhaustive()
    }
}

/// Closed response-packing failure classification for HTTP mapping.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SemanticGraphResponsePackingError {
    /// Required envelope or accepted explicit root shells cannot fit.
    #[error("semantic graph response required envelope exceeds {maximum} bytes")]
    ResponseTooLarge {
        /// Effective request/server response limit.
        maximum: usize,
    },
    /// Completed traversal material violates the packing contract.
    #[error("invalid semantic graph response packing input: {0}")]
    InvalidInput(String),
    /// The virtual Event could not be signed.
    #[error("semantic graph virtual result signing failed")]
    Signing,
    /// Postflight supplied a signer other than the Relay covered by packing.
    #[error("semantic graph postflight Relay signer changed after packing")]
    RelaySignerChanged,
    /// The exact estimator drifted from the final Event representation.
    #[error(
        "semantic graph Event size estimate drifted (estimated {estimated} bytes, actual {actual} bytes)"
    )]
    SizeEstimateDrift {
        /// Byte size predicted before postflight.
        estimated: usize,
        /// Actual signed Event-array byte size.
        actual: usize,
    },
    /// The exact Event array could not be serialized.
    #[error("semantic graph virtual result serialization failed")]
    Serialization,
}

impl SemanticGraphResponsePackingError {
    fn metric_code(&self) -> SemanticGraphQueryMetricError {
        match self {
            Self::ResponseTooLarge { .. } => SemanticGraphQueryMetricError::ResponseTooLarge,
            Self::InvalidInput(_) => SemanticGraphQueryMetricError::InvalidPackingInput,
            Self::Signing => SemanticGraphQueryMetricError::Signing,
            Self::RelaySignerChanged => SemanticGraphQueryMetricError::RelaySignerChanged,
            Self::SizeEstimateDrift { .. } => SemanticGraphQueryMetricError::SizeEstimateDrift,
            Self::Serialization => SemanticGraphQueryMetricError::Serialization,
        }
    }
}

/// Deterministically pack one unsigned semantic graph result without side effects.
///
/// `server_frame_safe_cap` is the deployment frame limit after subtracting its
/// independently frozen worst-case transport overhead. The effective cap is
/// the smaller of it and the caller's validated `max_response_bytes`.
pub(crate) fn pack_semantic_graph_response(
    input: SemanticGraphResponsePackingInput,
    expected_relay: &PublicKey,
    authenticated_caller: &PublicKey,
    server_frame_safe_cap: usize,
) -> Result<PackedSemanticGraphResponse, SemanticGraphResponsePackingError> {
    let _timer = stage_timer(SemanticGraphMetricStage::Packing);
    let result = pack_semantic_graph_response_inner(
        input,
        expected_relay,
        authenticated_caller,
        server_frame_safe_cap,
    );
    if let Err(error) = &result {
        record_query_error(SemanticGraphMetricStage::Packing, error.metric_code());
    }
    result
}

fn pack_semantic_graph_response_inner(
    input: SemanticGraphResponsePackingInput,
    expected_relay: &PublicKey,
    authenticated_caller: &PublicKey,
    server_frame_safe_cap: usize,
) -> Result<PackedSemanticGraphResponse, SemanticGraphResponsePackingError> {
    let canonical_query = input
        .query
        .clone()
        .validate_and_canonicalize()
        .map_err(|error| invalid_input(format!("invalid query: {error}")))?;
    if canonical_query != input.query {
        return Err(invalid_input("query is not canonical"));
    }
    let effective_limit = usize::try_from(canonical_query.budget.max_response_bytes)
        .unwrap_or(usize::MAX)
        .min(server_frame_safe_cap);

    validate_prepacking_input(&input)?;

    let mut explicit_roots = Vec::new();
    let mut automatic_roots = Vec::new();
    for root in input.roots.iter().cloned() {
        if root_is_explicit(&root) {
            explicit_roots.push(root);
        } else {
            automatic_roots.push(root);
        }
    }
    explicit_roots.sort_by_key(|root| root.root_id);
    automatic_roots.sort_by(|left, right| {
        right
            .semantic_score
            .cmp(&left.semantic_score)
            .then_with(|| left.root_id.cmp(&right.root_id))
    });
    let mut retained_paths = input.paths.clone();
    retained_paths.sort_by(|left, right| {
        right
            .path_score
            .cmp(&left.path_score)
            .then_with(|| left.path_id.cmp(&right.path_id))
    });

    let original_roots = input
        .roots
        .iter()
        .map(|root| (root.root_id, root.clone()))
        .collect::<BTreeMap<_, _>>();
    let original_paths = input
        .paths
        .iter()
        .map(|path| (path.path_id, path.clone()))
        .collect::<BTreeMap<_, _>>();
    let total_automatic_roots = automatic_roots.len();
    let total_paths = retained_paths.len();
    let base_completion_reason = input.completion_reason;
    let base_exhausted_dimensions = input.exhausted_dimensions.clone();

    let required_roots = explicit_roots
        .into_iter()
        .map(without_root_summary)
        .collect::<Vec<_>>();
    let mut result = SemanticGraphQueryResult {
        request_id: canonical_query.request_id,
        project_id: canonical_query.project_id,
        request_binding_digest: input.request_binding_digest,
        observations: input.observations,
        input_observations: input.input_observations,
        roots: required_roots,
        paths: Vec::new(),
        coverage: input.coverage,
        completion_reason: base_completion_reason,
        exhausted_dimensions: base_exhausted_dimensions.clone(),
    };
    refresh_response_accounting(
        &mut result,
        total_automatic_roots,
        total_paths,
        base_completion_reason,
        &base_exhausted_dimensions,
    )?;

    let created_at = Timestamp::now();
    let mut estimated_event_array_bytes = match estimate_candidate(
        &result,
        expected_relay,
        authenticated_caller,
        created_at,
        effective_limit,
    )? {
        EstimateAttempt::Fits(bytes) => bytes,
        EstimateAttempt::TooLarge => {
            return Err(SemanticGraphResponsePackingError::ResponseTooLarge {
                maximum: effective_limit,
            });
        }
    };

    for root in automatic_roots {
        let mut candidate = result.clone();
        candidate.roots.push(without_root_summary(root));
        refresh_response_accounting(
            &mut candidate,
            total_automatic_roots,
            total_paths,
            base_completion_reason,
            &base_exhausted_dimensions,
        )?;
        if let EstimateAttempt::Fits(candidate_bytes) = estimate_candidate(
            &candidate,
            expected_relay,
            authenticated_caller,
            created_at,
            effective_limit,
        )? {
            result = candidate;
            estimated_event_array_bytes = candidate_bytes;
        }
    }

    let returned_root_ids = result
        .roots
        .iter()
        .map(|root| root.root_id)
        .collect::<BTreeSet<_>>();
    for path in retained_paths {
        if !returned_root_ids.contains(&path.root_id) {
            continue;
        }
        let mut candidate = result.clone();
        candidate.paths.push(without_path_summaries(path));
        refresh_response_accounting(
            &mut candidate,
            total_automatic_roots,
            total_paths,
            base_completion_reason,
            &base_exhausted_dimensions,
        )?;
        if let EstimateAttempt::Fits(candidate_bytes) = estimate_candidate(
            &candidate,
            expected_relay,
            authenticated_caller,
            created_at,
            effective_limit,
        )? {
            result = candidate;
            estimated_event_array_bytes = candidate_bytes;
        }
    }

    for summary in summary_restore_order(&result, &original_roots, &original_paths)? {
        let mut candidate = result.clone();
        restore_summary(&mut candidate, &summary)?;
        refresh_response_accounting(
            &mut candidate,
            total_automatic_roots,
            total_paths,
            base_completion_reason,
            &base_exhausted_dimensions,
        )?;
        if let EstimateAttempt::Fits(candidate_bytes) = estimate_candidate(
            &candidate,
            expected_relay,
            authenticated_caller,
            created_at,
            effective_limit,
        )? {
            result = candidate;
            estimated_event_array_bytes = candidate_bytes;
        }
    }

    result
        .validate_for_request(&canonical_query)
        .map_err(|error| invalid_input(format!("packed result is invalid: {error}")))?;
    debug_assert!(estimated_event_array_bytes <= effective_limit);
    Ok(PackedSemanticGraphResponse {
        result,
        expected_relay: *expected_relay,
        authenticated_caller: *authenticated_caller,
        created_at,
        effective_limit,
        estimated_event_array_bytes,
    })
}

/// Sign a prepared semantic graph response exactly once after postflight.
///
/// The final exact `[Event]` bytes are checked against both the frozen cap and
/// the pre-postflight estimate. A mismatch fails closed; this function never
/// retries, repacks, persists, or publishes the Event.
pub(crate) fn sign_packed_semantic_graph_response(
    packed: PackedSemanticGraphResponse,
    relay_keys: &Keys,
) -> Result<SignedSemanticGraphResponse, SemanticGraphResponsePackingError> {
    let _timer = stage_timer(SemanticGraphMetricStage::Signing);
    let result = sign_packed_semantic_graph_response_inner(packed, relay_keys);
    if let Err(error) = &result {
        record_query_error(SemanticGraphMetricStage::Signing, error.metric_code());
    }
    result
}

fn sign_packed_semantic_graph_response_inner(
    packed: PackedSemanticGraphResponse,
    relay_keys: &Keys,
) -> Result<SignedSemanticGraphResponse, SemanticGraphResponsePackingError> {
    if relay_keys.public_key() != packed.expected_relay {
        return Err(SemanticGraphResponsePackingError::RelaySignerChanged);
    }
    let builder = buzz_sdk::semantic_graph::build_semantic_graph_query_result(
        &packed.result,
        &packed.authenticated_caller,
    )
    .map_err(|error| invalid_input(format!("SDK rejected packed semantic result: {error}")))?;
    let event = builder
        .custom_created_at(packed.created_at)
        .sign_with_keys(relay_keys)
        .map_err(|_| SemanticGraphResponsePackingError::Signing)?;
    let event_array_bytes = serde_json::to_vec(std::slice::from_ref(&event))
        .map_err(|_| SemanticGraphResponsePackingError::Serialization)?;
    let actual = event_array_bytes.len();
    if actual > packed.effective_limit {
        return Err(SemanticGraphResponsePackingError::ResponseTooLarge {
            maximum: packed.effective_limit,
        });
    }
    if actual != packed.estimated_event_array_bytes {
        return Err(SemanticGraphResponsePackingError::SizeEstimateDrift {
            estimated: packed.estimated_event_array_bytes,
            actual,
        });
    }
    Ok(SignedSemanticGraphResponse { event_array_bytes })
}

fn validate_prepacking_input(
    input: &SemanticGraphResponsePackingInput,
) -> Result<(), SemanticGraphResponsePackingError> {
    if input.coverage.roots_selected != input.roots.len() as u64
        || input.coverage.paths_retained != input.paths.len() as u64
        || input.coverage.roots_returned != 0
        || input.coverage.paths_returned != 0
        || input.coverage.omitted_for_response_budget.automatic_roots != 0
        || input.coverage.omitted_for_response_budget.paths != 0
        || input.coverage.omitted_for_response_budget.summaries != 0
        || input.coverage.truncation_counts_by_dimension.response_bytes != 0
        || input
            .coverage
            .degraded_mode_counts
            .summary_omitted_for_response_budget
            != 0
        || input
            .exhausted_dimensions
            .contains(&ExhaustedDimension::ResponseBytes)
    {
        return Err(invalid_input(
            "coverage must describe an un-packed completed forest",
        ));
    }
    if input.roots.iter().any(root_has_omitted_summary)
        || input.paths.iter().any(path_has_omitted_summary)
    {
        return Err(invalid_input(
            "summary response-budget omission must be owned by the packer",
        ));
    }
    if input
        .roots
        .iter()
        .any(|root| !root_is_explicit(root) && root.semantic_score.is_none())
    {
        return Err(invalid_input("automatic roots must have a semantic score"));
    }
    for accepted in &input.input_observations.accepted_initial_coordinates {
        let retained = input.roots.iter().any(|root| {
            root_is_explicit(root)
                && root
                    .structural_entrypoints
                    .iter()
                    .any(|entrypoint| match entrypoint {
                        buzz_semantic_query::RootStructuralEntrypoint::Coordinate {
                            coordinate,
                        } => coordinate == &accepted.coordinate,
                        buzz_semantic_query::RootStructuralEntrypoint::ContextDocument {
                            ..
                        } => false,
                    })
        });
        if !retained {
            return Err(invalid_input(
                "accepted explicit initial Coordinate lacks a required root shell",
            ));
        }
    }

    let mut complete = SemanticGraphQueryResult {
        request_id: input.query.request_id,
        project_id: input.query.project_id,
        request_binding_digest: input.request_binding_digest,
        observations: input.observations.clone(),
        input_observations: input.input_observations.clone(),
        roots: input.roots.clone(),
        paths: input.paths.clone(),
        coverage: input.coverage.clone(),
        completion_reason: input.completion_reason,
        exhausted_dimensions: input.exhausted_dimensions.clone(),
    };
    complete.coverage.roots_returned = complete.roots.len() as u64;
    complete.coverage.paths_returned = complete.paths.len() as u64;
    complete
        .validate_for_request(&input.query)
        .map_err(|error| invalid_input(format!("completed forest is invalid: {error}")))
}

fn refresh_response_accounting(
    result: &mut SemanticGraphQueryResult,
    total_automatic_roots: usize,
    total_paths: usize,
    base_completion_reason: CompletionReason,
    base_exhausted_dimensions: &[ExhaustedDimension],
) -> Result<(), SemanticGraphResponsePackingError> {
    let returned_automatic = result
        .roots
        .iter()
        .filter(|root| !root_is_explicit(root))
        .count();
    let automatic_roots = total_automatic_roots
        .checked_sub(returned_automatic)
        .ok_or_else(|| invalid_input("returned automatic root count exceeds selected count"))?;
    let omitted_paths = total_paths
        .checked_sub(result.paths.len())
        .ok_or_else(|| invalid_input("returned path count exceeds retained count"))?;
    let omitted_summaries = count_omitted_summaries(result);
    let automatic_roots = as_u64(automatic_roots, "automatic root omission count")?;
    let omitted_paths = as_u64(omitted_paths, "path omission count")?;
    let omitted_summaries = as_u64(omitted_summaries, "summary omission count")?;
    let response_omissions = automatic_roots
        .checked_add(omitted_paths)
        .and_then(|value| value.checked_add(omitted_summaries))
        .ok_or_else(|| invalid_input("response omission count overflow"))?;

    result.coverage.roots_returned = as_u64(result.roots.len(), "returned root count")?;
    result.coverage.paths_returned = as_u64(result.paths.len(), "returned path count")?;
    result.coverage.omitted_for_response_budget.automatic_roots = automatic_roots;
    result.coverage.omitted_for_response_budget.paths = omitted_paths;
    result.coverage.omitted_for_response_budget.summaries = omitted_summaries;
    result
        .coverage
        .truncation_counts_by_dimension
        .response_bytes = response_omissions;
    result
        .coverage
        .degraded_mode_counts
        .summary_omitted_for_response_budget = omitted_summaries;

    result.completion_reason = base_completion_reason;
    result.exhausted_dimensions = base_exhausted_dimensions.to_vec();
    if response_omissions > 0 && base_completion_reason != CompletionReason::WallTimeExhausted {
        result.completion_reason = CompletionReason::BudgetExhausted;
        if !result
            .exhausted_dimensions
            .contains(&ExhaustedDimension::ResponseBytes)
        {
            result
                .exhausted_dimensions
                .push(ExhaustedDimension::ResponseBytes);
            result.exhausted_dimensions.sort();
        }
    }
    result
        .coverage
        .validate()
        .map_err(|error| invalid_input(format!("response coverage is invalid: {error}")))
}

fn root_is_explicit(root: &SemanticRoot) -> bool {
    root.discovery_channels.iter().any(|channel| {
        matches!(
            channel,
            buzz_semantic_query::RootDiscoveryChannel::ExplicitInitial
        )
    })
}

fn without_root_summary(mut root: SemanticRoot) -> SemanticRoot {
    omit_preview_summary(&mut root.preview);
    root
}

fn without_path_summaries(mut path: SemanticPath) -> SemanticPath {
    for hop in &mut path.hops {
        omit_preview_summary(&mut hop.selected_relation_document.preview);
        omit_preview_summary(&mut hop.continued_to_coordinate.preview);
    }
    path
}

fn omit_preview_summary(preview: &mut buzz_semantic_query::SemanticSourcePreview) {
    if preview.summary.take().is_some() {
        preview.summary_omitted_reason = Some(SummaryOmittedReason::ResponseBudget);
    }
}

fn root_has_omitted_summary(root: &SemanticRoot) -> bool {
    root.preview.summary_omitted_reason.is_some()
}

fn path_has_omitted_summary(path: &SemanticPath) -> bool {
    path.hops.iter().any(|hop| {
        hop.selected_relation_document
            .preview
            .summary_omitted_reason
            .is_some()
            || hop
                .continued_to_coordinate
                .preview
                .summary_omitted_reason
                .is_some()
    })
}

fn count_omitted_summaries(result: &SemanticGraphQueryResult) -> usize {
    result
        .roots
        .iter()
        .filter(|root| root.preview.summary_omitted_reason.is_some())
        .count()
        + result
            .paths
            .iter()
            .flat_map(|path| &path.hops)
            .map(|hop| {
                usize::from(
                    hop.selected_relation_document
                        .preview
                        .summary_omitted_reason
                        .is_some(),
                ) + usize::from(
                    hop.continued_to_coordinate
                        .preview
                        .summary_omitted_reason
                        .is_some(),
                )
            })
            .sum::<usize>()
}

#[derive(Clone)]
enum SummaryRestore {
    Root {
        root_id: Digest32,
        summary: String,
    },
    RelationDocument {
        path_id: Digest32,
        hop_index: usize,
        summary: String,
    },
    TargetCoordinate {
        path_id: Digest32,
        hop_index: usize,
        summary: String,
    },
}

fn summary_restore_order(
    result: &SemanticGraphQueryResult,
    original_roots: &BTreeMap<Digest32, SemanticRoot>,
    original_paths: &BTreeMap<Digest32, SemanticPath>,
) -> Result<Vec<SummaryRestore>, SemanticGraphResponsePackingError> {
    let mut summaries = Vec::new();
    for root in &result.roots {
        let original = original_roots
            .get(&root.root_id)
            .ok_or_else(|| invalid_input("returned root lacks original packing material"))?;
        if let Some(summary) = &original.preview.summary {
            summaries.push(SummaryRestore::Root {
                root_id: root.root_id,
                summary: summary.clone(),
            });
        }
    }
    for path in &result.paths {
        let original = original_paths
            .get(&path.path_id)
            .ok_or_else(|| invalid_input("returned path lacks original packing material"))?;
        if original.hops.len() != path.hops.len() {
            return Err(invalid_input(
                "returned path hop count changed during packing",
            ));
        }
        for (hop_index, hop) in original.hops.iter().enumerate() {
            if let Some(summary) = &hop.selected_relation_document.preview.summary {
                summaries.push(SummaryRestore::RelationDocument {
                    path_id: path.path_id,
                    hop_index,
                    summary: summary.clone(),
                });
            }
            if let Some(summary) = &hop.continued_to_coordinate.preview.summary {
                summaries.push(SummaryRestore::TargetCoordinate {
                    path_id: path.path_id,
                    hop_index,
                    summary: summary.clone(),
                });
            }
        }
    }
    Ok(summaries)
}

fn restore_summary(
    result: &mut SemanticGraphQueryResult,
    restore: &SummaryRestore,
) -> Result<(), SemanticGraphResponsePackingError> {
    let preview = match restore {
        SummaryRestore::Root { root_id, .. } => {
            &mut result
                .roots
                .iter_mut()
                .find(|root| root.root_id == *root_id)
                .ok_or_else(|| invalid_input("summary restore root is not packed"))?
                .preview
        }
        SummaryRestore::RelationDocument {
            path_id, hop_index, ..
        } => {
            &mut result
                .paths
                .iter_mut()
                .find(|path| path.path_id == *path_id)
                .and_then(|path| path.hops.get_mut(*hop_index))
                .ok_or_else(|| invalid_input("summary restore relation hop is not packed"))?
                .selected_relation_document
                .preview
        }
        SummaryRestore::TargetCoordinate {
            path_id, hop_index, ..
        } => {
            &mut result
                .paths
                .iter_mut()
                .find(|path| path.path_id == *path_id)
                .and_then(|path| path.hops.get_mut(*hop_index))
                .ok_or_else(|| invalid_input("summary restore target hop is not packed"))?
                .continued_to_coordinate
                .preview
        }
    };
    let summary = match restore {
        SummaryRestore::Root { summary, .. }
        | SummaryRestore::RelationDocument { summary, .. }
        | SummaryRestore::TargetCoordinate { summary, .. } => summary,
    };
    preview.summary = Some(summary.clone());
    preview.summary_omitted_reason = None;
    Ok(())
}

enum EstimateAttempt {
    Fits(usize),
    TooLarge,
}

#[derive(Serialize)]
struct EstimatedVirtualEvent<'a> {
    id: &'static str,
    pubkey: &'a str,
    created_at: u64,
    kind: u32,
    tags: Vec<Vec<&'a str>>,
    content: &'a str,
    sig: &'static str,
}

fn estimate_candidate(
    result: &SemanticGraphQueryResult,
    expected_relay: &PublicKey,
    authenticated_caller: &PublicKey,
    created_at: Timestamp,
    limit: usize,
) -> Result<EstimateAttempt, SemanticGraphResponsePackingError> {
    result
        .validate()
        .map_err(|error| invalid_input(format!("candidate result is invalid: {error}")))?;
    let content = serde_json::to_string(result)
        .map_err(|_| SemanticGraphResponsePackingError::Serialization)?;
    let relay = expected_relay.to_hex();
    let caller = authenticated_caller.to_hex();
    let request_id = result.request_id.to_string();
    let request_binding = result.request_binding_digest.to_hex();
    let event = EstimatedVirtualEvent {
        id: PLACEHOLDER_EVENT_ID,
        pubkey: &relay,
        created_at: created_at.as_secs(),
        kind: KIND_SEMANTIC_GRAPH_QUERY_RESULT,
        tags: vec![
            vec!["p", &caller],
            vec!["request_id", &request_id],
            vec!["request_binding", &request_binding],
            vec!["t", RESULT_MARKER],
        ],
        content: &content,
        sig: PLACEHOLDER_SIGNATURE,
    };
    let event_array_bytes = serde_json::to_vec(std::slice::from_ref(&event))
        .map_err(|_| SemanticGraphResponsePackingError::Serialization)?;
    if event_array_bytes.len() > limit {
        return Ok(EstimateAttempt::TooLarge);
    }
    Ok(EstimateAttempt::Fits(event_array_bytes.len()))
}

fn as_u64(value: usize, field: &'static str) -> Result<u64, SemanticGraphResponsePackingError> {
    u64::try_from(value).map_err(|_| invalid_input(format!("{field} exceeds u64")))
}

fn invalid_input(message: impl Into<String>) -> SemanticGraphResponsePackingError {
    SemanticGraphResponsePackingError::InvalidInput(message.into())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use buzz_project_context::{canonicalize_coordinates, EdgeKey, ProjectContextCoordinate};
    use buzz_project_view::ProjectViewObjectType;
    use buzz_semantic::{
        ProjectDocumentSourceBasis, ProjectViewSemanticType, ProjectViewSourceBasis,
        SemanticCoverage, SemanticLifecycleClass, SemanticSourceBasis, SemanticSourceIdentity,
        SemanticSourceKind,
    };
    use buzz_semantic_query::{
        budget_profile_digest, candidate_score, derive_path_id, derive_root_id, document_score,
        harmonic_score, path_score, query_contract_digest, ranking_contract_digest,
        target_coordinate_score, AcceptedInitialCoordinateObservation, AnchorGain,
        BranchStopReason, CanonicalSourceProvenance, CompletionReason,
        ContextDocumentBindingObservation, CurrentGraphMembershipObservation, DegradedModeCounts,
        EmbeddingCoverageCounts, LifecycleFilter, OmittedContextChannelCounts,
        OmittedForResponseBudgetCounts, ProjectContextBindingProvenance,
        ProjectContextEdgeProvenance, RootDiscoveryChannel, RootStructuralEntrypoint, Score,
        ScoreExplanation, SeedOutcome, SemanticContinuedCoordinate, SemanticEdgeObservation,
        SemanticGraphQuery, SemanticGraphQueryBudget, SemanticGraphQueryCoverage,
        SemanticGraphQueryInputObservations, SemanticGraphQueryObservations,
        SemanticHeadProvenance, SemanticHeadState, SemanticHyperedgeHop, SemanticPath,
        SemanticProvenance, SemanticRelationDocument, SemanticRoot, SemanticScoreRole,
        SemanticSourcePreview, SummaryOmittedReason, TruncationCountsByDimension,
    };
    use chrono::{TimeZone, Utc};
    use nostr::Keys;
    use uuid::Uuid;

    use super::{
        pack_semantic_graph_response, sign_packed_semantic_graph_response,
        SemanticGraphResponsePackingError, SemanticGraphResponsePackingInput,
    };

    fn uuid(seed: u64) -> Uuid {
        Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("UUIDv4 fixture")
    }

    fn digest(seed: u8) -> buzz_semantic::Digest32 {
        buzz_semantic::Digest32::from_bytes([seed; 32])
    }

    fn coordinate(object_type: ProjectViewObjectType, seed: u64) -> ProjectContextCoordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id: uuid(seed),
        }
    }

    fn source(
        project_id: Uuid,
        subtype: ProjectViewSemanticType,
        source_id: Uuid,
    ) -> SemanticSourceIdentity {
        SemanticSourceIdentity {
            community_id: project_id,
            kind: SemanticSourceKind::ProjectView(subtype),
            source_id,
        }
    }

    fn project_view_basis(seed: u8) -> SemanticSourceBasis {
        SemanticSourceBasis::ProjectView(ProjectViewSourceBasis {
            schema_version: 3,
            object_revision: u64::from(seed) + 1,
            source_change_id: digest(seed),
        })
    }

    fn provenance(
        basis: SemanticSourceBasis,
        snapshot_seed: u8,
        has_summary: bool,
    ) -> CanonicalSourceProvenance {
        CanonicalSourceProvenance {
            source_basis: basis,
            source_invalidation_epoch: u64::from(snapshot_seed) + 1,
            source_snapshot_digest: digest(snapshot_seed),
            summary_coverage: if has_summary {
                SemanticCoverage::TitleAndSummary
            } else {
                SemanticCoverage::TitleOnly
            },
        }
    }

    fn semantic_provenance(generation_id: Uuid, seed: u8) -> SemanticProvenance {
        SemanticProvenance {
            generation_id,
            unit_key: "overview".to_owned(),
            source_snapshot_digest: digest(seed),
            source_generation_contract_digest: digest(90),
            embedding_space_fence: digest(91),
        }
    }

    fn candidate_explanation(problem_score: Score, anchor_gain: AnchorGain) -> ScoreExplanation {
        let final_score = candidate_score(problem_score, Score::ZERO, anchor_gain);
        ScoreExplanation {
            score_role: SemanticScoreRole::Candidate,
            problem_score,
            conditioned_evidence: Vec::new(),
            highest_gain: Score::ZERO,
            second_highest_gain: Score::ZERO,
            environment_gain: Score::ZERO,
            anchor_gain,
            local_coherence: None,
            document_score: None,
            target_coordinate_score: None,
            transition_score: None,
            penalties: Vec::new(),
            final_score,
        }
    }

    fn relation_explanation(problem_score: Score, coherence: Score) -> ScoreExplanation {
        let final_score = document_score(problem_score, Score::ZERO, Some(coherence));
        ScoreExplanation {
            score_role: SemanticScoreRole::RelationDocument,
            problem_score,
            conditioned_evidence: Vec::new(),
            highest_gain: Score::ZERO,
            second_highest_gain: Score::ZERO,
            environment_gain: Score::ZERO,
            anchor_gain: AnchorGain::None,
            local_coherence: Some(coherence),
            document_score: None,
            target_coordinate_score: None,
            transition_score: None,
            penalties: Vec::new(),
            final_score,
        }
    }

    fn target_explanation(problem_score: Score, coherence: Score) -> ScoreExplanation {
        let final_score = target_coordinate_score(problem_score, Score::ZERO, coherence);
        ScoreExplanation {
            score_role: SemanticScoreRole::TargetCoordinate,
            problem_score,
            conditioned_evidence: Vec::new(),
            highest_gain: Score::ZERO,
            second_highest_gain: Score::ZERO,
            environment_gain: Score::ZERO,
            anchor_gain: AnchorGain::None,
            local_coherence: Some(coherence),
            document_score: None,
            target_coordinate_score: None,
            transition_score: None,
            penalties: Vec::new(),
            final_score,
        }
    }

    fn preview(title: &str, summary_label: Option<&str>) -> SemanticSourcePreview {
        SemanticSourcePreview {
            title: title.to_owned(),
            summary: summary_label.map(|label| label.repeat(320)),
            summary_omitted_reason: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn root(
        project_id: Uuid,
        generation_id: Uuid,
        coordinate: ProjectContextCoordinate,
        source: SemanticSourceIdentity,
        explicit: bool,
        score: Score,
        seed: u8,
        with_summary: bool,
    ) -> SemanticRoot {
        let entrypoint = RootStructuralEntrypoint::Coordinate {
            coordinate: coordinate.clone(),
        };
        let root_id = derive_root_id(project_id, &source, std::slice::from_ref(&entrypoint))
            .expect("root id");
        let explanation = candidate_explanation(
            score,
            if explicit {
                AnchorGain::ExplicitInitial
            } else {
                AnchorGain::None
            },
        );
        SemanticRoot {
            root_id,
            discovery_channels: vec![if explicit {
                RootDiscoveryChannel::ExplicitInitial
            } else {
                RootDiscoveryChannel::ProblemNeutral
            }],
            structural_entrypoints: vec![entrypoint.clone()],
            source,
            preview: preview(
                &format!("root-{seed}"),
                with_summary.then_some("root summary "),
            ),
            lifecycle: SemanticLifecycleClass::Active,
            source_status: None,
            canonical_provenance: provenance(project_view_basis(seed), seed, with_summary),
            semantic_provenance: Some(semantic_provenance(generation_id, seed)),
            semantic_score: Some(explanation.final_score),
            score_explanation: Some(explanation),
            seed_outcomes: vec![SeedOutcome {
                structural_entrypoint: entrypoint,
                produced_path_count: 1,
                zero_hop_stop_reason: None,
            }],
        }
    }

    fn path(
        project_id: Uuid,
        generation_id: Uuid,
        root: &SemanticRoot,
        entered: ProjectContextCoordinate,
        target: ProjectContextCoordinate,
        document_seed: u64,
        with_summary: bool,
    ) -> SemanticPath {
        let coordinates = canonicalize_coordinates(vec![entered.clone(), target.clone()])
            .expect("canonical edge");
        let edge_key = EdgeKey::derive(project_id, &coordinates).expect("edge key");
        let document_id = uuid(document_seed);
        let edge_provenance = ProjectContextEdgeProvenance {
            last_context_revision: document_seed,
            source_change_id: digest(document_seed as u8),
        };
        let binding_provenance = ProjectContextBindingProvenance {
            binding_context_revision: document_seed + 1,
            source_change_id: digest(document_seed as u8 + 1),
            projection_event_id: digest(document_seed as u8 + 2),
        };
        let problem_score = Score::new(820_000).expect("score");
        let coherence = Score::new(760_000).expect("score");
        let relation_explanation = relation_explanation(problem_score, coherence);
        let target_explanation = target_explanation(problem_score, coherence);
        let transition_score = harmonic_score(
            relation_explanation.final_score,
            target_explanation.final_score,
        );
        let hop = SemanticHyperedgeHop {
            ordinal: 1,
            entered_from_coordinate: Some(entered),
            edge: SemanticEdgeObservation {
                edge_key,
                complete_coordinates: coordinates,
                provenance: edge_provenance,
                current_context_document_bindings: vec![ContextDocumentBindingObservation {
                    document_id,
                    provenance: binding_provenance.clone(),
                }],
            },
            selected_relation_document: SemanticRelationDocument {
                document_id,
                binding_provenance,
                preview: preview(
                    &format!("relation-{document_seed}"),
                    with_summary.then_some("relation summary "),
                ),
                canonical_provenance: provenance(
                    SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                        document_revision: document_seed,
                        source_change_id: digest(document_seed as u8 + 3),
                    }),
                    document_seed as u8 + 4,
                    with_summary,
                ),
                semantic_provenance: semantic_provenance(generation_id, document_seed as u8 + 4),
                document_score: relation_explanation.final_score,
                score_explanation: relation_explanation,
            },
            continued_to_coordinate: SemanticContinuedCoordinate {
                coordinate: target.clone(),
                preview: preview(
                    &format!("target-{document_seed}"),
                    with_summary.then_some("target summary "),
                ),
                lifecycle: SemanticLifecycleClass::Active,
                canonical_provenance: provenance(
                    project_view_basis(document_seed as u8 + 5),
                    document_seed as u8 + 5,
                    with_summary,
                ),
                semantic_provenance: semantic_provenance(generation_id, document_seed as u8 + 5),
                target_score: target_explanation.final_score,
                score_explanation: target_explanation,
            },
            transition_score,
        };
        let explanation = path_score(root.semantic_score, &[transition_score]).expect("path score");
        let path_id = derive_path_id(root.root_id, std::slice::from_ref(&hop)).expect("path id");
        SemanticPath {
            path_id,
            root_id: root.root_id,
            hops: vec![hop],
            terminal_coordinate: target,
            path_score: explanation.final_score.expect("scored path"),
            path_score_explanation: explanation,
            branch_stop_reason: BranchStopReason::FrontierExhausted,
        }
    }

    fn packing_input(with_summaries: bool) -> SemanticGraphResponsePackingInput {
        let project_id = uuid(1);
        let generation_id = uuid(2);
        let requirement = coordinate(ProjectViewObjectType::Requirement, 10);
        let work = coordinate(ProjectViewObjectType::Work, 11);
        let explicit = root(
            project_id,
            generation_id,
            requirement.clone(),
            source(project_id, ProjectViewSemanticType::Requirement, uuid(10)),
            true,
            Score::new(900_000).expect("score"),
            10,
            with_summaries,
        );
        let automatic = root(
            project_id,
            generation_id,
            work.clone(),
            source(project_id, ProjectViewSemanticType::Work, uuid(11)),
            false,
            Score::new(800_000).expect("score"),
            11,
            with_summaries,
        );
        let explicit_path = path(
            project_id,
            generation_id,
            &explicit,
            requirement.clone(),
            work.clone(),
            30,
            with_summaries,
        );
        let automatic_path = path(
            project_id,
            generation_id,
            &automatic,
            work.clone(),
            requirement.clone(),
            31,
            with_summaries,
        );
        let incident_edge_keys = vec![explicit_path.hops[0].edge.edge_key];
        let query = SemanticGraphQuery {
            request_id: uuid(3),
            project_id,
            problem: "why did this incident recur?".to_owned(),
            initial_coordinates: vec![requirement.clone()],
            context_coordinates: Vec::new(),
            lifecycle_filter: LifecycleFilter::AllCurrent,
            budget: SemanticGraphQueryBudget::default(),
        };
        SemanticGraphResponsePackingInput {
            query,
            request_binding_digest: digest(99),
            observations: SemanticGraphQueryObservations {
                semantic_generation_id: generation_id,
                source_generation_contract_digest: digest(90),
                embedding_space_fence: digest(91),
                query_contract_digest: query_contract_digest(),
                ranking_contract_digest: ranking_contract_digest().expect("ranking digest"),
                budget_profile_digest: budget_profile_digest().expect("budget digest"),
                extractor_version: "project-overview-v1".to_owned(),
                project_context_revision: 7,
                snapshot_observed_at: Utc
                    .timestamp_opt(1_700_000_000, 0)
                    .single()
                    .expect("timestamp"),
            },
            input_observations: SemanticGraphQueryInputObservations {
                accepted_initial_coordinates: vec![AcceptedInitialCoordinateObservation {
                    coordinate: requirement,
                    graph_membership: CurrentGraphMembershipObservation {
                        context_revision: 7,
                        incident_edge_keys,
                    },
                    source_basis: explicit.canonical_provenance.source_basis.clone(),
                    semantic_state: SemanticHeadState::Current(SemanticHeadProvenance {
                        generation_id,
                        unit_key: "overview".to_owned(),
                        snapshot_digest: explicit.canonical_provenance.source_snapshot_digest,
                    }),
                }],
                initial_not_in_graph: Vec::new(),
                omitted_initial_coordinates: Vec::new(),
                accepted_context_coordinates: Vec::new(),
                omitted_context_coordinates: Vec::new(),
            },
            roots: vec![automatic, explicit],
            paths: vec![automatic_path, explicit_path],
            coverage: SemanticGraphQueryCoverage {
                authorized_graph_sources: 2,
                current_indexed_graph_sources: 2,
                title_only_sources: u64::from(!with_summaries) * 2,
                embedding_coverage: EmbeddingCoverageCounts {
                    current: 2,
                    ..EmbeddingCoverageCounts::default()
                },
                query_channels_requested: 1,
                query_channels_executed: 1,
                omitted_context_channel_counts_by_reason: OmittedContextChannelCounts::default(),
                neutral_candidates_considered: 1,
                conditioned_candidates_considered: 0,
                roots_selected: 2,
                roots_returned: 0,
                expanded_coordinates: 2,
                incident_edges_materialized: 2,
                relation_options_materialized: 2,
                target_options_materialized: 2,
                paths_generated: 2,
                paths_retained: 2,
                paths_returned: 0,
                omitted_for_response_budget: OmittedForResponseBudgetCounts::default(),
                truncation_counts_by_dimension: TruncationCountsByDimension::default(),
                truncation_samples: Vec::new(),
                degraded_mode_counts: DegradedModeCounts::default(),
            },
            completion_reason: CompletionReason::FrontierExhausted,
            exhausted_dimensions: Vec::new(),
        }
    }

    #[test]
    fn packing_is_deterministic_and_never_returns_a_dangling_path() {
        let keys = Keys::generate();
        let caller = Keys::generate().public_key();
        let input = packing_input(true);
        let first =
            pack_semantic_graph_response(input.clone(), &keys.public_key(), &caller, 128 * 1024)
                .expect("pack semantic response");

        let mut shuffled = input;
        shuffled.roots.reverse();
        shuffled.paths.reverse();
        let second =
            pack_semantic_graph_response(shuffled, &keys.public_key(), &caller, 128 * 1024)
                .expect("pack shuffled semantic response");
        assert_eq!(first.result, second.result);
        assert_eq!(
            first.estimated_event_array_bytes,
            second.estimated_event_array_bytes
        );
        assert!(!format!("{first:?}").contains("root summary"));
        let root_ids = first
            .result
            .roots
            .iter()
            .map(|root| root.root_id)
            .collect::<BTreeSet<_>>();
        assert!(first
            .result
            .paths
            .iter()
            .all(|path| root_ids.contains(&path.root_id)));
        let estimated = first.estimated_event_array_bytes;
        let signed = sign_packed_semantic_graph_response(first, &keys)
            .expect("sign after successful postflight");
        assert!(!format!("{signed:?}").contains("root summary"));
        assert_eq!(signed.event_array_bytes.len(), estimated);
        let events: Vec<nostr::Event> =
            serde_json::from_slice(&signed.event_array_bytes).expect("exact Event array");
        assert_eq!(events.len(), 1);
        events[0].verify().expect("Relay signature");
        assert_eq!(
            signed.event_array_bytes,
            serde_json::to_vec(&events).expect("round-trip exact Event array")
        );
    }

    #[test]
    fn summaries_are_restored_whole_or_marked_whole_as_omitted() {
        let keys = Keys::generate();
        let caller = Keys::generate().public_key();
        let input = packing_input(true);
        let full =
            pack_semantic_graph_response(input.clone(), &keys.public_key(), &caller, 128 * 1024)
                .expect("full semantic response");
        let cap = full.estimated_event_array_bytes - 1;
        let packed = pack_semantic_graph_response(input, &keys.public_key(), &caller, cap)
            .expect("summary-truncated semantic response");
        assert_eq!(packed.result.roots.len(), 2);
        assert_eq!(packed.result.paths.len(), 2);
        assert!(packed.result.coverage.omitted_for_response_budget.summaries > 0);
        assert!(packed.estimated_event_array_bytes <= cap);
        let mut observed_omissions = 0_u64;
        for preview in packed.result.roots.iter().map(|root| &root.preview).chain(
            packed.result.paths.iter().flat_map(|path| {
                path.hops.iter().flat_map(|hop| {
                    [
                        &hop.selected_relation_document.preview,
                        &hop.continued_to_coordinate.preview,
                    ]
                })
            }),
        ) {
            assert!(
                preview.summary.is_some()
                    || preview.summary_omitted_reason == Some(SummaryOmittedReason::ResponseBudget)
            );
            observed_omissions += u64::from(preview.summary_omitted_reason.is_some());
        }
        assert_eq!(
            packed.result.coverage.omitted_for_response_budget.summaries,
            observed_omissions
        );
        assert_eq!(
            packed
                .result
                .coverage
                .degraded_mode_counts
                .summary_omitted_for_response_budget,
            observed_omissions
        );
        assert_eq!(
            packed.result.completion_reason,
            CompletionReason::BudgetExhausted
        );
        assert!(packed
            .result
            .exhausted_dimensions
            .contains(&buzz_semantic_query::ExhaustedDimension::ResponseBytes));
    }

    #[test]
    fn a_path_that_does_not_fit_is_dropped_as_one_atomic_object() {
        let keys = Keys::generate();
        let caller = Keys::generate().public_key();
        let input = packing_input(false);
        let full =
            pack_semantic_graph_response(input.clone(), &keys.public_key(), &caller, 128 * 1024)
                .expect("full semantic response");
        let cap = full.estimated_event_array_bytes - 1;
        let packed = pack_semantic_graph_response(input, &keys.public_key(), &caller, cap)
            .expect("path-bounded semantic response");
        assert_eq!(packed.result.roots.len(), 2);
        assert!(packed.result.paths.len() < 2);
        assert_eq!(
            packed.result.coverage.omitted_for_response_budget.paths,
            2 - packed.result.paths.len() as u64
        );
        assert!(packed.result.paths.iter().all(|path| {
            path.hops.len() == 1 && path.hops[0].edge.complete_coordinates.len() == 2
        }));
    }

    #[test]
    fn wall_time_completion_keeps_precedence_over_response_omissions() {
        let keys = Keys::generate();
        let caller = Keys::generate().public_key();
        let mut input = packing_input(false);
        input.completion_reason = CompletionReason::WallTimeExhausted;
        let full =
            pack_semantic_graph_response(input.clone(), &keys.public_key(), &caller, 128 * 1024)
                .expect("full semantic response");
        let packed = pack_semantic_graph_response(
            input,
            &keys.public_key(),
            &caller,
            full.estimated_event_array_bytes - 1,
        )
        .expect("response-bounded wall-time result");

        assert_eq!(
            packed.result.completion_reason,
            CompletionReason::WallTimeExhausted
        );
        assert!(packed.result.exhausted_dimensions.is_empty());
        assert!(
            packed
                .result
                .coverage
                .truncation_counts_by_dimension
                .response_bytes
                > 0
        );
    }

    #[test]
    fn required_explicit_shell_that_cannot_fit_returns_typed_response_too_large() {
        let keys = Keys::generate();
        let caller = Keys::generate().public_key();
        let error =
            pack_semantic_graph_response(packing_input(false), &keys.public_key(), &caller, 1)
                .expect_err("one byte cannot hold required envelope");
        assert!(matches!(
            error,
            SemanticGraphResponsePackingError::ResponseTooLarge { maximum: 1 }
        ));
    }

    #[test]
    fn final_signing_fails_closed_if_postflight_signer_changes() {
        let packed_with = Keys::generate();
        let changed_after_postflight = Keys::generate();
        let caller = Keys::generate().public_key();
        let packed = pack_semantic_graph_response(
            packing_input(false),
            &packed_with.public_key(),
            &caller,
            128 * 1024,
        )
        .expect("pack semantic response");

        let error = sign_packed_semantic_graph_response(packed, &changed_after_postflight)
            .expect_err("changed signer must fail closed");
        assert!(matches!(
            error,
            SemanticGraphResponsePackingError::RelaySignerChanged
        ));
    }

    #[test]
    fn accepted_initial_without_an_explicit_root_shell_is_rejected() {
        let keys = Keys::generate();
        let caller = Keys::generate().public_key();
        let mut input = packing_input(false);
        for root in &mut input.roots {
            root.discovery_channels = vec![RootDiscoveryChannel::ProblemNeutral];
        }

        let error = pack_semantic_graph_response(input, &keys.public_key(), &caller, 128 * 1024)
            .expect_err("accepted initial must retain an explicit root shell");
        assert!(matches!(
            error,
            SemanticGraphResponsePackingError::InvalidInput(message)
                if message.contains("required root shell")
        ));
    }

    #[test]
    fn final_exact_cap_is_rechecked_after_postflight_before_return() {
        let keys = Keys::generate();
        let caller = Keys::generate().public_key();
        let mut packed = pack_semantic_graph_response(
            packing_input(false),
            &keys.public_key(),
            &caller,
            128 * 1024,
        )
        .expect("pack semantic response");
        packed.effective_limit = packed.estimated_event_array_bytes - 1;

        let error = sign_packed_semantic_graph_response(packed, &keys)
            .expect_err("final exact bytes must still fit after postflight");
        assert!(matches!(
            error,
            SemanticGraphResponsePackingError::ResponseTooLarge { .. }
        ));
    }
}
