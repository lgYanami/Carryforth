//! Verified Project Document v1 boundary for the desktop client.

use std::collections::HashSet;

use buzz_core_pkg::kind::{
    KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META, KIND_PROJECT_DOCUMENT_REVISION,
};
use buzz_core_pkg::{CommunityId, EventId, PublicKey};
use buzz_project_document_pkg::{
    DocumentCommandRequest, DocumentHeadProjection, DocumentOperation, DocumentRevisionProjection,
    DocumentState, ProjectDocumentCommand, ProjectDocumentReceipt,
};
use buzz_sdk_pkg::project_document::{
    build_document_command, document_head_coordinate, document_revision_coordinate,
    parse_document_head, parse_document_meta, parse_document_revision, VerifiedDocumentHead,
    VerifiedDocumentMeta, VerifiedDocumentRevision,
};
use nostr::{Event, Keys};
use serde_json::json;
use tauri::State;
use uuid::Uuid;

use super::project_view::read_identity_at;
use crate::app_state::AppState;
use crate::relay::{
    query_relay_at_with_keys_typed, relay_api_base_url_with_override,
    submit_signed_event_at_with_keys_typed, RelayHttpErrorCategory, SubmitEventResponse,
};

const ACTIVE_PAGE_SIZE: u16 = 100;
const HISTORY_PAGE_SIZE: u16 = 20;

#[derive(Debug, Clone)]
struct DocumentContext {
    community_key: String,
    api_base_url: String,
    keys: Keys,
    relay_pubkey: PublicKey,
}

mod model;
pub use model::*;

/// Read and verify the active Community's Project Document catalog metadata.
#[tauri::command]
pub async fn get_project_document_meta(
    community_key: String,
    state: State<'_, AppState>,
) -> Result<ProjectDocumentMetaResult, ProjectDocumentCommandError> {
    let context = capture_context(community_key, &state).await?;
    let meta = read_meta(&state, &context).await?;
    Ok(meta_result(&context, &meta))
}

/// List active Document metadata without loading any Markdown body.
#[tauri::command]
pub async fn list_project_documents(
    input: ListProjectDocumentsInput,
    state: State<'_, AppState>,
) -> Result<ProjectDocumentListResult, ProjectDocumentCommandError> {
    let context = capture_context(input.identity.community_key.clone(), &state).await?;
    verify_expected_identity(&context, &input.identity)?;
    let meta = read_meta(&state, &context).await?;
    verify_meta_pin(&meta, &input.identity, Some(input.catalog_revision))?;
    let documents = read_active_list(&state, &context, &meta).await?;
    Ok(ProjectDocumentListResult {
        community_key: context.community_key,
        project_id: meta.projection.project_id,
        relay_pubkey: context.relay_pubkey.to_hex(),
        projection_generation: meta.projection.projection_generation,
        catalog_revision: meta.projection.catalog_revision,
        documents,
    })
}

/// Read and verify current or immutable pinned Document content.
#[tauri::command]
pub async fn get_project_document(
    input: GetProjectDocumentInput,
    state: State<'_, AppState>,
) -> Result<ProjectDocumentReadResult, ProjectDocumentCommandError> {
    let context = capture_context(input.identity.community_key.clone(), &state).await?;
    verify_expected_identity(&context, &input.identity)?;
    let meta = read_meta(&state, &context).await?;
    verify_meta_pin(&meta, &input.identity, None)?;
    read_document(&state, &context, &meta, input.document_id, input.revision).await
}

/// Read a complete, body-free revision history snapshot.
#[tauri::command]
pub async fn get_project_document_history(
    input: GetProjectDocumentHistoryInput,
    state: State<'_, AppState>,
) -> Result<ProjectDocumentHistoryResult, ProjectDocumentCommandError> {
    let context = capture_context(input.identity.community_key.clone(), &state).await?;
    verify_expected_identity(&context, &input.identity)?;
    let meta = read_meta(&state, &context).await?;
    verify_meta_pin(&meta, &input.identity, None)?;
    read_history(&state, &context, &meta, &input).await
}

/// Validate, exact-sign, submit, and read back one full-snapshot mutation.
#[tauri::command]
pub async fn mutate_project_document(
    input: MutateProjectDocumentInput,
    state: State<'_, AppState>,
) -> Result<ProjectDocumentMutationResult, ProjectDocumentCommandError> {
    execute_mutation(input, &state).await
}

