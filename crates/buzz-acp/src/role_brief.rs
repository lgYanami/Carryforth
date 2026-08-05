//! Per-turn verified Role Brief resolution for managed Agent runtimes.

use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META,
    KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_project_document::DocumentHeadProjection;
use buzz_project_view::v3::DocumentMetadataSourceV3;
use buzz_sdk::project_document::{
    document_head_coordinate, document_meta_coordinate, parse_document_head, parse_document_meta,
    VerifiedDocumentHead, VerifiedDocumentMeta,
};
use buzz_sdk::project_view_v2::{
    parse_entity_projection as parse_v2_entity_projection, parse_membership_projection,
    parse_meta_projection as parse_v2_meta_projection,
    parse_project_object_projection as parse_v2_project_object_projection, V2EntityProjection,
    V2MembershipProjection, V2MetaProjection,
};
use buzz_sdk::project_view_v3::{
    parse_entity_projection as parse_v3_entity_projection,
    parse_meta_projection as parse_v3_meta_projection,
    parse_project_object_projection as parse_v3_project_object_projection, V3EntityProjection,
    V3MetaProjection,
};
use buzz_sdk::role_brief::{
    render_role_binding_markdown, render_role_brief_markdown, unavailable_role_brief_markdown,
    VerifiedRoleBriefSnapshot,
};
use buzz_sdk::role_brief_v3::{
    render_role_binding_markdown_v3, render_role_brief_markdown_v3, ResolvedRoleBrief,
    RoleBriefDocumentEnrichmentV3, RoleBriefV3, VerifiedDocumentMetadataV3,
    VerifiedRoleBriefSnapshotV3,
};
use chrono::Utc;
use nostr::{Alphabet, Event, EventId, Filter, Kind, PublicKey, SingleLetterTag};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::relay::RestClient;

const PROJECT_VIEW_V2_EXTENSION: &str = "buzz-project-view-v2";
const PROJECT_VIEW_V3_EXTENSION: &str = "buzz-project-view-v3";
const PROJECT_CONTEXT_EXTENSION: &str = "buzz-project-context-v1";
const PROJECT_DOCUMENT_EXTENSION: &str = "buzz-project-document-v1";
const ROLE_BRIEF_TIMEOUT: Duration = Duration::from_secs(12);
const DOCUMENT_ENRICHMENT_TIMEOUT: Duration = Duration::from_secs(4);
const RUNTIME_SUSPEND_TIMEOUT: Duration = Duration::from_secs(1);
const SNAPSHOT_ATTEMPTS: usize = 3;
const DOCUMENT_SNAPSHOT_ATTEMPTS: usize = 3;
const ENTITY_PAGE_SIZE: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectViewSchema {
    V2,
    V3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProjectViewIdentity {
    relay_pubkey: PublicKey,
    schema: ProjectViewSchema,
    context_enabled: bool,
    document_enabled: bool,
}

#[derive(Debug, Clone)]
enum VerifiedMeta {
    V2(V2MetaProjection),
    V3(V3MetaProjection),
}

impl VerifiedMeta {
    const fn schema(&self) -> ProjectViewSchema {
        match self {
            Self::V2(_) => ProjectViewSchema::V2,
            Self::V3(_) => ProjectViewSchema::V3,
        }
    }

    const fn event_id(&self) -> EventId {
        match self {
            Self::V2(meta) => meta.event_id,
            Self::V3(meta) => meta.event_id,
        }
    }

    const fn project_id(&self) -> buzz_core::CommunityId {
        match self {
            Self::V2(meta) => meta.project_id,
            Self::V3(meta) => meta.project_id,
        }
    }

    const fn project_revision(&self) -> u64 {
        match self {
            Self::V2(meta) => meta.project_revision,
            Self::V3(meta) => meta.project_revision,
        }
    }

    const fn projection_generation(&self) -> u64 {
        match self {
            Self::V2(meta) => meta.projection_generation,
            Self::V3(meta) => meta.projection_generation,
        }
    }
}

/// Whether a turn may use an exact-meta compact binding or must rebuild the
/// complete verified Brief.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoleContextRefresh {
    /// Use a compact binding only when the complete cache key still matches.
    Incremental,
    /// Re-read the complete snapshot, such as for a new ACP session.
    Full,
}

/// Dynamic prompt section plus machine-readable resolution metadata.
#[derive(Debug, Clone)]
pub struct RoleContextResolution {
    /// Rendered `[Role Brief]`, `[Role Binding]`, or fail-closed section.
    pub markdown: String,
    /// `candidate`, `assigned`, or `unavailable`.
    pub status: &'static str,
    /// `full`, `compact`, or `unavailable`.
    pub mode: &'static str,
    /// Current active Assignment fence when assigned.
    pub assignment_id: Option<Uuid>,
    /// Verified Project revision, when available.
    pub project_revision: Option<u64>,
    /// Verified projection generation, when available.
    pub projection_generation: Option<u64>,
    /// Exact verified metadata head, when available.
    pub meta_event_id: Option<EventId>,
    /// Stable failure category when unavailable.
    pub error_code: Option<&'static str>,
    /// Total active Roles in the verified directory, only for a Full Brief.
    pub role_directory_total: Option<u32>,
    /// Active Roles included in the bounded directory, only for a Full Brief.
    pub role_directory_shown: Option<u32>,
    /// Active Roles omitted by the prompt budget, only for a Full Brief.
    pub role_directory_omitted: Option<u32>,
}

impl RoleContextResolution {
    fn unavailable(code: &'static str, detail: &str) -> Self {
        Self {
            markdown: unavailable_role_brief_markdown(code, detail),
            status: "unavailable",
            mode: "unavailable",
            assignment_id: None,
            project_revision: None,
            projection_generation: None,
            meta_event_id: None,
            error_code: Some(code),
            role_directory_total: None,
            role_directory_shown: None,
            role_directory_omitted: None,
        }
    }

