//! `buzz project-view` — typed reads and optimistic-concurrency mutations.

use std::collections::{BTreeMap, BTreeSet};

use buzz_core::PublicKey;
use buzz_project_view::v2::{CommunityMemberRole, RoleLevel};
use buzz_project_view::v3::{
    canonicalize_context_references, CreateProjectObjectV3, DeleteProjectObjectV3,
    DocumentReferenceMode, InitialGovernanceAssignmentV3, NewProjectViewObjectV3,
    ProjectContextReference, ProjectObjectCommandV3, ProjectObjectRequestV3, ProjectViewEntryV3,
    ProjectViewInitializeV3, ProjectViewInitializeV3Request, UpdateProjectObjectV3,
};
use buzz_project_view::ProjectViewObjectType;
use buzz_sdk::project_view_v3::{
    build_initialize_command as build_initialize_v3,
    build_project_object_command as build_project_object_command_v3, PROJECT_VIEW_V3_EXTENSION,
};
use buzz_sdk::role_brief_v3::VerifiedRoleBriefSnapshotV3;
use nostr::Event;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::client::{normalize_write_response, BuzzClient};
use crate::commands::project_view_snapshot::{
    read_identity, read_v3_bootstrap_identity, read_verified_v3_snapshot, ProjectViewIdentity,
    ProjectViewSchema,
};
use crate::error::CliError;
use crate::validate::{read_file_or_stdin, sdk_err};
use crate::{OutputFormat, ProjectViewCmd, ProjectViewContextCmd, ProjectViewV3ClientCmd};

#[derive(Serialize)]
struct ProjectViewV3Output<'a> {
    project_view_schema_version: u16,
    initialized: bool,
    project_revision: u64,
    projection_generation: u64,
    project: Option<&'a buzz_project_view::v3::ProjectViewObjectV3>,
    goals: Vec<&'a buzz_project_view::v3::ProjectViewObjectV3>,
    roles: Vec<&'a buzz_project_view::v3::ProjectViewObjectV3>,
    resources: Vec<&'a buzz_project_view::v3::ProjectViewObjectV3>,
    objects: Vec<&'a buzz_project_view::v3::ProjectViewObjectV3>,
}

