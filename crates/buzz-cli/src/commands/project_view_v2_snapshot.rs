//! Shared verified Project View v2 snapshot reader for CLI commands.

use std::collections::HashSet;
use std::time::Duration;

use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::PublicKey;
use buzz_sdk::project_view_v2::{
    parse_entity_projection, parse_membership_projection, parse_meta_projection,
    parse_project_object_projection, V2MembershipProjection, V2MetaProjection,
};
use buzz_sdk::role_brief::VerifiedRoleBriefSnapshot;
use nostr::Event;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::BuzzClient;
use crate::error::CliError;

pub(crate) const PROJECT_VIEW_V1_EXTENSION: &str = "buzz-project-view-v1";
pub(crate) const PROJECT_VIEW_V2_EXTENSION: &str = "buzz-project-view-v2";
const SNAPSHOT_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectViewSchema {
    V1,
    V2,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProjectViewIdentity {
    pub(crate) relay_pubkey: PublicKey,
    pub(crate) schema: ProjectViewSchema,
}

#[derive(Deserialize)]
struct Nip11Document {
    #[serde(default)]
    supported_extensions: Vec<String>,
    #[serde(rename = "self")]
    relay_self: Option<String>,
}

pub(crate) async fn read_identity(
    client: &BuzzClient,
) -> Result<Option<ProjectViewIdentity>, CliError> {
    let raw = client.get_public("/info").await?;
    let info: Nip11Document = serde_json::from_str(&raw)
        .map_err(|error| integrity_error(format!("invalid NIP-11 document: {error}")))?;
    let schema = if info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V2_EXTENSION)
    {
        ProjectViewSchema::V2
    } else if info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V1_EXTENSION)
    {
        ProjectViewSchema::V1
    } else {
        return Ok(None);
    };
    let relay_self = info.relay_self.ok_or_else(|| {
        integrity_error("NIP-11 advertises Project View without a relay `self` key")
    })?;
    let relay_pubkey = PublicKey::from_hex(&relay_self)
        .map_err(|error| integrity_error(format!("invalid NIP-11 relay `self`: {error}")))?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(integrity_error(
            "NIP-11 relay `self` is not canonical lowercase hex",
        ));
    }
    Ok(Some(ProjectViewIdentity {
        relay_pubkey,
        schema,
    }))
}

pub(crate) async fn require_v2_identity(
    client: &BuzzClient,
) -> Result<ProjectViewIdentity, CliError> {
    match read_identity(client).await? {
        Some(identity) if identity.schema == ProjectViewSchema::V2 => Ok(identity),
        _ => Err(CliError::Other(format!(
            "unsupported: relay does not advertise {PROJECT_VIEW_V2_EXTENSION}"
        ))),
    }
}

