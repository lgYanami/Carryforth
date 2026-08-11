//! Closed Tauri input, display result, and error DTOs for semantic query.

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ProjectContextCoordinateDto;

/// Closed, untrusted frontend input for one semantic graph query.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticProjectContextQueryInput {
    pub(super) community_key: String,
    pub(super) applied_workspace_token: String,
    pub(super) problem: String,
    pub(super) initial_coordinates: Vec<ProjectContextCoordinateDto>,
    pub(super) context_coordinates: Vec<ProjectContextCoordinateDto>,
}

/// Sanitized semantic query failure returned across the Tauri boundary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticProjectContextQueryError {
    pub(super) code: &'static str,
    pub(super) message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) status: Option<u16>,
    pub(super) retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) retry_after_seconds: Option<u64>,
}

impl SemanticProjectContextQueryError {
    pub(super) fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            status: None,
            retryable: false,
            retry_after_seconds: None,
        }
    }

    pub(super) fn invalid_input() -> Self {
        Self::new("invalid_input", "The semantic query input is invalid.")
    }

    pub(super) fn unsupported() -> Self {
        Self::new(
            "unsupported",
            "Semantic Project Context query is not available for this Community.",
        )
    }

    pub(super) fn restricted(status: u16) -> Self {
        Self {
            status: Some(status),
            ..Self::new(
                "restricted",
                "Semantic Project Context query requires current Community access.",
            )
        }
    }

    pub(super) fn busy(retry_after_seconds: Option<u64>) -> Self {
        Self {
            status: Some(StatusCode::TOO_MANY_REQUESTS.as_u16()),
            retryable: true,
            retry_after_seconds,
            ..Self::new("busy", "Semantic query capacity is temporarily busy.")
        }
    }

    pub(super) fn conflict(status: Option<u16>) -> Self {
        Self {
            status,
            retryable: true,
            ..Self::new(
                "conflict",
                "The applied workspace or Project snapshot changed.",
            )
        }
    }

    pub(super) fn timeout(status: Option<u16>) -> Self {
        Self {
            status,
            retryable: true,
            ..Self::new("timeout", "The semantic query timed out.")
        }
    }

    pub(super) fn too_large(status: Option<u16>) -> Self {
        Self {
            status,
            ..Self::new("too_large", "The semantic query response is too large.")
        }
    }

    pub(super) fn unavailable(status: Option<u16>) -> Self {
        Self {
            status,
            retryable: true,
            ..Self::new(
                "unavailable",
                "Semantic Project Context query is temporarily unavailable.",
            )
        }
    }

    pub(super) fn verification_failed() -> Self {
        Self::new(
            "verification_failed",
            "The Relay semantic result could not be verified.",
        )
    }

    pub(super) fn internal() -> Self {
        Self::new(
            "internal",
            "Desktop could not complete the semantic query safely.",
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SemanticProjectContextCoverage {
    pub(super) authorized_graph_sources: u64,
    pub(super) current_indexed_graph_sources: u64,
    pub(super) title_only_sources: u64,
    pub(super) roots_returned: u64,
    pub(super) paths_returned: u64,
    pub(super) omitted_initial_coordinates: u64,
    pub(super) omitted_context_coordinates: u64,
    pub(super) index_coverage_partial: u64,
    pub(super) omitted_for_response_budget: SemanticResponseBudgetOmissions,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SemanticResponseBudgetOmissions {
    pub(super) automatic_roots: u64,
    pub(super) paths: u64,
    pub(super) summaries: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SemanticCoordinateInputOutcome {
    pub(super) coordinate_key: String,
    pub(super) state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SemanticQueryInputOutcomes {
    pub(super) initial: Vec<SemanticCoordinateInputOutcome>,
    pub(super) context: Vec<SemanticCoordinateInputOutcome>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SemanticContextDocumentEntrypoint {
    pub(super) edge_key: String,
    pub(super) document_id: Uuid,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SemanticProjectContextRoot {
    pub(super) root_id: String,
    pub(super) coordinate_entrypoints: Vec<String>,
    pub(super) context_document_entrypoints: Vec<SemanticContextDocumentEntrypoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SemanticProjectContextHop {
    pub(super) ordinal: u16,
    pub(super) edge_key: String,
    pub(super) complete_coordinate_keys: Vec<String>,
    pub(super) current_context_document_ids: Vec<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) entered_from_coordinate_key: Option<String>,
    pub(super) selected_context_document_id: Uuid,
    pub(super) continued_to_coordinate_key: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SemanticProjectContextPath {
    pub(super) path_id: String,
    pub(super) root_id: String,
    pub(super) branch_stop_reason: &'static str,
    pub(super) hops: Vec<SemanticProjectContextHop>,
}

/// Verified, UI-only semantic graph result. Raw Event/content/provenance and
/// exact authenticated bytes never cross this boundary.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticProjectContextQueryResult {
    pub(super) community_key: String,
    pub(super) applied_workspace_token: String,
    pub(super) caller_pubkey: String,
    pub(super) request_id: Uuid,
    pub(super) project_id: Uuid,
    pub(super) relay_pubkey: String,
    pub(super) project_context_revision: u64,
    pub(super) snapshot_observed_at: chrono::DateTime<chrono::Utc>,
    pub(super) completion_reason: &'static str,
    pub(super) exhausted_dimensions: Vec<&'static str>,
    pub(super) coverage: SemanticProjectContextCoverage,
    pub(super) input_outcomes: SemanticQueryInputOutcomes,
    pub(super) roots: Vec<SemanticProjectContextRoot>,
    pub(super) paths: Vec<SemanticProjectContextPath>,
}
