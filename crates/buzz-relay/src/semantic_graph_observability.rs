//! Content-free, low-cardinality semantic graph query metrics.
//!
//! This boundary intentionally accepts only closed enums and aggregate counts.
//! It must never accept query text, source previews, vectors, Coordinates, or
//! any request/source identity as a metric label.

use std::time::{Duration, Instant};

use buzz_semantic_query::{
    BranchStopReason, CompletionReason, ExhaustedDimension, SemanticGraphQueryResult,
};

/// Closed execution stages used by duration and error metrics.
#[derive(Clone, Copy)]
pub(crate) enum SemanticGraphMetricStage {
    Total,
    Root,
    ProviderWait,
    Provider,
    Snapshot,
    Traversal,
    Packing,
    Postflight,
    Signing,
}

impl SemanticGraphMetricStage {
    #[cfg(test)]
    const ALL: [Self; 9] = [
        Self::Total,
        Self::Root,
        Self::ProviderWait,
        Self::Provider,
        Self::Snapshot,
        Self::Traversal,
        Self::Packing,
        Self::Postflight,
        Self::Signing,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Total => "total",
            Self::Root => "root",
            Self::ProviderWait => "provider_wait",
            Self::Provider => "provider",
            Self::Snapshot => "snapshot",
            Self::Traversal => "traversal",
            Self::Packing => "packing",
            Self::Postflight => "postflight",
            Self::Signing => "signing",
        }
    }
}

/// RAII timer that records one closed query stage on every return path.
pub(crate) struct SemanticGraphStageTimer {
    stage: SemanticGraphMetricStage,
    started_at: Instant,
}

impl Drop for SemanticGraphStageTimer {
    fn drop(&mut self) {
        record_stage_duration(self.stage, self.started_at.elapsed());
    }
}

/// Start one content-free stage-duration observation.
pub(crate) fn stage_timer(stage: SemanticGraphMetricStage) -> SemanticGraphStageTimer {
    SemanticGraphStageTimer {
        stage,
        started_at: Instant::now(),
    }
}

/// Record an already-measured content-free stage duration.
pub(crate) fn record_stage_duration(stage: SemanticGraphMetricStage, duration: Duration) {
    metrics::histogram!(
        "buzz_semantic_graph_query_duration_seconds",
        "stage" => stage.label()
    )
    .record(duration.as_secs_f64());
}

/// Closed whole-request outcome.
#[derive(Clone, Copy)]
pub(crate) enum SemanticGraphQueryMetricResult {
    Success,
    Error,
}

impl SemanticGraphQueryMetricResult {
    const fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
        }
    }
}

/// Record exactly one final outcome for an authenticated query execution.
pub(crate) fn record_query_outcome(result: SemanticGraphQueryMetricResult) {
    metrics::counter!(
        "buzz_semantic_graph_queries_total",
        "result" => result.label()
    )
    .increment(1);
}

/// Closed content-free failure code.
#[derive(Clone, Copy)]
pub(crate) enum SemanticGraphQueryMetricError {
    QueryDisabled,
    InvalidProject,
    ProcessBusy,
    ProviderUnavailable,
    ProviderBusy,
    SemanticGenerationChanged,
    ContextSourceChanged,
    AuthorizationChanged,
    DeadlineExceeded,
    Database,
    Contract,
    StableSigner,
    RequestBinding,
    Readiness,
    ResponseTooLarge,
    InvalidPackingInput,
    Signing,
    RelaySignerChanged,
    SizeEstimateDrift,
    Serialization,
    PostflightUnavailable,
    PostflightDenied,
}

impl SemanticGraphQueryMetricError {
    #[cfg(test)]
    const ALL: [Self; 22] = [
        Self::QueryDisabled,
        Self::InvalidProject,
        Self::ProcessBusy,
        Self::ProviderUnavailable,
        Self::ProviderBusy,
        Self::SemanticGenerationChanged,
        Self::ContextSourceChanged,
        Self::AuthorizationChanged,
        Self::DeadlineExceeded,
        Self::Database,
        Self::Contract,
        Self::StableSigner,
        Self::RequestBinding,
        Self::Readiness,
        Self::ResponseTooLarge,
        Self::InvalidPackingInput,
        Self::Signing,
        Self::RelaySignerChanged,
        Self::SizeEstimateDrift,
        Self::Serialization,
        Self::PostflightUnavailable,
        Self::PostflightDenied,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::QueryDisabled => "query_disabled",
            Self::InvalidProject => "invalid_project",
            Self::ProcessBusy => "process_busy",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ProviderBusy => "provider_busy",
            Self::SemanticGenerationChanged => "semantic_generation_changed",
            Self::ContextSourceChanged => "context_source_changed",
            Self::AuthorizationChanged => "authorization_changed",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::Database => "database",
            Self::Contract => "contract",
            Self::StableSigner => "stable_signer",
            Self::RequestBinding => "request_binding",
            Self::Readiness => "readiness",
            Self::ResponseTooLarge => "response_too_large",
            Self::InvalidPackingInput => "invalid_packing_input",
            Self::Signing => "signing",
            Self::RelaySignerChanged => "relay_signer_changed",
            Self::SizeEstimateDrift => "size_estimate_drift",
            Self::Serialization => "serialization",
            Self::PostflightUnavailable => "postflight_unavailable",
            Self::PostflightDenied => "postflight_denied",
        }
    }
}