async fn capture_context(
    community_key: String,
    state: &AppState,
) -> Result<DocumentContext, ProjectDocumentCommandError> {
    if community_key.trim().is_empty() {
        return Err(ProjectDocumentCommandError::internal(
            "the local Community key is empty",
        ));
    }
    // Capture every mutable workspace input before the first await. A switch
    // during identity discovery cannot retarget this operation.
    let api_base_url = relay_api_base_url_with_override(state);
    let keys = state
        .signing_keys()
        .map_err(ProjectDocumentCommandError::from)?;
    let identity = read_identity_at(state, &api_base_url)
        .await
        .map_err(ProjectDocumentCommandError::from)?
        .ok_or_else(ProjectDocumentCommandError::unsupported)?;
    if !identity.project_document_supported {
        return Err(ProjectDocumentCommandError::unsupported());
    }
    Ok(DocumentContext {
        community_key,
        api_base_url,
        keys,
        relay_pubkey: identity.relay_pubkey,
    })
}

fn verify_expected_identity(
    context: &DocumentContext,
    expected: &ProjectDocumentIdentityInput,
) -> Result<(), ProjectDocumentCommandError> {
    if expected.community_key != context.community_key
        || expected.relay_pubkey != context.relay_pubkey.to_hex()
    {
        return Err(ProjectDocumentCommandError::snapshot_conflict(
            "The Community or verified Relay identity changed; refresh Documents.",
        ));
    }
    Ok(())
}

fn verify_meta_pin(
    meta: &VerifiedDocumentMeta,
    expected: &ProjectDocumentIdentityInput,
    catalog_revision: Option<u64>,
) -> Result<(), ProjectDocumentCommandError> {
    if meta.projection.project_id != expected.project_id
        || meta.projection.projection_generation != expected.projection_generation
        || catalog_revision.is_some_and(|revision| revision != meta.projection.catalog_revision)
    {
        return Err(ProjectDocumentCommandError::snapshot_conflict(
            "The signed Project Document identity or catalog changed; refresh Documents.",
        ));
    }
    Ok(())
}

async fn query(
    state: &AppState,
    context: &DocumentContext,
    filter: serde_json::Value,
    conflict_is_snapshot: bool,
) -> Result<Vec<Event>, ProjectDocumentCommandError> {
    query_relay_at_with_keys_typed(state, &context.api_base_url, &[filter], &context.keys, None)
        .await
        .map_err(|error| ProjectDocumentCommandError::from_http(error, conflict_is_snapshot))
}

async fn read_meta(
    state: &AppState,
    context: &DocumentContext,
) -> Result<VerifiedDocumentMeta, ProjectDocumentCommandError> {
    let events = query(
        state,
        context,
        json!({
            "kinds": [KIND_PROJECT_DOCUMENT_META],
            "authors": [context.relay_pubkey.to_hex()],
            "#t": ["buzz-project-document-meta"],
            "limit": 2,
        }),
        true,
    )
    .await?;
    let [event] = events.as_slice() else {
        return Err(ProjectDocumentCommandError::internal(
            "metadata query did not return exactly one event",
        ));
    };
    parse_document_meta(event, &context.relay_pubkey)
        .map_err(|error| ProjectDocumentCommandError::internal(error.to_string()))
}

fn meta_result(
    context: &DocumentContext,
    meta: &VerifiedDocumentMeta,
) -> ProjectDocumentMetaResult {
    ProjectDocumentMetaResult {
        community_key: context.community_key.clone(),
        project_id: meta.projection.project_id,
        relay_pubkey: context.relay_pubkey.to_hex(),
        projection_generation: meta.projection.projection_generation,
        catalog_revision: meta.projection.catalog_revision,
        active_document_count: meta.projection.active_document_count,
        updated_at: meta.projection.updated_at,
        meta_event_id: meta.event_id.to_hex(),
    }
}

