//! Verified schema-v3 Project View bridge for the desktop client.

use buzz_core_pkg::PublicKey;
use buzz_project_view_pkg::v2::CommunityMemberRole;
use buzz_project_view_pkg::v3::ProjectViewObjectV3;
pub(crate) use buzz_sdk_pkg::project_view_v3::{
    PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION, PROJECT_VIEW_V3_EXTENSION,
};
use chrono::{DateTime, Utc};
use nostr::{Event, Keys};
use serde::Serialize;
use tauri::State;

use crate::app_state::AppState;
use crate::relay::{
    query_relay, query_relay_at_with_keys_and_client_typed, query_relay_at_with_keys_typed,
    RelayHttpError, RelayHttpErrorCategory,
};

pub(crate) const PROJECT_CONTEXT_EXTENSION: &str = "buzz-project-context-v1";
pub(crate) const SEMANTIC_QUERY_HTTP_EXTENSION: &str = "buzz-project-context-semantic-query-http";
const SNAPSHOT_PAGE_SIZE: usize = 500;
const SNAPSHOT_MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectViewSchema {
    V3,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProjectViewIdentity {
    pub(crate) relay_pubkey: PublicKey,
    pub(crate) schema: ProjectViewSchema,
    /// Whether NIP-11 advertises the strict-ready ordinary v3 runtime.
    pub(crate) runtime_ready: bool,
    /// Whether the independent Project Context Reference capability is ready.
    pub(crate) project_context_reference_supported: bool,
    /// Whether versioned Project Documents are ready.
    pub(crate) project_document_supported: bool,
    /// Whether the independent Project Context Edge capability is ready.
    pub(crate) project_context_edge_supported: bool,
    /// Whether semantic Project Context HTTP query readiness is advertised.
    pub(crate) semantic_query_http_available: bool,
    /// Whether this NIP-11 dynamic capability observation was incomplete.
    pub(crate) extensions_temporarily_unavailable: bool,
}

impl ProjectViewIdentity {
    /// Reject every ordinary Project View surface when NIP-11 exposes only the
    /// discovery-only greenfield bootstrap marker.
    pub(crate) fn require_runtime_ready(&self, surface: &str) -> Result<(), String> {
        if self.runtime_ready {
            Ok(())
        } else {
            Err(format!(
                "unavailable: {surface} requires an initialized and enabled Project View v3 runtime"
            ))
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectViewWorkResponsibility {
    work_id: uuid::Uuid,
    role_id: uuid::Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectViewMembershipMember {
    pubkey: String,
    role: CommunityMemberRole,
}

/// Failures produced while assembling a verified Project View read snapshot.
#[derive(Debug)]
pub(crate) enum ProjectViewReadError {
    /// The current identity is not permitted to read the Project View.
    Forbidden,
    /// A bounded read observed incompatible source revisions.
    Conflict(String),
    /// The verified source could not be reached temporarily.
    Unavailable(String),
    /// The response was malformed, unverifiable, or otherwise invalid.
    Other(String),
}

/// Result type shared by Project View's verified native readers.
pub(crate) type ProjectViewReadResult<T> = Result<T, ProjectViewReadError>;

/// Desktop-facing state of the active Community's Project View.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProjectViewLoadResult {
    /// The Relay does not advertise schema-v3 Project View support.
    Unsupported,
    /// The current identity may not read this Community's Project View.
    Forbidden,
    /// Project View v3 is supported but has not been initialized.
    Uninitialized {
        /// Canonical Relay signing identity established by NIP-11.
        relay_pubkey: String,
    },
    /// A complete, internally consistent, cryptographically verified v3 view.
    Ready {
        /// Canonical Relay signing identity established by NIP-11.
        relay_pubkey: String,
        /// Whether the independent Context sub-capability is currently ready.
        project_context_supported: bool,
        /// Fixed latest runtime schema.
        schema_version: u16,
        /// Current optimistic-concurrency revision.
        project_revision: u64,
        /// Current projection generation.
        projection_generation: u64,
        /// Number of active objects declared by the metadata projection.
        active_object_count: u32,
        /// Canonical server time of the projected state.
        updated_at: DateTime<Utc>,
        /// Strict flat schema-v3 objects. TypeScript assembles the hierarchy.
        objects_v3: Vec<ProjectViewObjectV3>,
        /// Verified schema-v3 Role continuity state.
        role_continuity: Box<ProjectViewRoleContinuityV3>,
    },
}

/// Load the active Community's complete, verified Project View v3 snapshot.
#[tauri::command]
pub async fn get_project_view(state: State<'_, AppState>) -> Result<ProjectViewLoadResult, String> {
    load_project_view(&state).await
}

mod identity;
use identity::read_identity;
pub(crate) use identity::{
    read_identity_at, read_identity_at_with_client, read_project_document_identity_at,
};
mod role_history;
pub use role_history::*;
mod v3;
pub use v3::ProjectViewRoleContinuityV3;
use v3::{fetch_consistent_v3_snapshot, read_v3_meta, V3ProjectSnapshot};
pub(crate) use v3::{
    fetch_consistent_verified_v3_snapshot_at, read_verified_v3_meta_at_with_client,
};

fn read_error_message(error: ProjectViewReadError) -> String {
    match error {
        ProjectViewReadError::Forbidden => {
            "restricted: Project View requires current Community membership".to_owned()
        }
        ProjectViewReadError::Conflict(message)
        | ProjectViewReadError::Unavailable(message)
        | ProjectViewReadError::Other(message) => message,
    }
}

async fn load_project_view(state: &AppState) -> Result<ProjectViewLoadResult, String> {
    let Some(identity) = read_identity(state).await? else {
        return Ok(ProjectViewLoadResult::Unsupported);
    };
    if identity.schema != ProjectViewSchema::V3 {
        return Ok(ProjectViewLoadResult::Unsupported);
    }
    if !identity.runtime_ready {
        return Ok(ProjectViewLoadResult::Uninitialized {
            relay_pubkey: identity.relay_pubkey.to_hex(),
        });
    }

    let loaded = fetch_consistent_v3_snapshot(state, identity)
        .await
        .map(|snapshot| {
            snapshot.map(
                |V3ProjectSnapshot {
                     meta,
                     objects,
                     role_continuity,
                 }| ProjectViewLoadResult::Ready {
                    relay_pubkey: identity.relay_pubkey.to_hex(),
                    project_context_supported: identity.project_context_reference_supported,
                    schema_version: 3,
                    project_revision: meta.project_revision,
                    projection_generation: meta.projection_generation,
                    active_object_count: meta.entity_counts.active_objects,
                    updated_at: meta.updated_at,
                    objects_v3: objects,
                    role_continuity: Box::new(role_continuity),
                },
            )
        });
    match loaded {
        Ok(Some(result)) => Ok(result),
        Ok(None) => Err(integrity_error(
            "NIP-11 advertises the ready Project View v3 runtime without canonical metadata",
        )),
        Err(ProjectViewReadError::Forbidden) => Ok(ProjectViewLoadResult::Forbidden),
        Err(ProjectViewReadError::Conflict(message))
        | Err(ProjectViewReadError::Unavailable(message))
        | Err(ProjectViewReadError::Other(message)) => Err(message),
    }
}

async fn query_project_view(
    state: &AppState,
    filters: &[serde_json::Value],
) -> ProjectViewReadResult<Vec<Event>> {
    query_relay(state, filters).await.map_err(|message| {
        if message.starts_with("relay returned 403") {
            ProjectViewReadError::Forbidden
        } else if message.starts_with("relay returned 409") {
            conflict_error("Project View changed during snapshot pagination")
        } else {
            ProjectViewReadError::Other(message)
        }
    })
}

/// Query Project View through a Relay URL and signer captured before any await.
pub(crate) async fn query_project_view_at_with_keys(
    state: &AppState,
    api_base_url: &str,
    keys: &Keys,
    filters: &[serde_json::Value],
) -> ProjectViewReadResult<Vec<Event>> {
    query_relay_at_with_keys_typed(state, api_base_url, filters, keys, None)
        .await
        .map_err(map_project_view_http_error)
}

/// Query a pinned Project View through an explicit HTTP redirect policy.
pub(crate) async fn query_project_view_at_with_keys_and_client(
    client: &reqwest::Client,
    api_base_url: &str,
    keys: &Keys,
    filters: &[serde_json::Value],
) -> ProjectViewReadResult<Vec<Event>> {
    query_relay_at_with_keys_and_client_typed(client, api_base_url, filters, keys, None)
        .await
        .map_err(map_project_view_http_error)
}

fn map_project_view_http_error(error: RelayHttpError) -> ProjectViewReadError {
    match error.category {
        RelayHttpErrorCategory::Forbidden => ProjectViewReadError::Forbidden,
        RelayHttpErrorCategory::Conflict => {
            conflict_error("Project View changed during snapshot pagination")
        }
        RelayHttpErrorCategory::Connect
        | RelayHttpErrorCategory::Timeout
        | RelayHttpErrorCategory::RateLimited
        | RelayHttpErrorCategory::Unavailable => ProjectViewReadError::Unavailable(error.message),
        RelayHttpErrorCategory::Http
            if error
                .status
                .is_some_and(|status| (500..=504).contains(&status)) =>
        {
            ProjectViewReadError::Unavailable(error.message)
        }
        RelayHttpErrorCategory::Http
        | RelayHttpErrorCategory::Malformed
        | RelayHttpErrorCategory::Internal => ProjectViewReadError::Other(error.message),
    }
}
fn conflict_error(message: impl Into<String>) -> ProjectViewReadError {
    ProjectViewReadError::Conflict(message.into())
}

fn integrity_read_error(message: impl Into<String>) -> ProjectViewReadError {
    ProjectViewReadError::Other(integrity_error(message))
}

fn integrity_error(message: impl Into<String>) -> String {
    format!("Project View integrity error: {}", message.into())
}

#[cfg(test)]
#[path = "project_view_tests.rs"]
mod tests;