/// Record one closed failure without carrying any source/request content.
pub(crate) fn record_query_error(
    stage: SemanticGraphMetricStage,
    error: SemanticGraphQueryMetricError,
) {
    metrics::counter!(
        "buzz_semantic_graph_query_errors_total",
        "stage" => stage.label(),
        "code" => error.label()
    )
    .increment(1);
}

/// Closed Provider failure class.
#[derive(Clone, Copy)]
pub(crate) enum SemanticGraphProviderFailure {
    Unavailable,
    Busy,
    RateLimited,
    Transport,
    Retryable,
    Rejected,
    InvalidResponse,
    Deadline,
}

impl SemanticGraphProviderFailure {
    #[cfg(test)]
    const ALL: [Self; 8] = [
        Self::Unavailable,
        Self::Busy,
        Self::RateLimited,
        Self::Transport,
        Self::Retryable,
        Self::Rejected,
        Self::InvalidResponse,
        Self::Deadline,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Busy => "busy",
            Self::RateLimited => "rate_limited",
            Self::Transport => "transport",
            Self::Retryable => "retryable",
            Self::Rejected => "rejected",
            Self::InvalidResponse => "invalid_response",
            Self::Deadline => "deadline",
        }
    }
}

/// Record one closed Provider failure class.
pub(crate) fn record_provider_failure(failure: SemanticGraphProviderFailure) {
    metrics::counter!(
        "buzz_semantic_graph_query_provider_failures_total",
        "code" => failure.label()
    )
    .increment(1);
}

/// Record the actual local wait before a reserved Provider request began.
pub(crate) fn record_provider_wait(duration: Duration) {
    metrics::histogram!("buzz_semantic_graph_query_provider_wait_seconds")
        .record(duration.as_secs_f64());
}

/// Closed exact-distance result-set stage.
#[derive(Clone, Copy)]
pub(crate) enum SemanticGraphDistanceStage {
    RootRecall,
    RootMatrix,
    RootRedundancy,
    Relation,
    Target,
}

impl SemanticGraphDistanceStage {
    #[cfg(test)]
    const ALL: [Self; 5] = [
        Self::RootRecall,
        Self::RootMatrix,
        Self::RootRedundancy,
        Self::Relation,
        Self::Target,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::RootRecall => "root_recall",
            Self::RootMatrix => "root_matrix",
            Self::RootRedundancy => "root_redundancy",
            Self::Relation => "relation",
            Self::Target => "target",
        }
    }
}

/// Record exact-distance rows returned to the Relay by one bounded DB call.
///
/// This does not claim to be PostgreSQL's internal scanned-row count.
pub(crate) fn record_db_distance_rows(stage: SemanticGraphDistanceStage, rows: usize) {
    metrics::histogram!(
        "buzz_semantic_graph_query_db_distance_rows",
        "stage" => stage.label()
    )
    .record(rows as f64);
}

/// Record the total lifetime of the committed Stage C snapshot transaction.
pub(crate) fn record_snapshot_transaction(duration: Duration) {
    metrics::histogram!("buzz_semantic_graph_query_snapshot_transaction_seconds")
        .record(duration.as_secs_f64());
    record_stage_duration(SemanticGraphMetricStage::Snapshot, duration);
}

/// Record one generation/context churn retry.
pub(crate) fn record_generation_retry() {
    metrics::counter!("buzz_semantic_graph_query_generation_retries_total").increment(1);
}

#[derive(Clone, Copy)]
enum PartialCoverageReason {
    EmbeddingMissing,
    EmbeddingBuilding,
    EmbeddingFailed,
    EmbeddingUnsupported,
    NonQueryableZeroVector,
    ContextSourceNotFound,
    ContextSourceIneligible,
    ContextSemanticHeadMissing,
    ContextSemanticHeadBuilding,
    ContextSemanticHeadFailed,
    ConditionedInputUnsupported,
}