async fn read_active_list(
    state: &AppState,
    context: &DocumentContext,
    meta: &VerifiedDocumentMeta,
) -> Result<Vec<ProjectDocumentListItem>, ProjectDocumentCommandError> {
    let project_id = CommunityId::from_uuid(meta.projection.project_id);
    let mut after_document_id: Option<Uuid> = None;
    let mut seen = HashSet::new();
    let mut documents = Vec::new();
    loop {
        let mut extension = json!({
            "scope": "active_heads",
            "projection_generation": meta.projection.projection_generation,
            "catalog_revision": meta.projection.catalog_revision,
        });
        if let Some(after) = after_document_id {
            extension["after_document_id"] = json!(after);
        }
        let events = query(
            state,
            context,
            json!({
                "kinds": [KIND_PROJECT_DOCUMENT_HEAD],
                "authors": [context.relay_pubkey.to_hex()],
                "#t": ["buzz-project-document-head"],
                "limit": ACTIVE_PAGE_SIZE,
                "buzz_project_document": extension,
            }),
            true,
        )
        .await?;
        if events.len() > usize::from(ACTIVE_PAGE_SIZE) {
            return Err(ProjectDocumentCommandError::internal(
                "active catalog page exceeded its requested limit",
            ));
        }
        let page_len = events.len();
        for event in events {
            let head = parse_document_head(&event, &context.relay_pubkey, project_id)
                .map_err(|error| ProjectDocumentCommandError::internal(error.to_string()))?;
            let DocumentHeadProjection::Active {
                projection_generation,
                catalog_revision,
                document_id,
                document_revision,
                title,
                summary,
                updated_at,
                updated_by,
                ..
            } = head.projection
            else {
                return Err(ProjectDocumentCommandError::internal(
                    "active catalog query returned a tombstone",
                ));
            };
            if projection_generation != meta.projection.projection_generation
                || catalog_revision > meta.projection.catalog_revision
                || after_document_id.is_some_and(|after| document_id <= after)
                || !seen.insert(document_id)
            {
                return Err(ProjectDocumentCommandError::internal(
                    "active catalog page violates its signed pin or UUID order",
                ));
            }
            after_document_id = Some(document_id);
            documents.push(ProjectDocumentListItem {
                document_id,
                title,
                summary,
                document_revision,
                updated_at,
                updated_by: updated_by.to_hex(),
                head_event_id: head.event_id.to_hex(),
            });
        }
        if page_len < usize::from(ACTIVE_PAGE_SIZE) {
            break;
        }
    }
    if u64::try_from(documents.len()).ok() != Some(meta.projection.active_document_count) {
        return Err(ProjectDocumentCommandError::internal(
            "active Document count differs from signed catalog metadata",
        ));
    }
    let after = read_meta(state, context).await?;
    if after.event_id != meta.event_id {
        return Err(ProjectDocumentCommandError::snapshot_conflict(
            "The signed catalog changed during pagination.",
        ));
    }
    Ok(documents)
}

async fn read_head(
    state: &AppState,
    context: &DocumentContext,
    project_id: CommunityId,
    document_id: Uuid,
) -> Result<Option<VerifiedDocumentHead>, ProjectDocumentCommandError> {
    let events = query(
        state,
        context,
        json!({
            "kinds": [KIND_PROJECT_DOCUMENT_HEAD],
            "authors": [context.relay_pubkey.to_hex()],
            "#d": [document_head_coordinate(project_id, document_id)],
            "#t": ["buzz-project-document-head"],
            "limit": 2,
        }),
        false,
    )
    .await?;
    match events.as_slice() {
        [] => Ok(None),
        [event] => parse_document_head(event, &context.relay_pubkey, project_id)
            .map(Some)
            .map_err(|error| ProjectDocumentCommandError::internal(error.to_string())),
        _ => Err(ProjectDocumentCommandError::internal(
            "head coordinate returned multiple current events",
        )),
    }
}

async fn read_revision(
    state: &AppState,
    context: &DocumentContext,
    project_id: CommunityId,
    document_id: Uuid,
    document_revision: u64,
) -> Result<VerifiedDocumentRevision, ProjectDocumentCommandError> {
    let events = query(
        state,
        context,
        json!({
            "kinds": [KIND_PROJECT_DOCUMENT_REVISION],
            "authors": [context.relay_pubkey.to_hex()],
            "#d": [document_revision_coordinate(project_id, document_id, document_revision)],
            "#t": ["buzz-project-document-revision"],
            "limit": 2,
        }),
        false,
    )
    .await?;
    let [event] = events.as_slice() else {
        return if events.is_empty() {
            Err(ProjectDocumentCommandError::not_found(format!(
                "Document revision {document_revision} was not found."
            )))
        } else {
            Err(ProjectDocumentCommandError::internal(
                "revision coordinate returned multiple events",
            ))
        };
    };
    let revision = parse_document_revision(event, &context.relay_pubkey, project_id)
        .map_err(|error| ProjectDocumentCommandError::internal(error.to_string()))?;
    if revision_document_id(&revision.projection) != document_id
        || revision_document_revision(&revision.projection) != document_revision
    {
        return Err(ProjectDocumentCommandError::internal(
            "revision does not match its requested coordinate",
        ));
    }
    Ok(revision)
}

