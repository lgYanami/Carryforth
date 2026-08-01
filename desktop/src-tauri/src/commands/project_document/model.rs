use buzz_core_pkg::{EventId, PublicKey};
use buzz_project_document_pkg::{
    DocumentHeadProjection, DocumentRevisionProjection, DocumentState,
};
use buzz_sdk_pkg::project_document::VerifiedDocumentRevision;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::relay::{RelayHttpError, RelayHttpErrorCategory};

/// Structured, body-free failure returned across the Tauri boundary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDocumentCommandError {
    pub(super) code: &'static str,
    pub(super) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) status: Option<u16>,
    pub(super) retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) retry_after_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) event_id: Option<String>,
}

impl ProjectDocumentCommandError {
    pub(super) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            status: None,
            retryable: false,
            retry_after_seconds: None,
            event_id: None,
        }
    }

    pub(super) fn unsupported() -> Self {
        Self::new(
            "unsupported",
            "This Community does not advertise Project Documents.",
        )
    }

    pub(super) fn restricted() -> Self {
        Self {
            status: Some(403),
            ..Self::new(
                "restricted",
                "Project Documents require current Community membership.",
            )
        }
    }

    pub(super) fn snapshot_conflict(message: impl Into<String>) -> Self {
        Self {
            status: Some(409),
            retryable: true,
            ..Self::new("snapshot_conflict", message)
        }
    }

    pub(super) fn internal(message: impl Into<String>) -> Self {
        Self::new(
            "internal",
            format!("Project Document integrity error: {}", message.into()),
        )
    }

    pub(super) fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    pub(super) fn delivery_unknown(event_id: EventId) -> Self {
        Self {
            retryable: false,
            event_id: Some(event_id.to_hex()),
            ..Self::new(
                "delivery_unknown",
                "The write may have reached the Relay, but exact revision read-back could not prove it. Do not submit a new edit until the current revision has been refreshed.",
            )
        }
    }

    pub(super) fn from_http(error: RelayHttpError, conflict_is_snapshot: bool) -> Self {
        match error.category {
            RelayHttpErrorCategory::Forbidden => Self::restricted(),
            RelayHttpErrorCategory::Conflict if conflict_is_snapshot => Self::snapshot_conflict(
                "The signed Project Document snapshot changed while it was being read.",
            ),
            RelayHttpErrorCategory::Conflict => Self {
                status: Some(409),
                ..Self::new(
                    "revision_conflict",
                    "The Document changed after this editor loaded.",
                )
            },
            RelayHttpErrorCategory::Connect
            | RelayHttpErrorCategory::Timeout
            | RelayHttpErrorCategory::RateLimited
            | RelayHttpErrorCategory::Unavailable => Self {
                code: "unavailable",
                message: "Project Documents are temporarily unavailable.".to_owned(),
                status: error.status,
                retryable: true,
                retry_after_seconds: error.retry_after_seconds,
                event_id: None,
            },
            RelayHttpErrorCategory::Http
            | RelayHttpErrorCategory::Malformed
            | RelayHttpErrorCategory::Internal => {
                Self::internal("the Relay returned an invalid Document response")
            }
        }
    }
}

impl From<String> for ProjectDocumentCommandError {
    fn from(message: String) -> Self {
        if message.starts_with("relay returned 403") {
            Self::restricted()
        } else if message.starts_with("relay unreachable:")
            || message.starts_with("relay rate-limited:")
        {
            Self {
                retryable: true,
                ..Self::new(
                    "unavailable",
                    "Project Documents are temporarily unavailable.",
                )
            }
        } else {
            Self::internal("the Relay identity could not be verified")
        }
    }
}

/// Verified identity and catalog observation used to bootstrap all UI queries.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDocumentMetaResult {
    pub(super) community_key: String,
    pub(super) project_id: Uuid,
    pub(super) relay_pubkey: String,
    pub(super) projection_generation: u64,
    pub(super) catalog_revision: u64,
    pub(super) active_document_count: u64,
    pub(super) updated_at: DateTime<Utc>,
    pub(super) meta_event_id: String,
}

