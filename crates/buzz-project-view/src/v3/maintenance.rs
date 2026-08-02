//! Durable Project View v3 maintenance state-machine contract.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::MAX_SAFE_REVISION;

/// Closed schema number for runtime-supervisor maintenance acknowledgements.
pub const PROJECT_VIEW_MAINTENANCE_ACK_SCHEMA_VERSION: u16 = 1;
/// Closed schema number for bounded mechanical v3 repair plans.
pub const PROJECT_VIEW_MAINTENANCE_REPAIR_SCHEMA_VERSION: u16 = 1;
/// Maximum actions accepted in one atomic repair.
pub const MAX_MAINTENANCE_REPAIR_ACTIONS: usize = 4_096;
/// Maximum bytes accepted by the closed Human-readable repair-plan envelope.
pub const MAX_MAINTENANCE_REPAIR_PLAN_JSON_BYTES: usize = 2 * 1024 * 1024;
/// Domain separator for the canonical postcard repair-plan digest.
pub const MAINTENANCE_REPAIR_PLAN_DIGEST_DOMAIN: &[u8] = b"buzz-pv3-maintenance-repair-plan-v1\0";

/// Closed, bounded plan for a monotonic schema-v3 mechanical repair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalMaintenanceRepairPlanV1 {
    /// Must equal [`PROJECT_VIEW_MAINTENANCE_REPAIR_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// Exact Community/Project identity.
    pub community_id: [u8; 16],
    /// Exact frozen maintenance epoch.
    pub maintenance_epoch: u64,
    /// Immutable cutover change ID.
    pub cutover_change_id: [u8; 32],
    /// Exact canonical revision before repair.
    pub expected_project_revision: u64,
    /// Exact projection generation before repair.
    pub expected_projection_generation: u64,
    /// Canonically sorted mechanical actions.
    pub actions: Vec<RepairActionV1>,
}

/// Mechanical repairs reconstructible from immutable evidence only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepairActionV1 {
    /// Reapply one exact committed reviewed Resource mapping.
    ReapplyCommittedResource {
        /// Stable Resource identity.
        resource_id: [u8; 16],
        /// Exact immutable mapping-entry digest.
        mapping_entry_digest: [u8; 32],
    },
    /// Repoint/rebuild object source provenance from immutable source evidence.
    RebuildObjectProvenance {
        /// Stable object identity.
        object_id: [u8; 16],
        /// Digest of the exact current business body.
        expected_business_body_digest: [u8; 32],
        /// Digest of the exact immutable source evidence.
        expected_source_digest: [u8; 32],
    },
    /// Rebuild normalized Context rows from the exact current closed body.
    RebuildNormalizedContext {
        /// Stable object identity.
        object_id: [u8; 16],
        /// Digest of the exact current business body.
        expected_business_body_digest: [u8; 32],
    },
}

/// Closed Human-readable JSON envelope for a canonical repair plan.
///
/// UUIDs and digests use canonical strings here; hashing always reconstructs
/// [`CanonicalMaintenanceRepairPlanV1`] and therefore never depends on JSON
/// whitespace or object-key ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalMaintenanceRepairPlanEnvelopeV1 {
    /// Must equal [`PROJECT_VIEW_MAINTENANCE_REPAIR_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// Exact Community/Project UUID.
    pub community_id: Uuid,
    /// Exact frozen maintenance epoch.
    pub maintenance_epoch: u64,
    /// Lowercase hex immutable cutover change ID.
    pub cutover_change_id: String,
    /// Exact canonical revision before repair.
    pub expected_project_revision: u64,
    /// Exact projection generation before repair.
    pub expected_projection_generation: u64,
    /// Canonically sorted bounded mechanical actions.
    pub actions: Vec<RepairActionEnvelopeV1>,
}

/// Human-readable form of one closed mechanical repair action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepairActionEnvelopeV1 {
    /// Reapply one exact committed reviewed Resource mapping.
    ReapplyCommittedResource {
        /// Stable Resource UUID.
        resource_id: Uuid,
        /// Lowercase hex immutable mapping-entry digest.
        mapping_entry_digest: String,
    },
    /// Rebuild source provenance from immutable source evidence.
    RebuildObjectProvenance {
        /// Stable object UUID.
        object_id: Uuid,
        /// Lowercase hex digest of the exact current business body.
        expected_business_body_digest: String,
        /// Lowercase hex digest of the exact immutable source evidence.
        expected_source_digest: String,
    },
    /// Rebuild normalized Context rows from the current closed body.
    RebuildNormalizedContext {
        /// Stable object UUID.
        object_id: Uuid,
        /// Lowercase hex digest of the exact current business body.
        expected_business_body_digest: String,
    },
}

