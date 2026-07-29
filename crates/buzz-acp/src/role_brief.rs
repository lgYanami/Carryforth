//! Per-turn verified Role Brief resolution for managed Agent runtimes.

use std::collections::HashSet;
use std::time::Duration;

use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_sdk::project_view_v2::{
    parse_entity_projection, parse_membership_projection, parse_meta_projection,
    parse_project_object_projection, V2MembershipProjection, V2MetaProjection,
};
use buzz_sdk::role_brief::{
    render_role_brief_markdown, unavailable_role_brief_markdown, RoleBriefMemberState,
    VerifiedRoleBriefSnapshot,
};
use chrono::Utc;
use nostr::{Alphabet, Event, Filter, Kind, PublicKey, SingleLetterTag};
use serde::Deserialize;

use crate::relay::RestClient;

const PROJECT_VIEW_V2_EXTENSION: &str = "buzz-project-view-v2";
const ROLE_BRIEF_TIMEOUT: Duration = Duration::from_secs(12);
const SNAPSHOT_ATTEMPTS: usize = 3;

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

/// Stateless resolver that re-reads and verifies the current v2 snapshot.
#[derive(Debug, Clone)]
pub struct RoleBriefResolver {
    rest_client: RestClient,
    member_pubkey: PublicKey,
}

impl RoleBriefResolver {
    /// Bind a resolver to the exact Relay client and managed Agent identity.
    #[must_use]
    pub const fn new(rest_client: RestClient, member_pubkey: PublicKey) -> Self {
        Self {
            rest_client,
            member_pubkey,
        }
    }

    /// Resolve with a hard outer bound and convert every failure into a
    /// fail-closed prompt section. No previous Brief is cached or reused.
    pub async fn resolve_bounded(&self) -> RoleContextResolution {
        match tokio::time::timeout(ROLE_BRIEF_TIMEOUT, self.resolve_verified()).await {
            Ok(Ok(snapshot)) => match snapshot.brief_for(self.member_pubkey, Utc::now()) {
                Ok(brief) => {
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
                Err(error) => RoleContextResolution::unavailable(
                    "project_view_unavailable",
                    &error.to_string(),
                ),
            },
            Ok(Err(error)) => {
                RoleContextResolution::unavailable("project_view_unavailable", &error)
            }
            Err(_) => RoleContextResolution::unavailable(
                "project_view_unavailable",
                "Role Brief resolution timed out",
            ),
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
            let entity_filter = Filter::new()
                .kind(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16))
                .author(relay_pubkey)
                .custom_tags(t_tag, ["buzz-project-view-v2-entity"]);
            let ordinary_events = parse_events(
                self.rest_client
                    .query(&[ordinary_filter])
                    .await
                    .map_err(|error| error.to_string())?,
            )?;
            let entity_events = parse_events(
                self.rest_client
                    .query(&[entity_filter])
                    .await
                    .map_err(|error| error.to_string())?,
            )?;

            let mut event_ids = HashSet::with_capacity(ordinary_events.len() + entity_events.len());
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
            let mut entity_projections = Vec::with_capacity(entity_events.len());
            for event in entity_events {
                if !event_ids.insert(event.id) {
                    return Err("entity query returned a duplicate event".to_owned());
                }
                entity_projections.push(
                    parse_entity_projection(&event, &relay_pubkey, before.project_id)
                        .map_err(|error| error.to_string())?,
                );
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
            return VerifiedRoleBriefSnapshot::new(
                before,
                membership,
                object_projections,
                entity_projections,
            )
            .map_err(|error| error.to_string());
        }
        Err("Project View snapshot could not be stabilized".to_owned())
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
