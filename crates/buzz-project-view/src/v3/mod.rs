//! Pure Project View v3 contracts.
//!
//! Stage 0 freezes these shapes without enabling v3 in the Relay. No type in
//! this module performs database access, network I/O, signing, or capability
//! advertisement.

mod contract;
mod maintenance;
mod migration;

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
    transition_maintenance, MaintenanceAction, MaintenanceContractError, MaintenanceState,
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

/// Project View v3 wire schema number.
pub const PROJECT_VIEW_V3_SCHEMA_VERSION: u16 = 3;