impl CanonicalMaintenanceRepairPlanEnvelopeV1 {
    /// Parse bounded closed JSON and reconstruct the canonical postcard value.
    pub fn parse_json(
        bytes: &[u8],
    ) -> Result<CanonicalMaintenanceRepairPlanV1, MaintenanceContractError> {
        if bytes.len() > MAX_MAINTENANCE_REPAIR_PLAN_JSON_BYTES {
            return Err(MaintenanceContractError::InvalidRepairPlan(format!(
                "repair-plan JSON exceeds {MAX_MAINTENANCE_REPAIR_PLAN_JSON_BYTES} bytes"
            )));
        }
        let envelope: Self = serde_json::from_slice(bytes)
            .map_err(|error| MaintenanceContractError::InvalidRepairPlan(error.to_string()))?;
        envelope.into_canonical()
    }

    /// Convert the Human envelope into the exact binary contract.
    pub fn into_canonical(
        self,
    ) -> Result<CanonicalMaintenanceRepairPlanV1, MaintenanceContractError> {
        let plan = CanonicalMaintenanceRepairPlanV1 {
            schema_version: self.schema_version,
            community_id: *self.community_id.as_bytes(),
            maintenance_epoch: self.maintenance_epoch,
            cutover_change_id: decode_lower_hex_32(&self.cutover_change_id, "cutover_change_id")?,
            expected_project_revision: self.expected_project_revision,
            expected_projection_generation: self.expected_projection_generation,
            actions: self
                .actions
                .into_iter()
                .map(RepairActionEnvelopeV1::into_canonical)
                .collect::<Result<Vec<_>, _>>()?,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Produce a deterministic pretty JSON envelope from a canonical plan.
    pub fn to_pretty_json(
        plan: &CanonicalMaintenanceRepairPlanV1,
    ) -> Result<Vec<u8>, MaintenanceContractError> {
        plan.validate()?;
        let envelope = Self {
            schema_version: plan.schema_version,
            community_id: Uuid::from_bytes(plan.community_id),
            maintenance_epoch: plan.maintenance_epoch,
            cutover_change_id: hex::encode(plan.cutover_change_id),
            expected_project_revision: plan.expected_project_revision,
            expected_projection_generation: plan.expected_projection_generation,
            actions: plan
                .actions
                .iter()
                .map(RepairActionEnvelopeV1::from_canonical)
                .collect(),
        };
        serde_json::to_vec_pretty(&envelope)
            .map_err(|error| MaintenanceContractError::InvalidRepairPlan(error.to_string()))
    }
}

impl RepairActionEnvelopeV1 {
    fn into_canonical(self) -> Result<RepairActionV1, MaintenanceContractError> {
        match self {
            Self::ReapplyCommittedResource {
                resource_id,
                mapping_entry_digest,
            } => Ok(RepairActionV1::ReapplyCommittedResource {
                resource_id: *resource_id.as_bytes(),
                mapping_entry_digest: decode_lower_hex_32(
                    &mapping_entry_digest,
                    "mapping_entry_digest",
                )?,
            }),
            Self::RebuildObjectProvenance {
                object_id,
                expected_business_body_digest,
                expected_source_digest,
            } => Ok(RepairActionV1::RebuildObjectProvenance {
                object_id: *object_id.as_bytes(),
                expected_business_body_digest: decode_lower_hex_32(
                    &expected_business_body_digest,
                    "expected_business_body_digest",
                )?,
                expected_source_digest: decode_lower_hex_32(
                    &expected_source_digest,
                    "expected_source_digest",
                )?,
            }),
            Self::RebuildNormalizedContext {
                object_id,
                expected_business_body_digest,
            } => Ok(RepairActionV1::RebuildNormalizedContext {
                object_id: *object_id.as_bytes(),
                expected_business_body_digest: decode_lower_hex_32(
                    &expected_business_body_digest,
                    "expected_business_body_digest",
                )?,
            }),
        }
    }

    fn from_canonical(action: &RepairActionV1) -> Self {
        match action {
            RepairActionV1::ReapplyCommittedResource {
                resource_id,
                mapping_entry_digest,
            } => Self::ReapplyCommittedResource {
                resource_id: Uuid::from_bytes(*resource_id),
                mapping_entry_digest: hex::encode(mapping_entry_digest),
            },
            RepairActionV1::RebuildObjectProvenance {
                object_id,
                expected_business_body_digest,
                expected_source_digest,
            } => Self::RebuildObjectProvenance {
                object_id: Uuid::from_bytes(*object_id),
                expected_business_body_digest: hex::encode(expected_business_body_digest),
                expected_source_digest: hex::encode(expected_source_digest),
            },
            RepairActionV1::RebuildNormalizedContext {
                object_id,
                expected_business_body_digest,
            } => Self::RebuildNormalizedContext {
                object_id: Uuid::from_bytes(*object_id),
                expected_business_body_digest: hex::encode(expected_business_body_digest),
            },
        }
    }
}

impl RepairActionV1 {
    fn sort_key(&self) -> (u8, [u8; 16]) {
        match self {
            Self::ReapplyCommittedResource { resource_id, .. } => (0, *resource_id),
            Self::RebuildObjectProvenance { object_id, .. } => (1, *object_id),
            Self::RebuildNormalizedContext { object_id, .. } => (2, *object_id),
        }
    }
}

impl CanonicalMaintenanceRepairPlanV1 {
    /// Validate the exact closed coordinate and canonical action ordering.
    pub fn validate(&self) -> Result<(), MaintenanceContractError> {
        if self.schema_version != PROJECT_VIEW_MAINTENANCE_REPAIR_SCHEMA_VERSION {
            return Err(MaintenanceContractError::InvalidRepairPlan(format!(
                "schema_version must be {PROJECT_VIEW_MAINTENANCE_REPAIR_SCHEMA_VERSION}"
            )));
        }
        if Uuid::from_bytes(self.community_id).is_nil() {
            return Err(MaintenanceContractError::InvalidRepairPlan(
                "community_id cannot be nil".to_owned(),
            ));
        }
        validate_repair_coordinate(self.maintenance_epoch, "maintenance_epoch")?;
        validate_repair_coordinate(self.expected_project_revision, "expected_project_revision")?;
        validate_repair_coordinate(
            self.expected_projection_generation,
            "expected_projection_generation",
        )?;
        if self.actions.is_empty() || self.actions.len() > MAX_MAINTENANCE_REPAIR_ACTIONS {
            return Err(MaintenanceContractError::InvalidRepairPlan(format!(
                "actions must contain 1..={MAX_MAINTENANCE_REPAIR_ACTIONS} entries"
            )));
        }
        let keys = self.actions.iter().map(RepairActionV1::sort_key);
        if keys.clone().any(|(_, id)| Uuid::from_bytes(id).is_nil()) {
            return Err(MaintenanceContractError::InvalidRepairPlan(
                "repair object IDs cannot be nil".to_owned(),
            ));
        }
        if keys
            .clone()
            .zip(keys.skip(1))
            .any(|(previous, current)| previous >= current)
        {
            return Err(MaintenanceContractError::InvalidRepairPlan(
                "actions must be unique and sorted by action type and object UUID bytes".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Compute the frozen repair plan digest.
pub fn maintenance_repair_plan_digest(
    plan: &CanonicalMaintenanceRepairPlanV1,
) -> Result<[u8; 32], MaintenanceContractError> {
    plan.validate()?;
    let bytes = postcard::to_stdvec(plan).map_err(|error| {
        MaintenanceContractError::InvalidRepairPlan(format!(
            "cannot encode canonical repair plan: {error}"
        ))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(MAINTENANCE_REPAIR_PLAN_DIGEST_DOMAIN);
    hasher.update(bytes);
    Ok(hasher.finalize().into())
}

/// Current operational state of one Community's Project View.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceState {
    /// Ordinary member writes, runtime admission, and capability reads may run.
    Normal,
    /// New work is stopped while exact runtime/Assignment baselines quiesce.
    Draining,
    /// Member/runtime mutation paths are fenced for cutover, verify, or repair.
    Frozen,
}

/// Closed operator actions that change the maintenance state pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceAction {
    /// Start a new immutable maintenance epoch and enter draining.
    Begin,
    /// Enter frozen after all exact baseline acknowledgements are durable.
    Freeze,
    /// Cancel a pre-commit epoch without reviving old runtime coordinates.
    Abort,
    /// Return a verified post-cutover v3 Community to normal operation.
    Resume,
}

/// Runtime baseline terminal state asserted by a trusted supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceRuntimeAckStatus {
    /// The server may monotonically retire an available/recovering baseline
    /// runtime for maintenance without creating failure evidence.
    Suspended,
    /// Existing graceful-stop or trusted terminal evidence already proves the
    /// baseline runtime is no longer live.
    Terminal,
}

impl MaintenanceRuntimeAckStatus {
    /// Stable database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Suspended => "suspended",
            Self::Terminal => "terminal",
        }
    }
}

/// Closed NIP-98 body submitted by one registered runtime supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceAckCommand {
    /// Must equal [`PROJECT_VIEW_MAINTENANCE_ACK_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// Exact baseline acknowledgement.
    pub request: MaintenanceAckRequest,
}

/// Assignment-level and Runtime-level acknowledgement union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaintenanceAckRequest {
    /// The Assignment watcher has stopped admission, joined all work, reaped
    /// owned children, and can no longer publish a baseline fence.
    AssignmentQuiesced {
        /// Exact active maintenance epoch.
        maintenance_epoch: u64,
        /// Exact supervisor binding captured at begin.
        binding_id: Uuid,
        /// Exact active Assignment captured at begin.
        assignment_id: Uuid,
        /// Ordered maintenance protocol version implemented by this watcher.
        client_protocol_version: u64,
        /// Bounded diagnostic build identifier; never compared lexically.
        client_build: String,
        /// Caller-stable idempotency key.
        idempotency_key: Uuid,
    },
    /// One exact baseline Runtime has been suspended or was already terminal.
    RuntimeSuspendedOrTerminal {
        /// Exact active maintenance epoch.
        maintenance_epoch: u64,
        /// Exact supervisor binding captured at begin.
        binding_id: Uuid,
        /// Exact active Assignment captured at begin.
        assignment_id: Uuid,
        /// Exact logical Runtime captured at begin.
        runtime_id: Uuid,
        /// Exact runtime fence epoch captured at begin.
        runtime_epoch: u64,
        /// Monotonic retirement proof.
        status: MaintenanceRuntimeAckStatus,
        /// Caller-stable idempotency key.
        idempotency_key: Uuid,
    },
}

impl MaintenanceAckCommand {
    /// Parse and validate one closed acknowledgement body.
    pub fn from_json(content: &str) -> Result<Self, MaintenanceContractError> {
        let command: Self = serde_json::from_str(content)
            .map_err(|error| MaintenanceContractError::InvalidAck(error.to_string()))?;
        command.validate()?;
        Ok(command)
    }

    /// Validate local bounds before canonical baseline lookup.
    pub fn validate(&self) -> Result<(), MaintenanceContractError> {
        if self.schema_version != PROJECT_VIEW_MAINTENANCE_ACK_SCHEMA_VERSION {
            return Err(MaintenanceContractError::InvalidAck(format!(
                "schema_version must be {PROJECT_VIEW_MAINTENANCE_ACK_SCHEMA_VERSION}"
            )));
        }
        match &self.request {
            MaintenanceAckRequest::AssignmentQuiesced {
                maintenance_epoch,
                binding_id,
                assignment_id,
                client_protocol_version,
                client_build,
                idempotency_key,
            } => {
                validate_coordinate(*maintenance_epoch, "maintenance_epoch")?;
                validate_coordinate(*client_protocol_version, "client_protocol_version")?;
                validate_uuid(*binding_id, "binding_id")?;
                validate_uuid(*assignment_id, "assignment_id")?;
                validate_uuid(*idempotency_key, "idempotency_key")?;
                if client_build.is_empty()
                    || client_build.contains('\0')
                    || client_build.len() > 256
                {
                    return Err(MaintenanceContractError::InvalidAck(
                        "client_build must contain 1..=256 non-NUL UTF-8 bytes".to_owned(),
                    ));
                }
            }
            MaintenanceAckRequest::RuntimeSuspendedOrTerminal {
                maintenance_epoch,
                binding_id,
                assignment_id,
                runtime_id,
                runtime_epoch,
                idempotency_key,
                ..
            } => {
                validate_coordinate(*maintenance_epoch, "maintenance_epoch")?;
                validate_coordinate(*runtime_epoch, "runtime_epoch")?;
                for (value, field) in [
                    (*binding_id, "binding_id"),
                    (*assignment_id, "assignment_id"),
                    (*runtime_id, "runtime_id"),
                    (*idempotency_key, "idempotency_key"),
                ] {
                    validate_uuid(value, field)?;
                }
            }
        }
        Ok(())
    }

    /// Exact maintenance epoch named by this acknowledgement.
    #[must_use]
    pub const fn maintenance_epoch(&self) -> u64 {
        match self.request {
            MaintenanceAckRequest::AssignmentQuiesced {
                maintenance_epoch, ..
            }
            | MaintenanceAckRequest::RuntimeSuspendedOrTerminal {
                maintenance_epoch, ..
            } => maintenance_epoch,
        }
    }

    /// Stable idempotency key named by this acknowledgement.
    #[must_use]
    pub const fn idempotency_key(&self) -> Uuid {
        match self.request {
            MaintenanceAckRequest::AssignmentQuiesced {
                idempotency_key, ..
            }
            | MaintenanceAckRequest::RuntimeSuspendedOrTerminal {
                idempotency_key, ..
            } => idempotency_key,
        }
    }

    /// Stable ledger discriminator.
    #[must_use]
    pub const fn ack_type(&self) -> &'static str {
        match self.request {
            MaintenanceAckRequest::AssignmentQuiesced { .. } => "assignment",
            MaintenanceAckRequest::RuntimeSuspendedOrTerminal { .. } => "runtime",
        }
    }
}

/// A maintenance transition violated the irreversible cutover boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MaintenanceContractError {
    /// An acknowledgement body violated its closed local contract.
    #[error("invalid maintenance acknowledgement: {0}")]
    InvalidAck(String),
    /// A bounded mechanical repair plan violated its frozen contract.
    #[error("invalid maintenance repair plan: {0}")]
    InvalidRepairPlan(String),
    /// The action is not an edge in the closed state machine.
    #[error("maintenance action {action:?} is invalid from state {state:?}")]
    InvalidTransition {
        /// Current durable state.
        state: MaintenanceState,
        /// Requested operator action.
        action: MaintenanceAction,
    },
    /// Abort was requested after the v3 cutover receipt committed.
    #[error("a committed v3 cutover cannot be rolled back; repair and resume forward")]
    CutoverAlreadyCommitted,
    /// Resume was requested without a committed and structurally verified v3 state.
    #[error("resume requires a committed, structurally verified v3 cutover")]
    V3NotVerified,
}