/// One body-free active Document list item.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDocumentListItem {
    pub(super) document_id: Uuid,
    pub(super) title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) summary: Option<String>,
    pub(super) document_revision: u64,
    pub(super) updated_at: DateTime<Utc>,
    pub(super) updated_by: String,
    pub(super) head_event_id: String,
}

/// A verified metadata-only catalog response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDocumentListResult {
    pub(super) community_key: String,
    pub(super) project_id: Uuid,
    pub(super) relay_pubkey: String,
    pub(super) projection_generation: u64,
    pub(super) catalog_revision: u64,
    pub(super) documents: Vec<ProjectDocumentListItem>,
}

/// Identity fields copied from one verified metadata bootstrap.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectDocumentIdentityInput {
    pub(super) community_key: String,
    pub(super) project_id: Uuid,
    pub(super) relay_pubkey: String,
    pub(super) projection_generation: u64,
}

/// Input for the generation- and catalog-pinned metadata list.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListProjectDocumentsInput {
    #[serde(flatten)]
    pub(super) identity: ProjectDocumentIdentityInput,
    pub(super) catalog_revision: u64,
}

/// Select current or one immutable pinned revision.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetProjectDocumentInput {
    #[serde(flatten)]
    pub(super) identity: ProjectDocumentIdentityInput,
    pub(super) document_id: Uuid,
    pub(super) revision: Option<u64>,
}

/// Verified active snapshot or bodyless tombstone returned to TypeScript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDocumentReadResult {
    pub(super) community_key: String,
    pub(super) project_id: Uuid,
    pub(super) relay_pubkey: String,
    pub(super) projection_generation: u64,
    pub(super) document_id: Uuid,
    pub(super) document_revision: u64,
    pub(super) state: DocumentState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) content_markdown: Option<String>,
    pub(super) created_at: DateTime<Utc>,
    pub(super) created_by: String,
    pub(super) revision_at: DateTime<Utc>,
    pub(super) revision_by: String,
    pub(super) revision_event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) head_event_id: Option<String>,
    pub(super) source_event_id: String,
}

/// Input for a history snapshot pinned to one observed current revision.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetProjectDocumentHistoryInput {
    #[serde(flatten)]
    pub(super) identity: ProjectDocumentIdentityInput,
    pub(super) document_id: Uuid,
    pub(super) max_document_revision: u64,
}

/// Body-free metadata for one immutable revision.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDocumentHistoryItem {
    pub(super) document_revision: u64,
    pub(super) state: DocumentState,
    pub(super) actor: String,
    pub(super) canonical_at: DateTime<Utc>,
    pub(super) revision_event_id: String,
}

/// Verified complete revision history metadata.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDocumentHistoryResult {
    pub(super) community_key: String,
    pub(super) project_id: Uuid,
    pub(super) relay_pubkey: String,
    pub(super) projection_generation: u64,
    pub(super) document_id: Uuid,
    pub(super) max_document_revision: u64,
    pub(super) revisions: Vec<ProjectDocumentHistoryItem>,
}

/// Closed full-snapshot mutation accepted from the desktop editor.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectDocumentMutation {
    Create {
        document_id: Option<Uuid>,
        title: String,
        summary: Option<String>,
        content_markdown: String,
    },
    Update {
        document_id: Uuid,
        expected_document_revision: u64,
        title: String,
        summary: Option<String>,
        content_markdown: String,
    },
    Delete {
        document_id: Uuid,
        expected_document_revision: u64,
    },
}

/// Input for one verified native mutation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutateProjectDocumentInput {
    #[serde(flatten)]
    pub(super) identity: ProjectDocumentIdentityInput,
    pub(super) mutation: ProjectDocumentMutation,
}

/// Applied or definitive revision-conflict mutation result.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProjectDocumentMutationResult {
    Applied {
        #[serde(rename = "communityKey")]
        community_key: String,
        #[serde(rename = "documentId")]
        document_id: Uuid,
        #[serde(rename = "documentRevision")]
        document_revision: u64,
        #[serde(rename = "catalogRevision")]
        catalog_revision: u64,
        #[serde(rename = "eventId")]
        event_id: String,
        confirmation: &'static str,
        state: DocumentState,
    },
    Conflict {
        #[serde(rename = "communityKey")]
        community_key: String,
        #[serde(rename = "documentId")]
        document_id: Uuid,
        #[serde(rename = "expectedDocumentRevision")]
        expected_document_revision: u64,
        #[serde(rename = "currentDocumentRevision")]
        #[serde(skip_serializing_if = "Option::is_none")]
        current_document_revision: Option<u64>,
    },
}