pub(crate) async fn read_verified_v2_snapshot(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
) -> Result<VerifiedRoleBriefSnapshot, CliError> {
    if identity.schema != ProjectViewSchema::V2 {
        return Err(CliError::Other(
            "unsupported: verified Role state requires Project View v2".to_owned(),
        ));
    }
    for attempt in 0..SNAPSHOT_ATTEMPTS {
        let before = read_meta(client, identity).await?;
        let ordinary_values = client
            .query_all(json!({
                "kinds": [KIND_PROJECT_VIEW_OBJECT],
                "authors": [identity.relay_pubkey.to_hex()],
                "#t": ["buzz-project-view-v2-object"],
            }))
            .await?;
        let entity_values = client
            .query_all(json!({
                "kinds": [KIND_PROJECT_VIEW_OBJECT],
                "authors": [identity.relay_pubkey.to_hex()],
                "#t": ["buzz-project-view-v2-entity"],
            }))
            .await?;

        let mut event_ids = HashSet::with_capacity(ordinary_values.len() + entity_values.len());
        let mut object_projections = Vec::with_capacity(ordinary_values.len());
        for value in ordinary_values {
            let event: Event = serde_json::from_value(value)
                .map_err(|error| integrity_error(format!("invalid v2 object event: {error}")))?;
            if !event_ids.insert(event.id) {
                return Err(integrity_error(
                    "v2 object query returned a duplicate event",
                ));
            }
            object_projections.push(
                parse_project_object_projection(&event, &identity.relay_pubkey, before.project_id)
                    .map_err(|error| integrity_error(error.to_string()))?,
            );
        }
        let mut entity_projections = Vec::with_capacity(entity_values.len());
        for value in entity_values {
            let event: Event = serde_json::from_value(value)
                .map_err(|error| integrity_error(format!("invalid v2 entity event: {error}")))?;
            if !event_ids.insert(event.id) {
                return Err(integrity_error(
                    "v2 entity query returned a duplicate event",
                ));
            }
            entity_projections.push(
                parse_entity_projection(&event, &identity.relay_pubkey, before.project_id)
                    .map_err(|error| integrity_error(error.to_string()))?,
            );
        }
        let membership = read_membership(client, identity, &before).await?;
        let after = read_meta(client, identity).await?;
        if before.event_id != after.event_id {
            if attempt + 1 < SNAPSHOT_ATTEMPTS {
                let backoff_ms = 25_u64 << attempt;
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                continue;
            }
            return Err(CliError::Conflict(
                "Project View v2 changed during every bounded snapshot attempt".to_owned(),
            ));
        }
        return VerifiedRoleBriefSnapshot::new(
            before,
            membership,
            object_projections,
            entity_projections,
        )
        .map_err(|error| integrity_error(error.to_string()));
    }
    Err(CliError::Conflict(
        "Project View v2 snapshot could not be stabilized".to_owned(),
    ))
}

pub(crate) async fn read_current_v2_snapshot(
    client: &BuzzClient,
) -> Result<VerifiedRoleBriefSnapshot, CliError> {
    let identity = require_v2_identity(client).await?;
    read_verified_v2_snapshot(client, identity).await
}

pub(crate) fn is_managed_runtime() -> bool {
    std::env::var("BUZZ_MANAGED_AGENT").as_deref() == Ok("1")
}

async fn read_meta(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
) -> Result<V2MetaProjection, CliError> {
    let filter = json!({
        "kinds": [KIND_PROJECT_VIEW_META],
        "authors": [identity.relay_pubkey.to_hex()],
        "limit": 2,
    });
    let values: Vec<Value> = serde_json::from_str(&client.query(&filter).await?)
        .map_err(|error| integrity_error(format!("invalid v2 metadata response: {error}")))?;
    let [value] = values.as_slice() else {
        return Err(integrity_error(
            "v2 metadata query did not return exactly one current head",
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|error| integrity_error(format!("invalid v2 metadata event: {error}")))?;
    parse_meta_projection(&event, &identity.relay_pubkey)
        .map_err(|error| integrity_error(error.to_string()))
}

async fn read_membership(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    meta: &V2MetaProjection,
) -> Result<V2MembershipProjection, CliError> {
    let filter = json!({
        "ids": [meta.membership_snapshot_event_id.to_hex()],
        "kinds": [KIND_NIP43_MEMBERSHIP_LIST],
        "authors": [identity.relay_pubkey.to_hex()],
        "limit": 2,
    });
    let values: Vec<Value> = serde_json::from_str(&client.query(&filter).await?)
        .map_err(|error| integrity_error(format!("invalid membership response: {error}")))?;
    let [value] = values.as_slice() else {
        return Err(integrity_error(
            "metadata membership pointer did not resolve to exactly one snapshot",
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|error| integrity_error(format!("invalid membership event: {error}")))?;
    if event.id != meta.membership_snapshot_event_id {
        return Err(integrity_error(
            "membership query returned an event other than the metadata pointer",
        ));
    }
    parse_membership_projection(&event, &identity.relay_pubkey)
        .map_err(|error| integrity_error(error.to_string()))
}

pub(crate) fn integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!(
        "Project View v2 integrity error: {}",
        message.into()
    ))
}