fn validate_repair_coordinate(value: u64, field: &str) -> Result<(), MaintenanceContractError> {
    if (1..=MAX_SAFE_REVISION).contains(&value) {
        Ok(())
    } else {
        Err(MaintenanceContractError::InvalidRepairPlan(format!(
            "{field} must be JavaScript-safe and positive"
        )))
    }
}

fn decode_lower_hex_32(value: &str, field: &str) -> Result<[u8; 32], MaintenanceContractError> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(MaintenanceContractError::InvalidRepairPlan(format!(
            "{field} must be exactly 64 lowercase hex characters"
        )));
    }
    let decoded = hex::decode(value)
        .map_err(|error| MaintenanceContractError::InvalidRepairPlan(error.to_string()))?;
    decoded.try_into().map_err(|decoded: Vec<u8>| {
        MaintenanceContractError::InvalidRepairPlan(format!(
            "{field} decoded to {} bytes instead of 32",
            decoded.len()
        ))
    })
}

fn validate_coordinate(value: u64, field: &str) -> Result<(), MaintenanceContractError> {
    if (1..=MAX_SAFE_REVISION).contains(&value) {
        Ok(())
    } else {
        Err(MaintenanceContractError::InvalidAck(format!(
            "{field} must be JavaScript-safe and positive"
        )))
    }
}

