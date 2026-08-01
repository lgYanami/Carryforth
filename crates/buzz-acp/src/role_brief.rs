//! Per-turn verified Role Brief resolution for managed Agent runtimes.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_sdk::project_view_v2::{
    parse_entity_projection, parse_membership_projection, parse_meta_projection,
    parse_project_object_projection, V2EntityProjection, V2MembershipProjection, V2MetaProjection,
};
use buzz_sdk::role_brief::{
    render_role_binding_markdown, render_role_brief_markdown, unavailable_role_brief_markdown,
    RoleBrief, VerifiedRoleBriefSnapshot,
};
use chrono::Utc;
use nostr::{Alphabet, Event, EventId, Filter, Kind, PublicKey, SingleLetterTag};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::relay::RestClient;

const PROJECT_VIEW_V2_EXTENSION: &str = "buzz-project-view-v2";
const ROLE_BRIEF_TIMEOUT: Duration = Duration::from_secs(12);
const RUNTIME_SUSPEND_TIMEOUT: Duration = Duration::from_secs(1);
const SNAPSHOT_ATTEMPTS: usize = 3;
const V2_ENTITY_PAGE_SIZE: usize = 500;

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

    fn full(brief: &RoleBrief) -> Self {
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
}

#[derive(Debug, Clone)]
struct CachedRoleBinding {
    relay_pubkey: PublicKey,
    project_id: Uuid,
    member_pubkey: PublicKey,
    meta_event_id: EventId,
    project_revision: u64,
    projection_generation: u64,
    markdown: String,
    status: &'static str,
    assignment_id: Option<Uuid>,
}

impl CachedRoleBinding {
    fn from_brief(relay_pubkey: PublicKey, brief: &RoleBrief) -> Self {
        Self {
            relay_pubkey,
            project_id: brief.project_id,
            member_pubkey: brief.member_pubkey,
            meta_event_id: brief.source_revisions.meta_event_id,
            project_revision: brief.project_revision,
            projection_generation: brief.projection_generation,
            markdown: render_role_binding_markdown(brief),
            status: brief.state.status(),
            assignment_id: brief.assignment_id(),
        }
    }

    fn matches(
        &self,
        relay_pubkey: PublicKey,
        member_pubkey: PublicKey,
        meta: &V2MetaProjection,
    ) -> bool {
        self.relay_pubkey == relay_pubkey
            && self.project_id == *meta.project_id.as_uuid()
            && self.member_pubkey == member_pubkey
            && self.meta_event_id == meta.event_id
            && self.project_revision == meta.project_revision
            && self.projection_generation == meta.projection_generation
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
    relay_pubkey: PublicKey,
    member_pubkey: PublicKey,
    meta: &V2MetaProjection,
) -> Option<RoleContextResolution> {
    (refresh == RoleContextRefresh::Incremental)
        .then_some(cache.as_ref())
        .flatten()
        .filter(|cached| cached.matches(relay_pubkey, member_pubkey, meta))
        .map(CachedRoleBinding::resolution)
}

#[derive(Debug)]
enum ResolutionFailure {
    Project(String),
    Runtime(String),
}

impl ResolutionFailure {
    const fn code(&self) -> &'static str {
        match self {
            Self::Project(_) => "project_view_unavailable",
            Self::Runtime(_) => "runtime_supervision_unavailable",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Project(detail) | Self::Runtime(detail) => detail,
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
        }
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
            Ok(resolution) => resolution,
            Err(failure) => {
                self.suspend_runtime().await;
                RoleContextResolution::unavailable(failure.code(), failure.detail())
            }
        }
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

        let relay_pubkey = tokio::time::timeout_at(deadline, self.read_relay_identity())
            .await
            .map_err(|_| {
                ResolutionFailure::Project("Relay identity verification timed out".to_owned())
            })?
            .map_err(ResolutionFailure::Project)?;

        // A Relay identity change is a hard cache realm boundary even if the
        // following meta read fails.
        if cache
            .as_ref()
            .is_some_and(|cached| cached.relay_pubkey != relay_pubkey)
        {
            *cache = None;
        }

