//! `buzz project-view` — typed reads and optimistic-concurrency mutations.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use buzz_core::kind::{KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT};
use buzz_core::{CommunityId, PublicKey};
use buzz_project_view::v2::{CommunityMemberRole, ProjectObjectCommand, RoleLevel};
use buzz_project_view::v3::{
    canonicalize_context_references, CreateProjectObjectV3, DeleteProjectObjectV3,
    DocumentReferenceMode, NewProjectViewObjectV3, ProjectContextReference, ProjectObjectCommandV3,
    ProjectObjectRequestV3, ProjectViewEntryV3, ProjectViewInitializeV3, UpdateProjectObjectV3,
};
use buzz_project_view::{
    CreateMutation, DeleteMutation, GoalView, InitializeGoal, IssueView, MutationRequest, PlanView,
    ProjectProfile, ProjectView, ProjectViewEntry, ProjectViewObject, ProjectViewObjectType,
    ProjectViewState, RequirementView, UpdateMutation,
};
use buzz_sdk::project_view::{
    build_create, build_delete, build_initialize, build_update, object_projection_coordinate,
    parse_meta_projection, parse_object_projection, MetaProjection, ObjectProjection,
    ProjectedObject,
};
use buzz_sdk::project_view_v2::{build_project_object_command, V2MetaProjection};
use buzz_sdk::project_view_v3::{
    build_initialize_command as build_initialize_v3,
    build_project_object_command as build_project_object_command_v3,
};
use buzz_sdk::role_brief::VerifiedRoleBriefSnapshot;
use buzz_sdk::role_brief_v3::VerifiedRoleBriefSnapshotV3;
use nostr::Event;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::client::{create_response_with_id, normalize_write_response, BuzzClient};
use crate::commands::project_view_v2_snapshot::{
    read_identity, read_verified_v2_snapshot, read_verified_v3_snapshot, ProjectViewIdentity,
    ProjectViewSchema, PROJECT_VIEW_V1_EXTENSION,
};
use crate::error::CliError;
use crate::validate::{read_file_or_stdin, sdk_err};
use crate::{OutputFormat, ProjectViewCmd, ProjectViewContextCmd, ProjectViewV3ClientCmd};