fn validate_uuid(value: Uuid, field: &str) -> Result<(), MaintenanceContractError> {
    if value.is_nil() {
        Err(MaintenanceContractError::InvalidAck(format!(
            "{field} cannot be nil"
        )))
    } else {
        Ok(())
    }
}

/// Apply the Stage 0 maintenance state-machine contract.
///
/// `cutover_committed` means the immutable cutover receipt exists. Once true,
/// Abort is permanently forbidden. `v3_verified` is meaningful only for
/// Resume and represents structural verification plus resolved post-cutover
/// invalidations under the exclusive Community lock.
pub fn transition_maintenance(
    state: MaintenanceState,
    action: MaintenanceAction,
    cutover_committed: bool,
    v3_verified: bool,
) -> Result<MaintenanceState, MaintenanceContractError> {
    match (state, action) {
        (MaintenanceState::Normal, MaintenanceAction::Begin) => Ok(MaintenanceState::Draining),
        (MaintenanceState::Draining, MaintenanceAction::Freeze) => Ok(MaintenanceState::Frozen),
        (MaintenanceState::Draining | MaintenanceState::Frozen, MaintenanceAction::Abort)
            if !cutover_committed =>
        {
            Ok(MaintenanceState::Normal)
        }
        (MaintenanceState::Draining | MaintenanceState::Frozen, MaintenanceAction::Abort) => {
            Err(MaintenanceContractError::CutoverAlreadyCommitted)
        }
        (MaintenanceState::Frozen, MaintenanceAction::Resume)
            if cutover_committed && v3_verified =>
        {
            Ok(MaintenanceState::Normal)
        }
        (MaintenanceState::Frozen, MaintenanceAction::Resume) => {
            Err(MaintenanceContractError::V3NotVerified)
        }
        (state, action) => Err(MaintenanceContractError::InvalidTransition { state, action }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maintenance_edges_and_irreversible_cutover_are_closed() {
        assert_eq!(
            transition_maintenance(
                MaintenanceState::Normal,
                MaintenanceAction::Begin,
                false,
                false,
            ),
            Ok(MaintenanceState::Draining)
        );
        assert_eq!(
            transition_maintenance(
                MaintenanceState::Draining,
                MaintenanceAction::Freeze,
                false,
                false,
            ),
            Ok(MaintenanceState::Frozen)
        );
        assert_eq!(
            transition_maintenance(
                MaintenanceState::Frozen,
                MaintenanceAction::Resume,
                true,
                true,
            ),
            Ok(MaintenanceState::Normal)
        );
        assert_eq!(
            transition_maintenance(
                MaintenanceState::Frozen,
                MaintenanceAction::Abort,
                true,
                false,
            ),
            Err(MaintenanceContractError::CutoverAlreadyCommitted)
        );
    }

    #[test]
    fn repair_plan_is_bounded_sorted_and_digest_stable() {
        let plan = CanonicalMaintenanceRepairPlanV1 {
            schema_version: PROJECT_VIEW_MAINTENANCE_REPAIR_SCHEMA_VERSION,
            community_id: *Uuid::parse_str("018f6f4f-1e10-7c0b-9b37-2e4094c9a111")
                .expect("Community UUID")
                .as_bytes(),
            maintenance_epoch: 4,
            cutover_change_id: [0xab; 32],
            expected_project_revision: 11,
            expected_projection_generation: 2,
            actions: vec![RepairActionV1::ReapplyCommittedResource {
                resource_id: *Uuid::parse_str("0f85e5f0-c7d5-4c30-a0f2-c18478d21001")
                    .expect("Resource UUID")
                    .as_bytes(),
                mapping_entry_digest: [9; 32],
            }],
        };
        assert!(plan.validate().is_ok());
        assert_eq!(
            hex::encode(maintenance_repair_plan_digest(&plan).expect("digest")),
            hex::encode(maintenance_repair_plan_digest(&plan).expect("repeat digest"))
        );

        let json = CanonicalMaintenanceRepairPlanEnvelopeV1::to_pretty_json(&plan)
            .expect("Human envelope");
        assert_eq!(
            CanonicalMaintenanceRepairPlanEnvelopeV1::parse_json(&json).expect("parse envelope"),
            plan
        );
        let uppercase = String::from_utf8(json)
            .expect("UTF-8")
            .replace(&hex::encode([0xab; 32]), &hex::encode_upper([0xab; 32]));
        assert!(
            CanonicalMaintenanceRepairPlanEnvelopeV1::parse_json(uppercase.as_bytes()).is_err()
        );
    }
}