impl PartialCoverageReason {
    const fn label(self) -> &'static str {
        match self {
            Self::EmbeddingMissing => "embedding_missing",
            Self::EmbeddingBuilding => "embedding_building",
            Self::EmbeddingFailed => "embedding_failed",
            Self::EmbeddingUnsupported => "embedding_unsupported",
            Self::NonQueryableZeroVector => "non_queryable_zero_vector",
            Self::ContextSourceNotFound => "context_source_not_found",
            Self::ContextSourceIneligible => "context_source_ineligible",
            Self::ContextSemanticHeadMissing => "context_semantic_head_missing",
            Self::ContextSemanticHeadBuilding => "context_semantic_head_building",
            Self::ContextSemanticHeadFailed => "context_semantic_head_failed",
            Self::ConditionedInputUnsupported => "conditioned_input_unsupported",
        }
    }
}

#[derive(Clone, Copy)]
enum DegradedReason {
    RelationEmbeddingMissing,
    TargetEmbeddingMissing,
    IndexCoveragePartial,
    SummaryOmittedForResponseBudget,
    HyperedgeTooLarge,
    AutomaticRootOmittedForResponseBudget,
    PathOmittedForResponseBudget,
}

impl DegradedReason {
    const fn label(self) -> &'static str {
        match self {
            Self::RelationEmbeddingMissing => "relation_embedding_missing",
            Self::TargetEmbeddingMissing => "target_embedding_missing",
            Self::IndexCoveragePartial => "index_coverage_partial",
            Self::SummaryOmittedForResponseBudget => "summary_omitted_for_response_budget",
            Self::HyperedgeTooLarge => "hyperedge_too_large",
            Self::AutomaticRootOmittedForResponseBudget => {
                "automatic_root_omitted_for_response_budget"
            }
            Self::PathOmittedForResponseBudget => "path_omitted_for_response_budget",
        }
    }
}

const BRANCH_STOP_REASON_COUNT: usize = 7;

const fn branch_stop_index(reason: BranchStopReason) -> usize {
    match reason {
        BranchStopReason::FrontierExhausted => 0,
        BranchStopReason::BelowRelevanceThreshold => 1,
        BranchStopReason::CycleOrDuplicate => 2,
        BranchStopReason::MaxHopsReached => 3,
        BranchStopReason::HyperedgeTooLarge => 4,
        BranchStopReason::GlobalBudgetExhausted => 5,
        BranchStopReason::WallTimeExhausted => 6,
    }
}

const fn branch_stop_label(index: usize) -> &'static str {
    match index {
        0 => "frontier_exhausted",
        1 => "below_relevance_threshold",
        2 => "cycle_or_duplicate",
        3 => "max_hops_reached",
        4 => "hyperedge_too_large",
        5 => "global_budget_exhausted",
        6 => "wall_time_exhausted",
        _ => "invalid",
    }
}

const fn completion_label(reason: CompletionReason) -> &'static str {
    match reason {
        CompletionReason::FrontierExhausted => "frontier_exhausted",
        CompletionReason::BudgetExhausted => "budget_exhausted",
        CompletionReason::WallTimeExhausted => "wall_time_exhausted",
    }
}

const fn budget_label(dimension: ExhaustedDimension) -> &'static str {
    match dimension {
        ExhaustedDimension::RecallPerChannel => "recall_per_channel",
        ExhaustedDimension::SemanticRoots => "semantic_roots",
        ExhaustedDimension::HopsPerPath => "hops_per_path",
        ExhaustedDimension::BeamWidth => "beam_width",
        ExhaustedDimension::ExpandedCoordinates => "expanded_coordinates",
        ExhaustedDimension::IncidentEdgesMaterialized => "incident_edges_materialized",
        ExhaustedDimension::RelationOptionsMaterialized => "relation_options_materialized",
        ExhaustedDimension::TargetOptionsMaterialized => "target_options_materialized",
        ExhaustedDimension::Paths => "paths",
        ExhaustedDimension::ResponseBytes => "response_bytes",
    }
}

/// Content-free aggregate snapshot captured after deterministic packing.
///
/// The snapshot deliberately cannot carry IDs, source previews, query text, or
/// vectors, so it remains safe to retain until final signing succeeds.
pub(crate) struct SemanticGraphResultMetricSnapshot {
    completion_reason: CompletionReason,
    exhausted_dimensions: Vec<ExhaustedDimension>,
    path_stops: [u64; BRANCH_STOP_REASON_COUNT],
    zero_hop_stops: [u64; BRANCH_STOP_REASON_COUNT],
    channels_requested: u64,
    channels_executed: u64,
    neutral_candidates: u64,
    conditioned_candidates: u64,
    roots_selected: u64,
    roots_returned: u64,
    paths_generated: u64,
    paths_retained: u64,
    paths_returned: u64,
    partial_coverage: Vec<(PartialCoverageReason, u64)>,
    degraded: Vec<(DegradedReason, u64)>,
}