        let meta = tokio::time::timeout_at(deadline, self.read_meta(relay_pubkey))
            .await
            .map_err(|_| {
                ResolutionFailure::Project("Project View meta verification timed out".to_owned())
            })?
            .map_err(ResolutionFailure::Project)?;

        if let Some(resolution) =
            cached_resolution(&cache, refresh, relay_pubkey, self.member_pubkey, &meta)
        {
            self.reconcile_runtime(deadline, resolution.assignment_id)
                .await?;
            return Ok(resolution);
        }

        // A non-matching or explicitly refreshed cache must not survive a
        // failed full rebuild and accidentally look eligible later.
        *cache = None;
        let snapshot = tokio::time::timeout_at(deadline, self.resolve_verified(relay_pubkey, meta))
            .await
            .map_err(|_| {
                ResolutionFailure::Project("Role Brief snapshot resolution timed out".to_owned())
            })?
            .map_err(ResolutionFailure::Project)?;
        let brief = snapshot
            .brief_for(self.member_pubkey, Utc::now())
            .map_err(|error| ResolutionFailure::Project(error.to_string()))?;

        self.reconcile_runtime(deadline, brief.assignment_id())
            .await?;

        let resolution = RoleContextResolution::full(&brief);
        *cache = Some(CachedRoleBinding::from_brief(relay_pubkey, &brief));
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
        tokio::time::timeout_at(deadline, supervisor.reconcile(assignment_id))
            .await
            .map_err(|_| {
                ResolutionFailure::Runtime(
                    "Runtime supervision reconciliation timed out".to_owned(),
                )
            })?
            .map_err(ResolutionFailure::Runtime)
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