async fn read_revision_by_event_id(
    state: &AppState,
    context: &DocumentContext,
    project_id: CommunityId,
    event_id: EventId,
) -> Result<VerifiedDocumentRevision, ProjectDocumentCommandError> {
    let events = query(
        state,
        context,
        json!({
            "ids": [event_id.to_hex()],
            "kinds": [KIND_PROJECT_DOCUMENT_REVISION],
            "authors": [context.relay_pubkey.to_hex()],
            "limit": 2,
        }),
        false,
    )
    .await?;
    let [event] = events.as_slice() else {
        return Err(ProjectDocumentCommandError::internal(
            "head revision pointer did not resolve exactly once",
        ));
    };
    if event.id != event_id {
        return Err(ProjectDocumentCommandError::internal(
            "revision query returned an event other than the head pointer",
        ));
    }
    parse_document_revision(event, &context.relay_pubkey, project_id)
        .map_err(|error| ProjectDocumentCommandError::internal(error.to_string()))
}

async fn read_document(
    state: &AppState,
    context: &DocumentContext,
    meta: &VerifiedDocumentMeta,
    document_id: Uuid,
    requested_revision: Option<u64>,
) -> Result<ProjectDocumentReadResult, ProjectDocumentCommandError> {
    let project_id = CommunityId::from_uuid(meta.projection.project_id);
    let (revision, head_event_id) = match requested_revision {
        Some(number) => (
            read_revision(state, context, project_id, document_id, number).await?,
            None,
        ),
        None => {
            let head = read_head(state, context, project_id, document_id)
                .await?
                .ok_or_else(|| ProjectDocumentCommandError::not_found("Document was not found."))?;
            let revision = read_revision_by_event_id(
                state,
                context,
                project_id,
                head_revision_event_id(&head.projection),
            )
            .await?;
            buzz_sdk_pkg::project_document::VerifiedCurrentDocument::new(
                head.clone(),
                revision.clone(),
            )
            .map_err(|error| ProjectDocumentCommandError::internal(error.to_string()))?;
            (revision, Some(head.event_id.to_hex()))
        }
    };
    if revision_document_id(&revision.projection) != document_id
        || revision_projection_generation(&revision.projection)
            != meta.projection.projection_generation
    {
        return Err(ProjectDocumentCommandError::internal(
            "revision belongs to another Document or projection generation",
        ));
    }
    if revision_catalog_revision(&revision.projection) > meta.projection.catalog_revision {
        return Err(ProjectDocumentCommandError::snapshot_conflict(
            "The signed Document revision is newer than the catalog snapshot; refresh Documents.",
        ));
    }
    Ok(read_result(context, meta, revision, head_event_id))
}

fn read_result(
    context: &DocumentContext,
    meta: &VerifiedDocumentMeta,
    revision: VerifiedDocumentRevision,
    head_event_id: Option<String>,
) -> ProjectDocumentReadResult {
    let common = (
        context.community_key.clone(),
        meta.projection.project_id,
        context.relay_pubkey.to_hex(),
        meta.projection.projection_generation,
        revision.event_id.to_hex(),
    );
    match revision.projection {
        DocumentRevisionProjection::Active {
            document_id,
            document_revision,
            title,
            summary,
            content_markdown,
            created_at,
            created_by,
            revision_at,
            revision_by,
            source_event_id,
            ..
        } => ProjectDocumentReadResult {
            community_key: common.0,
            project_id: common.1,
            relay_pubkey: common.2,
            projection_generation: common.3,
            document_id,
            document_revision,
            state: DocumentState::Active,
            title: Some(title),
            summary,
            content_markdown: Some(content_markdown),
            created_at,
            created_by: created_by.to_hex(),
            revision_at,
            revision_by: revision_by.to_hex(),
            revision_event_id: common.4,
            head_event_id,
            source_event_id: source_event_id.to_hex(),
        },
        DocumentRevisionProjection::Deleted {
            document_id,
            document_revision,
            created_at,
            created_by,
            revision_at,
            revision_by,
            source_event_id,
            ..
        } => ProjectDocumentReadResult {
            community_key: common.0,
            project_id: common.1,
            relay_pubkey: common.2,
            projection_generation: common.3,
            document_id,
            document_revision,
            state: DocumentState::Deleted,
            title: None,
            summary: None,
            content_markdown: None,
            created_at,
            created_by: created_by.to_hex(),
            revision_at,
            revision_by: revision_by.to_hex(),
            revision_event_id: common.4,
            head_event_id,
            source_event_id: source_event_id.to_hex(),
        },
    }
}

