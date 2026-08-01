//! Resource, Context, greenfield initialization, and Role Brief v3 shapes.

use std::cmp::Ordering;
use std::collections::HashSet;

use buzz_core::{EventId, PublicKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::{Uuid, Variant};

use super::PROJECT_VIEW_V3_SCHEMA_VERSION;
use crate::v2::RoleLevel;
use crate::{
    InitializeGoal, ProjectProfile, ProjectViewObjectData, MAX_INITIAL_GOALS, MAX_SAFE_REVISION,
};

/// NIP-11 capability for a ready Project View v3 Community.
pub const PROJECT_VIEW_V3_CAPABILITY: &str = "buzz-project-view-v3";
/// NIP-11 sub-capability enabling non-empty Context Reference writes.
pub const PROJECT_CONTEXT_CAPABILITY: &str = "buzz-project-context-v1";
/// Maximum canonical Context References on one Project View object.
pub const MAX_CONTEXT_REFERENCES: usize = 64;

const MAX_RESOURCE_NAME_BYTES: usize = 256;
const MAX_RESOURCE_KIND_BYTES: usize = 64;
const MAX_RESOURCE_SUMMARY_BYTES: usize = 4_096;

/// Fail-closed Project View v3 contract error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum V3ContractError {
    /// A closed JSON shape or major version did not match v3.
    #[error("invalid Project View v3 wire: {0}")]
    InvalidWire(String),
    /// A Resource body violated v3 canonical rules.
    #[error("invalid Project View v3 Resource: {0}")]
    InvalidResource(String),
    /// A Context Reference set violated shape, target, or canonical ordering.
    #[error("invalid Project View v3 Context Reference: {0}")]
    InvalidContext(String),
    /// A greenfield initialization payload violated its closed bootstrap shape.
    #[error("invalid Project View v3 initialization: {0}")]
    InvalidInitialization(String),
}

/// Project Resource v3 business body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectResourceV3 {
    /// Human-readable Resource name.
    pub name: String,
    /// Open canonical token describing the Resource kind.
    pub resource_kind: String,
    /// Optional short description; omitted rather than encoded as `null`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
    /// Required active Project Document containing the operational Guide.
    pub guide_document_id: Uuid,
}

impl ProjectResourceV3 {
    /// Parse the strict v3 body and apply canonical field validation.
    pub fn from_json(json: &str) -> Result<Self, V3ContractError> {
        let resource: Self = serde_json::from_str(json)
            .map_err(|error| V3ContractError::InvalidWire(error.to_string()))?;
        resource.validate()?;
        Ok(resource)
    }

    /// Validate Resource fields that do not require a Document target lookup.
    pub fn validate(&self) -> Result<(), V3ContractError> {
        validate_nonempty_text(
            "name",
            &self.name,
            MAX_RESOURCE_NAME_BYTES,
            V3ContractError::InvalidResource,
        )?;
        validate_resource_kind(&self.resource_kind)?;
        if let Some(summary) = &self.summary {
            validate_optional_text(
                "summary",
                summary,
                MAX_RESOURCE_SUMMARY_BYTES,
                V3ContractError::InvalidResource,
            )?;
        }
        require_uuid_v4(self.guide_document_id, "guide_document_id")
            .map_err(V3ContractError::InvalidResource)
    }
}

/// Document Context Reference behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentReferenceMode {
    /// Resolve the current active Document head when the Brief is assembled.
    Live,
    /// Resolve one exact historical active-content revision.
    Pinned,
}

/// Closed Context Reference union carried by every v3 Project View object.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectContextReference {
    /// Refer to another active Resource in the same Project.
    Resource {
        /// Stable Resource object identity.
        resource_id: Uuid,
    },
    /// Refer to Project Document metadata, live or at one pinned revision.
    Document {
        /// Stable Project Document identity.
        document_id: Uuid,
        /// Live or pinned resolution mode.
        mode: DocumentReferenceMode,
        /// Required only in pinned mode; explicit JSON `null` is rejected.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_optional_non_null"
        )]
        document_revision: Option<u64>,
    },
}

