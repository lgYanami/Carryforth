//! `buzz project-view` — typed reads and optimistic-concurrency mutations.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use buzz_core::kind::{KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT};
use buzz_core::{CommunityId, PublicKey};
use buzz_project_view::{
    GoalView, InitializeGoal, IssueView, PlanView, ProjectProfile, ProjectView, ProjectViewEntry,
    ProjectViewObject, ProjectViewObjectType, ProjectViewState, RequirementView, UpdateMutation,
};
use buzz_sdk::project_view::{
    build_create, build_delete, build_initialize, build_update, object_projection_coordinate,
    parse_meta_projection, parse_object_projection, MetaProjection, ObjectProjection,
    ProjectedObject,
};
use nostr::Event;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::client::{create_response_with_id, normalize_write_response, BuzzClient};
use crate::error::CliError;
use crate::validate::{read_file_or_stdin, sdk_err};
use crate::{OutputFormat, ProjectViewCmd};

const PROJECT_VIEW_EXTENSION: &str = "buzz-project-view-v1";
const SNAPSHOT_PAGE_SIZE: usize = 500;
const SNAPSHOT_MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy)]
struct ProjectViewIdentity {
    relay_pubkey: PublicKey,
}

#[derive(Deserialize)]
struct Nip11Document {
    #[serde(default)]
    supported_extensions: Vec<String>,
    #[serde(rename = "self")]
    relay_self: Option<String>,
}

struct ProjectSnapshot {
    meta: MetaProjection,
    view: ProjectView,
}

#[derive(Serialize)]
struct ProjectViewOutput {
    initialized: bool,
    project_revision: u64,
    project: Option<ProjectViewObject>,
    goals: Vec<GoalView>,
    unbound_plans: Vec<PlanView>,
    unplanned_requirements: Vec<RequirementView>,
    unplanned_issues: Vec<IssueView>,
    roles: Vec<ProjectViewObject>,
    resources: Vec<ProjectViewObject>,
    issue_references_by_target: BTreeMap<Uuid, Vec<buzz_project_view::ObjectRef>>,
}

impl ProjectViewOutput {
    fn uninitialized() -> Self {
        Self {
            initialized: false,
            project_revision: 0,
            project: None,
            goals: Vec::new(),
            unbound_plans: Vec::new(),
            unplanned_requirements: Vec::new(),
            unplanned_issues: Vec::new(),
            roles: Vec::new(),
            resources: Vec::new(),
            issue_references_by_target: BTreeMap::new(),
        }
    }

    fn initialized(meta: &MetaProjection, view: ProjectView) -> Self {
        let ProjectView {
            profile,
            goals,
            unbound_plans,
            unplanned_requirements,
            unplanned_issues,
            roles,
            resources,
            issue_references_by_target,
        } = view;
        Self {
            initialized: true,
            project_revision: meta.project_revision,
            project: Some(profile),
            goals,
            unbound_plans,
            unplanned_requirements,
            unplanned_issues,
            roles,
            resources,
            issue_references_by_target,
        }
    }
}

#[derive(Deserialize)]
struct RelayWriteResponse {
    event_id: String,
    accepted: bool,
    message: String,
}

#[derive(Debug, Deserialize)]
struct ProjectViewReceipt {
    project_revision: u64,
    object_id: Option<Uuid>,
    object_revision: Option<u64>,
    deleted: Option<bool>,
}

/// Dispatch a Project View command.
pub async fn dispatch(
    command: ProjectViewCmd,
    client: &BuzzClient,
    format: &OutputFormat,
) -> Result<(), CliError> {
    match command {
        ProjectViewCmd::Get => cmd_get(client, format).await,
        ProjectViewCmd::GetObject { object_type, id } => {
            cmd_get_object(client, object_type.into(), id, format).await
        }
        ProjectViewCmd::Init { profile, goal } => cmd_init(client, &profile, &goal).await,
        ProjectViewCmd::Create {
            object_type,
            expected_project_revision,
            data,
        } => cmd_create(client, object_type.into(), expected_project_revision, &data).await,
        ProjectViewCmd::Update {
            object_type,
            id,
            expected_project_revision,
            patch,
        } => {
            cmd_update(
                client,
                object_type.into(),
                id,
                expected_project_revision,
                &patch,
            )
            .await
        }
        ProjectViewCmd::Delete {
            object_type,
            id,
            expected_project_revision,
        } => cmd_delete(client, object_type.into(), id, expected_project_revision).await,
    }
}

