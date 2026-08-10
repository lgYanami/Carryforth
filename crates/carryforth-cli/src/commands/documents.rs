//! `cf documents` — verified Project Document v1 reads and writes.

use std::collections::HashSet;
use std::io::Write as _;
use std::time::Duration;

use buzz_core::kind::{
    KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META, KIND_PROJECT_DOCUMENT_REVISION,
};
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_document::{
    DocumentCommandRequest, DocumentHeadProjection, DocumentOperation, DocumentRevisionProjection,
    DocumentState, ProjectDocumentCommand, ProjectDocumentReceipt, MAX_COMMAND_CONTENT_BYTES,
    MAX_CONTENT_MARKDOWN_BYTES,
};
use buzz_sdk::project_document::{
    build_document_command, document_head_coordinate, document_revision_coordinate,
    parse_document_head, parse_document_meta, parse_document_revision, VerifiedDocumentHead,
    VerifiedDocumentMeta, VerifiedDocumentRevision,
};
use nostr::Event;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::client::{CarryforthClient, ProjectCommandDelivery};
use crate::error::CliError;
use crate::validate::{read_bounded_file_or_stdin, sdk_err};
use crate::{DocumentsCmd, OutputFormat};

const DOCUMENT_CAPABILITY: &str = "buzz-project-document-v1";
const ACTIVE_PAGE_SIZE: u16 = 100;
const HISTORY_PAGE_SIZE: u16 = 20;
const SNAPSHOT_ATTEMPTS: usize = 3;
const PATCH_INPUT_MAX_BYTES: usize = MAX_COMMAND_CONTENT_BYTES;
const SECRET_BOUNDARY_WARNING: &str = "warning: Project Documents are not a Secret Store; do not write passwords, tokens, private keys, or other credentials";

#[derive(Debug, Clone, Copy)]
struct DocumentIdentity {
    relay_pubkey: PublicKey,
}

struct PatchRequest {
    document_id: Uuid,
    expected_revision: u64,
    patch_file: String,
    output: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    clear_summary: bool,
}

#[derive(Deserialize)]
struct Nip11Document {
    #[serde(default)]
    supported_extensions: Vec<String>,
    #[serde(rename = "self")]
    relay_self: Option<String>,
}

#[derive(Serialize)]
struct DocumentListItem {
    document_id: Uuid,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    document_revision: u64,
    updated_at: chrono::DateTime<chrono::Utc>,
    updated_by: PublicKey,
    head_event_id: EventId,
}

#[derive(Serialize)]
pub(crate) struct DocumentReadOutput {
    pub(crate) document_id: Uuid,
    pub(crate) document_revision: u64,
    pub(crate) state: DocumentState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) content_markdown: Option<String>,
    pub(crate) created_at: chrono::DateTime<chrono::Utc>,
    pub(crate) created_by: PublicKey,
    pub(crate) revision_at: chrono::DateTime<chrono::Utc>,
    pub(crate) revision_by: PublicKey,
    pub(crate) revision_event_id: EventId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) head_event_id: Option<EventId>,
    pub(crate) source_event_id: EventId,
}

#[derive(Serialize)]
struct DocumentHistoryItem {
    document_revision: u64,
    state: DocumentState,
    actor: PublicKey,
    canonical_at: chrono::DateTime<chrono::Utc>,
    revision_event_id: EventId,
}

/// Dispatch one Project Document command.
pub async fn dispatch(
    command: DocumentsCmd,
    client: &CarryforthClient,
    format: &OutputFormat,
) -> Result<(), CliError> {
    match command {
        DocumentsCmd::List => cmd_list(client, format).await,
        DocumentsCmd::Get {
            document_id,
            revision,
            content_only,
        } => cmd_get(client, document_id, revision, content_only, format).await,
        DocumentsCmd::History { document_id } => cmd_history(client, document_id, format).await,
        DocumentsCmd::Create {
            title,
            summary,
            content,
            content_file,
            document_id,
        } => {
            let content_markdown = read_document_content(content, content_file)?;
            let document_id = document_id.unwrap_or_else(Uuid::new_v4);
            let command = ProjectDocumentCommand::new(
                0,
                DocumentCommandRequest::Create {
                    document_id,
                    title,
                    summary,
                    content_markdown,
                },
            );
            submit_write(client, command).await
        }
        DocumentsCmd::Update {
            document_id,
            expected_revision,
            title,
            summary,
            clear_summary,
            content,
            content_file,
        } => {
            let summary = complete_update_summary(summary, clear_summary)?;
            let content_markdown = read_document_content(content, content_file)?;
            let command = ProjectDocumentCommand::new(
                expected_revision,
                DocumentCommandRequest::Update {
                    document_id,
                    title,
                    summary,
                    content_markdown,
                },
            );
            submit_write(client, command).await
        }
        DocumentsCmd::Patch {
            document_id,
            expected_revision,
            patch_file,
            output,
            title,
            summary,
            clear_summary,
        } => {
            cmd_patch(
                client,
                PatchRequest {
                    document_id,
                    expected_revision,
                    patch_file,
                    output,
                    title,
                    summary,
                    clear_summary,
                },
            )
            .await
        }
        DocumentsCmd::Delete {
            document_id,
            expected_revision,
        } => {
            submit_write(
                client,
                ProjectDocumentCommand::new(
                    expected_revision,
                    DocumentCommandRequest::Delete { document_id },
                ),
            )
            .await
        }
    }
}