impl ProjectContextReference {
    /// Validate identity and live/pinned shape without resolving the target.
    pub fn validate(&self) -> Result<(), V3ContractError> {
        match self {
            Self::Resource { resource_id } => require_uuid_v4(*resource_id, "resource_id")
                .map_err(V3ContractError::InvalidContext),
            Self::Document {
                document_id,
                mode,
                document_revision,
            } => {
                require_uuid_v4(*document_id, "document_id")
                    .map_err(V3ContractError::InvalidContext)?;
                match (mode, document_revision) {
                    (DocumentReferenceMode::Live, None) => Ok(()),
                    (DocumentReferenceMode::Pinned, Some(revision))
                        if (1..=MAX_SAFE_REVISION).contains(revision) =>
                    {
                        Ok(())
                    }
                    (DocumentReferenceMode::Live, Some(_)) => Err(V3ContractError::InvalidContext(
                        "live Document reference must omit document_revision".to_owned(),
                    )),
                    (DocumentReferenceMode::Pinned, None) => Err(V3ContractError::InvalidContext(
                        "pinned Document reference requires document_revision".to_owned(),
                    )),
                    (DocumentReferenceMode::Pinned, Some(_)) => {
                        Err(V3ContractError::InvalidContext(
                            "pinned document_revision must be JavaScript-safe and positive"
                                .to_owned(),
                        ))
                    }
                }
            }
        }
    }

    fn canonical_cmp(&self, other: &Self) -> Ordering {
        context_key(self).cmp(&context_key(other))
    }
}

/// Validate and canonicalize a Context Reference set.
///
/// Ordering is Resource before Document, UUID bytes, live before pinned, then
/// pinned revision. Exact duplicate coordinates are rejected.
pub fn canonicalize_context_references(
    mut references: Vec<ProjectContextReference>,
) -> Result<Vec<ProjectContextReference>, V3ContractError> {
    if references.len() > MAX_CONTEXT_REFERENCES {
        return Err(V3ContractError::InvalidContext(format!(
            "at most {MAX_CONTEXT_REFERENCES} references are allowed"
        )));
    }
    for reference in &references {
        reference.validate()?;
    }
    references.sort_by(ProjectContextReference::canonical_cmp);
    if references.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(V3ContractError::InvalidContext(
            "duplicate Context Reference coordinate".to_owned(),
        ));
    }
    Ok(references)
}

/// Complete canonical Role definition projected by a v3 Relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleDefinitionV3 {
    /// Stable Project Role identity.
    pub role_id: Uuid,
    /// Human-readable Role name.
    pub name: String,
    /// Why this responsibility position exists.
    pub purpose: String,
    /// Responsibilities owned by the Role.
    pub responsibilities: Vec<String>,
    /// Explicit Role boundaries.
    pub boundaries: Vec<String>,
    /// Community permission granted by an active Assignment.
    pub level: RoleLevel,
    /// Whether the Role can receive an Assignment.
    pub active: bool,
    /// Canonical Context Reference set.
    pub context_references: Vec<ProjectContextReference>,
    /// Canonical object revision.
    pub object_revision: u64,
    /// Project revision at which this Role was last changed.
    pub project_revision: u64,
    /// Canonical creation time.
    pub created_at: DateTime<Utc>,
    /// Canonical update time.
    pub updated_at: DateTime<Utc>,
    /// Verified Role creator.
    pub created_by: PublicKey,
    /// Verified latest Role editor.
    pub updated_by: PublicKey,
}