async fn cmd_get(client: &BuzzClient, format: &OutputFormat) -> Result<(), CliError> {
    let identity = require_capability(client).await?;
    let output = match fetch_consistent_snapshot(client, identity).await? {
        Some(snapshot) => ProjectViewOutput::initialized(&snapshot.meta, snapshot.view),
        None => ProjectViewOutput::uninitialized(),
    };
    print_read_output(&output, format)
}

async fn cmd_get_object(
    client: &BuzzClient,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_capability(client).await?;
    let meta = read_meta(client, identity)
        .await?
        .ok_or_else(|| CliError::NotFound("Project View is not initialized".to_owned()))?;
    let projection = read_object(client, identity, meta.project_id, object_type, object_id)
        .await?
        .ok_or_else(|| {
            CliError::NotFound(format!(
                "Project View object {}:{} was not found",
                object_type.as_str(),
                object_id
            ))
        })?;
    validate_object_against_meta(&projection, &meta)?;
    print_read_output(&object_output(&projection, &meta), format)
}

async fn cmd_init(client: &BuzzClient, profile: &str, goals: &[String]) -> Result<(), CliError> {
    require_single_stdin(std::iter::once(profile).chain(goals.iter().map(String::as_str)))?;
    let identity = require_capability(client).await?;
    let profile: ProjectProfile = read_json_file(profile, "profile")?;
    let goals: Vec<InitializeGoal> = goals
        .iter()
        .map(|path| read_json_file(path, "goal"))
        .collect::<Result<_, _>>()?;
    let event = client.sign_event_exact(build_initialize(profile, goals).map_err(sdk_err)?)?;
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_receipt(&raw, &event)?;
    if receipt.object_id.is_some() || receipt.object_revision.is_some() || receipt.deleted.is_some()
    {
        return Err(integrity_error(
            "initialization receipt unexpectedly contains object fields",
        ));
    }
    let snapshot = fetch_consistent_snapshot(client, identity)
        .await?
        .ok_or_else(|| integrity_error("initialization succeeded but metadata is missing"))?;
    if snapshot.meta.project_revision < receipt.project_revision {
        return Err(integrity_error(
            "confirmed metadata is older than the successful initialization receipt",
        ));
    }
    println!("{}", normalize_write_response(&raw));
    Ok(())
}

async fn cmd_create(
    client: &BuzzClient,
    object_type: ProjectViewObjectType,
    expected_project_revision: u64,
    data_path: &str,
) -> Result<(), CliError> {
    if object_type == ProjectViewObjectType::ProjectProfile {
        return Err(CliError::Usage(
            "project_profile can only be created by `project-view init`".to_owned(),
        ));
    }
    let identity = require_capability(client).await?;
    let object_id = Uuid::new_v4();
    let object = create_input(object_type, object_id, read_json_value(data_path, "data")?)?;
    let event = client
        .sign_event_exact(build_create(expected_project_revision, object).map_err(sdk_err)?)?;
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_object_receipt(&raw, &event, object_id, false)?;
    confirm_object_receipt(client, identity, object_type, object_id, &receipt).await?;
    println!(
        "{}",
        create_response_with_id(
            &normalize_write_response(&raw),
            "object_id",
            &object_id.to_string()
        )
    );
    Ok(())
}

async fn cmd_update(
    client: &BuzzClient,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    expected_project_revision: u64,
    patch_path: &str,
) -> Result<(), CliError> {
    let identity = require_capability(client).await?;
    let update = update_input(
        object_type,
        object_id,
        read_json_value(patch_path, "patch")?,
    )?;
    let event = client
        .sign_event_exact(build_update(expected_project_revision, update).map_err(sdk_err)?)?;
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_object_receipt(&raw, &event, object_id, false)?;
    confirm_object_receipt(client, identity, object_type, object_id, &receipt).await?;
    println!("{}", normalize_write_response(&raw));
    Ok(())
}

