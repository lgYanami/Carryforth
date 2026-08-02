//! Pure Project View v3 contracts.
//!
//! Stage 0 freezes these shapes without enabling v3 in the Relay. No type in
//! this module performs database access, network I/O, signing, or capability
//! advertisement.

mod context_reference;
mod contract;
mod maintenance;
mod manifest_envelope;
mod migration;
mod model;
mod project_object;
mod projection;
mod role_continuity;
mod validation;

pub use context_reference::{
    introduced_document_targets, validate_context_replacement, validate_document_target_delta,
    DocumentCoordinate, DocumentTargetDelta, DocumentTargetState, ReferenceTargetProof,
    V3ReferenceError,
};
pub use contract::{
    canonicalize_context_references, ContextAvailabilityV3, ContextLiveDocumentV3,
    ContextPinnedDocumentV3, ContextResourceV3, ContextTruncationV3, DocumentMetadataSourceV3,
    DocumentReferenceMode, InitialGovernanceAssignmentV3, InitialRoleDefinitionV3,
    ProjectContextReference, ProjectResourceV3, ProjectViewInitializeV3,
    ProjectViewInitializeV3Request, RoleBriefContextV3, RoleBriefSourceRevisionsV3,
    RoleDefinitionV3, V3ContractError, MAX_CONTEXT_REFERENCES, PROJECT_CONTEXT_CAPABILITY,
    PROJECT_VIEW_V3_CAPABILITY,
};
pub use maintenance::{
    maintenance_repair_plan_digest, transition_maintenance,
    CanonicalMaintenanceRepairPlanEnvelopeV1, CanonicalMaintenanceRepairPlanV1,
    MaintenanceAckCommand, MaintenanceAckRequest, MaintenanceAction, MaintenanceContractError,
    MaintenanceRuntimeAckStatus, MaintenanceState, RepairActionEnvelopeV1, RepairActionV1,
    MAINTENANCE_REPAIR_PLAN_DIGEST_DOMAIN, MAX_MAINTENANCE_REPAIR_ACTIONS,
    MAX_MAINTENANCE_REPAIR_PLAN_JSON_BYTES, PROJECT_VIEW_MAINTENANCE_ACK_SCHEMA_VERSION,
    PROJECT_VIEW_MAINTENANCE_REPAIR_SCHEMA_VERSION,
};
pub use manifest_envelope::{
    CanonicalProjectResourceEnvelopeV1, CanonicalResourceCutoverEnvelopeV1,
    ResourceMappingManifestEnvelopeV1, ReviewedResourceMappingEnvelopeV1,
};
pub use migration::{
    guide_snapshot_digest, legacy_resource_digest, manifest_digest, mapping_entry_digest,
    resource_cutover_payload_digest, review_digest, CanonicalContextReferenceV1,
    CanonicalDocumentReferenceModeV1, CanonicalGuideSnapshotV1, CanonicalLegacyObjectStateV1,
    CanonicalLegacyResourceV1, CanonicalProjectResourceV3, CanonicalResourceCutoverV1,
    CanonicalResourceMappingEntryV1, CanonicalResourceReviewV1, MigrationContractError,
    ResourceMappingManifestV1, ReviewSignature, ReviewedResourceMappingV1,
    GUIDE_SNAPSHOT_DIGEST_DOMAIN, LEGACY_RESOURCE_DIGEST_DOMAIN, MANIFEST_DIGEST_DOMAIN,
    MAPPING_ENTRY_DIGEST_DOMAIN, MAX_MANIFEST_ENTRIES, MAX_MANIFEST_JSON_BYTES,
    RESOURCE_CUTOVER_PAYLOAD_DIGEST_DOMAIN, REVIEW_DIGEST_DOMAIN,
};
pub use model::{
    ProjectViewEntryV3, ProjectViewObjectDataV3, ProjectViewObjectV3, ProjectViewTombstoneV3,
};
pub use project_object::{
    CreateProjectObjectV3, DeleteProjectObjectV3, GoalPatchV3, IssuePatchV3,
    NewProjectViewObjectV3, PlanPatchV3, ProfilePatchV3, ProjectObjectCommandV3,
    ProjectObjectOutcomeV3, ProjectObjectRequestV3, ProjectViewStateV3, RequirementPatchV3,
    ResourcePatchV3, RolePatchV3, StagePatchV3, UpdateProjectObjectV3, V3ProjectObjectError,
    V3ReducerCapabilities, WorkPatchV3,
};
pub use projection::{ProjectedHeadV3, ProjectionPlanV3};
pub use role_continuity::{
    reduce_role_command_v3, validate_role_actor_for_v3_replay, RoleCommandV3,
};

/// Project View v3 wire schema number.
pub const PROJECT_VIEW_V3_SCHEMA_VERSION: u16 = 3;

/// Validate one active v3 projection object without requiring its complete
/// Project snapshot. Cross-object relation and target checks remain the
/// responsibility of the canonical state/repository boundary.
pub fn validate_projected_object_v3(
    object: &ProjectViewObjectV3,
) -> Result<(), V3ProjectObjectError> {
    validation::validate_object(object)
}
