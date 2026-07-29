//! Per-turn verified Role Brief resolution for managed Agent runtimes.

use std::collections::HashSet;
use std::time::Duration;

use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_sdk::project_view_v2::{
    parse_entity_projection, parse_membership_projection, parse_meta_projection,
    parse_project_object_projection, V2EntityProjection, V2MembershipProjection, V2MetaProjection,
};
use buzz_sdk::role_brief::{
    render_role_brief_markdown, unavailable_role_brief_markdown, RoleBriefMemberState,
    VerifiedRoleBriefSnapshot,
};
use chrono::Utc;
use nostr::{Alphabet, Event, Filter, Kind, PublicKey, SingleLetterTag};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::relay::RestClient;

const PROJECT_VIEW_V2_EXTENSION: &str = "buzz-project-view-v2";
const ROLE_BRIEF_TIMEOUT: Duration = Duration::from_secs(12);
const RUNTIME_SUSPEND_TIMEOUT: Duration = Duration::from_secs(1);
const SNAPSHOT_ATTEMPTS: usize = 3;
const V2_ENTITY_PAGE_SIZE: usize = 500;

/// Dynamic prompt section plus machine-readable resolution metadata.
#[derive(Debug, Clone)]
pub struct RoleContextResolution {
    /// Rendered `[Role Brief]` section, including fail-closed unavailable state.
    pub markdown: String,
    /// `candidate`, `assigned`, or `unavailable`.
    pub status: &'static str,
    /// Current active Assignment fence when assigned.
    pub assignment_id: Option<uuid::Uuid>,
    /// Verified Project revision, when available.
    pub project_revision: Option<u64>,
    /// Verified projection generation, when available.
    pub projection_generation: Option<u64>,
    /// Stable failure category when unavailable.
    pub error_code: Option<&'static str>,
}

impl RoleContextResolution {
    fn unavailable(code: &'static str, detail: &str) -> Self {
        Self {
            markdown: unavailable_role_brief_markdown(code, detail),
            status: "unavailable",
            assignment_id: None,
            project_revision: None,
            projection_generation: None,
            error_code: Some(code),
        }
    }
}

/// Resolver that re-reads the current v2 snapshot and optionally gates Runtime.
#[derive(Debug, Clone)]
pub struct RoleBriefResolver {
    rest_client: RestClient,
    member_pubkey: PublicKey,
    runtime_supervisor: Option<crate::runtime_supervisor::RuntimeSupervisorClient>,
}

impl RoleBriefResolver {
    /// Bind a resolver to the exact Relay client and managed Agent identity.
    #[must_use]
    pub const fn new(rest_client: RestClient, member_pubkey: PublicKey) -> Self {
        Self {
            rest_client,
            member_pubkey,
            runtime_supervisor: None,
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

    /// Resolve with a hard outer bound and convert every failure into a
    /// fail-closed prompt section. No previous Brief is cached or reused.
    pub async fn resolve_bounded(&self) -> RoleContextResolution {
        let deadline = tokio::time::Instant::now() + ROLE_BRIEF_TIMEOUT;
        let snapshot = match tokio::time::timeout_at(deadline, self.resolve_verified()).await {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                self.suspend_runtime().await;
                return RoleContextResolution::unavailable("project_view_unavailable", &error);
            }
            Err(_) => {
                self.suspend_runtime().await;
                return RoleContextResolution::unavailable(
                    "project_view_unavailable",
                    "Role Brief resolution timed out",
                );
            }
        };
        let brief = match snapshot.brief_for(self.member_pubkey, Utc::now()) {
            Ok(brief) => brief,
            Err(error) => {
                self.suspend_runtime().await;
                return RoleContextResolution::unavailable(
                    "project_view_unavailable",
                    &error.to_string(),
                );
            }
        };
        if let Some(supervisor) = &self.runtime_supervisor {
            match tokio::time::timeout_at(deadline, supervisor.reconcile(brief.assignment_id()))
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    self.suspend_runtime().await;
                    return RoleContextResolution::unavailable(
                        "runtime_supervision_unavailable",
                        &error,
                    );
                }
                Err(_) => {
                    self.suspend_runtime().await;
                    return RoleContextResolution::unavailable(
                        "runtime_supervision_unavailable",
                        "Runtime supervision reconciliation timed out",
                    );
                }
            }
        }
        let status = match &brief.state {
            RoleBriefMemberState::Candidate { .. } => "candidate",
            RoleBriefMemberState::Assigned { .. } => "assigned",
        };
        RoleContextResolution {
            markdown: render_role_brief_markdown(&brief),
            status,
            assignment_id: brief.assignment_id(),
            project_revision: Some(brief.project_revision),
            projection_generation: Some(brief.projection_generation),
            error_code: None,
        }
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

    async fn resolve_verified(&self) -> Result<VerifiedRoleBriefSnapshot, String> {
        let relay_pubkey = self.read_relay_identity().await?;
        for attempt in 0..SNAPSHOT_ATTEMPTS {
            let before = self.read_meta(relay_pubkey).await?;
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