impl<'a> ProjectViewV3Output<'a> {
    fn from_snapshot(snapshot: &'a VerifiedRoleBriefSnapshotV3) -> Self {
        let objects = snapshot.state().active_objects().collect::<Vec<_>>();
        Self {
            project_view_schema_version: 3,
            initialized: true,
            project_revision: snapshot.meta().project_revision,
            projection_generation: snapshot.meta().projection_generation,
            project: objects
                .iter()
                .copied()
                .find(|object| object.object_type == ProjectViewObjectType::ProjectProfile),
            goals: objects
                .iter()
                .copied()
                .filter(|object| object.object_type == ProjectViewObjectType::Goal)
                .collect(),
            roles: objects
                .iter()
                .copied()
                .filter(|object| object.object_type == ProjectViewObjectType::Role)
                .collect(),
            resources: objects
                .iter()
                .copied()
                .filter(|object| object.object_type == ProjectViewObjectType::Resource)
                .collect(),
            objects,
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
struct ProjectViewObjectReceipt {
    project_revision: u64,
    operation: String,
    object_id: Uuid,
    object_revision: u64,
    deleted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectViewObjectReceiptV3 {
    schema_version: u16,
    operation: String,
    project_revision: u64,
    objects: Vec<ProjectViewObjectReceiptEntryV3>,
    #[serde(rename = "continuity_entities")]
    _continuity_entities: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectViewObjectReceiptEntryV3 {
    object_id: Uuid,
    object_type: String,
    object_revision: u64,
    deleted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectViewInitializeReceiptV3 {
    schema_version: u16,
    operation: String,
    preparation_operation_id: Uuid,
    project_revision: u64,
    projection_generation: u64,
    object_ids: Vec<Uuid>,
    governance_assignments: Vec<InitialGovernanceAssignmentV3>,
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
        ProjectViewCmd::InitV3 { command } => cmd_init_v3(client, &command).await,
        ProjectViewCmd::V3 { command } => match command {
            ProjectViewV3ClientCmd::Resources { command } => {
                crate::commands::project_view_v3_approval::dispatch(command, client).await
            }
        },
        ProjectViewCmd::Context { command } => cmd_context(client, command, format).await,
        ProjectViewCmd::Create {
            object_type,
            expected_project_revision,
            id,
            data,
            role_level,
        } => {
            cmd_create(
                client,
                object_type.into(),
                expected_project_revision,
                id,
                &data,
                role_level.map(Into::into),
            )
            .await
        }
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

async fn cmd_context(
    client: &BuzzClient,
    command: ProjectViewContextCmd,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_v3_capability(client).await?;
    let (object_id, mutation) = match command {
        ProjectViewContextCmd::List { object_id } => (object_id, None),
        ProjectViewContextCmd::Add {
            object_id,
            resource,
            document,
            revision,
        } => {
            if !identity.context_enabled {
                return Err(CliError::Other(
                    "unavailable:project_view:context_capability".to_owned(),
                ));
            }
            if document.is_some() && !identity.document_enabled {
                return Err(CliError::Other(
                    "unavailable:project_view:document_capability".to_owned(),
                ));
            }
            (
                object_id,
                Some((true, context_reference(resource, document, revision)?)),
            )
        }
        ProjectViewContextCmd::Remove {
            object_id,
            resource,
            document,
            revision,
        } => (
            object_id,
            Some((false, context_reference(resource, document, revision)?)),
        ),
    };
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    let entry = snapshot.entry(object_id).ok_or_else(|| {
        CliError::NotFound(format!("Project View object {object_id} was not found"))
    })?;
    let ProjectViewEntryV3::Active(object) = entry else {
        return Err(CliError::NotFound(format!(
            "Project View object {object_id} is deleted"
        )));
    };
    let Some((add, reference)) = mutation else {
        return print_read_output(
            &json!({
                "project_view_schema_version": 3,
                "project_revision": snapshot.meta().project_revision,
                "projection_generation": snapshot.meta().projection_generation,
                "context_capability": identity.context_enabled,
                "object_id": object.id,
                "object_type": object.object_type,
                "context_references": object.context_references,
            }),
            format,
        );
    };

    let mut replacement = object.context_references.clone();
    if add {
        if replacement.contains(&reference) {
            return Err(CliError::Usage(
                "Context Reference already exists on the source object".to_owned(),
            ));
        }
        replacement.push(reference);
    } else {
        let before = replacement.len();
        replacement.retain(|candidate| candidate != &reference);
        if replacement.len() == before {
            return Err(CliError::Usage(
                "Context Reference does not exist on the source object".to_owned(),
            ));
        }
    }
    let replacement = canonicalize_context_references(replacement)
        .map_err(|error| CliError::Usage(error.to_string()))?;
    submit_context_replacement(client, identity, &snapshot, object, replacement).await
}

fn context_reference(
    resource: Option<Uuid>,
    document: Option<Uuid>,
    revision: Option<u64>,
) -> Result<ProjectContextReference, CliError> {
    let reference = match (resource, document) {
        (Some(resource_id), None) if revision.is_none() => {
            ProjectContextReference::Resource { resource_id }
        }
        (None, Some(document_id)) => ProjectContextReference::Document {
            document_id,
            mode: if revision.is_some() {
                DocumentReferenceMode::Pinned
            } else {
                DocumentReferenceMode::Live
            },
            document_revision: revision,
        },
        _ => {
            return Err(CliError::Usage(
                "select exactly one --resource or --document target".to_owned(),
            ));
        }
    };
    reference
        .validate()
        .map_err(|error| CliError::Usage(error.to_string()))?;
    Ok(reference)
}

async fn submit_context_replacement(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    snapshot: &VerifiedRoleBriefSnapshotV3,
    object: &buzz_project_view::v3::ProjectViewObjectV3,
    context_references: Vec<ProjectContextReference>,
) -> Result<(), CliError> {
    let update = update_input_v3(
        object.object_type,
        object.id,
        json!({ "context_references": context_references }),
    )?;
    let acting_assignment_id = if object.object_type == ProjectViewObjectType::Role {
        let governance = role_governance_from_v3(client.public_key(), snapshot);
        governance.authorize(governance.role_level(object.id)?)?
    } else {
        None
    };
    let command = ProjectObjectCommandV3::new(
        snapshot.meta().project_revision,
        acting_assignment_id,
        ProjectObjectRequestV3::Update(update),
    );
    let event =
        client.sign_event_exact(build_project_object_command_v3(command).map_err(sdk_err)?)?;
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt =
        parse_object_receipt(&raw, &event, "update", object.object_type, object.id, false)?;
    confirm_object_receipt(
        client,
        identity,
        object.object_type,
        object.id,
        None,
        &receipt,
    )
    .await?;
    println!("{}", normalize_write_response(&raw));
    Ok(())
}

async fn cmd_get(client: &BuzzClient, format: &OutputFormat) -> Result<(), CliError> {
    let identity = require_v3_capability(client).await?;
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    print_read_output(&ProjectViewV3Output::from_snapshot(&snapshot), format)
}

async fn cmd_get_object(
    client: &BuzzClient,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_v3_capability(client).await?;
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    let entry = snapshot.entry(object_id).ok_or_else(|| {
        CliError::NotFound(format!(
            "Project View object {}:{} was not found",
            object_type.as_str(),
            object_id
        ))
    })?;
    if entry.object_type() != object_type {
        return Err(integrity_error(
            "v3 point lookup found the object ID under a different type",
        ));
    }
    print_read_output(&v3_object_output(entry, &snapshot), format)
}

async fn cmd_init_v3(client: &BuzzClient, command_path: &str) -> Result<(), CliError> {
    let command: ProjectViewInitializeV3 = read_json_file(command_path, "v3 initialization")?;
    let event = client.sign_event_exact(build_initialize_v3(command.clone()).map_err(sdk_err)?)?;
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_initialize_receipt_v3(&raw, &event, &command)?;
    // Initialization deliberately leaves the ordinary runtime disabled until
    // the operator completes the checked enable step. Resolve only the Relay
    // signer here; requiring the runtime capability would turn a successful
    // initialization into a false failure before enable can run.
    let identity = read_v3_bootstrap_identity(client).await?;
    confirm_initialize_receipt_v3(client, identity, &event, &receipt).await?;
    println!("{}", normalize_write_response(&raw));
    Ok(())
}

async fn cmd_create(
    client: &BuzzClient,
    object_type: ProjectViewObjectType,
    expected_project_revision: u64,
    object_id: Option<Uuid>,
    data_path: &str,
    role_level: Option<RoleLevel>,
) -> Result<(), CliError> {
    if object_type == ProjectViewObjectType::ProjectProfile {
        return Err(CliError::Usage(
            "project_profile can only be created by `project-view init-v3`".to_owned(),
        ));
    }
    let identity = require_v3_capability(client).await?;
    if object_type != ProjectViewObjectType::Role && role_level.is_some() {
        return Err(CliError::Usage(
            "--role-level is valid only when creating a role".to_owned(),
        ));
    }
    let (acting_assignment_id, role_level) = if object_type == ProjectViewObjectType::Role {
        let level = role_level.unwrap_or(RoleLevel::Member);
        let governance = read_role_governance(client, identity).await?;
        (governance.authorize(level)?, Some(level))
    } else {
        (None, None)
    };
    let object_id = object_id.unwrap_or_else(Uuid::new_v4);
    if object_id.get_version_num() != 4 {
        return Err(CliError::Usage(
            "project-view create --id must be a UUID v4".to_owned(),
        ));
    }
    let data = read_json_value(data_path, "data")?;
    let object = create_input_v3(object_type, object_id, data)?;
    let mut command = ProjectObjectCommandV3::new(
        expected_project_revision,
        acting_assignment_id,
        ProjectObjectRequestV3::Create(CreateProjectObjectV3 { object }),
    );
    command.initial_role_level = role_level;
    let event =
        client.sign_event_exact(build_project_object_command_v3(command).map_err(sdk_err)?)?;
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_object_receipt(&raw, &event, "create", object_type, object_id, false)?;
    confirm_object_receipt(
        client,
        identity,
        object_type,
        object_id,
        Some(&event),
        &receipt,
    )
    .await?;
    print_object_write_result(&event, &receipt)
}

async fn cmd_update(
    client: &BuzzClient,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    expected_project_revision: u64,
    patch_path: &str,
) -> Result<(), CliError> {
    let identity = require_v3_capability(client).await?;
    let patch = read_json_value(patch_path, "patch")?;
    let acting_assignment_id = if object_type == ProjectViewObjectType::Role {
        let governance = read_role_governance(client, identity).await?;
        let level = governance.role_level(object_id)?;
        governance.authorize(level)?
    } else {
        None
    };
    let update = update_input_v3(object_type, object_id, patch)?;
    let command = ProjectObjectCommandV3::new(
        expected_project_revision,
        acting_assignment_id,
        ProjectObjectRequestV3::Update(update),
    );
    let event =
        client.sign_event_exact(build_project_object_command_v3(command).map_err(sdk_err)?)?;
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_object_receipt(&raw, &event, "update", object_type, object_id, false)?;
    confirm_object_receipt(client, identity, object_type, object_id, None, &receipt).await?;
    println!("{}", normalize_write_response(&raw));
    Ok(())
}

async fn cmd_delete(
    client: &BuzzClient,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    expected_project_revision: u64,
) -> Result<(), CliError> {
    let identity = require_v3_capability(client).await?;
    let acting_assignment_id = if object_type == ProjectViewObjectType::Role {
        let governance = read_role_governance(client, identity).await?;
        let level = governance.role_level(object_id)?;
        governance.authorize(level)?
    } else {
        None
    };
    let command = ProjectObjectCommandV3::new(
        expected_project_revision,
        acting_assignment_id,
        ProjectObjectRequestV3::Delete(DeleteProjectObjectV3 {
            object_type,
            object_id,
        }),
    );
    let event =
        client.sign_event_exact(build_project_object_command_v3(command).map_err(sdk_err)?)?;
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_object_receipt(&raw, &event, "delete", object_type, object_id, true)?;
    confirm_object_receipt(client, identity, object_type, object_id, None, &receipt).await?;
    println!("{}", normalize_write_response(&raw));
    Ok(())
}

struct CliRoleGovernance {
    is_owner: bool,
    leader_assignment_id: Option<Uuid>,
    role_levels: BTreeMap<Uuid, RoleLevel>,
}

impl CliRoleGovernance {
    fn authorize(&self, target_level: RoleLevel) -> Result<Option<Uuid>, CliError> {
        if self.is_owner {
            return Ok(None);
        }
        if target_level == RoleLevel::Admin {
            return Err(CliError::Auth(
                "owner_required: only the Community owner can govern an admin Role".to_owned(),
            ));
        }
        self.leader_assignment_id.map(Some).ok_or_else(|| {
            CliError::Auth(
                "authorization: Role governance requires Community admin membership and an active admin Assignment"
                    .to_owned(),
            )
        })
    }

    fn role_level(&self, role_id: Uuid) -> Result<RoleLevel, CliError> {
        self.role_levels.get(&role_id).copied().ok_or_else(|| {
            CliError::NotFound(format!(
                "active Project Role {role_id} was not found in the verified snapshot"
            ))
        })
    }
}

async fn read_role_governance(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
) -> Result<CliRoleGovernance, CliError> {
    let actor = client.public_key();
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    Ok(role_governance_from_v3(actor, &snapshot))
}

fn role_governance_from_v3(
    actor: PublicKey,
    snapshot: &VerifiedRoleBriefSnapshotV3,
) -> CliRoleGovernance {
    let membership_role = snapshot
        .membership()
        .members
        .iter()
        .find(|member| member.pubkey == actor)
        .map(|member| member.role);
    let role_levels = snapshot
        .roles()
        .map(|role| (role.role_id, (role.level, role.active)))
        .collect::<BTreeMap<_, _>>();
    let leader_assignment_id = current_admin_assignment(
        actor,
        membership_role,
        snapshot.assignments().map(|assignment| {
            (
                assignment.assignment_id,
                assignment.role_id,
                assignment.member_pubkey,
                assignment.is_active(),
            )
        }),
        &role_levels,
    );
    CliRoleGovernance {
        is_owner: membership_role == Some(CommunityMemberRole::Owner),
        leader_assignment_id,
        role_levels: role_levels
            .into_iter()
            .map(|(role_id, (level, _))| (role_id, level))
            .collect(),
    }
}

fn current_admin_assignment(
    actor: PublicKey,
    membership_role: Option<CommunityMemberRole>,
    assignments: impl Iterator<Item = (Uuid, Uuid, PublicKey, bool)>,
    roles: &BTreeMap<Uuid, (RoleLevel, bool)>,
) -> Option<Uuid> {
    if membership_role != Some(CommunityMemberRole::Admin) {
        return None;
    }
    assignments
        .filter(|(_, _, member_pubkey, active)| *active && *member_pubkey == actor)
        .find_map(|(assignment_id, role_id, _, _)| {
            roles
                .get(&role_id)
                .is_some_and(|(level, active)| *active && *level == RoleLevel::Admin)
                .then_some(assignment_id)
        })
}

async fn require_v3_capability(client: &BuzzClient) -> Result<ProjectViewIdentity, CliError> {
    match read_identity(client).await? {
        Some(identity) if identity.schema == ProjectViewSchema::V3 => Ok(identity),
        Some(_) => Err(CliError::Other(
            format!("migration_required: ordinary Project View commands require {PROJECT_VIEW_V3_EXTENSION}"),
        )),
        None => Err(CliError::Other(
            format!("unsupported: relay does not advertise {PROJECT_VIEW_V3_EXTENSION}"),
        )),
    }
}

fn v3_object_output(entry: &ProjectViewEntryV3, snapshot: &VerifiedRoleBriefSnapshotV3) -> Value {
    let source = snapshot
        .object_source(entry.id())
        .or_else(|| snapshot.role_source(entry.id()));
    match entry {
        ProjectViewEntryV3::Active(object) => json!({
            "project_view_schema_version": 3,
            "project_revision": snapshot.meta().project_revision,
            "projection_generation": snapshot.meta().projection_generation,
            "deleted": false,
            "object": object,
            "source": source,
        }),
        ProjectViewEntryV3::Tombstone(tombstone) => json!({
            "project_view_schema_version": 3,
            "project_revision": snapshot.meta().project_revision,
            "projection_generation": snapshot.meta().projection_generation,
            "deleted": true,
            "tombstone": tombstone,
        }),
    }
}

fn create_input_v3(
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    data: Value,
) -> Result<NewProjectViewObjectV3, CliError> {
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
        .map_err(|error| CliError::Usage(format!("invalid typed Project View v3 data: {error}")))
}

fn update_input_v3(
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    patch: Value,
) -> Result<UpdateProjectObjectV3, CliError> {
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
    .map_err(|error| CliError::Usage(format!("invalid typed Project View v3 patch: {error}")))
}

fn read_json_file<T: DeserializeOwned>(path: &str, label: &str) -> Result<T, CliError> {
    serde_json::from_str(&read_file_or_stdin(path)?)
        .map_err(|error| CliError::Usage(format!("invalid {label} JSON in {path:?}: {error}")))
}

fn read_json_value(path: &str, label: &str) -> Result<Value, CliError> {
    read_json_file(path, label)
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

fn parse_receipt_value(raw: &str, event: &Event) -> Result<Value, CliError> {
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
    expected_operation: &str,
    expected_object_type: ProjectViewObjectType,
    expected_object_id: Uuid,
    expected_deleted: bool,
) -> Result<ProjectViewObjectReceipt, CliError> {
    let receipt: ProjectViewObjectReceiptV3 =
        serde_json::from_value(parse_receipt_value(raw, event)?)
            .map_err(|error| integrity_error(format!("invalid v3 mutation receipt: {error}")))?;
    let [object] = receipt.objects.as_slice() else {
        return Err(integrity_error(
            "v3 mutation receipt must contain exactly one changed object",
        ));
    };
    if receipt.schema_version != 3
        || receipt.project_revision == 0
        || receipt.operation != expected_operation
        || object.object_type != expected_object_type.as_str()
        || object.object_id != expected_object_id
        || object.object_revision == 0
        || object.deleted != expected_deleted
    {
        return Err(integrity_error(
            "v3 mutation receipt does not match the requested object operation",
        ));
    }
    Ok(ProjectViewObjectReceipt {
        project_revision: receipt.project_revision,
        operation: receipt.operation,
        object_id: object.object_id,
        object_revision: object.object_revision,
        deleted: object.deleted,
    })
}

fn parse_initialize_receipt_v3(
    raw: &str,
    event: &Event,
    command: &ProjectViewInitializeV3,
) -> Result<ProjectViewInitializeReceiptV3, CliError> {
    let receipt: ProjectViewInitializeReceiptV3 =
        serde_json::from_value(parse_receipt_value(raw, event)?).map_err(|error| {
            integrity_error(format!("invalid v3 initialization receipt: {error}"))
        })?;
    let ProjectViewInitializeV3Request::Initialize {
        preparation_operation_id,
        goals,
        initial_roles,
        initial_governance_assignments,
        ..
    } = &command.request;
    let object_ids = receipt.object_ids.iter().copied().collect::<BTreeSet<_>>();
    let expected_object_count = 1_usize
        .checked_add(goals.len())
        .and_then(|count| count.checked_add(initial_roles.len()))
        .ok_or_else(|| integrity_error("v3 initialization object count overflow"))?;
    let expected_ids_present = goals
        .iter()
        .map(|goal| goal.id)
        .chain(initial_roles.iter().map(|role| role.role_id))
        .all(|object_id| object_ids.contains(&object_id));
    if receipt.schema_version != 3
        || receipt.operation != "initialize_v3"
        || receipt.preparation_operation_id != *preparation_operation_id
        || receipt.project_revision != 1
        || receipt.projection_generation == 0
        || receipt.object_ids.len() != expected_object_count
        || object_ids.len() != receipt.object_ids.len()
        || !expected_ids_present
        || receipt.governance_assignments != *initial_governance_assignments
    {
        return Err(integrity_error(
            "v3 initialization receipt does not match the submitted bootstrap command",
        ));
    }
    Ok(receipt)
}

async fn confirm_initialize_receipt_v3(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    event: &Event,
    receipt: &ProjectViewInitializeReceiptV3,
) -> Result<(), CliError> {
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    if snapshot.meta().project_revision != receipt.project_revision
        || snapshot.meta().projection_generation != receipt.projection_generation
        || !matches!(
            snapshot.meta().source,
            buzz_sdk::project_view_v3::V3ProjectionSource::NostrEvent {
                change_id,
                event_id,
            } if change_id == event.id && event_id == event.id
        )
        || receipt
            .object_ids
            .iter()
            .any(|object_id| snapshot.entry(*object_id).is_none())
        || receipt.governance_assignments.iter().any(|expected| {
            !snapshot.assignments().any(|assignment| {
                assignment.assignment_id == expected.assignment_id
                    && assignment.role_id == expected.role_id
                    && assignment.member_pubkey == expected.member_pubkey
                    && assignment.is_active()
            })
        })
    {
        return Err(integrity_error(
            "verified v3 snapshot does not confirm the initialization receipt",
        ));
    }
    Ok(())
}
async fn confirm_object_receipt(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    expected_source: Option<&Event>,
    receipt: &ProjectViewObjectReceipt,
) -> Result<(), CliError> {
    let snapshot = read_verified_v3_snapshot(client, identity).await?;
    if snapshot.meta().project_revision < receipt.project_revision {
        return Err(integrity_error(
            "v3 metadata projection is older than the successful mutation receipt",
        ));
    }
    let entry = snapshot
        .entry(object_id)
        .ok_or_else(|| integrity_error("successful v3 mutation has no object projection"))?;
    if entry.object_type() != object_type
        || entry.object_revision() < receipt.object_revision
        || receipt.deleted != matches!(entry, ProjectViewEntryV3::Tombstone(_))
    {
        return Err(integrity_error(
            "v3 object projection does not confirm the mutation receipt",
        ));
    }
    if let Some(event) = expected_source {
        let source = snapshot
            .object_source(object_id)
            .or_else(|| snapshot.role_source(object_id))
            .ok_or_else(|| integrity_error("successful v3 mutation has no active object source"))?;
        if source.change_id.to_hex() != event.id.to_hex() {
            return Err(integrity_error(
                "v3 object projection source does not match the submitted event",
            ));
        }
    }
    Ok(())
}

fn print_object_write_result(
    event: &Event,
    receipt: &ProjectViewObjectReceipt,
) -> Result<(), CliError> {
    print_json(&json!({
        "project_view_schema_version": 3,
        "event_id": event.id.to_hex(),
        "accepted": true,
        "operation": receipt.operation,
        "object_id": receipt.object_id,
        "object_revision": receipt.object_revision,
        "deleted": receipt.deleted,
        "accepted_project_revision": receipt.project_revision,
    }))
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

fn integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!("Project View integrity error: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use nostr::{EventBuilder, Keys, Kind};
    use tokio::net::TcpListener;

    use super::*;

    use crate::ProjectViewObjectTypeArg;

    #[test]
    fn create_input_injects_cli_owned_identity() {
        let id = Uuid::new_v4();
        let object = create_input_v3(
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
        let result = create_input_v3(
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
        let update = update_input_v3(
            ProjectViewObjectType::Plan,
            Uuid::new_v4(),
            json!({"under_goal_id": null}),
        )
        .expect("typed update input");
        let UpdateProjectObjectV3::Plan { patch, .. } = update else {
            panic!("expected plan update");
        };
        assert!(patch.under_goal_id.is_clear());
    }

    #[test]
    fn context_reference_flags_form_closed_coordinates() {
        let resource_id = Uuid::new_v4();
        assert_eq!(
            context_reference(Some(resource_id), None, None).expect("resource reference"),
            ProjectContextReference::Resource { resource_id }
        );

        let document_id = Uuid::new_v4();
        assert_eq!(
            context_reference(None, Some(document_id), None).expect("live document"),
            ProjectContextReference::Document {
                document_id,
                mode: DocumentReferenceMode::Live,
                document_revision: None,
            }
        );
        assert_eq!(
            context_reference(None, Some(document_id), Some(7)).expect("pinned document"),
            ProjectContextReference::Document {
                document_id,
                mode: DocumentReferenceMode::Pinned,
                document_revision: Some(7),
            }
        );
    }

    #[test]
    fn context_reference_flags_reject_ambiguous_or_invalid_coordinates() {
        assert!(matches!(
            context_reference(None, None, None),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            context_reference(Some(Uuid::new_v4()), Some(Uuid::new_v4()), None),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            context_reference(None, Some(Uuid::new_v4()), Some(0)),
            Err(CliError::Usage(_))
        ));
    }

    fn receipt_fixture_event() -> Event {
        EventBuilder::new(Kind::TextNote, "receipt fixture")
            .sign_with_keys(&Keys::generate())
            .expect("sign receipt fixture")
    }

    fn receipt_response(event: &Event, receipt: Value) -> String {
        json!({
            "event_id": event.id.to_hex(),
            "accepted": true,
            "message": format!("response:{receipt}"),
        })
        .to_string()
    }

    #[test]
    fn v3_object_receipt_array_is_strictly_normalized() {
        let event = receipt_fixture_event();
        let object_id = Uuid::new_v4();
        let raw = receipt_response(
            &event,
            json!({
                "schema_version": 3,
                "operation": "update",
                "project_revision": 8,
                "objects": [{
                    "object_id": object_id,
                    "object_type": "role",
                    "object_revision": 3,
                    "deleted": false,
                }],
                "continuity_entities": [{
                    "entity_type": "assignment",
                    "entity_id": Uuid::new_v4(),
                    "entity_revision": 2,
                }],
            }),
        );

        let receipt = parse_object_receipt(
            &raw,
            &event,
            "update",
            ProjectViewObjectType::Role,
            object_id,
            false,
        )
        .expect("normalize v3 object receipt");

        assert_eq!(receipt.project_revision, 8);
        assert_eq!(receipt.object_id, object_id);
        assert_eq!(receipt.object_revision, 3);
        assert!(!receipt.deleted);
    }

    #[test]
    fn v3_object_receipt_rejects_mismatched_operation_and_multi_object_result() {
        let event = receipt_fixture_event();
        let object_id = Uuid::new_v4();
        let object = json!({
            "object_id": object_id,
            "object_type": "role",
            "object_revision": 3,
            "deleted": false,
        });
        let wrong_operation = receipt_response(
            &event,
            json!({
                "schema_version": 3,
                "operation": "delete",
                "project_revision": 8,
                "objects": [object.clone()],
                "continuity_entities": [],
            }),
        );
        assert!(parse_object_receipt(
            &wrong_operation,
            &event,
            "update",
            ProjectViewObjectType::Role,
            object_id,
            false,
        )
        .is_err());

        let multiple_objects = receipt_response(
            &event,
            json!({
                "schema_version": 3,
                "operation": "update",
                "project_revision": 8,
                "objects": [object.clone(), object],
                "continuity_entities": [],
            }),
        );
        assert!(parse_object_receipt(
            &multiple_objects,
            &event,
            "update",
            ProjectViewObjectType::Role,
            object_id,
            false,
        )
        .is_err());
    }

    #[test]
    fn legacy_flat_object_receipt_is_rejected() {
        let event = receipt_fixture_event();
        let object_id = Uuid::new_v4();
        let raw = receipt_response(
            &event,
            json!({
                "project_revision": 5,
                "object_id": object_id,
                "object_revision": 2,
                "deleted": false,
            }),
        );

        assert!(parse_object_receipt(
            &raw,
            &event,
            "update",
            ProjectViewObjectType::Role,
            object_id,
            false,
        )
        .is_err());
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
    struct CapabilityServerState {
        relay_pubkey: String,
        supported_extensions: Vec<String>,
        query_requests: Arc<AtomicUsize>,
    }

    async fn capability_info(State(state): State<CapabilityServerState>) -> Json<Value> {
        Json(json!({
            "supported_extensions": state.supported_extensions,
            "self": state.relay_pubkey,
        }))
    }

    async fn capability_query(State(state): State<CapabilityServerState>) -> Json<Value> {
        state.query_requests.fetch_add(1, Ordering::SeqCst);
        Json(json!([]))
    }

    async fn spawn_capability_server(state: CapabilityServerState) -> String {
        let app = Router::new()
            .route("/info", get(capability_info))
            .route("/query", post(capability_query))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind capability test server");
        let address = listener
            .local_addr()
            .expect("capability test server address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve capability fixture");
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn ordinary_project_view_rejects_v2_only_capability_before_query() {
        let relay = Keys::generate();
        let query_requests = Arc::new(AtomicUsize::new(0));
        let state = CapabilityServerState {
            relay_pubkey: relay.public_key().to_hex(),
            supported_extensions: vec!["buzz-project-view-v2".to_owned()],
            query_requests: Arc::clone(&query_requests),
        };
        let url = spawn_capability_server(state).await;
        let client =
            BuzzClient::new(url, Keys::generate(), None, None).expect("Project View test client");

        let error = cmd_get(&client, &OutputFormat::Json)
            .await
            .expect_err("v2-only Relay must not enter the ordinary Project View read path");

        assert!(error.to_string().contains(PROJECT_VIEW_V3_EXTENSION));
        assert_eq!(
            query_requests.load(Ordering::SeqCst),
            0,
            "v2-only capability must fail before any Project View query"
        );
    }

    #[derive(Clone)]
    struct ConflictServerState {
        relay_pubkey: String,
    }

    async fn conflict_info(State(state): State<ConflictServerState>) -> Json<Value> {
        Json(json!({
            "supported_extensions": [PROJECT_VIEW_V3_EXTENSION],
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