impl RoleDefinitionV3 {
    /// Validate the closed canonical Role definition.
    pub fn validate(&self) -> Result<(), V3ContractError> {
        require_uuid_v4(self.role_id, "role_id").map_err(V3ContractError::InvalidWire)?;
        validate_nonempty_text("name", &self.name, 256, V3ContractError::InvalidWire)?;
        validate_nonempty_text(
            "purpose",
            &self.purpose,
            32_768,
            V3ContractError::InvalidWire,
        )?;
        validate_string_list("responsibilities", &self.responsibilities)?;
        validate_string_list("boundaries", &self.boundaries)?;
        validate_positive_safe(self.object_revision, "object_revision")?;
        validate_positive_safe(self.project_revision, "project_revision")?;
        if self.updated_at < self.created_at {
            return Err(V3ContractError::InvalidWire(
                "updated_at precedes created_at".to_owned(),
            ));
        }
        require_canonical_context(&self.context_references)
    }
}

/// Complete client-supplied Role definition in greenfield v3 initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialRoleDefinitionV3 {
    /// Client-generated Role UUID v4.
    pub role_id: Uuid,
    /// Human-readable Role name.
    pub name: String,
    /// Why this responsibility position exists.
    pub purpose: String,
    /// Responsibilities owned by the Role.
    pub responsibilities: Vec<String>,
    /// Explicit Role boundaries.
    pub boundaries: Vec<String>,
    /// Initial Community permission level; greenfield governance roles are admin.
    pub level: RoleLevel,
    /// Initial active state; greenfield governance roles must be active.
    pub active: bool,
    /// Must be the canonical empty set while Context is not advertised.
    pub context_references: Vec<ProjectContextReference>,
}

/// One consumed Proposal and active Human Assignment seeded at initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialGovernanceAssignmentV3 {
    /// Current direct Human owner or admin.
    pub member_pubkey: PublicKey,
    /// Distinct active admin-level Role assigned to this member.
    pub role_id: Uuid,
    /// Client-generated consumed Proposal UUID v4.
    pub proposal_id: Uuid,
    /// Client-generated active Assignment UUID v4.
    pub assignment_id: Uuid,
}

/// Closed greenfield v3 initialization command envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectViewInitializeV3 {
    /// Must equal three.
    pub schema_version: u16,
    /// Must be zero for an uninitialized Project View.
    pub expected_project_revision: u64,
    /// The only request accepted by the prepared-but-disabled bootstrap path.
    pub request: ProjectViewInitializeV3Request,
}

/// Closed v3 bootstrap request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectViewInitializeV3Request {
    /// Atomically seed Project objects and Human governance continuity.
    Initialize {
        /// Exact unconsumed `prepare_v3` provisioning operation.
        preparation_operation_id: Uuid,
        /// Complete Project Profile body.
        profile: ProjectProfile,
        /// Initial Goals with client-generated identities.
        goals: Vec<InitializeGoal>,
        /// Complete distinct active admin Role definitions.
        initial_roles: Vec<InitialRoleDefinitionV3>,
        /// Complete owner/admin-to-Role governance mapping.
        initial_governance_assignments: Vec<InitialGovernanceAssignmentV3>,
    },
}

impl ProjectViewInitializeV3 {
    /// Parse a strict greenfield v3 command and validate its local invariants.
    pub fn from_json(json: &str) -> Result<Self, V3ContractError> {
        let command: Self = serde_json::from_str(json)
            .map_err(|error| V3ContractError::InvalidWire(error.to_string()))?;
        command.validate()?;
        Ok(command)
    }