async fn cmd_delete(
    client: &BuzzClient,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    expected_project_revision: u64,
) -> Result<(), CliError> {
    let identity = require_capability(client).await?;
    let event = client.sign_event_exact(
        build_delete(expected_project_revision, object_type, object_id).map_err(sdk_err)?,
    )?;
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_object_receipt(&raw, &event, object_id, true)?;
    confirm_object_receipt(client, identity, object_type, object_id, &receipt).await?;
    println!("{}", normalize_write_response(&raw));
    Ok(())
}

async fn require_capability(client: &BuzzClient) -> Result<ProjectViewIdentity, CliError> {
    let raw = client.get_public("/info").await?;
    let info: Nip11Document = serde_json::from_str(&raw)
        .map_err(|error| integrity_error(format!("invalid NIP-11 document: {error}")))?;
    if !info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_EXTENSION)
    {
        return Err(CliError::Other(format!(
            "unsupported: relay does not advertise {PROJECT_VIEW_EXTENSION}"
        )));
    }
    let relay_self = info.relay_self.ok_or_else(|| {
        integrity_error("NIP-11 advertises Project View without a relay `self` key")
    })?;
    let relay_pubkey = PublicKey::from_hex(&relay_self)
        .map_err(|error| integrity_error(format!("invalid NIP-11 relay `self`: {error}")))?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(integrity_error(
            "NIP-11 relay `self` is not canonical lowercase hex",
        ));
    }
    Ok(ProjectViewIdentity { relay_pubkey })
}

async fn read_meta(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
) -> Result<Option<MetaProjection>, CliError> {
    let filter = json!({
        "kinds": [KIND_PROJECT_VIEW_META],
        "authors": [identity.relay_pubkey.to_hex()],
        "limit": 2,
    });
    let events = parse_event_array(&client.query(&filter).await?, "metadata query")?;
    match events.as_slice() {
        [] => Ok(None),
        [event] => parse_meta_projection(event, &identity.relay_pubkey)
            .map(Some)
            .map_err(projection_error),
        _ => Err(integrity_error(
            "metadata query returned multiple current heads",
        )),
    }
}

async fn read_object(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    project_id: CommunityId,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
) -> Result<Option<ObjectProjection>, CliError> {
    let coordinate = object_projection_coordinate(project_id, object_type, object_id);
    let filter = json!({
        "kinds": [KIND_PROJECT_VIEW_OBJECT],
        "authors": [identity.relay_pubkey.to_hex()],
        "#d": [coordinate],
        "limit": 2,
    });
    let events = parse_event_array(&client.query(&filter).await?, "object query")?;
    match events.as_slice() {
        [] => Ok(None),
        [event] => {
            let projection = parse_object_projection(event, &identity.relay_pubkey, project_id)
                .map_err(projection_error)?;
            if projection.object.object_type() != object_type || projection.object.id() != object_id
            {
                return Err(integrity_error(
                    "point query returned a different Project View coordinate",
                ));
            }
            Ok(Some(projection))
        }
        _ => Err(integrity_error(
            "object query returned multiple current heads",
        )),
    }
}