pub(super) fn history_item(revision: &VerifiedDocumentRevision) -> ProjectDocumentHistoryItem {
    match &revision.projection {
        DocumentRevisionProjection::Active {
            document_revision,
            revision_at,
            revision_by,
            ..
        }
        | DocumentRevisionProjection::Deleted {
            document_revision,
            revision_at,
            revision_by,
            ..
        } => ProjectDocumentHistoryItem {
            document_revision: *document_revision,
            state: revision.projection.state(),
            actor: revision_by.to_hex(),
            canonical_at: *revision_at,
            revision_event_id: revision.event_id.to_hex(),
        },
    }
}

pub(super) fn head_revision_event_id(projection: &DocumentHeadProjection) -> EventId {
    match projection {
        DocumentHeadProjection::Active {
            revision_event_id, ..
        }
        | DocumentHeadProjection::Deleted {
            revision_event_id, ..
        } => *revision_event_id,
    }
}

pub(super) fn head_projection_generation(projection: &DocumentHeadProjection) -> u64 {
    match projection {
        DocumentHeadProjection::Active {
            projection_generation,
            ..
        }
        | DocumentHeadProjection::Deleted {
            projection_generation,
            ..
        } => *projection_generation,
    }
}

pub(super) fn head_catalog_revision(projection: &DocumentHeadProjection) -> u64 {
    match projection {
        DocumentHeadProjection::Active {
            catalog_revision, ..
        }
        | DocumentHeadProjection::Deleted {
            catalog_revision, ..
        } => *catalog_revision,
    }
}

pub(super) fn head_document_revision(projection: &DocumentHeadProjection) -> u64 {
    match projection {
        DocumentHeadProjection::Active {
            document_revision, ..
        }
        | DocumentHeadProjection::Deleted {
            document_revision, ..
        } => *document_revision,
    }
}

pub(super) fn revision_document_id(projection: &DocumentRevisionProjection) -> Uuid {
    match projection {
        DocumentRevisionProjection::Active { document_id, .. }
        | DocumentRevisionProjection::Deleted { document_id, .. } => *document_id,
    }
}

pub(super) fn revision_document_revision(projection: &DocumentRevisionProjection) -> u64 {
    match projection {
        DocumentRevisionProjection::Active {
            document_revision, ..
        }
        | DocumentRevisionProjection::Deleted {
            document_revision, ..
        } => *document_revision,
    }
}

pub(super) fn revision_projection_generation(projection: &DocumentRevisionProjection) -> u64 {
    match projection {
        DocumentRevisionProjection::Active {
            projection_generation,
            ..
        }
        | DocumentRevisionProjection::Deleted {
            projection_generation,
            ..
        } => *projection_generation,
    }
}

pub(super) fn revision_catalog_revision(projection: &DocumentRevisionProjection) -> u64 {
    match projection {
        DocumentRevisionProjection::Active {
            catalog_revision, ..
        }
        | DocumentRevisionProjection::Deleted {
            catalog_revision, ..
        } => *catalog_revision,
    }
}

pub(super) fn revision_actor_and_at(
    projection: &DocumentRevisionProjection,
) -> (PublicKey, DateTime<Utc>) {
    match projection {
        DocumentRevisionProjection::Active {
            revision_by,
            revision_at,
            ..
        }
        | DocumentRevisionProjection::Deleted {
            revision_by,
            revision_at,
            ..
        } => (*revision_by, *revision_at),
    }
}

pub(super) fn revision_source_event_id(projection: &DocumentRevisionProjection) -> EventId {
    match projection {
        DocumentRevisionProjection::Active {
            source_event_id, ..
        }
        | DocumentRevisionProjection::Deleted {
            source_event_id, ..
        } => *source_event_id,
    }
}