async fn read_history(
    state: &AppState,
    context: &DocumentContext,
    meta: &VerifiedDocumentMeta,
    input: &GetProjectDocumentHistoryInput,
) -> Result<ProjectDocumentHistoryResult, ProjectDocumentCommandError> {
    let project_id = CommunityId::from_uuid(meta.projection.project_id);
    let head = read_head(state, context, project_id, input.document_id)
        .await?
        .ok_or_else(|| ProjectDocumentCommandError::not_found("Document was not found."))?;
    if head_projection_generation(&head.projection) != meta.projection.projection_generation
        || head_document_revision(&head.projection) != input.max_document_revision
        || head_catalog_revision(&head.projection) > meta.projection.catalog_revision
    {
        return Err(ProjectDocumentCommandError::snapshot_conflict(
            "The Document head changed before history could be pinned.",
        ));
    }
    let mut before_revision = None;
    let mut previous_revision = None;
    let mut revisions = Vec::new();
    loop {
        let mut extension = json!({
            "scope": "history",
            "projection_generation": meta.projection.projection_generation,
            "document_id": input.document_id,
            "max_document_revision": input.max_document_revision,
        });
        if let Some(before) = before_revision {
            extension["before_revision"] = json!(before);
        }
        let events = query(
            state,
            context,
            json!({
                "kinds": [KIND_PROJECT_DOCUMENT_REVISION],
                "authors": [context.relay_pubkey.to_hex()],
                "#t": ["buzz-project-document-revision"],
                "limit": HISTORY_PAGE_SIZE,
                "buzz_project_document": extension,
            }),
            true,
        )
        .await?;
        if events.len() > usize::from(HISTORY_PAGE_SIZE) {
            return Err(ProjectDocumentCommandError::internal(
                "history page exceeded its requested limit",
            ));
        }
        let page_len = events.len();
        for event in events {
            let revision = parse_document_revision(&event, &context.relay_pubkey, project_id)
                .map_err(|error| ProjectDocumentCommandError::internal(error.to_string()))?;
            let number = revision_document_revision(&revision.projection);
            if revision_document_id(&revision.projection) != input.document_id
                || revision_projection_generation(&revision.projection)
                    != meta.projection.projection_generation
                || revision_catalog_revision(&revision.projection)
                    > meta.projection.catalog_revision
                || number > input.max_document_revision
                || previous_revision.is_some_and(|previous| number >= previous)
            {
                return Err(ProjectDocumentCommandError::internal(
                    "history violates its pinned coordinate or revision order",
                ));
            }
            previous_revision = Some(number);
            before_revision = Some(number);
            revisions.push(history_item(&revision));
        }
        if page_len < usize::from(HISTORY_PAGE_SIZE) {
            break;
        }
    }
    if revisions.len() != usize::try_from(input.max_document_revision).unwrap_or(usize::MAX) {
        return Err(ProjectDocumentCommandError::internal(
            "history does not contain every immutable revision",
        ));
    }
    Ok(ProjectDocumentHistoryResult {
        community_key: context.community_key.clone(),
        project_id: meta.projection.project_id,
        relay_pubkey: context.relay_pubkey.to_hex(),
        projection_generation: meta.projection.projection_generation,
        document_id: input.document_id,
        max_document_revision: input.max_document_revision,
        revisions,
    })
}