async fn fetch_consistent_snapshot(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
) -> Result<Option<ProjectSnapshot>, CliError> {
    for attempt in 0..SNAPSHOT_MAX_ATTEMPTS {
        match fetch_snapshot_once(client, identity).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(CliError::Conflict(_)) if attempt + 1 < SNAPSHOT_MAX_ATTEMPTS => {
                let backoff_ms = 25_u64 << attempt;
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(CliError::Conflict(
        "Project View changed during every bounded snapshot attempt".to_owned(),
    ))
}

async fn fetch_snapshot_once(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
) -> Result<Option<ProjectSnapshot>, CliError> {
    let Some(meta) = read_meta(client, identity).await? else {
        return Ok(None);
    };

    let mut after: Option<(String, String)> = None;
    let mut entries = Vec::new();
    let mut object_ids = HashSet::new();
    loop {
        let mut extension = json!({
            "revision": meta.project_revision,
            "projection_generation": meta.projection_generation,
        });
        if let Some((object_type, object_id)) = &after {
            extension["after"] = json!({
                "object_type": object_type,
                "object_id": object_id,
            });
        }
        let filter = json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": ["buzz-project-view-active"],
            "limit": SNAPSHOT_PAGE_SIZE,
            "buzz_project_view": extension,
        });
        let raw = match client.query(&filter).await {
            Ok(raw) => raw,
            Err(CliError::Relay { status: 409, body }) => {
                return Err(CliError::Conflict(format!(
                    "Project View changed during snapshot pagination: {body}"
                )));
            }
            Err(error) => return Err(error),
        };
        let page = parse_event_array(&raw, "snapshot page")?;
        if page.len() > SNAPSHOT_PAGE_SIZE {
            return Err(integrity_error(
                "snapshot page exceeded the requested page size",
            ));
        }
        for event in &page {
            let projection =
                parse_object_projection(event, &identity.relay_pubkey, meta.project_id)
                    .map_err(projection_error)?;
            validate_object_against_meta(&projection, &meta)?;
            let object = match projection.object {
                ProjectedObject::Active(object) => *object,
                ProjectedObject::Tombstone(_) => {
                    return Err(integrity_error(
                        "active snapshot query returned a tombstone",
                    ));
                }
            };
            let cursor = (
                object.object_type.as_str().to_owned(),
                object.id.to_string(),
            );
            if after.as_ref().is_some_and(|previous| cursor <= *previous) {
                return Err(integrity_error(
                    "snapshot page order is not strictly increasing",
                ));
            }
            if !object_ids.insert(object.id) {
                return Err(integrity_error(
                    "snapshot contains a duplicate active object id",
                ));
            }
            after = Some(cursor);
            entries.push(ProjectViewEntry::Active(object));
            if entries.len() > meta.active_object_count as usize {
                return Err(integrity_error(
                    "snapshot contains more objects than metadata declares",
                ));
            }
        }
        if page.len() < SNAPSHOT_PAGE_SIZE {
            break;
        }
    }

    let final_meta = read_meta(client, identity)
        .await?
        .ok_or_else(|| CliError::Conflict("Project View metadata disappeared".to_owned()))?;
    if final_meta.projection_generation != meta.projection_generation
        || final_meta.project_revision != meta.project_revision
        || final_meta.event_id != meta.event_id
    {
        return Err(CliError::Conflict(
            "Project View changed while assembling the snapshot".to_owned(),
        ));
    }
    if entries.len() != meta.active_object_count as usize {
        return Err(integrity_error(format!(
            "snapshot contains {} active objects but metadata declares {}",
            entries.len(),
            meta.active_object_count
        )));
    }

    let initialized_at = entries.iter().find_map(|entry| match entry {
        ProjectViewEntry::Active(object)
            if object.object_type == ProjectViewObjectType::ProjectProfile =>
        {
            Some(object.created_at)
        }
        _ => None,
    });
    let state = ProjectViewState::from_snapshot(
        meta.project_id,
        meta.project_revision,
        initialized_at,
        Some(meta.updated_at),
        entries,
    )
    .map_err(|error| integrity_error(format!("invalid Project View snapshot: {error}")))?;
    let view = ProjectView::assemble(&state)
        .map_err(|error| integrity_error(format!("cannot assemble Project View: {error}")))?;
    Ok(Some(ProjectSnapshot { meta, view }))
}

fn validate_object_against_meta(
    projection: &ObjectProjection,
    meta: &MetaProjection,
) -> Result<(), CliError> {
    if projection.project_id != meta.project_id {
        return Err(integrity_error(
            "object projection belongs to a different project than metadata",
        ));
    }
    if projection.projection_generation != meta.projection_generation {
        return Err(CliError::Conflict(
            "object projection generation differs from current metadata".to_owned(),
        ));
    }
    if projection.project_revision > meta.project_revision {
        return Err(integrity_error(
            "object projection is newer than current metadata",
        ));
    }
    Ok(())
}

fn object_output(projection: &ObjectProjection, meta: &MetaProjection) -> Value {
    match &projection.object {
        ProjectedObject::Active(object) => json!({
            "project_revision": meta.project_revision,
            "projection_generation": meta.projection_generation,
            "deleted": false,
            "object": object,
        }),
        ProjectedObject::Tombstone(tombstone) => json!({
            "project_revision": meta.project_revision,
            "projection_generation": meta.projection_generation,
            "deleted": true,
            "tombstone": tombstone,
        }),
    }
}