impl SemanticGraphResultMetricSnapshot {
    /// Reduce a validated result to content-free aggregate metrics.
    pub(crate) fn from_result(result: &SemanticGraphQueryResult) -> Self {
        let coverage = &result.coverage;
        let mut path_stops = [0_u64; BRANCH_STOP_REASON_COUNT];
        for path in &result.paths {
            let index = branch_stop_index(path.branch_stop_reason);
            path_stops[index] = path_stops[index].saturating_add(1);
        }
        let mut zero_hop_stops = [0_u64; BRANCH_STOP_REASON_COUNT];
        for outcome in result
            .roots
            .iter()
            .flat_map(|root| root.seed_outcomes.iter())
        {
            if let Some(reason) = outcome.zero_hop_stop_reason {
                let index = branch_stop_index(reason);
                zero_hop_stops[index] = zero_hop_stops[index].saturating_add(1);
            }
        }

        let embedding = &coverage.embedding_coverage;
        let omitted = &coverage.omitted_context_channel_counts_by_reason;
        let partial_coverage = vec![
            (PartialCoverageReason::EmbeddingMissing, embedding.missing),
            (PartialCoverageReason::EmbeddingBuilding, embedding.building),
            (PartialCoverageReason::EmbeddingFailed, embedding.failed),
            (
                PartialCoverageReason::EmbeddingUnsupported,
                embedding.unsupported,
            ),
            (
                PartialCoverageReason::NonQueryableZeroVector,
                embedding.non_queryable_zero_vector,
            ),
            (
                PartialCoverageReason::ContextSourceNotFound,
                omitted.source_not_found,
            ),
            (
                PartialCoverageReason::ContextSourceIneligible,
                omitted.source_ineligible,
            ),
            (
                PartialCoverageReason::ContextSemanticHeadMissing,
                omitted.semantic_head_missing,
            ),
            (
                PartialCoverageReason::ContextSemanticHeadBuilding,
                omitted.semantic_head_building,
            ),
            (
                PartialCoverageReason::ContextSemanticHeadFailed,
                omitted.semantic_head_failed,
            ),
            (
                PartialCoverageReason::ConditionedInputUnsupported,
                omitted.conditioned_input_unsupported,
            ),
        ];
        let degraded_counts = &coverage.degraded_mode_counts;
        let response_omissions = &coverage.omitted_for_response_budget;
        let degraded = vec![
            (
                DegradedReason::RelationEmbeddingMissing,
                degraded_counts.relation_embedding_missing,
            ),
            (
                DegradedReason::TargetEmbeddingMissing,
                degraded_counts.target_embedding_missing,
            ),
            (
                DegradedReason::IndexCoveragePartial,
                degraded_counts.index_coverage_partial,
            ),
            (
                DegradedReason::SummaryOmittedForResponseBudget,
                degraded_counts.summary_omitted_for_response_budget,
            ),
            (
                DegradedReason::HyperedgeTooLarge,
                degraded_counts.hyperedge_too_large,
            ),
            (
                DegradedReason::AutomaticRootOmittedForResponseBudget,
                response_omissions.automatic_roots,
            ),
            (
                DegradedReason::PathOmittedForResponseBudget,
                response_omissions.paths,
            ),
        ];

        Self {
            completion_reason: result.completion_reason,
            exhausted_dimensions: result.exhausted_dimensions.clone(),
            path_stops,
            zero_hop_stops,
            channels_requested: coverage.query_channels_requested,
            channels_executed: coverage.query_channels_executed,
            neutral_candidates: coverage.neutral_candidates_considered,
            conditioned_candidates: coverage.conditioned_candidates_considered,
            roots_selected: coverage.roots_selected,
            roots_returned: coverage.roots_returned,
            paths_generated: coverage.paths_generated,
            paths_retained: coverage.paths_retained,
            paths_returned: coverage.paths_returned,
            partial_coverage,
            degraded,
        }
    }