async fn execute_mutation(
    input: MutateProjectDocumentInput,
    state: &AppState,
) -> Result<ProjectDocumentMutationResult, ProjectDocumentCommandError> {
    let context = capture_context(input.identity.community_key.clone(), state).await?;
    verify_expected_identity(&context, &input.identity)?;
    let meta = read_meta(state, &context).await?;
    verify_meta_pin(&meta, &input.identity, None)?;
    let command = mutation_command(input.mutation);
    command
        .validate_for_submission()
        .map_err(|error| ProjectDocumentCommandError::new("invalid_input", error.to_string()))?;
    let operation = command.operation();
    let document_id = command.document_id();
    let expected_revision = command.expected_document_revision;
    let committed_revision = expected_revision.checked_add(1).ok_or_else(|| {
        ProjectDocumentCommandError::new("invalid_input", "Document revision overflow")
    })?;
    let expected_state = if operation == DocumentOperation::Delete {
        DocumentState::Deleted
    } else {
        DocumentState::Active
    };
    let event = build_document_command(command)
        .map_err(|error| ProjectDocumentCommandError::new("invalid_input", error.to_string()))?
        .sign_with_keys(&context.keys)
        .map_err(|_| ProjectDocumentCommandError::internal("failed to sign mutation"))?;

    let response = match submit_signed_event_at_with_keys_typed(
        &event,
        state,
        &context.api_base_url,
        &context.keys,
    )
    .await
    {
        Ok(response) => response,
        Err(error) if error.category == RelayHttpErrorCategory::Conflict => {
            return Ok(
                conflict_result(state, &context, &meta, document_id, expected_revision).await,
            );
        }
        Err(error) if error.request_may_have_reached_relay => {
            if let Ok(catalog_revision) = confirm_write(
                state,
                &context,
                document_id,
                committed_revision,
                expected_state,
                event.id,
                None,
            )
            .await
            {
                return Ok(applied_result(
                    &context,
                    document_id,
                    committed_revision,
                    catalog_revision,
                    event.id,
                    expected_state,
                    "readback",
                ));
            }
            return Err(ProjectDocumentCommandError::delivery_unknown(event.id));
        }
        Err(first) if first.category == RelayHttpErrorCategory::Connect => {
            match submit_signed_event_at_with_keys_typed(
                &event,
                state,
                &context.api_base_url,
                &context.keys,
            )
            .await
            {
                Ok(response) => response,
                Err(error) if error.category == RelayHttpErrorCategory::Conflict => {
                    return Ok(conflict_result(
                        state,
                        &context,
                        &meta,
                        document_id,
                        expected_revision,
                    )
                    .await);
                }
                Err(error) if error.request_may_have_reached_relay => {
                    if let Ok(catalog_revision) = confirm_write(
                        state,
                        &context,
                        document_id,
                        committed_revision,
                        expected_state,
                        event.id,
                        None,
                    )
                    .await
                    {
                        return Ok(applied_result(
                            &context,
                            document_id,
                            committed_revision,
                            catalog_revision,
                            event.id,
                            expected_state,
                            "readback",
                        ));
                    }
                    return Err(ProjectDocumentCommandError::delivery_unknown(event.id));
                }
                Err(error) => {
                    return Err(ProjectDocumentCommandError::from_http(error, false));
                }
            }
        }
        Err(error) => return Err(ProjectDocumentCommandError::from_http(error, false)),
    };

    let receipt = parse_receipt(&response, &event)?;
    validate_receipt(
        &receipt,
        &event,
        operation,
        document_id,
        expected_revision,
        committed_revision,
        expected_state,
    )?;
    let catalog_revision = confirm_write(
        state,
        &context,
        document_id,
        committed_revision,
        expected_state,
        event.id,
        Some(&receipt),
    )
    .await?;
    Ok(applied_result(
        &context,
        document_id,
        committed_revision,
        catalog_revision,
        event.id,
        expected_state,
        "receipt_and_readback",
    ))
}

fn mutation_command(mutation: ProjectDocumentMutation) -> ProjectDocumentCommand {
    match mutation {
        ProjectDocumentMutation::Create {
            document_id,
            title,
            summary,
            content_markdown,
        } => ProjectDocumentCommand::new(
            0,
            DocumentCommandRequest::Create {
                document_id: document_id.unwrap_or_else(Uuid::new_v4),
                title,
                summary,
                content_markdown,
            },
        ),
        ProjectDocumentMutation::Update {
            document_id,
            expected_document_revision,
            title,
            summary,
            content_markdown,
        } => ProjectDocumentCommand::new(
            expected_document_revision,
            DocumentCommandRequest::Update {
                document_id,
                title,
                summary,
                content_markdown,
            },
        ),
        ProjectDocumentMutation::Delete {
            document_id,
            expected_document_revision,
        } => ProjectDocumentCommand::new(
            expected_document_revision,
            DocumentCommandRequest::Delete { document_id },
        ),
    }
}