fn create_input(
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    data: Value,
) -> Result<buzz_project_view::NewProjectViewObject, CliError> {
    let mut object = data.as_object().cloned().ok_or_else(|| {
        CliError::Usage("Project View --data must contain one JSON object".to_owned())
    })?;
    if object.contains_key("id") || object.contains_key("object_type") {
        return Err(CliError::Usage(
            "--data must omit id and object_type; the CLI supplies both".to_owned(),
        ));
    }
    object.insert("id".to_owned(), Value::String(object_id.to_string()));
    object.insert(
        "object_type".to_owned(),
        Value::String(object_type.as_str().to_owned()),
    );
    serde_json::from_value(Value::Object(object))
        .map_err(|error| CliError::Usage(format!("invalid typed Project View data: {error}")))
}

fn update_input(
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    patch: Value,
) -> Result<UpdateMutation, CliError> {
    if !patch.is_object() {
        return Err(CliError::Usage(
            "Project View --patch must contain one JSON object".to_owned(),
        ));
    }
    serde_json::from_value(json!({
        "object_type": object_type.as_str(),
        "object_id": object_id,
        "patch": patch,
    }))
    .map_err(|error| CliError::Usage(format!("invalid typed Project View patch: {error}")))
}

fn read_json_file<T: DeserializeOwned>(path: &str, label: &str) -> Result<T, CliError> {
    serde_json::from_str(&read_file_or_stdin(path)?)
        .map_err(|error| CliError::Usage(format!("invalid {label} JSON in {path:?}: {error}")))
}

fn read_json_value(path: &str, label: &str) -> Result<Value, CliError> {
    read_json_file(path, label)
}

fn require_single_stdin<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<(), CliError> {
    if paths.into_iter().filter(|path| *path == "-").count() > 1 {
        return Err(CliError::Usage(
            "stdin (`-`) can be used for only one Project View input".to_owned(),
        ));
    }
    Ok(())
}

fn parse_event_array(raw: &str, context: &str) -> Result<Vec<Event>, CliError> {
    serde_json::from_str(raw)
        .map_err(|error| integrity_error(format!("invalid {context} response: {error}")))
}

async fn submit_mutation(client: &BuzzClient, event: Event) -> Result<String, CliError> {
    match client.submit_event(event).await {
        Err(CliError::Relay { status: 409, body }) => Err(CliError::Conflict(body)),
        Err(CliError::Relay { status: 400, body }) if body.contains("unsupported:") => {
            Err(CliError::Other(body))
        }
        Err(CliError::Relay { status: 400, body }) => Err(CliError::Usage(body)),
        result => result,
    }
}

fn parse_receipt(raw: &str, event: &Event) -> Result<ProjectViewReceipt, CliError> {
    let response: RelayWriteResponse = serde_json::from_str(raw)
        .map_err(|error| integrity_error(format!("invalid mutation response: {error}")))?;
    if !response.accepted {
        return Err(integrity_error(
            "Relay returned a successful HTTP response with accepted=false",
        ));
    }
    if response.event_id != event.id.to_hex() {
        return Err(integrity_error(
            "mutation response event_id differs from the submitted event",
        ));
    }
    let receipt = response
        .message
        .strip_prefix("response:")
        .ok_or_else(|| integrity_error("mutation response has no canonical receipt"))?;
    serde_json::from_str(receipt)
        .map_err(|error| integrity_error(format!("invalid mutation receipt: {error}")))
}

fn parse_object_receipt(
    raw: &str,
    event: &Event,
    expected_object_id: Uuid,
    expected_deleted: bool,
) -> Result<ProjectViewReceipt, CliError> {
    let receipt = parse_receipt(raw, event)?;
    if receipt.object_id != Some(expected_object_id)
        || receipt.object_revision.is_none()
        || receipt.deleted != Some(expected_deleted)
    {
        return Err(integrity_error(
            "mutation receipt does not match the requested object operation",
        ));
    }
    Ok(receipt)
}