    /// Validate all bootstrap rules that do not require current membership.
    pub fn validate(&self) -> Result<(), V3ContractError> {
        if self.schema_version != PROJECT_VIEW_V3_SCHEMA_VERSION {
            return Err(V3ContractError::InvalidInitialization(format!(
                "schema_version must be {PROJECT_VIEW_V3_SCHEMA_VERSION}"
            )));
        }
        if self.expected_project_revision != 0 {
            return Err(V3ContractError::InvalidInitialization(
                "expected_project_revision must be zero".to_owned(),
            ));
        }
        let ProjectViewInitializeV3Request::Initialize {
            preparation_operation_id,
            profile,
            goals,
            initial_roles,
            initial_governance_assignments,
        } = &self.request;
        require_uuid_v4(*preparation_operation_id, "preparation_operation_id")
            .map_err(V3ContractError::InvalidInitialization)?;
        crate::validation::validate_data(&ProjectViewObjectData::ProjectProfile(profile.clone()))
            .map_err(|error| V3ContractError::InvalidInitialization(error.to_string()))?;
        if goals.len() > MAX_INITIAL_GOALS {
            return Err(V3ContractError::InvalidInitialization(format!(
                "at most {MAX_INITIAL_GOALS} initial Goals are allowed"
            )));
        }
        let mut object_ids = HashSet::with_capacity(goals.len() + initial_roles.len());
        for goal in goals {
            require_uuid_v4(goal.id, "goal.id").map_err(V3ContractError::InvalidInitialization)?;
            if !object_ids.insert(goal.id) {
                return Err(V3ContractError::InvalidInitialization(
                    "initial Goal and Role IDs must be globally unique".to_owned(),
                ));
            }
            crate::validation::validate_data(&ProjectViewObjectData::Goal(
                goal.clone().into_goal(),
            ))
            .map_err(|error| V3ContractError::InvalidInitialization(error.to_string()))?;
        }
        if initial_roles.is_empty() || initial_governance_assignments.is_empty() {
            return Err(V3ContractError::InvalidInitialization(
                "initial_roles and initial_governance_assignments must be non-empty".to_owned(),
            ));
        }
        let mut role_ids = HashSet::with_capacity(initial_roles.len());
        for role in initial_roles {
            validate_initial_role(role)?;
            if !role_ids.insert(role.role_id) || !object_ids.insert(role.role_id) {
                return Err(V3ContractError::InvalidInitialization(
                    "initial Goal and Role IDs must be globally unique".to_owned(),
                ));
            }
        }
        let mut members = HashSet::with_capacity(initial_governance_assignments.len());
        let mut assigned_roles = HashSet::with_capacity(initial_governance_assignments.len());
        let mut proposals = HashSet::with_capacity(initial_governance_assignments.len());
        let mut assignments = HashSet::with_capacity(initial_governance_assignments.len());
        for assignment in initial_governance_assignments {
            for (id, field) in [
                (assignment.role_id, "role_id"),
                (assignment.proposal_id, "proposal_id"),
                (assignment.assignment_id, "assignment_id"),
            ] {
                require_uuid_v4(id, field).map_err(V3ContractError::InvalidInitialization)?;
            }
            if !role_ids.contains(&assignment.role_id) {
                return Err(V3ContractError::InvalidInitialization(
                    "every governance assignment must target an initial Role".to_owned(),
                ));
            }
            if !members.insert(assignment.member_pubkey)
                || !assigned_roles.insert(assignment.role_id)
                || !proposals.insert(assignment.proposal_id)
                || !assignments.insert(assignment.assignment_id)
            {
                return Err(V3ContractError::InvalidInitialization(
                    "members, Roles, Proposals, and Assignments must each be unique".to_owned(),
                ));
            }
        }
        if assigned_roles != role_ids {
            return Err(V3ContractError::InvalidInitialization(
                "every initial governance Role must be assigned exactly once".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Context availability state in a base Role Brief v3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextAvailabilityV3 {
    /// Context was not advertised and the canonical set is empty.
    NotAdvertisedEmpty,
    /// Context is advertised and the supplied metadata is verified.
    Ready,
    /// Context coordinates remain canonical but are intentionally not injected.
    UnavailablePreserved {
        /// Number of preserved Resource coordinates.
        resource_count: u32,
        /// Number of preserved live and pinned Document coordinates.
        document_count: u32,
    },
}

/// Resource metadata exposed in a Role Brief v3 Context slice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextResourceV3 {
    /// Stable Resource identity.
    pub resource_id: Uuid,
    /// Untrusted project-provided name.
    pub name: String,
    /// Open Resource kind token.
    pub resource_kind: String,
    /// Optional untrusted project-provided summary.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
    /// Mandatory Guide Document identity.
    pub guide_document_id: Uuid,
    /// Current verified Guide revision, omitted when metadata is unavailable.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub guide_document_revision: Option<u64>,
    /// Explicit CLI command for opt-in body retrieval.
    pub fetch: String,
    /// Optional descriptive metadata did not fit the escaped prompt budget.
    pub metadata_omitted_due_to_budget: bool,
}

/// Live Document metadata exposed in a Role Brief v3 Context slice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextLiveDocumentV3 {
    /// Stable Document identity.
    pub document_id: Uuid,
    /// Verified current active revision, omitted when metadata is unavailable.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub document_revision: Option<u64>,
    /// Untrusted current title, omitted when metadata is unavailable.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub title: Option<String>,
    /// Untrusted current optional summary.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
    /// Explicit CLI command for opt-in body retrieval.
    pub fetch: String,
    /// Optional descriptive metadata did not fit the escaped prompt budget.
    pub metadata_omitted_due_to_budget: bool,
}

/// Pinned Document coordinate exposed without current metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextPinnedDocumentV3 {
    /// Stable Document identity.
    pub document_id: Uuid,
    /// Exact verified active-content revision.
    pub document_revision: u64,
    /// Explicit CLI command for opt-in pinned body retrieval.
    pub fetch: String,
}

/// Bounded Context selection result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextTruncationV3 {
    /// Whether at least one eligible coordinate was omitted by the budget.
    pub truncated: bool,
    /// Number of omitted Resource items.
    pub omitted_resources: u32,
    /// Number of omitted live Document items.
    pub omitted_live_documents: u32,
    /// Number of omitted pinned Document items.
    pub omitted_pinned_documents: u32,
}

/// Additive Context section present in every Role Brief v3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefContextV3 {
    /// Capability/degradation state.
    pub availability: ContextAvailabilityV3,
    /// Verified Resource metadata selected for the bounded slice.
    pub resources: Vec<ContextResourceV3>,
    /// Verified live Document metadata selected for the bounded slice.
    pub live_documents: Vec<ContextLiveDocumentV3>,
    /// Verified pinned Document coordinates selected for the bounded slice.
    pub pinned_documents: Vec<ContextPinnedDocumentV3>,
    /// Deterministic selection-budget result.
    pub truncation: ContextTruncationV3,
}

