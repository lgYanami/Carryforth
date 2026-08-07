use buzz_project_view_pkg::ProjectViewObjectType;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::relay::{RelayHttpError, RelayHttpErrorCategory};

/// Structured, body-free failure returned by the Project Context read boundary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextCommandError {
    pub(super) code: &'static str,
    pub(super) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) status: Option<u16>,
    pub(super) retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) retry_after_seconds: Option<u64>,
}

impl ProjectContextCommandError {
    pub(super) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            status: None,
            retryable: false,
            retry_after_seconds: None,
        }
    }

    pub(super) fn unsupported(message: impl Into<String>) -> Self {
        Self::new("unsupported", message)
    }

    pub(super) fn restricted() -> Self {
        Self {
            status: Some(403),
            ..Self::new(
                "restricted",
                "Project Context requires current Community membership.",
            )
        }
    }

    pub(super) fn unavailable(message: impl Into<String>) -> Self {
        Self {
            retryable: true,
            ..Self::new("unavailable", message)
        }
    }

    pub(super) fn snapshot_conflict(message: impl Into<String>) -> Self {
        Self {
            status: Some(409),
            retryable: true,
            ..Self::new("snapshot_conflict", message)
        }
    }

    pub(super) fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("invalid_input", message)
    }

    pub(super) fn verification_failed(message: impl Into<String>) -> Self {
        Self::new(
            "verification_failed",
            format!("Project Context verification failed: {}", message.into()),
        )
    }

    pub(super) fn internal() -> Self {
        Self::new(
            "internal",
            "Project Context could not be read because of an internal Desktop error.",
        )
    }

    pub(super) fn from_http(error: RelayHttpError) -> Self {
        match error.category {
            RelayHttpErrorCategory::Forbidden => Self::restricted(),
            RelayHttpErrorCategory::Conflict => Self::snapshot_conflict(
                "The signed Project Context snapshot changed while it was being read.",
            ),
            RelayHttpErrorCategory::Connect
            | RelayHttpErrorCategory::Timeout
            | RelayHttpErrorCategory::RateLimited
            | RelayHttpErrorCategory::Unavailable => Self {
                status: error.status,
                retry_after_seconds: error.retry_after_seconds,
                ..Self::unavailable("Project Context is temporarily unavailable.")
            },
            RelayHttpErrorCategory::Http
                if error
                    .status
                    .is_some_and(|status| (500..=504).contains(&status)) =>
            {
                Self {
                    status: error.status,
                    retry_after_seconds: error.retry_after_seconds,
                    ..Self::unavailable("Project Context is temporarily unavailable.")
                }
            }
            RelayHttpErrorCategory::Malformed => {
                Self::verification_failed("the Relay returned a malformed query response")
            }
            RelayHttpErrorCategory::Http | RelayHttpErrorCategory::Internal => Self::internal(),
        }
    }

    pub(super) fn from_identity_error(message: &str) -> Self {
        if message.starts_with("relay returned 403") {
            Self::restricted()
        } else if message.starts_with("relay unreachable:")
            || message.starts_with("relay rate-limited:")
            || message.starts_with("relay returned 409")
            || message.starts_with("relay returned 5")
        {
            Self::unavailable("Project Context is temporarily unavailable.")
        } else if message.starts_with("Project View integrity error:") {
            Self::verification_failed("the Relay identity document is invalid")
        } else {
            Self::internal()
        }
    }

    pub(super) fn hydration_can_degrade(&self) -> bool {
        matches!(self.code, "unavailable" | "snapshot_conflict")
    }
}

/// One closed Project Context coordinate accepted and returned by Desktop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProjectContextCoordinateDto {
    /// Stable Project View object identity.
    ProjectViewObject {
        object_type: ProjectViewObjectType,
        object_id: Uuid,
    },
    /// Stable Project Document identity.
    Document { document_id: Uuid },
    /// Stable terminal Meeting identity.
    Meeting { meeting_id: Uuid },
}

/// One of the three Project Context set queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProjectContextQueryDto {
    /// Match one exact canonical coordinate set.
    Exact {
        coordinates: Vec<ProjectContextCoordinateDto>,
    },
    /// Match every Edge containing one coordinate.
    Incident {
        coordinate: ProjectContextCoordinateDto,
    },
    /// Match every Edge containing all supplied coordinates; empty means all.
    ContainsAll {
        coordinates: Vec<ProjectContextCoordinateDto>,
    },
}