async fn confirm_object_receipt(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    receipt: &ProjectViewReceipt,
) -> Result<(), CliError> {
    let meta = read_meta(client, identity)
        .await?
        .ok_or_else(|| integrity_error("successful mutation has no metadata projection"))?;
    if meta.project_revision < receipt.project_revision {
        return Err(integrity_error(
            "metadata projection is older than the successful mutation receipt",
        ));
    }
    let projection = read_object(client, identity, meta.project_id, object_type, object_id)
        .await?
        .ok_or_else(|| integrity_error("successful mutation has no object projection"))?;
    validate_object_against_meta(&projection, &meta)?;
    if projection.object.object_revision() < receipt.object_revision.unwrap_or_default() {
        return Err(integrity_error(
            "object projection is older than the successful mutation receipt",
        ));
    }
    if receipt.deleted == Some(true) && !matches!(projection.object, ProjectedObject::Tombstone(_))
    {
        return Err(integrity_error(
            "delete receipt was not confirmed by a tombstone projection",
        ));
    }
    Ok(())
}

fn print_json(value: &impl Serialize) -> Result<(), CliError> {
    let output = serde_json::to_string(value)
        .map_err(|error| CliError::Other(format!("failed to serialize output: {error}")))?;
    println!("{output}");
    Ok(())
}

fn print_read_output(value: &impl Serialize, format: &OutputFormat) -> Result<(), CliError> {
    match format {
        OutputFormat::Json => print_json(value),
        OutputFormat::Compact => {
            let mut value = serde_json::to_value(value)
                .map_err(|error| CliError::Other(format!("failed to serialize output: {error}")))?;
            compact_project_objects(&mut value);
            print_json(&value)
        }
    }
}

fn compact_project_objects(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                compact_project_objects(value);
            }
        }
        Value::Object(object) => {
            if object.contains_key("id")
                && object.contains_key("object_type")
                && object.contains_key("object_revision")
            {
                for field in [
                    "object_revision",
                    "project_revision",
                    "created_at",
                    "updated_at",
                    "created_by",
                    "updated_by",
                ] {
                    object.remove(field);
                }
            }
            for value in object.values_mut() {
                compact_project_objects(value);
            }
        }
        _ => {}
    }
}

fn projection_error(error: buzz_sdk::SdkError) -> CliError {
    integrity_error(error.to_string())
}