async fn require_identity(client: &CarryforthClient) -> Result<DocumentIdentity, CliError> {
    let raw = client.get_public("/info").await?;
    let info: Nip11Document = serde_json::from_str(&raw)
        .map_err(|_| integrity_error("Relay returned an invalid NIP-11 document"))?;
    if !info
        .supported_extensions
        .iter()
        .any(|extension| extension == DOCUMENT_CAPABILITY)
    {
        return Err(CliError::Other(format!(
            "unavailable: Relay does not advertise {DOCUMENT_CAPABILITY}"
        )));
    }
    let relay_self = info.relay_self.ok_or_else(|| {
        integrity_error("NIP-11 advertises Project Document without a relay `self` key")
    })?;
    let relay_pubkey = PublicKey::from_hex(&relay_self)
        .map_err(|_| integrity_error("NIP-11 relay `self` is invalid"))?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(integrity_error(
            "NIP-11 relay `self` is not canonical lowercase hex",
        ));
    }
    Ok(DocumentIdentity { relay_pubkey })
}

async fn read_meta(
    client: &CarryforthClient,
    identity: DocumentIdentity,
) -> Result<VerifiedDocumentMeta, CliError> {
    let values = query_values(
        client,
        json!({
            "kinds": [KIND_PROJECT_DOCUMENT_META],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": ["buzz-project-document-meta"],
            "limit": 2,
        }),
        "Document metadata",
    )
    .await?;
    let [value] = values.as_slice() else {
        return Err(integrity_error(
            "Document metadata query did not return exactly one event",
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|_| integrity_error("Document metadata event is invalid"))?;
    parse_document_meta(&event, &identity.relay_pubkey)
        .map_err(|error| integrity_error(error.to_string()))
}

async fn cmd_list(client: &CarryforthClient, format: &OutputFormat) -> Result<(), CliError> {
    let identity = require_identity(client).await?;
    for attempt in 0..SNAPSHOT_ATTEMPTS {
        match read_active_snapshot(client, identity).await {
            Ok(items) => return print_read_output(&items, format),
            Err(error) if is_snapshot_conflict(&error) && attempt + 1 < SNAPSHOT_ATTEMPTS => {
                tokio::time::sleep(Duration::from_millis(25_u64 << attempt)).await;
            }
            Err(error) if is_snapshot_conflict(&error) => {
                return Err(CliError::Conflict(
                    "Project Document catalog changed during every bounded list attempt".to_owned(),
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Err(CliError::Conflict(
        "Project Document list could not be stabilized".to_owned(),
    ))
}

async fn read_active_snapshot(
    client: &CarryforthClient,
    identity: DocumentIdentity,
) -> Result<Vec<DocumentListItem>, CliError> {
    let before = read_meta(client, identity).await?;
    let project_id = CommunityId::from_uuid(before.projection.project_id);
    let mut after_document_id: Option<Uuid> = None;
    let mut seen = HashSet::new();
    let mut items = Vec::new();

    loop {
        let mut extension = json!({
            "scope": "active_heads",
            "projection_generation": before.projection.projection_generation,
            "catalog_revision": before.projection.catalog_revision,
        });
        if let Some(after) = after_document_id {
            extension["after_document_id"] = json!(after);
        }
        let values = query_values(
            client,
            json!({
                "kinds": [KIND_PROJECT_DOCUMENT_HEAD],
                "authors": [identity.relay_pubkey.to_hex()],
                "#t": ["buzz-project-document-head"],
                "limit": ACTIVE_PAGE_SIZE,
                "buzz_project_document": extension,
            }),
            "active Document page",
        )
        .await?;
        let page_len = values.len();
        for value in values {
            let event: Event = serde_json::from_value(value)
                .map_err(|_| integrity_error("active Document page contains an invalid event"))?;
            let head = parse_document_head(&event, &identity.relay_pubkey, project_id)
                .map_err(|error| integrity_error(error.to_string()))?;
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
                return Err(integrity_error(
                    "active Document page contains a tombstone head",
                ));
            };
            if projection_generation != before.projection.projection_generation
                || catalog_revision > before.projection.catalog_revision
                || after_document_id.is_some_and(|after| document_id <= after)
                || !seen.insert(document_id)
            {
                return Err(integrity_error(
                    "active Document page violates its pinned catalog or UUID order",
                ));
            }
            after_document_id = Some(document_id);
            items.push(DocumentListItem {
                document_id,
                title,
                summary,
                document_revision,
                updated_at,
                updated_by,
                head_event_id: head.event_id,
            });
        }
        if page_len < usize::from(ACTIVE_PAGE_SIZE) {
            break;
        }
    }

    let after = read_meta(client, identity).await?;
    if after.event_id != before.event_id {
        return Err(snapshot_conflict());
    }
    if u64::try_from(items.len()).ok() != Some(before.projection.active_document_count) {
        return Err(integrity_error(
            "active Document count does not match signed catalog metadata",
        ));
    }
    Ok(items)
}

async fn cmd_get(
    client: &CarryforthClient,
    document_id: Uuid,
    revision: Option<u64>,
    content_only: bool,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let output = read_verified_document(client, document_id, revision).await?;
    if content_only {
        let content = output.content_markdown.as_deref().ok_or_else(|| {
            CliError::NotFound(format!(
                "Document {document_id} revision {} is a tombstone",
                output.document_revision
            ))
        })?;
        std::io::stdout()
            .lock()
            .write_all(content.as_bytes())
            .map_err(|error| CliError::Other(format!("failed to write stdout: {error}")))?;
        return Ok(());
    }
    print_read_output(&output, format)
}

/// Resolve one current or pinned Document through the strict SDK boundary.
pub(crate) async fn read_verified_document(
    client: &CarryforthClient,
    document_id: Uuid,
    requested_revision: Option<u64>,
) -> Result<DocumentReadOutput, CliError> {
    let identity = require_identity(client).await?;
    let meta = read_meta(client, identity).await?;
    let project_id = CommunityId::from_uuid(meta.projection.project_id);
    let (revision, head_event_id, current_tombstone) = match requested_revision {
        Some(revision) => (
            read_revision(client, identity, project_id, document_id, revision).await?,
            None,
            false,
        ),
        None => {
            let head = read_head(client, identity, project_id, document_id)
                .await?
                .ok_or_else(|| {
                    CliError::NotFound(format!("Document {document_id} was not found"))
                })?;
            let revision_event_id = head_revision_event_id(&head.projection);
            let revision =
                read_revision_by_event_id(client, identity, project_id, revision_event_id).await?;
            buzz_sdk::project_document::VerifiedCurrentDocument::new(
                head.clone(),
                revision.clone(),
            )
            .map_err(|error| integrity_error(error.to_string()))?;
            let tombstone = head.projection.state() == DocumentState::Deleted;
            (revision, Some(head.event_id), tombstone)
        }
    };
    if revision_document_id(&revision.projection) != document_id {
        return Err(integrity_error(
            "revision query returned a different Document identity",
        ));
    }
    if revision_projection_generation(&revision.projection) != meta.projection.projection_generation
    {
        return Err(integrity_error(
            "revision belongs to a different projection generation",
        ));
    }
    if current_tombstone {
        return Err(CliError::NotFound(format!(
            "Document {document_id} is deleted at revision {}",
            revision_document_revision(&revision.projection)
        )));
    }
    Ok(document_read_output(revision, head_event_id))
}

async fn cmd_history(
    client: &CarryforthClient,
    document_id: Uuid,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_identity(client).await?;
    for attempt in 0..SNAPSHOT_ATTEMPTS {
        match read_history(client, identity, document_id).await {
            Ok(history) => return print_read_output(&history, format),
            Err(error) if is_snapshot_conflict(&error) && attempt + 1 < SNAPSHOT_ATTEMPTS => {
                tokio::time::sleep(Duration::from_millis(25_u64 << attempt)).await;
            }
            Err(error) if is_snapshot_conflict(&error) => {
                return Err(CliError::Conflict(
                    "Project Document generation changed during every bounded history attempt"
                        .to_owned(),
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Err(CliError::Conflict(
        "Project Document history could not be stabilized".to_owned(),
    ))
}

async fn read_history(
    client: &CarryforthClient,
    identity: DocumentIdentity,
    document_id: Uuid,
) -> Result<Vec<DocumentHistoryItem>, CliError> {
    let meta = read_meta(client, identity).await?;
    let project_id = CommunityId::from_uuid(meta.projection.project_id);
    let head = read_head(client, identity, project_id, document_id)
        .await?
        .ok_or_else(|| CliError::NotFound(format!("Document {document_id} was not found")))?;
    let projection_generation = head_projection_generation(&head.projection);
    if projection_generation != meta.projection.projection_generation {
        return Err(integrity_error(
            "Document head belongs to a different projection generation",
        ));
    }
    let max_document_revision = head_document_revision(&head.projection);
    let mut before_revision: Option<u64> = None;
    let mut previous_revision: Option<u64> = None;
    let mut history = Vec::new();

    loop {
        let mut extension = json!({
            "scope": "history",
            "projection_generation": projection_generation,
            "document_id": document_id,
            "max_document_revision": max_document_revision,
        });
        if let Some(before) = before_revision {
            extension["before_revision"] = json!(before);
        }
        let values = query_values(
            client,
            json!({
                "kinds": [KIND_PROJECT_DOCUMENT_REVISION],
                "authors": [identity.relay_pubkey.to_hex()],
                "#t": ["buzz-project-document-revision"],
                "limit": HISTORY_PAGE_SIZE,
                "buzz_project_document": extension,
            }),
            "Document history page",
        )
        .await?;
        let page_len = values.len();
        for value in values {
            let event: Event = serde_json::from_value(value)
                .map_err(|_| integrity_error("Document history contains an invalid event"))?;
            let revision = parse_document_revision(&event, &identity.relay_pubkey, project_id)
                .map_err(|error| integrity_error(error.to_string()))?;
            let revision_number = revision_document_revision(&revision.projection);
            if revision_document_id(&revision.projection) != document_id
                || revision_projection_generation(&revision.projection) != projection_generation
                || revision_number > max_document_revision
                || previous_revision.is_some_and(|previous| revision_number >= previous)
            {
                return Err(integrity_error(
                    "Document history violates its pinned coordinate or revision order",
                ));
            }
            previous_revision = Some(revision_number);
            before_revision = Some(revision_number);
            history.push(history_item(&revision));
        }
        if page_len < usize::from(HISTORY_PAGE_SIZE) {
            break;
        }
    }
    if history.len() != usize::try_from(max_document_revision).unwrap_or(usize::MAX) {
        return Err(integrity_error(
            "Document history does not contain every immutable revision",
        ));
    }
    Ok(history)
}

async fn read_head(
    client: &CarryforthClient,
    identity: DocumentIdentity,
    project_id: CommunityId,
    document_id: Uuid,
) -> Result<Option<VerifiedDocumentHead>, CliError> {
    let coordinate = document_head_coordinate(project_id, document_id);
    let values = query_values(
        client,
        json!({
            "kinds": [KIND_PROJECT_DOCUMENT_HEAD],
            "authors": [identity.relay_pubkey.to_hex()],
            "#d": [coordinate],
            "#t": ["buzz-project-document-head"],
            "limit": 2,
        }),
        "Document head",
    )
    .await?;
    if values.is_empty() {
        return Ok(None);
    }
    let [value] = values.as_slice() else {
        return Err(integrity_error(
            "Document head query returned multiple current events",
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|_| integrity_error("Document head event is invalid"))?;
    parse_document_head(&event, &identity.relay_pubkey, project_id)
        .map(Some)
        .map_err(|error| integrity_error(error.to_string()))
}

async fn read_revision(
    client: &CarryforthClient,
    identity: DocumentIdentity,
    project_id: CommunityId,
    document_id: Uuid,
    document_revision: u64,
) -> Result<VerifiedDocumentRevision, CliError> {
    let coordinate = document_revision_coordinate(project_id, document_id, document_revision);
    let values = query_values(
        client,
        json!({
            "kinds": [KIND_PROJECT_DOCUMENT_REVISION],
            "authors": [identity.relay_pubkey.to_hex()],
            "#d": [coordinate],
            "#t": ["buzz-project-document-revision"],
            "limit": 2,
        }),
        "Document revision",
    )
    .await?;
    let [value] = values.as_slice() else {
        return if values.is_empty() {
            Err(CliError::NotFound(format!(
                "Document {document_id} revision {document_revision} was not found"
            )))
        } else {
            Err(integrity_error(
                "Document revision coordinate returned multiple events",
            ))
        };
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|_| integrity_error("Document revision event is invalid"))?;
    let revision = parse_document_revision(&event, &identity.relay_pubkey, project_id)
        .map_err(|error| integrity_error(error.to_string()))?;
    if revision_document_id(&revision.projection) != document_id
        || revision_document_revision(&revision.projection) != document_revision
    {
        return Err(integrity_error(
            "Document revision event does not match the requested coordinate",
        ));
    }
    Ok(revision)
}

async fn read_revision_by_event_id(
    client: &CarryforthClient,
    identity: DocumentIdentity,
    project_id: CommunityId,
    event_id: EventId,
) -> Result<VerifiedDocumentRevision, CliError> {
    let values = query_values(
        client,
        json!({
            "ids": [event_id.to_hex()],
            "kinds": [KIND_PROJECT_DOCUMENT_REVISION],
            "authors": [identity.relay_pubkey.to_hex()],
            "limit": 2,
        }),
        "pointed Document revision",
    )
    .await?;
    let [value] = values.as_slice() else {
        return Err(integrity_error(
            "Document head revision pointer did not resolve exactly once",
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|_| integrity_error("pointed Document revision is invalid"))?;
    if event.id != event_id {
        return Err(integrity_error(
            "Document revision query returned an event other than the head pointer",
        ));
    }
    parse_document_revision(&event, &identity.relay_pubkey, project_id)
        .map_err(|error| integrity_error(error.to_string()))
}

async fn query_values(
    client: &CarryforthClient,
    filter: Value,
    context: &str,
) -> Result<Vec<Value>, CliError> {
    let raw = client.query(&filter).await?;
    serde_json::from_str(&raw)
        .map_err(|_| integrity_error(format!("{context} response is not a JSON event array")))
}

async fn cmd_patch(client: &CarryforthClient, request: PatchRequest) -> Result<(), CliError> {
    let PatchRequest {
        document_id,
        expected_revision,
        patch_file,
        output,
        title,
        summary,
        clear_summary,
    } = request;
    let identity = require_identity(client).await?;
    let meta = read_meta(client, identity).await?;
    let project_id = CommunityId::from_uuid(meta.projection.project_id);
    let base = read_revision(client, identity, project_id, document_id, expected_revision).await?;
    let DocumentRevisionProjection::Active {
        title: base_title,
        summary: base_summary,
        content_markdown,
        ..
    } = base.projection
    else {
        return Err(CliError::NotFound(format!(
            "Document {document_id} revision {expected_revision} is a tombstone"
        )));
    };
    let diff_text = read_bounded_file_or_stdin(&patch_file, PATCH_INPUT_MAX_BYTES)?;
    if diff_text
        .lines()
        .filter(|line| line.starts_with("--- "))
        .count()
        > 1
    {
        return Err(CliError::Usage(
            "multi-file patch is not supported for one Project Document".to_owned(),
        ));
    }
    let patch = diffy::Patch::from_str(&diff_text)
        .map_err(|error| CliError::Usage(format!("malformed unified diff: {error}")))?;
    verify_hunks_at_declared_position(&content_markdown, &patch).map_err(|message| {
        CliError::Usage(format!(
            "patch does not match revision {expected_revision} exactly: {message}; no fuzz or offset is allowed"
        ))
    })?;
    let next_content = diffy::apply(&content_markdown, &patch).map_err(|error| {
        CliError::Usage(format!(
            "patch does not apply to revision {expected_revision}: {error}"
        ))
    })?;
    if next_content.len() > MAX_CONTENT_MARKDOWN_BYTES {
        return Err(CliError::Usage(format!(
            "patched Markdown exceeds the {MAX_CONTENT_MARKDOWN_BYTES}-byte limit"
        )));
    }
    if let Some(output) = output {
        std::fs::write(&output, next_content.as_bytes())
            .map_err(|error| CliError::Usage(format!("failed to write {output:?}: {error}")))?;
    }
    let next_summary = match (summary, clear_summary) {
        (Some(summary), false) => Some(summary),
        (None, true) => None,
        (None, false) => base_summary,
        (Some(_), true) => {
            return Err(CliError::Usage(
                "--summary and --clear-summary are mutually exclusive".to_owned(),
            ));
        }
    };
    submit_write(
        client,
        ProjectDocumentCommand::new(
            expected_revision,
            DocumentCommandRequest::Update {
                document_id,
                title: title.unwrap_or(base_title),
                summary: next_summary,
                content_markdown: next_content,
            },
        ),
    )
    .await
}

async fn submit_write(
    client: &CarryforthClient,
    command: ProjectDocumentCommand,
) -> Result<(), CliError> {
    let operation = command.operation();
    if matches!(
        operation,
        DocumentOperation::Create | DocumentOperation::Update
    ) {
        eprintln!("{SECRET_BOUNDARY_WARNING}");
    }
    let identity = require_identity(client).await?;
    command
        .validate_for_submission()
        .map_err(|error| CliError::Usage(error.to_string()))?;
    let document_id = command.document_id();
    let expected_revision = command.expected_document_revision;
    let committed_revision = expected_revision
        .checked_add(1)
        .ok_or_else(|| CliError::Usage("Document revision overflow".to_owned()))?;
    let expected_state = if operation == DocumentOperation::Delete {
        DocumentState::Deleted
    } else {
        DocumentState::Active
    };
    let event = client.sign_event_exact(build_document_command(command).map_err(sdk_err)?)?;

    match client.submit_project_command(&event).await? {
        ProjectCommandDelivery::Accepted { raw, receipt } => {
            let receipt: ProjectDocumentReceipt =
                serde_json::from_value(receipt).map_err(|_| {
                    integrity_error("Relay returned a receipt for another Project protocol")
                })?;
            validate_receipt(
                &receipt,
                &event,
                operation,
                document_id,
                expected_revision,
                committed_revision,
                expected_state,
            )?;
            confirm_write_revision(
                client,
                identity,
                document_id,
                committed_revision,
                expected_state,
                event.id,
            )
            .await
            .map_err(|error| {
                CliError::Other(format!(
                    "Project Document was accepted but read-back verification failed: {error}"
                ))
            })?;
            print_write_output(
                &raw,
                document_id,
                committed_revision,
                "receipt_and_readback",
            )
        }
        ProjectCommandDelivery::Ambiguous { reason } => {
            if confirm_write_revision(
                client,
                identity,
                document_id,
                committed_revision,
                expected_state,
                event.id,
            )
            .await
            .is_err()
            {
                return Err(CliError::DeliveryUnknown(format!(
                    "Project Document command {} may have reached the Relay ({reason}); exact revision read-back did not prove acceptance",
                    event.id.to_hex()
                )));
            }
            let raw = json!({
                "event_id": event.id.to_hex(),
                "accepted": true,
                "message": "confirmed:project_document:read_back",
            })
            .to_string();
            print_write_output(&raw, document_id, committed_revision, "readback")
        }
    }
}

async fn confirm_write_revision(
    client: &CarryforthClient,
    identity: DocumentIdentity,
    document_id: Uuid,
    document_revision: u64,
    expected_state: DocumentState,
    source_event_id: EventId,
) -> Result<(), CliError> {
    let meta = read_meta(client, identity).await?;
    let project_id = CommunityId::from_uuid(meta.projection.project_id);
    let revision =
        read_revision(client, identity, project_id, document_id, document_revision).await?;
    if revision.projection.state() != expected_state
        || revision_projection_generation(&revision.projection)
            != meta.projection.projection_generation
        || revision_source_event_id(&revision.projection) != source_event_id
    {
        return Err(integrity_error(
            "immutable revision does not prove the submitted command was accepted",
        ));
    }
    Ok(())
}

fn validate_receipt(
    receipt: &ProjectDocumentReceipt,
    event: &Event,
    operation: DocumentOperation,
    document_id: Uuid,
    expected_revision: u64,
    committed_revision: u64,
    state: DocumentState,
) -> Result<(), CliError> {
    receipt
        .validate()
        .map_err(|error| integrity_error(error.to_string()))?;
    if receipt.change_id != event.id
        || receipt.actor != event.pubkey
        || receipt.operation != operation
        || receipt.document_id != document_id
        || receipt.expected_document_revision != expected_revision
        || receipt.document_revision != committed_revision
        || receipt.state != state
    {
        return Err(integrity_error(
            "Relay receipt does not match the submitted Document command",
        ));
    }
    Ok(())
}

fn print_write_output(
    raw: &str,
    document_id: Uuid,
    document_revision: u64,
    confirmation: &str,
) -> Result<(), CliError> {
    let mut value: Value = serde_json::from_str(raw)
        .map_err(|_| integrity_error("accepted write response is not valid JSON"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| integrity_error("accepted write response is not an object"))?;
    object.insert("document_id".to_owned(), json!(document_id));
    object.insert("document_revision".to_owned(), json!(document_revision));
    object.insert("confirmation".to_owned(), json!(confirmation));
    println!(
        "{}",
        serde_json::to_string(&value)
            .map_err(|_| CliError::Other("failed to serialize write output".to_owned()))?
    );
    Ok(())
}

fn read_document_content(
    content: Option<String>,
    content_file: Option<String>,
) -> Result<String, CliError> {
    match (content, content_file) {
        (Some(content), None) if content == "-" => {
            read_bounded_file_or_stdin("-", MAX_CONTENT_MARKDOWN_BYTES)
        }
        (Some(content), None) => {
            if content.len() > MAX_CONTENT_MARKDOWN_BYTES {
                return Err(CliError::Usage(format!(
                    "Markdown exceeds the {MAX_CONTENT_MARKDOWN_BYTES}-byte limit"
                )));
            }
            Ok(content)
        }
        (None, Some(path)) => read_bounded_file_or_stdin(&path, MAX_CONTENT_MARKDOWN_BYTES),
        (None, None) => Err(CliError::Usage(
            "exactly one of --content or --content-file is required".to_owned(),
        )),
        (Some(_), Some(_)) => Err(CliError::Usage(
            "--content and --content-file are mutually exclusive".to_owned(),
        )),
    }
}

fn complete_update_summary(
    summary: Option<String>,
    clear_summary: bool,
) -> Result<Option<String>, CliError> {
    match (summary, clear_summary) {
        (Some(summary), false) => Ok(Some(summary)),
        (None, true) => Ok(None),
        (None, false) => Err(CliError::Usage(
            "update requires exactly one of --summary or --clear-summary".to_owned(),
        )),
        (Some(_), true) => Err(CliError::Usage(
            "--summary and --clear-summary are mutually exclusive".to_owned(),
        )),
    }
}

fn verify_hunks_at_declared_position(
    current: &str,
    patch: &diffy::Patch<'_, str>,
) -> Result<(), String> {
    let current_lines: Vec<&str> = current.split_inclusive('\n').collect();
    for (index, hunk) in patch.hunks().iter().enumerate() {
        let preimage: Vec<&str> = hunk
            .lines()
            .iter()
            .filter_map(|line| match line {
                diffy::Line::Context(value) | diffy::Line::Delete(value) => Some(*value),
                diffy::Line::Insert(_) => None,
            })
            .collect();
        if preimage.is_empty() {
            if hunk.old_range().start() == 0 && current.is_empty() {
                continue;
            }
            return Err(format!(
                "hunk #{} has no position-verifiable preimage",
                index + 1
            ));
        }
        let start = hunk
            .old_range()
            .start()
            .checked_sub(1)
            .ok_or_else(|| format!("hunk #{} has line number zero", index + 1))?;
        let end = start
            .checked_add(preimage.len())
            .ok_or_else(|| format!("hunk #{} line range overflows", index + 1))?;
        if end > current_lines.len() {
            return Err(format!(
                "hunk #{} extends past the base snapshot",
                index + 1
            ));
        }
        for (offset, expected) in preimage.iter().enumerate() {
            if current_lines[start + offset] != *expected {
                return Err(format!(
                    "hunk #{} differs at declared line {}",
                    index + 1,
                    start + offset + 1
                ));
            }
        }
    }
    Ok(())
}

fn document_read_output(
    revision: VerifiedDocumentRevision,
    head_event_id: Option<EventId>,
) -> DocumentReadOutput {
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
        } => DocumentReadOutput {
            document_id,
            document_revision,
            state: DocumentState::Active,
            title: Some(title),
            summary,
            content_markdown: Some(content_markdown),
            created_at,
            created_by,
            revision_at,
            revision_by,
            revision_event_id: revision.event_id,
            head_event_id,
            source_event_id,
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
        } => DocumentReadOutput {
            document_id,
            document_revision,
            state: DocumentState::Deleted,
            title: None,
            summary: None,
            content_markdown: None,
            created_at,
            created_by,
            revision_at,
            revision_by,
            revision_event_id: revision.event_id,
            head_event_id,
            source_event_id,
        },
    }
}

fn history_item(revision: &VerifiedDocumentRevision) -> DocumentHistoryItem {
    match &revision.projection {
        DocumentRevisionProjection::Active {
            document_revision,
            revision_at,
            revision_by,
            ..
        } => DocumentHistoryItem {
            document_revision: *document_revision,
            state: DocumentState::Active,
            actor: *revision_by,
            canonical_at: *revision_at,
            revision_event_id: revision.event_id,
        },
        DocumentRevisionProjection::Deleted {
            document_revision,
            revision_at,
            revision_by,
            ..
        } => DocumentHistoryItem {
            document_revision: *document_revision,
            state: DocumentState::Deleted,
            actor: *revision_by,
            canonical_at: *revision_at,
            revision_event_id: revision.event_id,
        },
    }
}

fn head_revision_event_id(projection: &DocumentHeadProjection) -> EventId {
    match projection {
        DocumentHeadProjection::Active {
            revision_event_id, ..
        }
        | DocumentHeadProjection::Deleted {
            revision_event_id, ..
        } => *revision_event_id,
    }
}

fn head_projection_generation(projection: &DocumentHeadProjection) -> u64 {
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

fn head_document_revision(projection: &DocumentHeadProjection) -> u64 {
    match projection {
        DocumentHeadProjection::Active {
            document_revision, ..
        }
        | DocumentHeadProjection::Deleted {
            document_revision, ..
        } => *document_revision,
    }
}

fn revision_document_id(projection: &DocumentRevisionProjection) -> Uuid {
    match projection {
        DocumentRevisionProjection::Active { document_id, .. }
        | DocumentRevisionProjection::Deleted { document_id, .. } => *document_id,
    }
}

fn revision_document_revision(projection: &DocumentRevisionProjection) -> u64 {
    match projection {
        DocumentRevisionProjection::Active {
            document_revision, ..
        }
        | DocumentRevisionProjection::Deleted {
            document_revision, ..
        } => *document_revision,
    }
}

fn revision_projection_generation(projection: &DocumentRevisionProjection) -> u64 {
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

fn revision_source_event_id(projection: &DocumentRevisionProjection) -> EventId {
    match projection {
        DocumentRevisionProjection::Active {
            source_event_id, ..
        }
        | DocumentRevisionProjection::Deleted {
            source_event_id, ..
        } => *source_event_id,
    }
}

fn snapshot_conflict() -> CliError {
    CliError::Relay {
        status: 409,
        body: "conflict:project_document:snapshot_changed".to_owned(),
    }
}

fn is_snapshot_conflict(error: &CliError) -> bool {
    matches!(
        error,
        CliError::Relay { status: 409, body }
            if body.starts_with("conflict:project_document:snapshot_changed")
    )
}

fn print_read_output(value: &impl Serialize, _format: &OutputFormat) -> Result<(), CliError> {
    println!(
        "{}",
        serde_json::to_string(value)
            .map_err(|_| CliError::Other("failed to serialize Document output".to_owned()))?
    );
    Ok(())
}

fn integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!(
        "Project Document integrity error: {}",
        message.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_summary_is_closed() {
        assert_eq!(
            complete_update_summary(Some("summary".to_owned()), false).expect("summary"),
            Some("summary".to_owned())
        );
        assert_eq!(complete_update_summary(None, true).expect("clear"), None);
        assert!(complete_update_summary(None, false).is_err());
        assert!(complete_update_summary(Some("summary".to_owned()), true).is_err());
    }

    #[test]
    fn exact_patch_rejects_offset_match() {
        let current = "inserted\nalpha\nbeta\n";
        let patch = diffy::Patch::from_str(
            "--- a/doc\n+++ b/doc\n@@ -1,2 +1,2 @@\n alpha\n-beta\n+gamma\n",
        )
        .expect("patch");
        assert!(verify_hunks_at_declared_position(current, &patch).is_err());
    }

    #[test]
    fn literal_content_is_bounded_before_signing() {
        let too_large = "x".repeat(MAX_CONTENT_MARKDOWN_BYTES + 1);
        assert!(read_document_content(Some(too_large), None).is_err());
        assert!(read_document_content(None, None).is_err());
    }
}