    /// Record this result after Stage D and final exact signing have succeeded.
    pub(crate) fn record_success(&self, response_bytes: usize) {
        metrics::histogram!("buzz_semantic_graph_query_channels", "result" => "requested")
            .record(self.channels_requested as f64);
        metrics::histogram!("buzz_semantic_graph_query_channels", "result" => "executed")
            .record(self.channels_executed as f64);
        metrics::histogram!("buzz_semantic_graph_query_channels", "result" => "omitted").record(
            self.channels_requested
                .saturating_sub(self.channels_executed) as f64,
        );
        metrics::histogram!("buzz_semantic_graph_query_candidates", "role" => "neutral")
            .record(self.neutral_candidates as f64);
        metrics::histogram!("buzz_semantic_graph_query_candidates", "role" => "conditioned")
            .record(self.conditioned_candidates as f64);
        metrics::histogram!("buzz_semantic_graph_query_result_items", "kind" => "roots_selected")
            .record(self.roots_selected as f64);
        metrics::histogram!("buzz_semantic_graph_query_result_items", "kind" => "roots_returned")
            .record(self.roots_returned as f64);
        metrics::histogram!("buzz_semantic_graph_query_result_items", "kind" => "paths_generated")
            .record(self.paths_generated as f64);
        metrics::histogram!("buzz_semantic_graph_query_result_items", "kind" => "paths_retained")
            .record(self.paths_retained as f64);
        metrics::histogram!("buzz_semantic_graph_query_result_items", "kind" => "paths_returned")
            .record(self.paths_returned as f64);
        metrics::histogram!("buzz_semantic_graph_query_response_bytes")
            .record(response_bytes as f64);

        for (index, count) in self.path_stops.iter().copied().enumerate() {
            if count > 0 {
                metrics::histogram!(
                    "buzz_semantic_graph_query_paths",
                    "branch_stop_reason" => branch_stop_label(index)
                )
                .record(count as f64);
            }
        }
        for (index, count) in self.zero_hop_stops.iter().copied().enumerate() {
            if count > 0 {
                metrics::histogram!(
                    "buzz_semantic_graph_query_zero_hop_stops",
                    "branch_stop_reason" => branch_stop_label(index)
                )
                .record(count as f64);
            }
        }
        metrics::counter!(
            "buzz_semantic_graph_query_completions_total",
            "completion_reason" => completion_label(self.completion_reason)
        )
        .increment(1);
        for dimension in &self.exhausted_dimensions {
            metrics::counter!(
                "buzz_semantic_graph_query_budget_exhausted_total",
                "budget_kind" => budget_label(*dimension)
            )
            .increment(1);
        }
        for (reason, count) in &self.partial_coverage {
            if *count > 0 {
                metrics::counter!(
                    "buzz_semantic_graph_query_partial_coverage_total",
                    "reason" => reason.label()
                )
                .increment(*count);
            }
        }
        for (reason, count) in &self.degraded {
            if *count > 0 {
                metrics::counter!(
                    "buzz_semantic_graph_query_degraded_total",
                    "reason" => reason.label()
                )
                .increment(*count);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        branch_stop_label, budget_label, completion_label, SemanticGraphDistanceStage,
        SemanticGraphMetricStage, SemanticGraphProviderFailure, SemanticGraphQueryMetricError,
    };
    use buzz_semantic_query::{CompletionReason, ExhaustedDimension};

    fn assert_safe_label(label: &str) {
        assert!(!label.is_empty());
        assert!(label.len() <= 48);
        assert!(label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_'));
    }

    #[test]
    fn every_metric_label_comes_from_a_bounded_content_free_closed_set() {
        for stage in SemanticGraphMetricStage::ALL {
            assert_safe_label(stage.label());
        }
        for error in SemanticGraphQueryMetricError::ALL {
            assert_safe_label(error.label());
        }
        for failure in SemanticGraphProviderFailure::ALL {
            assert_safe_label(failure.label());
        }
        for stage in SemanticGraphDistanceStage::ALL {
            assert_safe_label(stage.label());
        }
        for index in 0..7 {
            assert_safe_label(branch_stop_label(index));
        }
        for completion in [
            CompletionReason::FrontierExhausted,
            CompletionReason::BudgetExhausted,
            CompletionReason::WallTimeExhausted,
        ] {
            assert_safe_label(completion_label(completion));
        }
        for dimension in [
            ExhaustedDimension::RecallPerChannel,
            ExhaustedDimension::SemanticRoots,
            ExhaustedDimension::HopsPerPath,
            ExhaustedDimension::BeamWidth,
            ExhaustedDimension::ExpandedCoordinates,
            ExhaustedDimension::IncidentEdgesMaterialized,
            ExhaustedDimension::RelationOptionsMaterialized,
            ExhaustedDimension::TargetOptionsMaterialized,
            ExhaustedDimension::Paths,
            ExhaustedDimension::ResponseBytes,
        ] {
            assert_safe_label(budget_label(dimension));
        }
    }
}