/// Input for one trusted Project Context query.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryProjectContextInput {
    pub(super) community_key: String,
    pub(super) query: ProjectContextQueryDto,
}

/// Source observation state for independent Project View and Document hydration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectContextSourceState {
    /// The source was not needed by this query result.
    NotRequested,
    /// A complete signed source observation supplied the details.
    Observed,
    /// The Edge structure is trusted but source details are temporarily unavailable.
    Unavailable,
}

/// Lifecycle state of one hydrated coordinate or Document identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectContextDetailState {
    /// The source object is active.
    Active,
    /// The Meeting has a verified immutable terminal outcome.
    Terminal,
    /// The source identity is retained as a tombstone.
    Tombstoned,
    /// The source identity could not currently be hydrated.
    Unavailable,
}

/// Independent verification state for one Meeting coordinate hydration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectContextMeetingObservationState {
    Observed,
    Unavailable,
    VerificationFailed,
}

/// Frozen participant preview carried by a metadata-first Meeting read.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextMeetingParticipant {
    pub(super) pubkey: String,
    pub(super) participant_type: String,
}

/// Bounded Action Finalization summary; Board and Speech remain on Meeting.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextMeetingActionSummary {
    pub(super) condition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) terminal_status: Option<String>,
    pub(super) actions_attested: bool,
}

/// Body-free Meeting metadata shown before opening the full Meeting route.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextMeetingDetail {
    pub(super) discussion_goal: Option<String>,
    pub(super) terminal_outcome: String,
    pub(super) host_pubkey: String,
    pub(super) participant_count: usize,
    pub(super) participant_preview: Vec<ProjectContextMeetingParticipant>,
    pub(super) created_at: DateTime<Utc>,
    pub(super) ended_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) action_finalization: Option<ProjectContextMeetingActionSummary>,
}

/// Verification evidence for one unique Meeting coordinate in the result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextMeetingObservation {
    pub(super) meeting_id: Uuid,
    pub(super) state: ProjectContextMeetingObservationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) state_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) create_event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) state_event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) end_event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) updated_at: Option<DateTime<Utc>>,
}

/// Signed Context catalog observation bounding every returned Edge.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextObservation {
    pub(super) context_revision: u64,
    pub(super) projection_generation: u64,
    pub(super) active_edge_count: u64,
    pub(super) bound_document_count: u64,
    pub(super) updated_at: DateTime<Utc>,
    pub(super) meta_event_id: String,
    pub(super) capability_enabled: bool,
}

/// Independent signed Project View observation used for coordinate details.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextProjectViewObservation {
    pub(super) state: ProjectContextSourceState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) project_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) projection_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) meta_event_id: Option<String>,
}

/// Independent signed Document catalog observation used for metadata details.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextDocumentObservation {
    pub(super) state: ProjectContextSourceState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) catalog_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) projection_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) meta_event_id: Option<String>,
}

/// One normalized active Context Edge without presentation-only graph state.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextEdgeDto {
    pub(super) edge_key: String,
    pub(super) coordinate_keys: Vec<String>,
    pub(super) context_document_ids: Vec<Uuid>,
}

/// One unique, body-free coordinate detail referenced by the result or query.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextCoordinateDetail {
    pub(super) coordinate_key: String,
    pub(super) coordinate: ProjectContextCoordinateDto,
    pub(super) state: ProjectContextDetailState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) status: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) object_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) document_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) meeting: Option<ProjectContextMeetingDetail>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) updated_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) unavailable_reason: Option<&'static str>,
}

/// One unique, body-free Document detail used by coordinates or Edge content.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextDocumentDetail {
    pub(super) document_id: Uuid,
    pub(super) state: ProjectContextDetailState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) document_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) updated_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) unavailable_reason: Option<&'static str>,
}

/// Complete trusted, body-free result returned to the Desktop graph client.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContextQueryResult {
    pub(super) community_key: String,
    pub(super) project_id: Uuid,
    pub(super) relay_pubkey: String,
    pub(super) context: ProjectContextObservation,
    pub(super) query: ProjectContextQueryDto,
    pub(super) project_view_observation: ProjectContextProjectViewObservation,
    pub(super) document_observation: ProjectContextDocumentObservation,
    pub(super) meeting_observations: Vec<ProjectContextMeetingObservation>,
    pub(super) edges: Vec<ProjectContextEdgeDto>,
    pub(super) coordinate_details: Vec<ProjectContextCoordinateDetail>,
    pub(super) document_details: Vec<ProjectContextDocumentDetail>,
}