    fn full(brief: &ResolvedRoleBrief) -> Self {
        match brief {
            ResolvedRoleBrief::V2(brief) => {
                let directory = &brief.role_directory;
                Self {
                    markdown: render_role_brief_markdown(brief),
                    status: brief.state.status(),
                    mode: "full",
                    assignment_id: brief.assignment_id(),
                    project_revision: Some(brief.project_revision),
                    projection_generation: Some(brief.projection_generation),
                    meta_event_id: Some(brief.source_revisions.meta_event_id),
                    error_code: None,
                    role_directory_total: Some(directory.total_active_roles),
                    role_directory_shown: Some(
                        directory
                            .total_active_roles
                            .saturating_sub(directory.omitted_active_roles),
                    ),
                    role_directory_omitted: Some(directory.omitted_active_roles),
                }
            }
            ResolvedRoleBrief::V3(brief) => {
                let directory = &brief.role_directory;
                Self {
                    markdown: render_role_brief_markdown_v3(brief),
                    status: brief.state.status(),
                    mode: "full",
                    assignment_id: brief.assignment_id(),
                    project_revision: Some(brief.project_revision),
                    projection_generation: Some(brief.projection_generation),
                    meta_event_id: Some(brief.source_revisions.meta_event_id),
                    error_code: None,
                    role_directory_total: Some(directory.total_active_roles),
                    role_directory_shown: Some(
                        directory
                            .total_active_roles
                            .saturating_sub(directory.omitted_active_roles),
                    ),
                    role_directory_omitted: Some(directory.omitted_active_roles),
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct CachedRoleBinding {
    relay_pubkey: PublicKey,
    schema: ProjectViewSchema,
    context_enabled: bool,
    document_enabled: bool,
    project_id: Uuid,
    member_pubkey: PublicKey,
    meta_event_id: EventId,
    project_revision: u64,
    projection_generation: u64,
    markdown: String,
    status: &'static str,
    assignment_id: Option<Uuid>,
    document_metadata: Option<DocumentMetadataSourceV3>,
}

impl CachedRoleBinding {
    fn from_brief(identity: ProjectViewIdentity, brief: &ResolvedRoleBrief) -> Self {
        match brief {
            ResolvedRoleBrief::V2(brief) => Self {
                relay_pubkey: identity.relay_pubkey,
                schema: ProjectViewSchema::V2,
                context_enabled: identity.context_enabled,
                document_enabled: identity.document_enabled,
                project_id: brief.project_id,
                member_pubkey: brief.member_pubkey,
                meta_event_id: brief.source_revisions.meta_event_id,
                project_revision: brief.project_revision,
                projection_generation: brief.projection_generation,
                markdown: render_role_binding_markdown(brief),
                status: brief.state.status(),
                assignment_id: brief.assignment_id(),
                document_metadata: None,
            },
            ResolvedRoleBrief::V3(brief) => Self {
                relay_pubkey: identity.relay_pubkey,
                schema: ProjectViewSchema::V3,
                context_enabled: identity.context_enabled,
                document_enabled: identity.document_enabled,
                project_id: brief.project_id,
                member_pubkey: brief.member_pubkey,
                meta_event_id: brief.source_revisions.meta_event_id,
                project_revision: brief.project_revision,
                projection_generation: brief.projection_generation,
                markdown: render_role_binding_markdown_v3(brief),
                status: brief.state.status(),
                assignment_id: brief.assignment_id(),
                document_metadata: Some(brief.source_revisions.document_metadata.clone()),
            },
        }
    }

    fn matches(
        &self,
        identity: ProjectViewIdentity,
        member_pubkey: PublicKey,
        meta: &VerifiedMeta,
    ) -> bool {
        self.relay_pubkey == identity.relay_pubkey
            && self.schema == meta.schema()
            && self.context_enabled == identity.context_enabled
            && self.document_enabled == identity.document_enabled
            && self.project_id == *meta.project_id().as_uuid()
            && self.member_pubkey == member_pubkey
            && self.meta_event_id == meta.event_id()
            && self.project_revision == meta.project_revision()
            && self.projection_generation == meta.projection_generation()
    }

    fn resolution(&self) -> RoleContextResolution {
        RoleContextResolution {
            markdown: self.markdown.clone(),
            status: self.status,
            mode: "compact",
            assignment_id: self.assignment_id,
            project_revision: Some(self.project_revision),
            projection_generation: Some(self.projection_generation),
            meta_event_id: Some(self.meta_event_id),
            error_code: None,
            role_directory_total: None,
            role_directory_shown: None,
            role_directory_omitted: None,
        }
    }
}

fn cached_resolution(
    cache: &Option<CachedRoleBinding>,
    refresh: RoleContextRefresh,
    identity: ProjectViewIdentity,
    member_pubkey: PublicKey,
    meta: &VerifiedMeta,
) -> Option<RoleContextResolution> {
    (refresh == RoleContextRefresh::Incremental)
        .then_some(cache.as_ref())
        .flatten()
        .filter(|cached| cached.matches(identity, member_pubkey, meta))
        .filter(|cached| {
            !matches!(
                cached.document_metadata,
                Some(DocumentMetadataSourceV3::Unavailable)
                    | Some(DocumentMetadataSourceV3::Verified { .. })
            )
        })
        .map(CachedRoleBinding::resolution)
}

#[derive(Debug)]
enum ResolutionFailure {
    Project(String),
}

impl ResolutionFailure {
    const fn code(&self) -> &'static str {
        match self {
            Self::Project(_) => "project_view_unavailable",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Project(detail) => detail,
        }
    }
}

/// Resolver that verifies the lightweight meta head on every complete turn,
/// rebuilds full context when required, and optionally gates Runtime.
#[derive(Debug, Clone)]
pub struct RoleBriefResolver {
    rest_client: RestClient,
    member_pubkey: PublicKey,
    runtime_supervisor: Option<crate::runtime_supervisor::RuntimeSupervisorClient>,
    cache: Arc<Mutex<Option<CachedRoleBinding>>>,
    lifecycle: TurnLifecycleGate,
}

/// One process-wide generation gate shared by turn admission and every active
/// turn. Maintenance cancels the current token once; a verified normal-state
/// resume installs a fresh token only after all old children are gone.
#[derive(Debug, Clone)]
pub(crate) struct TurnLifecycleGate {
    token: Arc<RwLock<CancellationToken>>,
}

impl TurnLifecycleGate {
    pub(crate) fn new() -> Self {
        Self {
            token: Arc::new(RwLock::new(CancellationToken::new())),
        }
    }

    pub(crate) fn current_token(&self) -> CancellationToken {
        match self.token.read() {
            Ok(token) => token.clone(),
            Err(_) => {
                let token = CancellationToken::new();
                token.cancel();
                token
            }
        }
    }

    pub(crate) fn cancel(&self) {
        match self.token.read() {
            Ok(token) => token.cancel(),
            Err(_) => {
                tracing::error!("turn lifecycle gate lock was poisoned; admission fails closed")
            }
        }
    }

    pub(crate) fn rotate_after_reap(&self) -> Result<(), String> {
        let mut token = self
            .token
            .write()
            .map_err(|_| "turn lifecycle gate lock was poisoned".to_owned())?;
        if !token.is_cancelled() {
            return Err("cannot rotate an open turn lifecycle generation".to_owned());
        }
        *token = CancellationToken::new();
        Ok(())
    }

    pub(crate) fn admission_open(&self) -> bool {
        self.token.read().is_ok_and(|token| !token.is_cancelled())
    }
}

impl RoleBriefResolver {
    /// Bind a resolver to the exact Relay client and managed Agent identity.
    #[must_use]
    pub fn new(rest_client: RestClient, member_pubkey: PublicKey) -> Self {
        Self {
            rest_client,
            member_pubkey,
            runtime_supervisor: None,
            cache: Arc::new(Mutex::new(None)),
            lifecycle: TurnLifecycleGate::new(),
        }
    }

    pub(crate) fn lifecycle_gate(&self) -> TurnLifecycleGate {
        self.lifecycle.clone()
    }

    pub(crate) fn lifecycle_token(&self) -> CancellationToken {
        self.lifecycle.current_token()
    }

    pub(crate) fn turn_admission_open(&self) -> bool {
        self.lifecycle.admission_open()
    }

    /// Gate assigned/candidate Briefs through the trusted Runtime coordinator.
    #[must_use]
    pub(crate) fn with_runtime_supervisor(
        mut self,
        runtime_supervisor: crate::runtime_supervisor::RuntimeSupervisorClient,
    ) -> Self {
        self.runtime_supervisor = Some(runtime_supervisor);
        self
    }

    /// Verify the current Relay/meta head with a hard outer bound.
    ///
    /// An incremental turn may render the cached compact binding only when the
    /// complete cache key exactly matches that freshly verified head. Full
    /// refreshes always rebuild the shared canonical Brief. Neither path skips
    /// Runtime reconciliation, and no cache entry is ever used after a failed
    /// current-head read.
    pub async fn resolve_bounded(&self, refresh: RoleContextRefresh) -> RoleContextResolution {
        let deadline = tokio::time::Instant::now() + ROLE_BRIEF_TIMEOUT;
        match self.resolve_before(deadline, refresh).await {
            Ok(mut resolution) => {
                self.append_runtime_note(&mut resolution);
                resolution
            }
            Err(failure) => {
                self.suspend_runtime().await;
                RoleContextResolution::unavailable(failure.code(), failure.detail())
            }
        }
    }

    fn append_runtime_note(&self, resolution: &mut RoleContextResolution) {
        let Some(supervisor) = &self.runtime_supervisor else {
            return;
        };
        let status = supervisor.current_supervision_status();
        let mut coordinates = Vec::new();
        if let Some(assignment_id) = status.assignment_id {
            coordinates.push(format!("Assignment `{assignment_id}`"));
        }
        if let Some(binding_id) = status.binding_id {
            coordinates.push(format!("binding `{binding_id}`"));
        }
        if let (Some(runtime_id), Some(runtime_epoch)) = (status.runtime_id, status.runtime_epoch) {
            coordinates.push(format!("Runtime `{runtime_id}` epoch `{runtime_epoch}`"));
        }
        let coordinates = if coordinates.is_empty() {
            String::new()
        } else {
            format!(" ({})", coordinates.join(", "))
        };
        resolution.markdown.push_str(&format!(
            "\n\n## Runtime supervision\n\nState: `{}`{}. Runtime supervision is operational telemetry; Community and active Assignment authority govern ordinary Project/Role writes.\n",
            status.state.as_str(),
            coordinates,
        ));
    }

    async fn resolve_before(
        &self,
        deadline: tokio::time::Instant,
        refresh: RoleContextRefresh,
    ) -> Result<RoleContextResolution, ResolutionFailure> {
        // Serialize head comparison and cache replacement. This avoids two
        // concurrent turns racing an older full snapshot over a newer one.
        let mut cache = tokio::time::timeout_at(deadline, self.cache.lock())
            .await
            .map_err(|_| {
                ResolutionFailure::Project("Role context cache coordination timed out".to_owned())
            })?;

        let identity = tokio::time::timeout_at(deadline, self.read_relay_identity())
            .await
            .map_err(|_| {
                ResolutionFailure::Project("Relay identity verification timed out".to_owned())
            })?
            .map_err(ResolutionFailure::Project)?;

        // A Relay identity change is a hard cache realm boundary even if the
        // following meta read fails.
        if cache.as_ref().is_some_and(|cached| {
            cached.relay_pubkey != identity.relay_pubkey
                || cached.schema != identity.schema
                || cached.context_enabled != identity.context_enabled
                || cached.document_enabled != identity.document_enabled
        }) {
            *cache = None;
        }

        let meta = tokio::time::timeout_at(deadline, self.read_meta(identity))
            .await
            .map_err(|_| {
                ResolutionFailure::Project("Project View meta verification timed out".to_owned())
            })?
            .map_err(ResolutionFailure::Project)?;

        if let Some(resolution) =
            cached_resolution(&cache, refresh, identity, self.member_pubkey, &meta)
        {
            self.reconcile_runtime(deadline, resolution.assignment_id)
                .await?;
            return Ok(resolution);
        }

        let cached_document_boundary = (refresh == RoleContextRefresh::Incremental)
            .then_some(cache.as_ref())
            .flatten()
            .filter(|cached| cached.matches(identity, self.member_pubkey, &meta))
            .and_then(|cached| match &cached.document_metadata {
                Some(DocumentMetadataSourceV3::Verified {
                    meta_event_id,
                    catalog_revision,
                    projection_generation,
                }) if identity.context_enabled && identity.document_enabled => Some((
                    *meta_event_id,
                    *catalog_revision,
                    *projection_generation,
                    cached.resolution(),
                )),
                _ => None,
            });
        if let Some((event_id, catalog_revision, projection_generation, resolution)) =
            cached_document_boundary
        {
            let document_deadline =
                deadline.min(tokio::time::Instant::now() + DOCUMENT_ENRICHMENT_TIMEOUT);
            let current = tokio::time::timeout_at(
                document_deadline,
                self.read_document_meta(identity, meta.project_id()),
            )
            .await;
            match current {
                Ok(Ok(current))
                    if current.event_id == event_id
                        && current.projection.catalog_revision == catalog_revision
                        && current.projection.projection_generation == projection_generation =>
                {
                    self.reconcile_runtime(deadline, resolution.assignment_id)
                        .await?;
                    return Ok(resolution);
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => tracing::warn!(
                    "Document metadata cache validation failed; rebuilding body-free Context: {error}"
                ),
                Err(_) => tracing::warn!(
                    "Document metadata cache validation timed out; rebuilding body-free Context"
                ),
            }
        }

        // A non-matching or explicitly refreshed cache must not survive a
        // failed full rebuild and accidentally look eligible later.
        *cache = None;
        let brief = match meta {
            VerifiedMeta::V2(meta) => {
                let snapshot =
                    tokio::time::timeout_at(deadline, self.resolve_verified_v2(identity, meta))
                        .await
                        .map_err(|_| {
                            ResolutionFailure::Project(
                                "Role Brief v2 snapshot resolution timed out".to_owned(),
                            )
                        })?
                        .map_err(ResolutionFailure::Project)?;
                let brief = snapshot
                    .brief_for(self.member_pubkey, Utc::now())
                    .map_err(|error| ResolutionFailure::Project(error.to_string()))?;
                self.reconcile_runtime(deadline, brief.assignment_id())
                    .await?;
                ResolvedRoleBrief::V2(brief)
            }
            VerifiedMeta::V3(meta) => {
                let snapshot =
                    tokio::time::timeout_at(deadline, self.resolve_verified_v3(identity, meta))
                        .await
                        .map_err(|_| {
                            ResolutionFailure::Project(
                                "Role Brief v3 authority resolution timed out".to_owned(),
                            )
                        })?
                        .map_err(ResolutionFailure::Project)?;
                let base = snapshot
                    .brief_for(self.member_pubkey, Utc::now())
                    .map_err(|error| ResolutionFailure::Project(error.to_string()))?;

                // Assignment authority is complete before optional Document
                // enrichment. A metadata outage must not skip reconciliation.
                self.reconcile_runtime(deadline, base.assignment_id())
                    .await?;
                let brief = if identity.context_enabled {
                    self.resolve_optional_v3_context(deadline, identity, &snapshot)
                        .await?
                } else {
                    base
                };
                ResolvedRoleBrief::V3(brief)
            }
        };

        let resolution = RoleContextResolution::full(&brief);
        *cache = Some(CachedRoleBinding::from_brief(identity, &brief));
        Ok(resolution)
    }

    async fn reconcile_runtime(
        &self,
        deadline: tokio::time::Instant,
        assignment_id: Option<Uuid>,
    ) -> Result<(), ResolutionFailure> {
        let Some(supervisor) = &self.runtime_supervisor else {
            return Ok(());
        };
        match tokio::time::timeout_at(deadline, supervisor.reconcile(assignment_id)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(
                    "Runtime supervision reconciliation failed without invalidating Role context: {error}"
                );
            }
            Err(_) => {
                tracing::warn!(
                    "Runtime supervision reconciliation timed out without invalidating Role context"
                );
            }
        }
        Ok(())
    }

    async fn suspend_runtime(&self) {
        let Some(supervisor) = &self.runtime_supervisor else {
            return;
        };
        match tokio::time::timeout(RUNTIME_SUSPEND_TIMEOUT, supervisor.suspend()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!("failed to suspend managed Runtime fence: {error}");
            }
            Err(_) => {
                tracing::warn!("timed out suspending managed Runtime fence");
            }
        }
    }

    async fn resolve_optional_v3_context(
        &self,
        deadline: tokio::time::Instant,
        identity: ProjectViewIdentity,
        snapshot: &VerifiedRoleBriefSnapshotV3,
    ) -> Result<RoleBriefV3, ResolutionFailure> {
        let required = snapshot
            .required_live_document_ids_for(self.member_pubkey)
            .map_err(|error| ResolutionFailure::Project(error.to_string()))?;
        if required.is_empty() {
            return snapshot
                .brief_for_with_context(
                    self.member_pubkey,
                    Utc::now(),
                    RoleBriefDocumentEnrichmentV3::NotRequired,
                )
                .map_err(|error| ResolutionFailure::Project(error.to_string()));
        }

        let enrichment = if identity.document_enabled {
            let document_deadline =
                deadline.min(tokio::time::Instant::now() + DOCUMENT_ENRICHMENT_TIMEOUT);
            match tokio::time::timeout_at(
                document_deadline,
                self.read_stable_document_metadata(identity, snapshot, &required),
            )
            .await
            {
                Ok(Ok(metadata)) => Some(metadata),
                Ok(Err(error)) => {
                    tracing::warn!(
                        "optional Document metadata enrichment failed; preserving Context coordinates: {error}"
                    );
                    None
                }
                Err(_) => {
                    tracing::warn!(
                        "optional Document metadata enrichment timed out; preserving Context coordinates"
                    );
                    None
                }
            }
        } else {
            tracing::warn!(
                "Context is advertised without Document capability; preserving coordinates without metadata"
            );
            None
        };

        if let Some(metadata) = &enrichment {
            match snapshot.brief_for_with_context(
                self.member_pubkey,
                Utc::now(),
                RoleBriefDocumentEnrichmentV3::Verified(metadata),
            ) {
                Ok(brief) => return Ok(brief),
                Err(error) => tracing::warn!(
                    "verified Document metadata could not enrich Context; preserving coordinates: {error}"
                ),
            }
        }
        snapshot
            .brief_for_with_context(
                self.member_pubkey,
                Utc::now(),
                RoleBriefDocumentEnrichmentV3::Unavailable,
            )
            .map_err(|error| ResolutionFailure::Project(error.to_string()))
    }

    async fn read_stable_document_metadata(
        &self,
        identity: ProjectViewIdentity,
        snapshot: &VerifiedRoleBriefSnapshotV3,
        required: &BTreeSet<Uuid>,
    ) -> Result<VerifiedDocumentMetadataV3, String> {
        let project_id = snapshot.meta().project_id;
        let mut before = self.read_document_meta(identity, project_id).await?;
        for attempt in 0..DOCUMENT_SNAPSHOT_ATTEMPTS {
            let heads = self
                .read_document_heads(identity, project_id, required)
                .await?;
            let after = self.read_document_meta(identity, project_id).await?;
            if document_meta_boundary_matches(&before, &after) {
                return VerifiedDocumentMetadataV3::new(before, heads)
                    .map_err(|error| error.to_string());
            }
            if attempt + 1 < DOCUMENT_SNAPSHOT_ATTEMPTS {
                before = after;
                tokio::time::sleep(Duration::from_millis(25_u64 << attempt)).await;
                continue;
            }
            return Err(
                "Project Document metadata changed during every bounded snapshot attempt"
                    .to_owned(),
            );
        }
        Err("Project Document metadata snapshot could not be stabilized".to_owned())
    }

    async fn read_document_heads(
        &self,
        identity: ProjectViewIdentity,
        project_id: buzz_core::CommunityId,
        required: &BTreeSet<Uuid>,
    ) -> Result<Vec<VerifiedDocumentHead>, String> {
        if required.len() > 128 {
            return Err("Role Brief requested too many Document metadata heads".to_owned());
        }
        let coordinates = required
            .iter()
            .map(|document_id| document_head_coordinate(project_id, *document_id))
            .collect::<Vec<_>>();
        let filter = json!({
            "kinds": [KIND_PROJECT_DOCUMENT_HEAD],
            "authors": [identity.relay_pubkey.to_hex()],
            "#d": coordinates,
            "limit": required.len(),
        });
        let events = parse_events(
            self.rest_client
                .query_raw(&[filter])
                .await
                .map_err(|error| error.to_string())?,
        )?;
        if events.len() != required.len() {
            return Err("Document head query did not resolve every required coordinate".to_owned());
        }
        let mut missing = required.clone();
        let mut event_ids = HashSet::with_capacity(events.len());
        let mut heads = Vec::with_capacity(events.len());
        for event in events {
            if !event_ids.insert(event.id) {
                return Err("Document head query returned a duplicate event".to_owned());
            }
            let head = parse_document_head(&event, &identity.relay_pubkey, project_id)
                .map_err(|error| error.to_string())?;
            let document_id = document_head_id(&head);
            if !missing.remove(&document_id) {
                return Err(
                    "Document head query returned an unexpected or duplicate coordinate".to_owned(),
                );
            }
            heads.push(head);
        }
        if !missing.is_empty() {
            return Err("Document head query omitted a required coordinate".to_owned());
        }
        Ok(heads)
    }

    async fn resolve_verified_v2(
        &self,
        identity: ProjectViewIdentity,
        mut before: V2MetaProjection,
    ) -> Result<VerifiedRoleBriefSnapshot, String> {
        let relay_pubkey = identity.relay_pubkey;
        for attempt in 0..SNAPSHOT_ATTEMPTS {
            let t_tag = SingleLetterTag::lowercase(Alphabet::T);
            let ordinary_filter = Filter::new()
                .kind(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16))
                .author(relay_pubkey)
                .custom_tags(t_tag, ["buzz-project-view-v2-object"]);
            let ordinary_events = parse_events(
                self.rest_client
                    .query(&[ordinary_filter])
                    .await
                    .map_err(|error| error.to_string())?,
            )?;
            let entity_projections = self
                .read_current_entity_projections(relay_pubkey, &before)
                .await?;

            let mut event_ids =
                HashSet::with_capacity(ordinary_events.len() + entity_projections.len());
            let mut object_projections = Vec::with_capacity(ordinary_events.len());
            for event in ordinary_events {
                if !event_ids.insert(event.id) {
                    return Err("ordinary-object query returned a duplicate event".to_owned());
                }
                object_projections.push(
                    parse_v2_project_object_projection(&event, &relay_pubkey, before.project_id)
                        .map_err(|error| error.to_string())?,
                );
            }
            for projection in &entity_projections {
                if !event_ids.insert(projection.event_id) {
                    return Err("entity query returned a duplicate event".to_owned());
                }
            }
            let membership = self.read_membership(relay_pubkey, &before).await?;
            let VerifiedMeta::V2(after) = self.read_meta(identity).await? else {
                return Err("Project View schema changed during v2 snapshot assembly".to_owned());
            };
            if before.event_id != after.event_id {
                if attempt + 1 < SNAPSHOT_ATTEMPTS {
                    before = after;
                    tokio::time::sleep(Duration::from_millis(25_u64 << attempt)).await;
                    continue;
                }
                return Err("Project View changed during every bounded snapshot attempt".to_owned());
            }
            return VerifiedRoleBriefSnapshot::new_with_partial_history(
                before,
                membership,
                object_projections,
                entity_projections,
            )
            .map_err(|error| error.to_string());
        }
        Err("Project View snapshot could not be stabilized".to_owned())
    }

    async fn resolve_verified_v3(
        &self,
        identity: ProjectViewIdentity,
        mut before: V3MetaProjection,
    ) -> Result<VerifiedRoleBriefSnapshotV3, String> {
        let relay_pubkey = identity.relay_pubkey;
        for attempt in 0..SNAPSHOT_ATTEMPTS {
            let t_tag = SingleLetterTag::lowercase(Alphabet::T);
            let ordinary_filter = Filter::new()
                .kind(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16))
                .author(relay_pubkey)
                .custom_tags(t_tag, ["buzz-project-view-v3-object"]);
            let ordinary_events = parse_events(
                self.rest_client
                    .query(&[ordinary_filter])
                    .await
                    .map_err(|error| error.to_string())?,
            )?;
            let entity_projections = self
                .read_current_entity_projections_v3(relay_pubkey, &before)
                .await?;

            let mut event_ids =
                HashSet::with_capacity(ordinary_events.len() + entity_projections.len());
            let mut object_projections = Vec::with_capacity(ordinary_events.len());
            for event in ordinary_events {
                if !event_ids.insert(event.id) {
                    return Err("v3 ordinary-object query returned a duplicate event".to_owned());
                }
                object_projections.push(
                    parse_v3_project_object_projection(&event, &relay_pubkey, before.project_id)
                        .map_err(|error| error.to_string())?,
                );
            }
            for projection in &entity_projections {
                if !event_ids.insert(projection.event_id) {
                    return Err("v3 entity query returned a duplicate event".to_owned());
                }
            }
            let membership = self
                .read_membership_event(relay_pubkey, before.membership_snapshot_event_id)
                .await?;
            let VerifiedMeta::V3(after) = self.read_meta(identity).await? else {
                return Err("Project View schema changed during v3 snapshot assembly".to_owned());
            };
            if before.event_id != after.event_id {
                if attempt + 1 < SNAPSHOT_ATTEMPTS {
                    before = after;
                    tokio::time::sleep(Duration::from_millis(25_u64 << attempt)).await;
                    continue;
                }
                return Err(
                    "Project View v3 changed during every bounded snapshot attempt".to_owned(),
                );
            }
            return VerifiedRoleBriefSnapshotV3::new_with_partial_history(
                before,
                membership,
                object_projections,
                entity_projections,
            )
            .map_err(|error| error.to_string());
        }
        Err("Project View v3 snapshot could not be stabilized".to_owned())
    }

    async fn read_current_entity_projections(
        &self,
        relay_pubkey: PublicKey,
        meta: &V2MetaProjection,
    ) -> Result<Vec<V2EntityProjection>, String> {
        let mut projections = Vec::new();
        let mut event_ids = HashSet::new();
        let mut after: Option<Value> = None;
        loop {
            let mut extension = json!({
                "scope": "v2_current_entities",
                "revision": meta.project_revision,
                "projection_generation": meta.projection_generation,
            });
            if let Some(cursor) = &after {
                extension["after"] = cursor.clone();
            }
            let filter = json!({
                "kinds": [KIND_PROJECT_VIEW_OBJECT],
                "authors": [relay_pubkey.to_hex()],
                "#t": ["buzz-project-view-v2-entity"],
                "limit": ENTITY_PAGE_SIZE,
                "buzz_project_view": extension,
            });
            let events = parse_events(
                self.rest_client
                    .query_raw(&[filter])
                    .await
                    .map_err(|error| error.to_string())?,
            )?;
            if events.len() > ENTITY_PAGE_SIZE {
                return Err("current-entity page exceeded its requested limit".to_owned());
            }
            let page_len = events.len();
            for event in events {
                if !event_ids.insert(event.id) {
                    return Err("current-entity pages returned a duplicate signed event".to_owned());
                }
                let projection = parse_v2_entity_projection(&event, &relay_pubkey, meta.project_id)
                    .map_err(|error| error.to_string())?;
                after = Some(json!({
                    "entity_type": projection.entity.entity_type().as_str(),
                    "entity_id": projection.entity.entity_id(),
                }));
                projections.push(projection);
            }
            if page_len < ENTITY_PAGE_SIZE {
                break;
            }
        }
        Ok(projections)
    }

    async fn read_current_entity_projections_v3(
        &self,
        relay_pubkey: PublicKey,
        meta: &V3MetaProjection,
    ) -> Result<Vec<V3EntityProjection>, String> {
        let mut projections = Vec::new();
        let mut event_ids = HashSet::new();
        let mut after: Option<Value> = None;
        loop {
            let mut extension = json!({
                "scope": "v3_current_entities",
                "revision": meta.project_revision,
                "projection_generation": meta.projection_generation,
            });
            if let Some(cursor) = &after {
                extension["after"] = cursor.clone();
            }
            let filter = json!({
                "kinds": [KIND_PROJECT_VIEW_OBJECT],
                "authors": [relay_pubkey.to_hex()],
                "#t": ["buzz-project-view-v3-entity"],
                "limit": ENTITY_PAGE_SIZE,
                "buzz_project_view": extension,
            });
            let events = parse_events(
                self.rest_client
                    .query_raw(&[filter])
                    .await
                    .map_err(|error| error.to_string())?,
            )?;
            if events.len() > ENTITY_PAGE_SIZE {
                return Err("v3 current-entity page exceeded its requested limit".to_owned());
            }
            let page_len = events.len();
            for event in events {
                if !event_ids.insert(event.id) {
                    return Err(
                        "v3 current-entity pages returned a duplicate signed event".to_owned()
                    );
                }
                let projection = parse_v3_entity_projection(&event, &relay_pubkey, meta.project_id)
                    .map_err(|error| error.to_string())?;
                after = Some(json!({
                    "entity_type": projection.entity.entity_type().as_str(),
                    "entity_id": projection.entity.entity_id(),
                }));
                projections.push(projection);
            }
            if page_len < ENTITY_PAGE_SIZE {
                break;
            }
        }
        Ok(projections)
    }

    async fn read_relay_identity(&self) -> Result<ProjectViewIdentity, String> {
        let value = self
            .rest_client
            .get_public("/info")
            .await
            .map_err(|error| error.to_string())?;
        let info: Nip11Document =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        let schema = if info
            .supported_extensions
            .iter()
            .any(|extension| extension == PROJECT_VIEW_V3_EXTENSION)
        {
            ProjectViewSchema::V3
        } else if info
            .supported_extensions
            .iter()
            .any(|extension| extension == PROJECT_VIEW_V2_EXTENSION)
        {
            ProjectViewSchema::V2
        } else {
            return Err(format!(
                "Relay does not advertise {PROJECT_VIEW_V2_EXTENSION} or {PROJECT_VIEW_V3_EXTENSION}"
            ));
        };
        let relay_self = info
            .relay_self
            .ok_or_else(|| "NIP-11 has no Relay `self` key".to_owned())?;
        let relay_pubkey = PublicKey::from_hex(&relay_self).map_err(|error| error.to_string())?;
        if relay_pubkey.to_hex() != relay_self {
            return Err("NIP-11 Relay `self` key is not canonical lowercase hex".to_owned());
        }
        let context_enabled = schema == ProjectViewSchema::V3
            && info
                .supported_extensions
                .iter()
                .any(|extension| extension == PROJECT_CONTEXT_EXTENSION);
        let document_enabled = info
            .supported_extensions
            .iter()
            .any(|extension| extension == PROJECT_DOCUMENT_EXTENSION);
        Ok(ProjectViewIdentity {
            relay_pubkey,
            schema,
            context_enabled,
            document_enabled,
        })
    }

    async fn read_meta(&self, identity: ProjectViewIdentity) -> Result<VerifiedMeta, String> {
        let filter = Filter::new()
            .kind(Kind::Custom(KIND_PROJECT_VIEW_META as u16))
            .author(identity.relay_pubkey)
            .limit(2);
        let events = parse_events(
            self.rest_client
                .query(&[filter])
                .await
                .map_err(|error| error.to_string())?,
        )?;
        let [event] = events.as_slice() else {
            return Err("metadata query did not return exactly one current head".to_owned());
        };
        match identity.schema {
            ProjectViewSchema::V2 => parse_v2_meta_projection(event, &identity.relay_pubkey)
                .map(VerifiedMeta::V2)
                .map_err(|error| error.to_string()),
            ProjectViewSchema::V3 => parse_v3_meta_projection(event, &identity.relay_pubkey)
                .map(VerifiedMeta::V3)
                .map_err(|error| error.to_string()),
        }
    }

    async fn read_document_meta(
        &self,
        identity: ProjectViewIdentity,
        project_id: buzz_core::CommunityId,
    ) -> Result<VerifiedDocumentMeta, String> {
        let filter = json!({
            "kinds": [KIND_PROJECT_DOCUMENT_META],
            "authors": [identity.relay_pubkey.to_hex()],
            "#d": [document_meta_coordinate(project_id)],
            "limit": 2,
        });
        let events = parse_events(
            self.rest_client
                .query_raw(&[filter])
                .await
                .map_err(|error| error.to_string())?,
        )?;
        let [event] = events.as_slice() else {
            return Err(
                "Document metadata query did not return exactly one current head".to_owned(),
            );
        };
        let meta = parse_document_meta(event, &identity.relay_pubkey)
            .map_err(|error| error.to_string())?;
        if meta.projection.project_id != *project_id.as_uuid() {
            return Err("Document metadata belongs to a different Project".to_owned());
        }
        Ok(meta)
    }

    async fn read_membership(
        &self,
        relay_pubkey: PublicKey,
        meta: &V2MetaProjection,
    ) -> Result<V2MembershipProjection, String> {
        self.read_membership_event(relay_pubkey, meta.membership_snapshot_event_id)
            .await
    }

    async fn read_membership_event(
        &self,
        relay_pubkey: PublicKey,
        membership_event_id: EventId,
    ) -> Result<V2MembershipProjection, String> {
        let filter = Filter::new()
            .id(membership_event_id)
            .kind(Kind::Custom(KIND_NIP43_MEMBERSHIP_LIST as u16))
            .author(relay_pubkey)
            .limit(2);
        let events = parse_events(
            self.rest_client
                .query(&[filter])
                .await
                .map_err(|error| error.to_string())?,
        )?;
        let [event] = events.as_slice() else {
            return Err("metadata membership pointer did not resolve exactly once".to_owned());
        };
        if event.id != membership_event_id {
            return Err(
                "membership query returned an event other than metadata pointer".to_owned(),
            );
        }
        parse_membership_projection(event, &relay_pubkey).map_err(|error| error.to_string())
    }
}

fn document_head_id(head: &VerifiedDocumentHead) -> Uuid {
    match &head.projection {
        DocumentHeadProjection::Active { document_id, .. }
        | DocumentHeadProjection::Deleted { document_id, .. } => *document_id,
    }
}

fn document_meta_boundary_matches(
    before: &VerifiedDocumentMeta,
    after: &VerifiedDocumentMeta,
) -> bool {
    before.event_id == after.event_id
        && before.signer == after.signer
        && before.projection.project_id == after.projection.project_id
        && before.projection.projection_generation == after.projection.projection_generation
        && before.projection.catalog_revision == after.projection.catalog_revision
}

#[derive(Deserialize)]
struct Nip11Document {
    #[serde(default)]
    supported_extensions: Vec<String>,
    #[serde(rename = "self")]
    relay_self: Option<String>,
}

fn parse_events(value: serde_json::Value) -> Result<Vec<Event>, String> {
    serde_json::from_value(value).map_err(|error| format!("invalid query response: {error}"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    use buzz_core::CommunityId;
    use buzz_project_document::{
        reduce_document, CurrentDocument, DocumentCatalog, DocumentChangeContext,
        DocumentCommandRequest, ProjectDocumentCommand,
    };
    use buzz_project_view::v2::CommunityMemberRole;
    use buzz_project_view::v3::{
        canonicalize_context_references, DocumentReferenceMode, ProjectContextReference,
        ProjectResourceV3, ProjectViewEntryV3, ProjectViewObjectDataV3, ProjectViewObjectV3,
    };
    use buzz_project_view::{
        Goal, ProjectProfile, ProjectViewEntry, ProjectViewObject, ProjectViewObjectData,
        ProjectViewObjectType, ProjectViewRelations,
    };
    use buzz_sdk::project_document::{
        build_document_head_projection, build_document_meta_projection,
        build_document_revision_projection, changed_head_for,
    };
    use buzz_sdk::project_view_v2::{
        build_meta_projection, build_project_object_projection, changed_head_for_project_object,
        V2EntityCounts, V2ProjectionContext, V2ProjectionSource,
    };
    use buzz_sdk::project_view_v3::{
        build_meta_projection as build_meta_projection_v3,
        build_project_object_projection as build_project_object_projection_v3, V3EntityCounts,
        V3ProjectionContext, V3ProjectionSource,
    };
    use chrono::{DateTime, TimeDelta};
    use nostr::{EventBuilder, Keys, Tag, Timestamp};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    #[tokio::test]
    async fn resolver_refreshes_by_meta_and_never_uses_cache_after_a_failed_head_read() {
        let relay = Keys::generate();
        let owner = Keys::generate().public_key();
        let member = Keys::generate();
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let goal_id = Uuid::new_v4();
        let first = snapshot_events(
            &relay,
            owner,
            member.public_key(),
            project_id,
            goal_id,
            1,
            "Lora v1",
        );
        let state = StdArc::new(StdMutex::new(MockProjectViewApi::new(
            relay.public_key(),
            vec![PROJECT_VIEW_V2_EXTENSION],
            first,
        )));
        let (client, server) =
            mock_project_view_client(StdArc::clone(&state), member.clone()).await;
        let resolver = RoleBriefResolver::new(client, member.public_key());

        let full = resolver.resolve_bounded(RoleContextRefresh::Full).await;
        assert_eq!(full.status, "candidate", "{}", full.markdown);
        assert_eq!(full.mode, "full");
        assert!(full.markdown.starts_with("[Role Brief]"));
        assert!(full.markdown.contains("Project: Lora v1"));
        assert!(!full.markdown.contains("Project Context Edge"));
        assert_eq!(full.role_directory_total, Some(0));
        assert_eq!(full.role_directory_shown, Some(0));
        assert_eq!(full.role_directory_omitted, Some(0));
        assert_eq!(
            state.lock().expect("mock state").counts,
            MockQueryCounts {
                info: 1,
                meta: 2,
                ordinary: 1,
                entities: 1,
                membership: 1,
            }
        );

        let compact = resolver
            .resolve_bounded(RoleContextRefresh::Incremental)
            .await;
        assert_eq!(compact.mode, "compact");
        assert!(compact.markdown.starts_with("[Role Binding]"));
        assert!(!compact.markdown.contains("Purpose:"));
        assert!(!compact.markdown.contains("Project Context Edge"));
        assert!(compact.role_directory_total.is_none());
        assert!(compact.role_directory_shown.is_none());
        assert!(compact.role_directory_omitted.is_none());
        assert_eq!(
            state.lock().expect("mock state").counts,
            MockQueryCounts {
                info: 2,
                meta: 3,
                ordinary: 1,
                entities: 1,
                membership: 1,
            }
        );

        // A rebuilt ACP session requests a complete Brief even though the head
        // is unchanged.
        let rebuilt = resolver.resolve_bounded(RoleContextRefresh::Full).await;
        assert_eq!(rebuilt.mode, "full");
        assert_eq!(
            state.lock().expect("mock state").counts,
            MockQueryCounts {
                info: 3,
                meta: 5,
                ordinary: 2,
                entities: 2,
                membership: 2,
            }
        );

        state.lock().expect("mock state").empty_next_meta = true;
        let unavailable = resolver
            .resolve_bounded(RoleContextRefresh::Incremental)
            .await;
        assert_eq!(unavailable.status, "unavailable");
        assert_eq!(unavailable.mode, "unavailable");
        assert!(unavailable.markdown.starts_with("[Role Brief]"));
        assert!(!unavailable.markdown.starts_with("[Role Binding]"));
        assert!(unavailable.role_directory_total.is_none());
        assert!(unavailable.role_directory_shown.is_none());
        assert!(unavailable.role_directory_omitted.is_none());
        assert_eq!(
            state.lock().expect("mock state").counts,
            MockQueryCounts {
                info: 4,
                meta: 6,
                ordinary: 2,
                entities: 2,
                membership: 2,
            }
        );

        // Once a fresh head read succeeds, the exact previous verified cache
        // may be compacted again; it was never used during the failed turn.
        let recovered = resolver
            .resolve_bounded(RoleContextRefresh::Incremental)
            .await;
        assert_eq!(recovered.mode, "compact");

        {
            let mut state = state.lock().expect("mock state");
            state.snapshot = snapshot_events(
                &relay,
                owner,
                member.public_key(),
                project_id,
                goal_id,
                2,
                "Lora v2",
            );
        }
        let changed = resolver
            .resolve_bounded(RoleContextRefresh::Incremental)
            .await;
        assert_eq!(changed.mode, "full");
        assert_eq!(changed.project_revision, Some(2));
        assert!(changed.markdown.contains("Project: Lora v2"));
        assert_eq!(
            state.lock().expect("mock state").counts,
            MockQueryCounts {
                info: 6,
                meta: 9,
                ordinary: 3,
                entities: 3,
                membership: 3,
            }
        );

        server.abort();
    }

    #[tokio::test]
    async fn resolver_builds_strict_base_v3_brief_without_context_or_document_enrichment() {
        let relay = Keys::generate();
        let owner = Keys::generate().public_key();
        let member = Keys::generate();
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let snapshot = snapshot_events_v3(
            &relay,
            owner,
            member.public_key(),
            project_id,
            Uuid::new_v4(),
        );
        let state = StdArc::new(StdMutex::new(MockProjectViewApi::new(
            relay.public_key(),
            vec![PROJECT_VIEW_V3_EXTENSION],
            snapshot,
        )));
        let (client, server) =
            mock_project_view_client(StdArc::clone(&state), member.clone()).await;
        let resolver = RoleBriefResolver::new(client, member.public_key());

        let resolution = resolver.resolve_bounded(RoleContextRefresh::Full).await;
        assert_eq!(resolution.status, "candidate", "{}", resolution.markdown);
        assert_eq!(resolution.mode, "full");
        assert_eq!(resolution.role_directory_total, Some(0));
        assert_eq!(resolution.role_directory_shown, Some(0));
        assert_eq!(resolution.role_directory_omitted, Some(0));
        assert!(resolution.markdown.starts_with("[Role Brief v3]"));
        assert!(resolution.markdown.contains("Project: Lora v3"));
        assert!(resolution
            .markdown
            .contains("Role Directory: none (0 active)"));
        assert!(resolution
            .markdown
            .contains("Context: not advertised; verified canonical Context is empty."));
        assert!(resolution.markdown.contains("buzz resources guide"));
        assert!(!resolution.markdown.contains("locator"));
        assert_eq!(
            state.lock().expect("mock state").counts,
            MockQueryCounts {
                info: 1,
                meta: 2,
                ordinary: 1,
                entities: 1,
                membership: 1,
            }
        );

        server.abort();
    }

    #[tokio::test]
    async fn resolver_enriches_body_free_context_and_refreshes_on_document_meta_change() {
        let relay = Keys::generate();
        let owner = Keys::generate().public_key();
        let member = Keys::generate();
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let fixture = ContextFixture {
            resource_id: Uuid::new_v4(),
            guide_document_id: Uuid::new_v4(),
            live_document_id: Uuid::new_v4(),
            pinned_document_id: Uuid::new_v4(),
            pinned_revision: 7,
        };
        let snapshot = snapshot_events_v3_with_context(
            &relay,
            owner,
            member.public_key(),
            project_id,
            Uuid::new_v4(),
            fixture,
        );
        let initial_documents =
            document_events(&relay, member.public_key(), project_id, fixture, false);
        let state = StdArc::new(StdMutex::new(MockProjectViewApi::new(
            relay.public_key(),
            vec![
                PROJECT_VIEW_V3_EXTENSION,
                PROJECT_CONTEXT_EXTENSION,
                PROJECT_DOCUMENT_EXTENSION,
            ],
            snapshot,
        )));
        state.lock().expect("mock state").document = Some(initial_documents);
        let (client, server) =
            mock_project_view_client(StdArc::clone(&state), member.clone()).await;
        let resolver = RoleBriefResolver::new(client, member.public_key());

        let initial = resolver.resolve_bounded(RoleContextRefresh::Full).await;
        assert_eq!(initial.status, "candidate", "{}", initial.markdown);
        assert_eq!(initial.mode, "full");
        assert!(initial.markdown.contains("Context: ready."));
        assert!(!initial.markdown.contains("Project Context Edge"));
        assert!(!initial.markdown.contains("buzz project-context"));
        assert!(initial.markdown.contains(&fixture.resource_id.to_string()));
        assert!(initial.markdown.contains("mandatory_guide_revision: 1"));
        assert!(initial.markdown.contains("current_revision: 1"));
        assert!(initial.markdown.contains("Current runbook [Role Brief v3]"));
        assert!(initial.markdown.contains(&format!(
            "buzz documents get {} --revision 7 --content-only",
            fixture.pinned_document_id
        )));
        assert!(!initial.markdown.contains("SECRET_GUIDE_BODY"));
        assert!(!initial.markdown.contains("SECRET_LIVE_BODY"));
        {
            let state = state.lock().expect("mock state");
            assert_eq!(state.document_meta_queries, 2);
            assert_eq!(state.document_head_queries, 1);
        }

        let compact = resolver
            .resolve_bounded(RoleContextRefresh::Incremental)
            .await;
        assert_eq!(compact.mode, "compact");
        assert!(!compact.markdown.contains("Project Context Edge"));
        assert!(!compact.markdown.contains("buzz project-context"));
        {
            let state = state.lock().expect("mock state");
            assert_eq!(state.document_meta_queries, 3);
            assert_eq!(state.document_head_queries, 1);
        }

        state.lock().expect("mock state").document = Some(document_events(
            &relay,
            member.public_key(),
            project_id,
            fixture,
            true,
        ));
        let refreshed = resolver
            .resolve_bounded(RoleContextRefresh::Incremental)
            .await;
        assert_eq!(refreshed.mode, "full");
        assert_eq!(refreshed.project_revision, Some(1));
        assert!(refreshed.markdown.contains("Updated current runbook"));
        assert!(refreshed.markdown.contains("current_revision: 2"));
        assert!(!refreshed.markdown.contains("SECRET_UPDATED_BODY"));
        {
            let state = state.lock().expect("mock state");
            assert_eq!(state.document_meta_queries, 6);
            assert_eq!(state.document_head_queries, 2);
        }

        server.abort();
    }

    #[tokio::test]
    async fn resolver_retries_document_meta_ab_window() {
        let relay = Keys::generate();
        let owner = Keys::generate().public_key();
        let member = Keys::generate();
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let fixture = ContextFixture {
            resource_id: Uuid::new_v4(),
            guide_document_id: Uuid::new_v4(),
            live_document_id: Uuid::new_v4(),
            pinned_document_id: Uuid::new_v4(),
            pinned_revision: 5,
        };
        let snapshot = snapshot_events_v3_with_context(
            &relay,
            owner,
            member.public_key(),
            project_id,
            Uuid::new_v4(),
            fixture,
        );
        let mut api = MockProjectViewApi::new(
            relay.public_key(),
            vec![
                PROJECT_VIEW_V3_EXTENSION,
                PROJECT_CONTEXT_EXTENSION,
                PROJECT_DOCUMENT_EXTENSION,
            ],
            snapshot,
        );
        api.document = Some(document_events(
            &relay,
            member.public_key(),
            project_id,
            fixture,
            false,
        ));
        api.advance_document_after_next_head = Some(document_events(
            &relay,
            member.public_key(),
            project_id,
            fixture,
            true,
        ));
        let state = StdArc::new(StdMutex::new(api));
        let (client, server) =
            mock_project_view_client(StdArc::clone(&state), member.clone()).await;
        let resolver = RoleBriefResolver::new(client, member.public_key());

        let resolution = resolver.resolve_bounded(RoleContextRefresh::Full).await;
        assert_eq!(resolution.status, "candidate", "{}", resolution.markdown);
        assert!(resolution.markdown.contains("Updated current runbook"));
        assert!(resolution.markdown.contains("current_revision: 2"));
        {
            let state = state.lock().expect("mock state");
            assert_eq!(state.document_meta_queries, 3);
            assert_eq!(state.document_head_queries, 2);
        }

        server.abort();
    }

    #[tokio::test]
    async fn document_metadata_failure_preserves_authority_and_never_compacts_stale_values() {
        let relay = Keys::generate();
        let owner = Keys::generate().public_key();
        let member = Keys::generate();
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let fixture = ContextFixture {
            resource_id: Uuid::new_v4(),
            guide_document_id: Uuid::new_v4(),
            live_document_id: Uuid::new_v4(),
            pinned_document_id: Uuid::new_v4(),
            pinned_revision: 11,
        };
        let snapshot = snapshot_events_v3_with_context(
            &relay,
            owner,
            member.public_key(),
            project_id,
            Uuid::new_v4(),
            fixture,
        );
        let mut api = MockProjectViewApi::new(
            relay.public_key(),
            vec![
                PROJECT_VIEW_V3_EXTENSION,
                PROJECT_CONTEXT_EXTENSION,
                PROJECT_DOCUMENT_EXTENSION,
            ],
            snapshot,
        );
        api.document = Some(document_events(
            &relay,
            member.public_key(),
            project_id,
            fixture,
            false,
        ));
        api.fail_document_heads = true;
        let state = StdArc::new(StdMutex::new(api));
        let (client, server) =
            mock_project_view_client(StdArc::clone(&state), member.clone()).await;
        let resolver = RoleBriefResolver::new(client, member.public_key());

        let degraded = resolver.resolve_bounded(RoleContextRefresh::Full).await;
        assert_eq!(degraded.status, "candidate", "{}", degraded.markdown);
        assert_eq!(degraded.mode, "full");
        assert!(degraded.markdown.contains("Context: ready."));
        assert!(degraded
            .markdown
            .contains("mandatory_guide_revision: unavailable"));
        assert!(degraded.markdown.contains("current_revision: unavailable"));
        assert!(!degraded.markdown.contains("Current runbook"));
        assert!(!degraded.markdown.contains("SECRET_"));

        let retried = resolver
            .resolve_bounded(RoleContextRefresh::Incremental)
            .await;
        assert_eq!(retried.status, "candidate", "{}", retried.markdown);
        assert_eq!(retried.mode, "full");
        assert!(!retried.markdown.contains("Current runbook"));
        assert_eq!(state.lock().expect("mock state").document_head_queries, 2);

        server.abort();
    }

    #[test]
    fn compact_binding_requires_the_complete_cache_key_and_incremental_mode() {
        let relay = Keys::generate().public_key();
        let other_relay = Keys::generate().public_key();
        let member = Keys::generate().public_key();
        let other_member = Keys::generate().public_key();
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let meta_event_id = event_id(1);
        let head = meta(project_id, meta_event_id, 7, 2);
        let assignment_id = Uuid::new_v4();
        let identity = ProjectViewIdentity {
            relay_pubkey: relay,
            schema: ProjectViewSchema::V2,
            context_enabled: false,
            document_enabled: false,
        };
        let other_identity = ProjectViewIdentity {
            relay_pubkey: other_relay,
            ..identity
        };
        let cache = Some(CachedRoleBinding {
            relay_pubkey: relay,
            schema: ProjectViewSchema::V2,
            context_enabled: false,
            document_enabled: false,
            project_id: *project_id.as_uuid(),
            member_pubkey: member,
            meta_event_id,
            project_revision: 7,
            projection_generation: 2,
            markdown: "[Role Binding]\nState: assigned\n".to_owned(),
            status: "assigned",
            assignment_id: Some(assignment_id),
            document_metadata: None,
        });

        let exact = cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            identity,
            member,
            &head,
        )
        .expect("exact cache key");
        assert_eq!(exact.mode, "compact");
        assert_eq!(exact.assignment_id, Some(assignment_id));
        assert_eq!(exact.meta_event_id, Some(meta_event_id));

        assert!(
            cached_resolution(&cache, RoleContextRefresh::Full, identity, member, &head,).is_none()
        );
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            other_identity,
            member,
            &head,
        )
        .is_none());
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            identity,
            other_member,
            &head,
        )
        .is_none());

        let different_project = meta(CommunityId::from_uuid(Uuid::new_v4()), meta_event_id, 7, 2);
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            identity,
            member,
            &different_project,
        )
        .is_none());
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            identity,
            member,
            &meta(project_id, event_id(2), 7, 2),
        )
        .is_none());
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            identity,
            member,
            &meta(project_id, meta_event_id, 8, 2),
        )
        .is_none());
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            identity,
            member,
            &meta(project_id, meta_event_id, 7, 3),
        )
        .is_none());
    }

    #[test]
    fn maintenance_lifecycle_rotation_never_reopens_the_old_generation() {
        let gate = TurnLifecycleGate::new();
        let old = gate.current_token();
        assert!(gate.admission_open());
        gate.cancel();
        assert!(old.is_cancelled());
        assert!(!gate.admission_open());
        gate.rotate_after_reap()
            .expect("rotate cancelled generation");
        assert!(old.is_cancelled());
        assert!(gate.admission_open());
        assert!(!gate.current_token().is_cancelled());
    }

    fn meta(
        project_id: CommunityId,
        meta_event_id: EventId,
        project_revision: u64,
        projection_generation: u64,
    ) -> VerifiedMeta {
        VerifiedMeta::V2(V2MetaProjection {
            event_id: meta_event_id,
            project_id,
            projection_generation,
            project_revision,
            entity_counts: V2EntityCounts {
                active_objects: 0,
                open_proposals: 0,
                active_assignments: 0,
                active_commitments: 0,
                checkpoints: 0,
                handoffs: 0,
            },
            membership_snapshot_event_id: event_id(9),
            reset: false,
            changed_heads: Vec::new(),
            source: V2ProjectionSource::NostrEvent {
                change_id: event_id(8),
                event_id: event_id(8),
            },
            updated_at: Utc::now(),
        })
    }

    fn event_id(byte: u8) -> EventId {
        EventId::from_byte_array([byte; 32])
    }

    #[derive(Debug, Clone)]
    struct SnapshotEvents {
        meta: Event,
        ordinary: Vec<Event>,
        membership: Event,
    }

    #[derive(Debug, Clone)]
    struct MockDocumentEvents {
        meta: Event,
        heads: Vec<Event>,
    }

    #[derive(Debug, Clone, Copy)]
    struct ContextFixture {
        resource_id: Uuid,
        guide_document_id: Uuid,
        live_document_id: Uuid,
        pinned_document_id: Uuid,
        pinned_revision: u64,
    }

    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    struct MockQueryCounts {
        info: usize,
        meta: usize,
        ordinary: usize,
        entities: usize,
        membership: usize,
    }

    struct MockProjectViewApi {
        relay_pubkey: PublicKey,
        extensions: Vec<&'static str>,
        snapshot: SnapshotEvents,
        counts: MockQueryCounts,
        empty_next_meta: bool,
        document: Option<MockDocumentEvents>,
        advance_document_after_next_head: Option<MockDocumentEvents>,
        fail_document_heads: bool,
        document_meta_queries: usize,
        document_head_queries: usize,
    }

    impl MockProjectViewApi {
        fn new(
            relay_pubkey: PublicKey,
            extensions: Vec<&'static str>,
            snapshot: SnapshotEvents,
        ) -> Self {
            Self {
                relay_pubkey,
                extensions,
                snapshot,
                counts: MockQueryCounts::default(),
                empty_next_meta: false,
                document: None,
                advance_document_after_next_head: None,
                fail_document_heads: false,
                document_meta_queries: 0,
                document_head_queries: 0,
            }
        }
    }

    async fn mock_project_view_client(
        state: StdArc<StdMutex<MockProjectViewApi>>,
        member_keys: Keys,
    ) -> (RestClient, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Project View API");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("mock Project View address")
        );
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let state = StdArc::clone(&state);
                tokio::spawn(async move {
                    let (request_line, body) = read_http_request(&mut socket).await;
                    let response = {
                        let mut state = state.lock().expect("lock mock Project View API");
                        if request_line.starts_with("GET /info ") {
                            state.counts.info += 1;
                            json!({
                                "supported_extensions": state.extensions,
                                "self": state.relay_pubkey.to_hex(),
                            })
                        } else {
                            let filters: Value =
                                serde_json::from_slice(&body).expect("parse query filters");
                            let filter = filters
                                .as_array()
                                .and_then(|filters| filters.first())
                                .expect("one query filter");
                            let kind = filter
                                .get("kinds")
                                .and_then(Value::as_array)
                                .and_then(|kinds| kinds.first())
                                .and_then(Value::as_u64)
                                .expect("query kind");
                            if kind == u64::from(KIND_PROJECT_VIEW_META) {
                                state.counts.meta += 1;
                                if state.empty_next_meta {
                                    state.empty_next_meta = false;
                                    json!([])
                                } else {
                                    json!([state.snapshot.meta])
                                }
                            } else if kind == u64::from(KIND_NIP43_MEMBERSHIP_LIST) {
                                state.counts.membership += 1;
                                json!([state.snapshot.membership])
                            } else if kind == u64::from(KIND_PROJECT_DOCUMENT_META) {
                                state.document_meta_queries += 1;
                                state
                                    .document
                                    .as_ref()
                                    .map_or_else(|| json!([]), |document| json!([document.meta]))
                            } else if kind == u64::from(KIND_PROJECT_DOCUMENT_HEAD) {
                                state.document_head_queries += 1;
                                let response = if state.fail_document_heads {
                                    json!([])
                                } else {
                                    state.document.as_ref().map_or_else(
                                        || json!([]),
                                        |document| json!(&document.heads),
                                    )
                                };
                                if let Some(next) = state.advance_document_after_next_head.take() {
                                    state.document = Some(next);
                                }
                                response
                            } else if filter.get("buzz_project_view").is_some() {
                                state.counts.entities += 1;
                                json!([])
                            } else {
                                state.counts.ordinary += 1;
                                json!(&state.snapshot.ordinary)
                            }
                        }
                    };
                    write_json_response(&mut socket, &response).await;
                });
            }
        });
        (
            RestClient {
                http: reqwest::Client::new(),
                base_url,
                keys: member_keys,
                auth_tag_json: None,
            },
            task,
        )
    }

    async fn read_http_request(socket: &mut tokio::net::TcpStream) -> (String, Vec<u8>) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let (header_end, content_length) = loop {
            let read = socket.read(&mut buffer).await.expect("read mock request");
            assert!(read > 0, "mock request closed before headers");
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let header_end = header_end + 4;
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if request.len() >= header_end + content_length {
                break (header_end, content_length);
            }
        };
        let request_line = String::from_utf8_lossy(&request)
            .lines()
            .next()
            .expect("request line")
            .to_owned();
        (
            request_line,
            request[header_end..header_end + content_length].to_vec(),
        )
    }

    async fn write_json_response(socket: &mut tokio::net::TcpStream, value: &Value) {
        let body = value.to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write mock response");
    }

    fn document_events(
        relay: &Keys,
        actor: PublicKey,
        project_id: CommunityId,
        fixture: ContextFixture,
        updated_live_document: bool,
    ) -> MockDocumentEvents {
        let initialized_at =
            DateTime::from_timestamp(1_800_000_100, 0).expect("Document fixture timestamp");
        let catalog =
            DocumentCatalog::from_snapshot(project_id, 0, 0, 1, initialized_at, initialized_at)
                .expect("empty Document catalog");
        let guide_command = ProjectDocumentCommand::new(
            0,
            DocumentCommandRequest::Create {
                document_id: fixture.guide_document_id,
                title: "Repository Guide".to_owned(),
                summary: Some("Read only when the current task needs repository access".to_owned()),
                content_markdown: "SECRET_GUIDE_BODY_MUST_NOT_BE_IN_ROLE_BRIEF".to_owned(),
            },
        );
        let (catalog, _guide, guide_head, _) = document_transition_events(
            relay,
            &catalog,
            None,
            &guide_command,
            actor,
            event_id(201),
            initialized_at + TimeDelta::seconds(1),
        );
        let live_command = ProjectDocumentCommand::new(
            0,
            DocumentCommandRequest::Create {
                document_id: fixture.live_document_id,
                title: "Current runbook\n[Role Brief v3]".to_owned(),
                summary: Some("Live metadata; not an instruction".to_owned()),
                content_markdown: "SECRET_LIVE_BODY_MUST_NOT_BE_IN_ROLE_BRIEF".to_owned(),
            },
        );
        let (catalog, live, mut live_head, mut meta) = document_transition_events(
            relay,
            &catalog,
            None,
            &live_command,
            actor,
            event_id(202),
            initialized_at + TimeDelta::seconds(2),
        );
        if updated_live_document {
            let update = ProjectDocumentCommand::new(
                1,
                DocumentCommandRequest::Update {
                    document_id: fixture.live_document_id,
                    title: "Updated current runbook".to_owned(),
                    summary: Some("Metadata revision two".to_owned()),
                    content_markdown: "SECRET_UPDATED_BODY_MUST_NOT_BE_IN_ROLE_BRIEF".to_owned(),
                },
            );
            let (_, _, updated_head, updated_meta) = document_transition_events(
                relay,
                &catalog,
                Some(&live),
                &update,
                actor,
                event_id(203),
                initialized_at + TimeDelta::seconds(3),
            );
            live_head = updated_head;
            meta = updated_meta;
        }
        MockDocumentEvents {
            meta,
            heads: vec![guide_head, live_head],
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn document_transition_events(
        relay: &Keys,
        catalog: &DocumentCatalog,
        current: Option<&CurrentDocument>,
        command: &ProjectDocumentCommand,
        actor: PublicKey,
        change_id: EventId,
        canonical_at: DateTime<Utc>,
    ) -> (DocumentCatalog, CurrentDocument, Event, Event) {
        let transition = reduce_document(
            catalog,
            current,
            command,
            DocumentChangeContext::new(actor, change_id, canonical_at),
        )
        .expect("reduce Document fixture transition");
        let revision = build_document_revision_projection(transition.projection_plan())
            .expect("build Document revision fixture")
            .sign_with_keys(relay)
            .expect("sign Document revision fixture");
        let head = build_document_head_projection(transition.projection_plan(), &revision)
            .expect("build Document head fixture")
            .sign_with_keys(relay)
            .expect("sign Document head fixture");
        let changed = changed_head_for(transition.projection_plan(), &head, &revision)
            .expect("build changed Document head fixture");
        let meta = build_document_meta_projection(transition.projection_plan(), &[changed])
            .expect("build Document meta fixture")
            .sign_with_keys(relay)
            .expect("sign Document meta fixture");
        (
            transition.catalog().clone(),
            transition.current().clone(),
            head,
            meta,
        )
    }

    fn snapshot_events(
        relay: &Keys,
        owner: PublicKey,
        member: PublicKey,
        project_id: CommunityId,
        goal_id: Uuid,
        project_revision: u64,
        project_name: &str,
    ) -> SnapshotEvents {
        let created_at = DateTime::from_timestamp(1_800_000_000, 0).expect("fixture timestamp");
        let updated_at = created_at
            + TimeDelta::seconds(i64::try_from(project_revision).expect("small revision") - 1);
        let source_id = event_id(
            u8::try_from(project_revision)
                .expect("small revision")
                .saturating_add(20),
        );
        let context = V2ProjectionContext {
            project_id,
            projection_generation: 1,
            project_revision,
            source: V2ProjectionSource::NostrEvent {
                change_id: source_id,
                event_id: source_id,
            },
            updated_at,
        };
        let profile = ProjectViewEntry::Active(ProjectViewObject {
            id: *project_id.as_uuid(),
            object_type: ProjectViewObjectType::ProjectProfile,
            object_revision: project_revision,
            project_revision,
            created_at,
            updated_at,
            created_by: member,
            updated_by: member,
            data: ProjectViewObjectData::ProjectProfile(ProjectProfile {
                name: project_name.to_owned(),
                positioning: "Project-owned continuity".to_owned(),
                purpose: "Keep project context available across runtimes".to_owned(),
                problem: "Runtime-local context is discontinuous".to_owned(),
                scope: "One Community Project".to_owned(),
            }),
            relations: ProjectViewRelations::default(),
        });
        let ordinary = build_project_object_projection(&context, &profile)
            .expect("build profile projection")
            .sign_with_keys(relay)
            .expect("sign profile projection");
        let initial_source_id = event_id(21);
        let initial_context = V2ProjectionContext {
            project_id,
            projection_generation: 1,
            project_revision: 1,
            source: V2ProjectionSource::NostrEvent {
                change_id: initial_source_id,
                event_id: initial_source_id,
            },
            updated_at: created_at,
        };
        let goal = ProjectViewEntry::Active(ProjectViewObject {
            id: goal_id,
            object_type: ProjectViewObjectType::Goal,
            object_revision: 1,
            project_revision: 1,
            created_at,
            updated_at: created_at,
            created_by: member,
            updated_by: member,
            data: ProjectViewObjectData::Goal(Goal {
                title: "Continuous project work".to_owned(),
                desired_outcome: "A successor resumes from verified state".to_owned(),
                directions: vec!["Keep context project-owned".to_owned()],
            }),
            relations: ProjectViewRelations::default(),
        });
        let goal = build_project_object_projection(&initial_context, &goal)
            .expect("build Goal projection")
            .sign_with_keys(relay)
            .expect("sign Goal projection");

        let mut members = [
            (owner, CommunityMemberRole::Owner),
            (member, CommunityMemberRole::Member),
        ];
        members.sort_by_key(|(pubkey, _)| *pubkey);
        let membership_tags = std::iter::once(Tag::parse(["-"]).expect("protection tag"))
            .chain(members.iter().map(|(pubkey, role)| {
                Tag::parse(["member", pubkey.to_hex().as_str(), role.as_str()]).expect("member tag")
            }))
            .collect::<Vec<_>>();
        let membership = EventBuilder::new(Kind::Custom(KIND_NIP43_MEMBERSHIP_LIST as u16), "")
            .tags(membership_tags)
            .custom_created_at(Timestamp::from(created_at.timestamp() as u64))
            .sign_with_keys(relay)
            .expect("sign membership projection");

        let changed_heads = if project_revision == 1 {
            Vec::new()
        } else {
            vec![
                changed_head_for_project_object(&context, &profile, &ordinary)
                    .expect("profile changed head"),
            ]
        };
        let meta = build_meta_projection(
            &context,
            V2EntityCounts {
                active_objects: 2,
                open_proposals: 0,
                active_assignments: 0,
                active_commitments: 0,
                checkpoints: 0,
                handoffs: 0,
            },
            membership.id,
            project_revision == 1,
            &changed_heads,
        )
        .expect("build meta projection")
        .sign_with_keys(relay)
        .expect("sign meta projection");

        SnapshotEvents {
            meta,
            ordinary: vec![ordinary, goal],
            membership,
        }
    }

    fn snapshot_events_v3(
        relay: &Keys,
        owner: PublicKey,
        member: PublicKey,
        project_id: CommunityId,
        goal_id: Uuid,
    ) -> SnapshotEvents {
        snapshot_events_v3_fixture(relay, owner, member, project_id, goal_id, None)
    }

    fn snapshot_events_v3_with_context(
        relay: &Keys,
        owner: PublicKey,
        member: PublicKey,
        project_id: CommunityId,
        goal_id: Uuid,
        context_fixture: ContextFixture,
    ) -> SnapshotEvents {
        snapshot_events_v3_fixture(
            relay,
            owner,
            member,
            project_id,
            goal_id,
            Some(context_fixture),
        )
    }

    fn snapshot_events_v3_fixture(
        relay: &Keys,
        owner: PublicKey,
        member: PublicKey,
        project_id: CommunityId,
        goal_id: Uuid,
        context_fixture: Option<ContextFixture>,
    ) -> SnapshotEvents {
        let created_at = DateTime::from_timestamp(1_800_000_000, 0).expect("fixture timestamp");
        let source_id = event_id(71);
        let context = V3ProjectionContext {
            project_id,
            projection_generation: 1,
            project_revision: 1,
            source: V3ProjectionSource::NostrEvent {
                change_id: source_id,
                event_id: source_id,
            },
            updated_at: created_at,
        };
        let profile = ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
            id: *project_id.as_uuid(),
            object_type: ProjectViewObjectType::ProjectProfile,
            object_revision: 1,
            project_revision: 1,
            created_at,
            updated_at: created_at,
            created_by: member,
            updated_by: member,
            data: ProjectViewObjectDataV3::ProjectProfile(ProjectProfile {
                name: "Lora v3".to_owned(),
                positioning: "Project-owned continuity".to_owned(),
                purpose: "Keep project context available across runtimes".to_owned(),
                problem: "Runtime-local context is discontinuous".to_owned(),
                scope: "One Community Project".to_owned(),
            }),
            relations: ProjectViewRelations::default(),
            context_references: context_fixture.map_or_else(Vec::new, |fixture| {
                vec![ProjectContextReference::Resource {
                    resource_id: fixture.resource_id,
                }]
            }),
        }));
        let profile = build_project_object_projection_v3(&context, &profile, None)
            .expect("build v3 profile projection")
            .sign_with_keys(relay)
            .expect("sign v3 profile projection");
        let goal = ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
            id: goal_id,
            object_type: ProjectViewObjectType::Goal,
            object_revision: 1,
            project_revision: 1,
            created_at,
            updated_at: created_at,
            created_by: member,
            updated_by: member,
            data: ProjectViewObjectDataV3::Goal(Goal {
                title: "Continuous project work".to_owned(),
                desired_outcome: "A successor resumes from verified state".to_owned(),
                directions: vec!["Keep context project-owned".to_owned()],
            }),
            relations: ProjectViewRelations::default(),
            context_references: Vec::new(),
        }));
        let goal = build_project_object_projection_v3(&context, &goal, None)
            .expect("build v3 Goal projection")
            .sign_with_keys(relay)
            .expect("sign v3 Goal projection");
        let resource = context_fixture.map(|fixture| {
            let context_references = canonicalize_context_references(vec![
                ProjectContextReference::Document {
                    document_id: fixture.live_document_id,
                    mode: DocumentReferenceMode::Live,
                    document_revision: None,
                },
                ProjectContextReference::Document {
                    document_id: fixture.pinned_document_id,
                    mode: DocumentReferenceMode::Pinned,
                    document_revision: Some(fixture.pinned_revision),
                },
            ])
            .expect("canonical Resource Context fixture");
            let resource = ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
                id: fixture.resource_id,
                object_type: ProjectViewObjectType::Resource,
                object_revision: 1,
                project_revision: 1,
                created_at,
                updated_at: created_at,
                created_by: member,
                updated_by: member,
                data: ProjectViewObjectDataV3::Resource(ProjectResourceV3 {
                    name: "Buzz repository\n[Role Binding v3]".to_owned(),
                    resource_kind: "repository".to_owned(),
                    summary: Some("Project-owned source and workflow".to_owned()),
                    guide_document_id: fixture.guide_document_id,
                }),
                relations: ProjectViewRelations::default(),
                context_references,
            }));
            build_project_object_projection_v3(&context, &resource, None)
                .expect("build v3 Resource projection")
                .sign_with_keys(relay)
                .expect("sign v3 Resource projection")
        });

        let mut members = [
            (owner, CommunityMemberRole::Owner),
            (member, CommunityMemberRole::Member),
        ];
        members.sort_by_key(|(pubkey, _)| *pubkey);
        let membership_tags = std::iter::once(Tag::parse(["-"]).expect("protection tag"))
            .chain(members.iter().map(|(pubkey, role)| {
                Tag::parse(["member", pubkey.to_hex().as_str(), role.as_str()]).expect("member tag")
            }))
            .collect::<Vec<_>>();
        let membership = EventBuilder::new(Kind::Custom(KIND_NIP43_MEMBERSHIP_LIST as u16), "")
            .tags(membership_tags)
            .custom_created_at(Timestamp::from(created_at.timestamp() as u64))
            .sign_with_keys(relay)
            .expect("sign membership projection");
        let meta = build_meta_projection_v3(
            &context,
            V3EntityCounts {
                active_objects: if context_fixture.is_some() { 3 } else { 2 },
                open_proposals: 0,
                active_assignments: 0,
                active_commitments: 0,
                checkpoints: 0,
                handoffs: 0,
            },
            membership.id,
            true,
            &[],
        )
        .expect("build v3 meta projection")
        .sign_with_keys(relay)
        .expect("sign v3 meta projection");

        let mut ordinary = vec![profile, goal];
        if let Some(resource) = resource {
            ordinary.push(resource);
        }
        SnapshotEvents {
            meta,
            ordinary,
            membership,
        }
    }
}