fn integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!("Project View integrity error: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use buzz_project_view::{
        InitializeMutation, Mutation, MutationRequest, ProjectViewState, ProjectionPlan,
    };
    use buzz_sdk::project_view::{
        build_meta_projection, build_object_projection, changed_head_for,
        meta_projection_coordinate,
    };
    use chrono::{DateTime, Utc};
    use nostr::Keys;
    use tokio::net::TcpListener;

    use super::*;

    use crate::ProjectViewObjectTypeArg;

    #[test]
    fn create_input_injects_cli_owned_identity() {
        let id = Uuid::new_v4();
        let object = create_input(
            ProjectViewObjectType::Goal,
            id,
            json!({
                "title": "Ship",
                "desired_outcome": "A working CLI",
                "directions": []
            }),
        )
        .expect("typed create input");
        assert_eq!(object.id(), id);
        assert_eq!(object.object_type(), ProjectViewObjectType::Goal);
    }

    #[test]
    fn create_input_rejects_caller_owned_id() {
        let result = create_input(
            ProjectViewObjectType::Goal,
            Uuid::new_v4(),
            json!({
                "id": Uuid::new_v4(),
                "title": "Ship",
                "desired_outcome": "A working CLI",
                "directions": []
            }),
        );
        assert!(matches!(result, Err(CliError::Usage(_))));
    }

    #[test]
    fn update_input_preserves_explicit_null_patch_semantics() {
        let update = update_input(
            ProjectViewObjectType::Plan,
            Uuid::new_v4(),
            json!({"under_goal_id": null}),
        )
        .expect("typed update input");
        let UpdateMutation::Plan { patch, .. } = update else {
            panic!("expected plan update");
        };
        assert!(patch.under_goal_id.is_clear());
    }

    #[test]
    fn only_one_stdin_source_is_allowed() {
        assert!(require_single_stdin(["profile.json", "-", "goal.json"]).is_ok());
        assert!(require_single_stdin(["-", "-"]).is_err());
    }

    #[test]
    fn compact_output_removes_object_provenance_but_keeps_view_revision() {
        let mut value = json!({
            "project_revision": 8,
            "project": {
                "id": Uuid::new_v4(),
                "object_type": "goal",
                "object_revision": 2,
                "project_revision": 7,
                "created_at": "2027-01-01T00:00:00Z",
                "updated_at": "2027-01-02T00:00:00Z",
                "created_by": "a",
                "updated_by": "b",
                "data": {"object_type": "goal", "data": {"title": "Ship"}},
                "relations": {}
            }
        });
        compact_project_objects(&mut value);
        assert_eq!(value["project_revision"], 8);
        assert!(value["project"].get("object_revision").is_none());
        assert!(value["project"].get("updated_by").is_none());
        assert_eq!(value["project"]["data"]["data"]["title"], "Ship");
    }

    #[test]
    fn metadata_coordinate_is_not_derived_from_cli_input() {
        let project = CommunityId::from_uuid(Uuid::new_v4());
        assert_eq!(
            meta_projection_coordinate(project),
            format!("project-view:{project}:meta")
        );
    }

    #[test]
    fn project_object_arg_mapping_is_total() {
        let all = [
            ProjectViewObjectTypeArg::ProjectProfile,
            ProjectViewObjectTypeArg::Goal,
            ProjectViewObjectTypeArg::Role,
            ProjectViewObjectTypeArg::Plan,
            ProjectViewObjectTypeArg::Stage,
            ProjectViewObjectTypeArg::Requirement,
            ProjectViewObjectTypeArg::Issue,
            ProjectViewObjectTypeArg::Work,
            ProjectViewObjectTypeArg::Resource,
        ];
        let mapped: HashSet<ProjectViewObjectType> =
            all.into_iter().map(ProjectViewObjectType::from).collect();
        assert_eq!(mapped.len(), 9);
    }

    #[derive(Clone)]
    struct SnapshotServerState {
        relay_pubkey: String,
        meta: Event,
        objects: Vec<Event>,
        meta_queries: Arc<AtomicUsize>,
        snapshot_queries: Arc<AtomicUsize>,
    }

    async fn snapshot_info(State(state): State<SnapshotServerState>) -> Json<Value> {
        Json(json!({
            "supported_extensions": [PROJECT_VIEW_EXTENSION],
            "self": state.relay_pubkey,
        }))
    }

    async fn snapshot_query(
        State(state): State<SnapshotServerState>,
        Json(filters): Json<Vec<Value>>,
    ) -> Json<Value> {
        let filter = filters.first().cloned().unwrap_or_else(|| json!({}));
        if filter.get("buzz_project_view").is_some() {
            state.snapshot_queries.fetch_add(1, Ordering::SeqCst);
            Json(serde_json::to_value(state.objects).expect("serialize object projections"))
        } else {
            state.meta_queries.fetch_add(1, Ordering::SeqCst);
            Json(serde_json::to_value([state.meta]).expect("serialize metadata projection"))
        }
    }

    async fn spawn_snapshot_server(state: SnapshotServerState) -> String {
        let app = Router::new()
            .route("/info", get(snapshot_info))
            .route("/query", post(snapshot_query))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve snapshot fixture");
        });
        format!("http://{address}")
    }

    fn projection_fixture() -> SnapshotServerState {
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let mutation = Mutation::new(
            0,
            MutationRequest::Initialize(InitializeMutation {
                profile: ProjectProfile {
                    name: "CLI integration".to_owned(),
                    positioning: "Verified snapshots".to_owned(),
                    purpose: "Exercise the real HTTP bridge client".to_owned(),
                    problem: "Mixed revisions".to_owned(),
                    scope: "Project View".to_owned(),
                },
                goals: vec![InitializeGoal {
                    id: Uuid::new_v4(),
                    title: "Ship".to_owned(),
                    desired_outcome: "One consistent read model".to_owned(),
                    directions: Vec::new(),
                }],
            }),
        );
        let (state, outcome) = ProjectViewState::new(project_id)
            .reduce(
                &mutation,
                Keys::generate().public_key(),
                DateTime::<Utc>::from_timestamp(1_800_000_000, 0).expect("fixture timestamp"),
            )
            .expect("initialize fixture");
        let plan =
            ProjectionPlan::for_mutation(&state, &outcome, [0x44; 32], 1).expect("projection plan");
        let relay = Keys::generate();
        let mut paired = plan
            .entries()
            .iter()
            .map(|entry| {
                let event = build_object_projection(&plan, entry)
                    .expect("object projection")
                    .sign_with_keys(&relay)
                    .expect("sign object projection");
                let head = changed_head_for(&plan, entry, &event).expect("changed head");
                (
                    entry.object_type().as_str().to_owned(),
                    entry.id(),
                    event,
                    head,
                )
            })
            .collect::<Vec<_>>();
        paired.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
        });
        let heads = paired
            .iter()
            .map(|(_, _, _, head)| head.clone())
            .collect::<Vec<_>>();
        let objects = paired.into_iter().map(|(_, _, event, _)| event).collect();
        let meta = build_meta_projection(&plan, &heads)
            .expect("metadata projection")
            .sign_with_keys(&relay)
            .expect("sign metadata projection");
        SnapshotServerState {
            relay_pubkey: relay.public_key().to_hex(),
            meta,
            objects,
            meta_queries: Arc::new(AtomicUsize::new(0)),
            snapshot_queries: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[tokio::test]
    async fn cli_http_snapshot_verifies_and_assembles_read_model() {
        let state = projection_fixture();
        let counters = state.clone();
        let url = spawn_snapshot_server(state).await;
        let client =
            BuzzClient::new(url, Keys::generate(), None, None).expect("Project View test client");
        let identity = require_capability(&client).await.expect("capability");
        let snapshot = fetch_consistent_snapshot(&client, identity)
            .await
            .expect("consistent snapshot")
            .expect("initialized snapshot");

        assert_eq!(snapshot.meta.project_revision, 1);
        assert_eq!(snapshot.meta.active_object_count, 2);
        assert_eq!(snapshot.view.goals.len(), 1);
        assert_eq!(
            counters.meta_queries.load(Ordering::SeqCst),
            2,
            "snapshot must bracket pagination with metadata reads"
        );
        assert_eq!(
            counters.snapshot_queries.load(Ordering::SeqCst),
            1,
            "small fixture fits in one revision-pinned page"
        );
    }

    #[derive(Clone)]
    struct ConflictServerState {
        relay_pubkey: String,
    }

    async fn conflict_info(State(state): State<ConflictServerState>) -> Json<Value> {
        Json(json!({
            "supported_extensions": [PROJECT_VIEW_EXTENSION],
            "self": state.relay_pubkey,
        }))
    }

    async fn conflict_event() -> (StatusCode, Json<Value>) {
        (
            StatusCode::CONFLICT,
            Json(json!({"error": "conflict:project_view:revision"})),
        )
    }

    async fn spawn_conflict_server() -> String {
        let relay = Keys::generate();
        let app = Router::new()
            .route("/info", get(conflict_info))
            .route("/events", post(conflict_event))
            .with_state(ConflictServerState {
                relay_pubkey: relay.public_key().to_hex(),
            });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind conflict server");
        let address = listener.local_addr().expect("conflict server address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve conflict fixture");
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn project_view_revision_conflict_maps_to_cli_exit_five() {
        let url = spawn_conflict_server().await;
        let mut data = tempfile::NamedTempFile::new().expect("create JSON fixture");
        write!(
            data,
            "{}",
            json!({
                "title": "Conflicting goal",
                "desired_outcome": "Exit five",
                "directions": []
            })
        )
        .expect("write JSON fixture");
        let path = data.path().to_string_lossy().into_owned();
        let exit = crate::run_from_args([
            "buzz",
            "--relay",
            &url,
            "--private-key",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "project-view",
            "create",
            "goal",
            "--expected-project-revision",
            "1",
            "--data",
            &path,
        ])
        .await;
        assert_eq!(exit, 5);
    }
}