/// Document metadata boundary used to assemble a Role Brief v3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentMetadataSourceV3 {
    /// No metadata was needed because Context was empty or unavailable.
    NotRequired,
    /// Exact verified Project Document catalog metadata.
    Verified {
        /// Signed Document meta event.
        meta_event_id: EventId,
        /// Verified Document catalog revision.
        catalog_revision: u64,
        /// Verified Document projection signer generation.
        projection_generation: u64,
    },
    /// Context was advertised but Document metadata could not be verified.
    Unavailable,
}

/// Snapshot boundaries embedded in every Role Brief v3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefSourceRevisionsV3 {
    /// Exact Project View metadata head.
    pub meta_event_id: EventId,
    /// Stable Project View change represented by that head.
    pub meta_change_id: EventId,
    /// Exact NIP-43 membership snapshot referenced by metadata.
    pub membership_event_id: EventId,
    /// Canonical update time of the Project View metadata head.
    pub project_updated_at: DateTime<Utc>,
    /// Project Document metadata boundary used for Context.
    pub document_metadata: DocumentMetadataSourceV3,
}

fn validate_initial_role(role: &InitialRoleDefinitionV3) -> Result<(), V3ContractError> {
    require_uuid_v4(role.role_id, "role_id").map_err(V3ContractError::InvalidInitialization)?;
    validate_nonempty_text(
        "name",
        &role.name,
        256,
        V3ContractError::InvalidInitialization,
    )?;
    validate_nonempty_text(
        "purpose",
        &role.purpose,
        32_768,
        V3ContractError::InvalidInitialization,
    )?;
    validate_string_list("responsibilities", &role.responsibilities)?;
    validate_string_list("boundaries", &role.boundaries)?;
    if role.level != RoleLevel::Admin || !role.active || !role.context_references.is_empty() {
        return Err(V3ContractError::InvalidInitialization(
            "initial governance Roles must be active admin Roles with empty Context".to_owned(),
        ));
    }
    Ok(())
}

