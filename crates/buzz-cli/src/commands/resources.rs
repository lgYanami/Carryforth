//! `buzz resources` — verified Resource-to-Guide convenience reads.

use std::io::Write as _;

use buzz_project_view::v3::{ProjectViewEntryV3, ProjectViewObjectDataV3};
use serde::Serialize;
use uuid::Uuid;

use crate::client::BuzzClient;
use crate::commands::documents::{read_verified_document, DocumentReadOutput};
use crate::commands::project_view_v2_snapshot::{
    read_identity, read_v3_meta, read_verified_v3_snapshot, ProjectViewSchema,
    PROJECT_VIEW_V3_EXTENSION,
};
use crate::error::CliError;
use crate::{OutputFormat, ResourcesCmd};

#[derive(Serialize)]
struct ResourceGuideOutput {
    resource_id: Uuid,
    resource_project_revision: u64,
    resource_object_revision: u64,
    resource_head_event_id: buzz_core::EventId,
    guide_document_id: Uuid,
    guide_document_revision: u64,
    guide_head_or_revision_event_id: buzz_core::EventId,
    guide: DocumentReadOutput,
}

/// Dispatch one Resource convenience command.
pub async fn dispatch(
    command: ResourcesCmd,
    client: &BuzzClient,
    format: &OutputFormat,
) -> Result<(), CliError> {
    match command {
        ResourcesCmd::Guide {
            resource_id,
            revision,
            content_only,
        } => guide(client, resource_id, revision, content_only, format).await,
    }
}

async fn guide(
    client: &BuzzClient,
    resource_id: Uuid,
    revision: Option<u64>,
    content_only: bool,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = read_identity(client).await?.ok_or_else(|| {
        CliError::Other(format!(
            "unsupported: Resource Guides require {PROJECT_VIEW_V3_EXTENSION}"
        ))
    })?;
    if identity.schema != ProjectViewSchema::V3 {
        return Err(CliError::Other(
            "unsupported: Project View v3 / Resource Guide is unavailable; a legacy locator is not a Guide"
                .to_owned(),
        ));
    }
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    let entry = snapshot
        .entry(resource_id)
        .ok_or_else(|| CliError::NotFound(format!("Resource {resource_id} was not found")))?;
    let ProjectViewEntryV3::Active(resource) = entry else {
        return Err(CliError::NotFound(format!(
            "Resource {resource_id} is deleted"
        )));
    };
    let ProjectViewObjectDataV3::Resource(resource_data) = &resource.data else {
        return Err(CliError::NotFound(format!(
            "Project View object {resource_id} is not a Resource"
        )));
    };
    let source = snapshot.object_source(resource_id).ok_or_else(|| {
        CliError::Other(
            "Project View v3 integrity error: Resource has no verified signed source".to_owned(),
        )
    })?;
    let document =
        read_verified_document(client, resource_data.guide_document_id, revision).await?;
    let final_meta = read_v3_meta(client, identity).await?;
    if final_meta.event_id != snapshot.meta().event_id {
        return Err(CliError::Conflict(
            "Project View v3 changed while resolving the Resource Guide; retry the read".to_owned(),
        ));
    }
    if content_only {
        let content = document.content_markdown.as_deref().ok_or_else(|| {
            CliError::NotFound(format!(
                "Guide {} revision {} is deleted",
                document.document_id, document.document_revision
            ))
        })?;
        std::io::stdout()
            .lock()
            .write_all(content.as_bytes())
            .map_err(|error| CliError::Other(format!("failed to write stdout: {error}")))?;
        return Ok(());
    }
    let guide_event_id = document.head_event_id.unwrap_or(document.revision_event_id);
    let output = ResourceGuideOutput {
        resource_id,
        resource_project_revision: resource.project_revision,
        resource_object_revision: resource.object_revision,
        resource_head_event_id: source.event_id,
        guide_document_id: resource_data.guide_document_id,
        guide_document_revision: document.document_revision,
        guide_head_or_revision_event_id: guide_event_id,
        guide: document,
    };
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string(&output)
                .map_err(|error| CliError::Other(format!("serialize Resource Guide: {error}")))?
        ),
        OutputFormat::Compact => println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "resource_id": output.resource_id,
                "resource_project_revision": output.resource_project_revision,
                "resource_object_revision": output.resource_object_revision,
                "resource_head_event_id": output.resource_head_event_id,
                "guide_document_id": output.guide_document_id,
                "guide_document_revision": output.guide_document_revision,
                "guide_head_or_revision_event_id": output.guide_head_or_revision_event_id,
                "title": output.guide.title,
                "summary": output.guide.summary,
            }))
            .map_err(|error| CliError::Other(format!("serialize Resource Guide: {error}")))?
        ),
    }
    Ok(())
}