fn parse_receipt(
    response: &SubmitEventResponse,
    event: &Event,
) -> Result<ProjectDocumentReceipt, ProjectDocumentCommandError> {
    if response.event_id != event.id.to_hex() {
        return Err(ProjectDocumentCommandError::internal(
            "mutation response event ID differs from the submitted event",
        ));
    }
    let payload = response.message.strip_prefix("response:").ok_or_else(|| {
        ProjectDocumentCommandError::internal(
            "mutation receipt is missing the canonical response prefix",
        )
    })?;
    serde_json::from_str(payload)
        .map_err(|_| ProjectDocumentCommandError::internal("mutation receipt is invalid"))
}

fn validate_receipt(
    receipt: &ProjectDocumentReceipt,
    event: &Event,
    operation: DocumentOperation,
    document_id: Uuid,
    expected_revision: u64,
    committed_revision: u64,
    state: DocumentState,
) -> Result<(), ProjectDocumentCommandError> {
    receipt
        .validate()
        .map_err(|error| ProjectDocumentCommandError::internal(error.to_string()))?;
    if receipt.change_id != event.id
        || receipt.actor != event.pubkey
        || receipt.operation != operation
        || receipt.document_id != document_id
        || receipt.expected_document_revision != expected_revision
        || receipt.document_revision != committed_revision
        || receipt.state != state
    {
        return Err(ProjectDocumentCommandError::internal(
            "Relay receipt does not match the submitted command",
        ));
    }
    Ok(())
}

async fn confirm_write(
    state: &AppState,
    context: &DocumentContext,
    document_id: Uuid,
    document_revision: u64,
    expected_state: DocumentState,
    source_event_id: EventId,
    receipt: Option<&ProjectDocumentReceipt>,
) -> Result<u64, ProjectDocumentCommandError> {
    let meta = read_meta(state, context).await?;
    let revision = read_revision(
        state,
        context,
        CommunityId::from_uuid(meta.projection.project_id),
        document_id,
        document_revision,
    )
    .await?;
    let catalog_revision = revision_catalog_revision(&revision.projection);
    if revision.projection.state() != expected_state
        || revision_projection_generation(&revision.projection)
            != meta.projection.projection_generation
        || catalog_revision > meta.projection.catalog_revision
        || revision_source_event_id(&revision.projection) != source_event_id
    {
        return Err(ProjectDocumentCommandError::internal(
            "immutable revision does not prove the submitted command was accepted",
        ));
    }
    if let Some(receipt) = receipt {
        let (actor, accepted_at) = revision_actor_and_at(&revision.projection);
        if catalog_revision != receipt.catalog_revision
            || actor != receipt.actor
            || accepted_at != receipt.accepted_at
        {
            return Err(ProjectDocumentCommandError::internal(
                "signed revision does not match the Relay receipt",
            ));
        }
    }
    Ok(catalog_revision)
}

async fn conflict_result(
    state: &AppState,
    context: &DocumentContext,
    meta: &VerifiedDocumentMeta,
    document_id: Uuid,
    expected_document_revision: u64,
) -> ProjectDocumentMutationResult {
    let current_document_revision = read_head(
        state,
        context,
        CommunityId::from_uuid(meta.projection.project_id),
        document_id,
    )
    .await
    .ok()
    .flatten()
    .map(|head| head_document_revision(&head.projection));
    ProjectDocumentMutationResult::Conflict {
        community_key: context.community_key.clone(),
        document_id,
        expected_document_revision,
        current_document_revision,
    }
}

fn applied_result(
    context: &DocumentContext,
    document_id: Uuid,
    document_revision: u64,
    catalog_revision: u64,
    event_id: EventId,
    state: DocumentState,
    confirmation: &'static str,
) -> ProjectDocumentMutationResult {
    ProjectDocumentMutationResult::Applied {
        community_key: context.community_key.clone(),
        document_id,
        document_revision,
        catalog_revision,
        event_id: event_id.to_hex(),
        confirmation,
        state,
    }
}

#[cfg(test)]
#[path = "project_document_tests.rs"]
mod tests;