fn require_canonical_context(
    references: &[ProjectContextReference],
) -> Result<(), V3ContractError> {
    let canonical = canonicalize_context_references(references.to_vec())?;
    if canonical != references {
        return Err(V3ContractError::InvalidContext(
            "Context References are not in canonical order".to_owned(),
        ));
    }
    Ok(())
}

fn validate_resource_kind(value: &str) -> Result<(), V3ContractError> {
    if value.is_empty()
        || value.len() > MAX_RESOURCE_KIND_BYTES
        || value.contains('\0')
        || !value.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' | b'0'..=b'9' => true,
            b'.' | b'_' | b'-' => index > 0,
            _ => false,
        })
    {
        return Err(V3ContractError::InvalidResource(
            "resource_kind must match [a-z0-9][a-z0-9._-]{0,63}".to_owned(),
        ));
    }
    Ok(())
}

fn validate_string_list(field: &str, values: &[String]) -> Result<(), V3ContractError> {
    for value in values {
        validate_nonempty_text(field, value, 32_768, V3ContractError::InvalidWire)?;
    }
    Ok(())
}

fn validate_nonempty_text(
    field: &str,
    value: &str,
    max: usize,
    error: fn(String) -> V3ContractError,
) -> Result<(), V3ContractError> {
    if value.is_empty() || value.trim() != value || value.contains('\0') || value.len() > max {
        return Err(error(format!(
            "{field} must be canonical non-empty text of at most {max} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn validate_optional_text(
    field: &str,
    value: &str,
    max: usize,
    error: fn(String) -> V3ContractError,
) -> Result<(), V3ContractError> {
    if value.is_empty() || value.contains('\0') || value.len() > max {
        return Err(error(format!(
            "{field} must be omitted or contain 1..={max} UTF-8 bytes without NUL"
        )));
    }
    Ok(())
}

fn validate_positive_safe(value: u64, field: &str) -> Result<(), V3ContractError> {
    if !(1..=MAX_SAFE_REVISION).contains(&value) {
        return Err(V3ContractError::InvalidWire(format!(
            "{field} must be JavaScript-safe and positive"
        )));
    }
    Ok(())
}

fn require_uuid_v4(value: Uuid, field: &str) -> Result<(), String> {
    if value.is_nil() || value.get_version_num() != 4 || value.get_variant() != Variant::RFC4122 {
        return Err(format!("{field} must be an RFC 4122 UUID v4"));
    }
    Ok(())
}

fn context_key(reference: &ProjectContextReference) -> (u8, [u8; 16], u8, u64) {
    match reference {
        ProjectContextReference::Resource { resource_id } => (0, *resource_id.as_bytes(), 0, 0),
        ProjectContextReference::Document {
            document_id,
            mode: DocumentReferenceMode::Live,
            ..
        } => (1, *document_id.as_bytes(), 0, 0),
        ProjectContextReference::Document {
            document_id,
            mode: DocumentReferenceMode::Pinned,
            document_revision,
        } => (
            1,
            *document_id.as_bytes(),
            1,
            document_revision.unwrap_or(0),
        ),
    }
}

fn deserialize_optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