    async fn resolve_verified(
        &self,
        relay_pubkey: PublicKey,
        mut before: V2MetaProjection,
    ) -> Result<VerifiedRoleBriefSnapshot, String> {
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
                    parse_project_object_projection(&event, &relay_pubkey, before.project_id)
                        .map_err(|error| error.to_string())?,
                );
            }
            for projection in &entity_projections {
                if !event_ids.insert(projection.event_id) {
                    return Err("entity query returned a duplicate event".to_owned());
                }
            }
            let membership = self.read_membership(relay_pubkey, &before).await?;
            let after = self.read_meta(relay_pubkey).await?;
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
                "limit": V2_ENTITY_PAGE_SIZE,
                "buzz_project_view": extension,
            });
            let events = parse_events(
                self.rest_client
                    .query_raw(&[filter])
                    .await
                    .map_err(|error| error.to_string())?,
            )?;
            if events.len() > V2_ENTITY_PAGE_SIZE {
                return Err("current-entity page exceeded its requested limit".to_owned());
            }
            let page_len = events.len();
            for event in events {
                if !event_ids.insert(event.id) {
                    return Err("current-entity pages returned a duplicate signed event".to_owned());
                }
                let projection = parse_entity_projection(&event, &relay_pubkey, meta.project_id)
                    .map_err(|error| error.to_string())?;
                after = Some(json!({
                    "entity_type": projection.entity.entity_type().as_str(),
                    "entity_id": projection.entity.entity_id(),
                }));
                projections.push(projection);
            }
            if page_len < V2_ENTITY_PAGE_SIZE {
                break;
            }
        }
        Ok(projections)
    }

    async fn read_relay_identity(&self) -> Result<PublicKey, String> {
        let value = self
            .rest_client
            .get_public("/info")
            .await
            .map_err(|error| error.to_string())?;
        let info: Nip11Document =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        if !info
            .supported_extensions
            .iter()
            .any(|extension| extension == PROJECT_VIEW_V2_EXTENSION)
        {
            return Err(format!(
                "Relay does not advertise {PROJECT_VIEW_V2_EXTENSION}"
            ));
        }
        let relay_self = info
            .relay_self
            .ok_or_else(|| "NIP-11 has no Relay `self` key".to_owned())?;
        let relay_pubkey = PublicKey::from_hex(&relay_self).map_err(|error| error.to_string())?;
        if relay_pubkey.to_hex() != relay_self {
            return Err("NIP-11 Relay `self` key is not canonical lowercase hex".to_owned());
        }
        Ok(relay_pubkey)
    }

    async fn read_meta(&self, relay_pubkey: PublicKey) -> Result<V2MetaProjection, String> {
        let filter = Filter::new()
            .kind(Kind::Custom(KIND_PROJECT_VIEW_META as u16))
            .author(relay_pubkey)
            .limit(2);
        let events = parse_events(
            self.rest_client
                .query(&[filter])
                .await
                .map_err(|error| error.to_string())?,
        )?;
        let [event] = events.as_slice() else {
            return Err("metadata query did not return exactly one v2 head".to_owned());
        };
        parse_meta_projection(event, &relay_pubkey).map_err(|error| error.to_string())
    }

    async fn read_membership(
        &self,
        relay_pubkey: PublicKey,
        meta: &V2MetaProjection,
    ) -> Result<V2MembershipProjection, String> {
        let filter = Filter::new()
            .id(meta.membership_snapshot_event_id)
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
        if event.id != meta.membership_snapshot_event_id {
            return Err(
                "membership query returned an event other than metadata pointer".to_owned(),
            );
        }
        parse_membership_projection(event, &relay_pubkey).map_err(|error| error.to_string())
    }
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
    use buzz_project_view::v2::CommunityMemberRole;
    use buzz_project_view::{
        Goal, ProjectProfile, ProjectViewEntry, ProjectViewObject, ProjectViewObjectData,
        ProjectViewObjectType, ProjectViewRelations,
    };
    use buzz_sdk::project_view_v2::{
        build_meta_projection, build_project_object_projection, changed_head_for_project_object,
        V2EntityCounts, V2ProjectionContext, V2ProjectionSource,
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
        let state = StdArc::new(StdMutex::new(MockProjectViewApi {
            relay_pubkey: relay.public_key(),
            snapshot: first,
            counts: MockQueryCounts::default(),
            empty_next_meta: false,
        }));
        let (client, server) =
            mock_project_view_client(StdArc::clone(&state), member.clone()).await;
        let resolver = RoleBriefResolver::new(client, member.public_key());

        let full = resolver.resolve_bounded(RoleContextRefresh::Full).await;
        assert_eq!(full.status, "candidate", "{}", full.markdown);
        assert_eq!(full.mode, "full");
        assert!(full.markdown.starts_with("[Role Brief]"));
        assert!(full.markdown.contains("Project: Lora v1"));
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
        let cache = Some(CachedRoleBinding {
            relay_pubkey: relay,
            project_id: *project_id.as_uuid(),
            member_pubkey: member,
            meta_event_id,
            project_revision: 7,
            projection_generation: 2,
            markdown: "[Role Binding]\nState: assigned\n".to_owned(),
            status: "assigned",
            assignment_id: Some(assignment_id),
        });

        let exact = cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            relay,
            member,
            &head,
        )
        .expect("exact cache key");
        assert_eq!(exact.mode, "compact");
        assert_eq!(exact.assignment_id, Some(assignment_id));
        assert_eq!(exact.meta_event_id, Some(meta_event_id));

        assert!(
            cached_resolution(&cache, RoleContextRefresh::Full, relay, member, &head,).is_none()
        );
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            other_relay,
            member,
            &head,
        )
        .is_none());
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            relay,
            other_member,
            &head,
        )
        .is_none());

        let different_project = meta(CommunityId::from_uuid(Uuid::new_v4()), meta_event_id, 7, 2);
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            relay,
            member,
            &different_project,
        )
        .is_none());
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            relay,
            member,
            &meta(project_id, event_id(2), 7, 2),
        )
        .is_none());
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            relay,
            member,
            &meta(project_id, meta_event_id, 8, 2),
        )
        .is_none());
        assert!(cached_resolution(
            &cache,
            RoleContextRefresh::Incremental,
            relay,
            member,
            &meta(project_id, meta_event_id, 7, 3),
        )
        .is_none());
    }

    fn meta(
        project_id: CommunityId,
        meta_event_id: EventId,
        project_revision: u64,
        projection_generation: u64,
    ) -> V2MetaProjection {
        V2MetaProjection {
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
        }
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
        snapshot: SnapshotEvents,
        counts: MockQueryCounts,
        empty_next_meta: bool,
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
                                "supported_extensions": [PROJECT_VIEW_V2_EXTENSION],
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
}