const SNAPSHOT_PAGE_SIZE: usize = 500;
const SNAPSHOT_MAX_ATTEMPTS: usize = 3;

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

    fn initialized_v2(snapshot: &VerifiedRoleBriefSnapshot) -> Self {
        let ProjectView {
            profile,
            goals,
            unbound_plans,
            unplanned_requirements,
            unplanned_issues,
            roles,
            resources,
            issue_references_by_target,
        } = snapshot.project_view().clone();
        Self {
            initialized: true,
            project_revision: snapshot.meta().project_revision,
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
            data,
            role_level,
        } => {
            cmd_create(
                client,
                object_type.into(),
                expected_project_revision,
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
    let identity = require_capability(client).await?;
    if identity.schema != ProjectViewSchema::V3 {
        return Err(CliError::Other(
            "unsupported: Context Reference requires buzz-project-view-v3".to_owned(),
        ));
    }
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
    confirm_object_receipt(client, identity, object.object_type, object.id, &receipt).await?;
    println!("{}", normalize_write_response(&raw));
    Ok(())
}

async fn cmd_get(client: &BuzzClient, format: &OutputFormat) -> Result<(), CliError> {
    let identity = require_capability(client).await?;
    match identity.schema {
        ProjectViewSchema::V1 => match fetch_consistent_snapshot(client, identity).await? {
            Some(snapshot) => print_read_output(
                &ProjectViewOutput::initialized(&snapshot.meta, snapshot.view),
                format,
            ),
            None => print_read_output(&ProjectViewOutput::uninitialized(), format),
        },
        ProjectViewSchema::V2 => {
            let snapshot = read_verified_v2_snapshot(client, identity).await?;
            print_read_output(&ProjectViewOutput::initialized_v2(&snapshot), format)
        }
        ProjectViewSchema::V3 => {
            let snapshot = read_verified_v3_snapshot(client, identity).await?;
            print_read_output(&ProjectViewV3Output::from_snapshot(&snapshot), format)
        }
    }
}

async fn cmd_get_object(
    client: &BuzzClient,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let identity = require_capability(client).await?;
    match identity.schema {
        ProjectViewSchema::V2 => {
            let snapshot = read_verified_v2_snapshot(client, identity).await?;
            let entry = snapshot.entry(object_id).ok_or_else(|| {
                CliError::NotFound(format!(
                    "Project View object {}:{} was not found",
                    object_type.as_str(),
                    object_id
                ))
            })?;
            if entry.object_type() != object_type {
                return Err(integrity_error(
                    "point lookup found the object ID under a different type",
                ));
            }
            return print_read_output(&v2_object_output(entry, snapshot.meta()), format);
        }
        ProjectViewSchema::V3 => {
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
            return print_read_output(&v3_object_output(entry, &snapshot), format);
        }
        ProjectViewSchema::V1 => {}
    }
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
    if identity.schema != ProjectViewSchema::V1 {
        return Err(CliError::Usage(
            "`project-view init` only applies to schema v1; use `project-view init-v3` for a prepared empty v3 Community"
                .to_owned(),
        ));
    }
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

async fn cmd_init_v3(client: &BuzzClient, command_path: &str) -> Result<(), CliError> {
    let command: ProjectViewInitializeV3 = read_json_file(command_path, "v3 initialization")?;
    let event = client.sign_event_exact(build_initialize_v3(command).map_err(sdk_err)?)?;
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_receipt(&raw, &event)?;
    if receipt.object_id.is_some() || receipt.object_revision.is_some() || receipt.deleted.is_some()
    {
        return Err(integrity_error(
            "v3 initialization receipt unexpectedly contains object fields",
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
    role_level: Option<RoleLevel>,
) -> Result<(), CliError> {
    if object_type == ProjectViewObjectType::ProjectProfile {
        return Err(CliError::Usage(
            "project_profile can only be created by `project-view init`".to_owned(),
        ));
    }
    let identity = require_capability(client).await?;
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
    let object_id = Uuid::new_v4();
    let data = read_json_value(data_path, "data")?;
    let event = match identity.schema {
        ProjectViewSchema::V1 => {
            if role_level.is_some() {
                return Err(CliError::Other(
                    "unsupported: governed Role creation requires Project View schema v2 or v3"
                        .to_owned(),
                ));
            }
            let object = create_input(object_type, object_id, data)?;
            client.sign_event_exact(
                build_create(expected_project_revision, object).map_err(sdk_err)?,
            )?
        }
        ProjectViewSchema::V2 => {
            let object = create_input(object_type, object_id, data)?;
            let mut command = ProjectObjectCommand::new(
                expected_project_revision,
                acting_assignment_id,
                MutationRequest::Create(CreateMutation { object }),
            );
            command.initial_role_level = role_level;
            client.sign_event_exact(build_project_object_command(command).map_err(sdk_err)?)?
        }
        ProjectViewSchema::V3 => {
            let object = create_input_v3(object_type, object_id, data)?;
            let mut command = ProjectObjectCommandV3::new(
                expected_project_revision,
                acting_assignment_id,
                ProjectObjectRequestV3::Create(CreateProjectObjectV3 { object }),
            );
            command.initial_role_level = role_level;
            client.sign_event_exact(build_project_object_command_v3(command).map_err(sdk_err)?)?
        }
    };
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_object_receipt(&raw, &event, "create", object_type, object_id, false)?;
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
    let patch = read_json_value(patch_path, "patch")?;
    let acting_assignment_id = if object_type == ProjectViewObjectType::Role {
        let governance = read_role_governance(client, identity).await?;
        let level = governance.role_level(object_id)?;
        governance.authorize(level)?
    } else {
        None
    };
    let event = match identity.schema {
        ProjectViewSchema::V1 => {
            if object_type == ProjectViewObjectType::Role {
                return Err(CliError::Other(
                    "unsupported: governed Role updates require Project View schema v2 or v3"
                        .to_owned(),
                ));
            }
            let update = update_input(object_type, object_id, patch)?;
            client.sign_event_exact(
                build_update(expected_project_revision, update).map_err(sdk_err)?,
            )?
        }
        ProjectViewSchema::V2 => {
            let update = update_input(object_type, object_id, patch)?;
            let command = ProjectObjectCommand::new(
                expected_project_revision,
                acting_assignment_id,
                MutationRequest::Update(update),
            );
            client.sign_event_exact(build_project_object_command(command).map_err(sdk_err)?)?
        }
        ProjectViewSchema::V3 => {
            let update = update_input_v3(object_type, object_id, patch)?;
            let command = ProjectObjectCommandV3::new(
                expected_project_revision,
                acting_assignment_id,
                ProjectObjectRequestV3::Update(update),
            );
            client.sign_event_exact(build_project_object_command_v3(command).map_err(sdk_err)?)?
        }
    };
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_object_receipt(&raw, &event, "update", object_type, object_id, false)?;
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
    let acting_assignment_id = if object_type == ProjectViewObjectType::Role {
        let governance = read_role_governance(client, identity).await?;
        let level = governance.role_level(object_id)?;
        governance.authorize(level)?
    } else {
        None
    };
    let event = match identity.schema {
        ProjectViewSchema::V1 => {
            if object_type == ProjectViewObjectType::Role {
                return Err(CliError::Other(
                    "unsupported: governed Role deletion requires Project View schema v2 or v3"
                        .to_owned(),
                ));
            }
            client.sign_event_exact(
                build_delete(expected_project_revision, object_type, object_id).map_err(sdk_err)?,
            )?
        }
        ProjectViewSchema::V2 => {
            let command = ProjectObjectCommand::new(
                expected_project_revision,
                acting_assignment_id,
                MutationRequest::Delete(DeleteMutation {
                    object_type,
                    object_id,
                }),
            );
            client.sign_event_exact(build_project_object_command(command).map_err(sdk_err)?)?
        }
        ProjectViewSchema::V3 => {
            let command = ProjectObjectCommandV3::new(
                expected_project_revision,
                acting_assignment_id,
                ProjectObjectRequestV3::Delete(DeleteProjectObjectV3 {
                    object_type,
                    object_id,
                }),
            );
            client.sign_event_exact(build_project_object_command_v3(command).map_err(sdk_err)?)?
        }
    };
    let raw = submit_mutation(client, event.clone()).await?;
    let receipt = parse_object_receipt(&raw, &event, "delete", object_type, object_id, true)?;
    confirm_object_receipt(client, identity, object_type, object_id, &receipt).await?;
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
    match identity.schema {
        ProjectViewSchema::V1 => Err(CliError::Other(
            "unsupported: governed Role mutations require Project View schema v2 or v3".to_owned(),
        )),
        ProjectViewSchema::V2 => {
            let snapshot = read_verified_v2_snapshot(client, identity).await?;
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
            Ok(CliRoleGovernance {
                is_owner: membership_role == Some(CommunityMemberRole::Owner),
                leader_assignment_id,
                role_levels: role_levels
                    .into_iter()
                    .map(|(role_id, (level, _))| (role_id, level))
                    .collect(),
            })
        }
        ProjectViewSchema::V3 => {
            let snapshot = read_verified_v3_snapshot(client, identity).await?;
            Ok(role_governance_from_v3(actor, &snapshot))
        }
    }
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

async fn require_capability(client: &BuzzClient) -> Result<ProjectViewIdentity, CliError> {
    read_identity(client).await?.ok_or_else(|| {
        CliError::Other(format!(
            "unsupported: relay does not advertise {PROJECT_VIEW_V1_EXTENSION}, buzz-project-view-v2, or buzz-project-view-v3"
        ))
    })
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

fn v2_object_output(entry: &ProjectViewEntry, meta: &V2MetaProjection) -> Value {
    match entry {
        ProjectViewEntry::Active(object) => json!({
            "project_revision": meta.project_revision,
            "projection_generation": meta.projection_generation,
            "deleted": false,
            "object": object,
        }),
        ProjectViewEntry::Tombstone(tombstone) => json!({
            "project_revision": meta.project_revision,
            "projection_generation": meta.projection_generation,
            "deleted": true,
            "tombstone": {
                "object_id": tombstone.id,
                "object_type": tombstone.object_type,
                "object_revision": tombstone.object_revision,
                "project_revision": tombstone.project_revision,
                "created_at": tombstone.created_at,
                "deleted_at": tombstone.deleted_at,
                "created_by": tombstone.created_by,
                "deleted_by": tombstone.deleted_by,
            },
        }),
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
    serde_json::from_value(parse_receipt_value(raw, event)?)
        .map_err(|error| integrity_error(format!("invalid mutation receipt: {error}")))
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
) -> Result<ProjectViewReceipt, CliError> {
    let value = parse_receipt_value(raw, event)?;
    if value.get("schema_version").is_some() {
        let receipt: ProjectViewObjectReceiptV3 = serde_json::from_value(value)
            .map_err(|error| integrity_error(format!("invalid v3 mutation receipt: {error}")))?;
        let [object] = receipt.objects.as_slice() else {
            return Err(integrity_error(
                "v3 mutation receipt must contain exactly one changed object",
            ));
        };
        if receipt.schema_version != 3
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
        return Ok(ProjectViewReceipt {
            project_revision: receipt.project_revision,
            object_id: Some(object.object_id),
            object_revision: Some(object.object_revision),
            deleted: Some(object.deleted),
        });
    }

    let receipt: ProjectViewReceipt = serde_json::from_value(value)
        .map_err(|error| integrity_error(format!("invalid mutation receipt: {error}")))?;
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
    if identity.schema == ProjectViewSchema::V3 {
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
            || entry.object_revision() < receipt.object_revision.unwrap_or_default()
        {
            return Err(integrity_error(
                "v3 object projection does not confirm the mutation receipt",
            ));
        }
        if receipt.deleted == Some(true) && !matches!(entry, ProjectViewEntryV3::Tombstone(_)) {
            return Err(integrity_error(
                "v3 delete receipt was not confirmed by a tombstone projection",
            ));
        }
        return Ok(());
    }
    if identity.schema == ProjectViewSchema::V2 {
        let snapshot = read_verified_v2_snapshot(client, identity).await?;
        if snapshot.meta().project_revision < receipt.project_revision {
            return Err(integrity_error(
                "v2 metadata projection is older than the successful mutation receipt",
            ));
        }
        let entry = snapshot
            .entry(object_id)
            .ok_or_else(|| integrity_error("successful v2 mutation has no object projection"))?;
        if entry.object_type() != object_type
            || entry.object_revision() < receipt.object_revision.unwrap_or_default()
        {
            return Err(integrity_error(
                "v2 object projection does not confirm the mutation receipt",
            ));
        }
        if receipt.deleted == Some(true) && !matches!(entry, ProjectViewEntry::Tombstone(_)) {
            return Err(integrity_error(
                "v2 delete receipt was not confirmed by a tombstone projection",
            ));
        }
        return Ok(());
    }
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
    use nostr::{EventBuilder, Keys, Kind};
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
        assert_eq!(receipt.object_id, Some(object_id));
        assert_eq!(receipt.object_revision, Some(3));
        assert_eq!(receipt.deleted, Some(false));
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
    fn legacy_flat_object_receipt_remains_supported() {
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

        let receipt = parse_object_receipt(
            &raw,
            &event,
            "update",
            ProjectViewObjectType::Role,
            object_id,
            false,
        )
        .expect("parse legacy object receipt");
        assert_eq!(receipt.project_revision, 5);
        assert_eq!(receipt.object_revision, Some(2));
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
            "supported_extensions": [PROJECT_VIEW_V1_EXTENSION],
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
            "supported_extensions": [PROJECT_VIEW_V1_EXTENSION],
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
