//! Verified Project View v3 runtime reads plus an isolated legacy cutover reader.

use std::collections::HashSet;
#[cfg(test)]
use std::io::Read as _;
#[cfg(test)]
use std::path::Path;
use std::time::Duration;

use buzz_core::agent_process_env::MANAGED_RUNTIME_MODE_ENV;
use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::PublicKey;
#[cfg(test)]
use buzz_project_view::v2::RuntimeFence;
use buzz_sdk::project_view_v2::{
    parse_entity_projection, parse_membership_projection as parse_v2_membership_projection,
    parse_meta_projection, parse_project_object_projection, V2EntityProjection,
    V2MembershipProjection, V2MetaProjection,
};
pub(crate) use buzz_sdk::project_view_v3::PROJECT_VIEW_V3_EXTENSION;
use buzz_sdk::project_view_v3::{
    parse_entity_projection as parse_v3_entity_projection,
    parse_membership_projection as parse_v3_membership_projection,
    parse_meta_projection as parse_v3_meta_projection,
    parse_project_object_projection as parse_v3_project_object_projection, V3EntityProjection,
    V3MembershipProjection, V3MetaProjection, PROJECT_VIEW_V3_CURRENT_ENTITIES_SCOPE,
    PROJECT_VIEW_V3_ENTITY_TAG, PROJECT_VIEW_V3_META_TAG, PROJECT_VIEW_V3_OBJECT_TAG,
};
use buzz_sdk::role_brief::VerifiedRoleBriefSnapshot;
use buzz_sdk::role_brief_v3::VerifiedRoleBriefSnapshotV3;
use nostr::Event;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::BuzzClient;
use crate::error::CliError;

pub(crate) const PROJECT_CONTEXT_EXTENSION: &str = "buzz-project-context-v1";
pub(crate) const PROJECT_DOCUMENT_EXTENSION: &str = "buzz-project-document-v1";
const SNAPSHOT_ATTEMPTS: usize = 3;
const V2_OBJECT_PAGE_SIZE: usize = 500;
const ENTITY_PAGE_SIZE: usize = 500;
#[cfg(test)]
const RUNTIME_FENCE_FILE_MAX_BYTES: u64 = 4 * 1024;
#[cfg(test)]
const RUNTIME_FENCE_PATH_ENV: &str = "BUZZ_RUNTIME_FENCE_PATH";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectViewSchema {
    V2,
    V3,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProjectViewIdentity {
    pub(crate) relay_pubkey: PublicKey,
    pub(crate) schema: ProjectViewSchema,
    pub(crate) context_enabled: bool,
    pub(crate) document_enabled: bool,
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
    v3_identity_from_nip11(read_nip11(client).await?)
}

/// Resolve the Relay signer for the closed schema-v3 bootstrap confirmation.
///
/// A successful `init-v3` deliberately leaves ordinary Project View disabled
/// until the operator runs the checked enable step, so NIP-11 must not yet
/// advertise the runtime extension. The signed initialization receipt proves
/// that the Relay accepted the v3 command; this helper obtains only its
/// canonical signing identity for strict projection readback and never makes
/// an older runtime major available.
pub(crate) async fn read_v3_bootstrap_identity(
    client: &BuzzClient,
) -> Result<ProjectViewIdentity, CliError> {
    identity_from_nip11(read_nip11(client).await?, ProjectViewSchema::V3)
}

fn v3_identity_from_nip11(info: Nip11Document) -> Result<Option<ProjectViewIdentity>, CliError> {
    if !info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V3_EXTENSION)
    {
        return Ok(None);
    }
    identity_from_nip11(info, ProjectViewSchema::V3).map(Some)
}

/// Resolve the canonical Relay identity for an explicit operator-controlled
/// v2-to-v3 cutover. This deliberately does not depend on v2 being advertised:
/// a v3-only Relay may still expose the frozen legacy projections that the
/// reviewed approval manifest was built from.
pub(crate) async fn read_legacy_v2_identity(
    client: &BuzzClient,
) -> Result<ProjectViewIdentity, CliError> {
    identity_from_nip11(read_nip11(client).await?, ProjectViewSchema::V2)
}

async fn read_nip11(client: &BuzzClient) -> Result<Nip11Document, CliError> {
    let raw = client.get_public("/info").await?;
    serde_json::from_str(&raw)
        .map_err(|error| project_view_integrity_error(format!("invalid NIP-11 document: {error}")))
}

fn identity_from_nip11(
    info: Nip11Document,
    schema: ProjectViewSchema,
) -> Result<ProjectViewIdentity, CliError> {
    let relay_self = info.relay_self.ok_or_else(|| {
        project_view_integrity_error("NIP-11 document has no canonical Relay `self` key")
    })?;
    let relay_pubkey = PublicKey::from_hex(&relay_self).map_err(|error| {
        project_view_integrity_error(format!("invalid NIP-11 relay `self`: {error}"))
    })?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(project_view_integrity_error(
            "NIP-11 relay `self` is not canonical lowercase hex",
        ));
    }
    Ok(ProjectViewIdentity {
        relay_pubkey,
        schema,
        context_enabled: info
            .supported_extensions
            .iter()
            .any(|extension| extension == PROJECT_CONTEXT_EXTENSION),
        document_enabled: info
            .supported_extensions
            .iter()
            .any(|extension| extension == PROJECT_DOCUMENT_EXTENSION),
    })
}

/// Read and validate one bounded schema-v3 current snapshot.
pub(crate) async fn read_verified_v3_snapshot(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
) -> Result<VerifiedRoleBriefSnapshotV3, CliError> {
    if identity.schema != ProjectViewSchema::V3 {
        return Err(CliError::Other(
            "unsupported: verified Role state requires Project View v3".to_owned(),
        ));
    }
    for attempt in 0..SNAPSHOT_ATTEMPTS {
        let before = read_v3_meta(client, identity).await?;
        let ordinary_values = client
            .query_all(json!({
                "kinds": [KIND_PROJECT_VIEW_OBJECT],
                "authors": [identity.relay_pubkey.to_hex()],
                "#t": [PROJECT_VIEW_V3_OBJECT_TAG],
            }))
            .await?;
        let entity_projections =
            read_current_v3_entity_projections(client, identity, &before).await?;

        let mut event_ids =
            HashSet::with_capacity(ordinary_values.len() + entity_projections.len());
        let mut object_projections = Vec::with_capacity(ordinary_values.len());
        for value in ordinary_values {
            let event: Event = serde_json::from_value(value)
                .map_err(|error| v3_integrity_error(format!("invalid v3 object event: {error}")))?;
            if !event_ids.insert(event.id) {
                return Err(v3_integrity_error(
                    "v3 object query returned a duplicate event",
                ));
            }
            object_projections.push(
                parse_v3_project_object_projection(
                    &event,
                    &identity.relay_pubkey,
                    before.project_id,
                )
                .map_err(|error| v3_integrity_error(error.to_string()))?,
            );
        }
        for projection in &entity_projections {
            if !event_ids.insert(projection.event_id) {
                return Err(v3_integrity_error(
                    "v3 entity query returned a duplicate event",
                ));
            }
        }
        let membership = read_v3_membership(client, identity, &before).await?;
        let after = read_v3_meta(client, identity).await?;
        if before.event_id != after.event_id {
            if attempt + 1 < SNAPSHOT_ATTEMPTS {
                let backoff_ms = 25_u64 << attempt;
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                continue;
            }
            return Err(CliError::Conflict(
                "Project View v3 changed during every bounded snapshot attempt".to_owned(),
            ));
        }
        return VerifiedRoleBriefSnapshotV3::new_with_partial_history(
            before,
            membership,
            object_projections,
            entity_projections,
        )
        .map_err(|error| v3_integrity_error(error.to_string()));
    }
    Err(CliError::Conflict(
        "Project View v3 snapshot could not be stabilized".to_owned(),
    ))
}

async fn read_current_v3_entity_projections(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    meta: &V3MetaProjection,
) -> Result<Vec<V3EntityProjection>, CliError> {
    let mut projections = Vec::new();
    let mut event_ids = HashSet::new();
    let mut after: Option<Value> = None;
    loop {
        let mut extension = json!({
            "scope": PROJECT_VIEW_V3_CURRENT_ENTITIES_SCOPE,
            "revision": meta.project_revision,
            "projection_generation": meta.projection_generation,
        });
        if let Some(cursor) = &after {
            extension["after"] = cursor.clone();
        }
        let filter = json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": [PROJECT_VIEW_V3_ENTITY_TAG],
            "limit": ENTITY_PAGE_SIZE,
            "buzz_project_view": extension,
        });
        let values: Vec<Value> = serde_json::from_str(&client.query(&filter).await?)
            .map_err(|error| v3_integrity_error(format!("invalid v3 entity page: {error}")))?;
        if values.len() > ENTITY_PAGE_SIZE {
            return Err(v3_integrity_error(
                "v3 current-entity page exceeded the requested limit",
            ));
        }
        let page_len = values.len();
        for value in values {
            let event: Event = serde_json::from_value(value)
                .map_err(|error| v3_integrity_error(format!("invalid v3 entity event: {error}")))?;
            if !event_ids.insert(event.id) {
                return Err(v3_integrity_error(
                    "v3 current-entity pages contain a duplicate event",
                ));
            }
            let projection =
                parse_v3_entity_projection(&event, &identity.relay_pubkey, meta.project_id)
                    .map_err(|error| v3_integrity_error(error.to_string()))?;
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

pub(crate) async fn read_v3_meta(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
) -> Result<V3MetaProjection, CliError> {
    let filter = json!({
        "kinds": [KIND_PROJECT_VIEW_META],
        "authors": [identity.relay_pubkey.to_hex()],
        "#t": [PROJECT_VIEW_V3_META_TAG],
        "limit": 2,
    });
    let values: Vec<Value> = serde_json::from_str(&client.query(&filter).await?)
        .map_err(|error| v3_integrity_error(format!("invalid v3 metadata response: {error}")))?;
    let [value] = values.as_slice() else {
        return Err(v3_integrity_error(
            "v3 metadata query did not return exactly one current head",
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|error| v3_integrity_error(format!("invalid v3 metadata event: {error}")))?;
    parse_v3_meta_projection(&event, &identity.relay_pubkey)
        .map_err(|error| v3_integrity_error(error.to_string()))
}

async fn read_v3_membership(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    meta: &V3MetaProjection,
) -> Result<V3MembershipProjection, CliError> {
    let filter = json!({
        "ids": [meta.membership_snapshot_event_id.to_hex()],
        "kinds": [KIND_NIP43_MEMBERSHIP_LIST],
        "authors": [identity.relay_pubkey.to_hex()],
        "limit": 2,
    });
    let values: Vec<Value> = serde_json::from_str(&client.query(&filter).await?)
        .map_err(|error| v3_integrity_error(format!("invalid membership response: {error}")))?;
    let [value] = values.as_slice() else {
        return Err(v3_integrity_error(
            "v3 metadata membership pointer did not resolve exactly once",
        ));
    };
    let event: Event = serde_json::from_value(value.clone())
        .map_err(|error| v3_integrity_error(format!("invalid membership event: {error}")))?;
    if event.id != meta.membership_snapshot_event_id {
        return Err(v3_integrity_error(
            "membership query returned an event other than the v3 metadata pointer",
        ));
    }
    parse_v3_membership_projection(&event, &identity.relay_pubkey)
        .map_err(|error| v3_integrity_error(error.to_string()))
}

pub(crate) async fn read_legacy_v2_migration_snapshot(
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
        let object_projections =
            read_legacy_v2_migration_objects(client, identity, &before).await?;
        let entity_projections =
            read_legacy_v2_migration_current_entities(client, identity, &before).await?;

        let mut event_ids =
            HashSet::with_capacity(object_projections.len() + entity_projections.len());
        for projection in &object_projections {
            if !event_ids.insert(projection.event_id) {
                return Err(integrity_error(
                    "v2 object query returned a duplicate event",
                ));
            }
        }
        for projection in &entity_projections {
            if !event_ids.insert(projection.event_id) {
                return Err(integrity_error(
                    "v2 entity query returned a duplicate event",
                ));
            }
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
        return VerifiedRoleBriefSnapshot::new_with_partial_history(
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

async fn read_legacy_v2_migration_objects(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    meta: &V2MetaProjection,
) -> Result<Vec<buzz_sdk::project_view_v2::V2ProjectObjectProjection>, CliError> {
    let mut projections = Vec::new();
    let mut event_ids = HashSet::new();
    let mut after: Option<(String, uuid::Uuid)> = None;
    loop {
        let mut extension = json!({
            "scope": "v2_migration_objects",
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
            "#t": ["buzz-project-view-v2-object"],
            "limit": V2_OBJECT_PAGE_SIZE,
            "buzz_project_view": extension,
        });
        let values: Vec<Value> =
            serde_json::from_str(&client.query(&filter).await?).map_err(|error| {
                integrity_error(format!("invalid v2 migration object page: {error}"))
            })?;
        if values.len() > V2_OBJECT_PAGE_SIZE {
            return Err(integrity_error(
                "v2 migration object page exceeded the requested limit",
            ));
        }
        let page_len = values.len();
        for value in values {
            let event: Event = serde_json::from_value(value).map_err(|error| {
                integrity_error(format!("invalid v2 migration object event: {error}"))
            })?;
            if !event_ids.insert(event.id) {
                return Err(integrity_error(
                    "v2 migration object pages contain a duplicate event",
                ));
            }
            let projection =
                parse_project_object_projection(&event, &identity.relay_pubkey, meta.project_id)
                    .map_err(|error| integrity_error(error.to_string()))?;
            if projection.projection_generation != meta.projection_generation
                || projection.project_revision > meta.project_revision
            {
                return Err(integrity_error(
                    "v2 migration object projection is outside the pinned snapshot",
                ));
            }
            let key = (
                projection.object.object_type().as_str().to_owned(),
                projection.object.id(),
            );
            if after.as_ref().is_some_and(|cursor| key <= *cursor) {
                return Err(integrity_error(
                    "v2 migration object pages are not in strict keyset order",
                ));
            }
            after = Some(key);
            projections.push(projection);
        }
        if page_len < V2_OBJECT_PAGE_SIZE {
            break;
        }
    }
    Ok(projections)
}

async fn read_legacy_v2_migration_current_entities(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
    meta: &V2MetaProjection,
) -> Result<Vec<V2EntityProjection>, CliError> {
    let mut projections = Vec::new();
    let mut event_ids = HashSet::new();
    let mut after: Option<Value> = None;
    loop {
        let mut extension = json!({
            "scope": "v2_migration_current_entities",
            "revision": meta.project_revision,
            "projection_generation": meta.projection_generation,
        });
        if let Some(cursor) = &after {
            extension["after"] = cursor.clone();
        }
        let filter = json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": ["buzz-project-view-v2-entity"],
            "limit": ENTITY_PAGE_SIZE,
            "buzz_project_view": extension,
        });
        let values: Vec<Value> = serde_json::from_str(&client.query(&filter).await?)
            .map_err(|error| integrity_error(format!("invalid v2 entity page: {error}")))?;
        if values.len() > ENTITY_PAGE_SIZE {
            return Err(integrity_error(
                "v2 current-entity page exceeded the requested limit",
            ));
        }
        let page_len = values.len();
        for value in values {
            let event: Event = serde_json::from_value(value)
                .map_err(|error| integrity_error(format!("invalid v2 entity event: {error}")))?;
            if !event_ids.insert(event.id) {
                return Err(integrity_error(
                    "v2 current-entity pages contain a duplicate event",
                ));
            }
            let projection =
                parse_entity_projection(&event, &identity.relay_pubkey, meta.project_id)
                    .map_err(|error| integrity_error(error.to_string()))?;
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

pub(crate) fn is_managed_runtime() -> bool {
    is_managed_runtime_value(std::env::var(MANAGED_RUNTIME_MODE_ENV).ok().as_deref())
}

fn is_managed_runtime_value(value: Option<&str>) -> bool {
    value == Some("1")
}

#[cfg(test)]
fn runtime_fence_from_file(path: &Path) -> Result<Option<RuntimeFence>, CliError> {
    if !path.is_absolute() {
        return Err(CliError::Auth(format!(
            "{RUNTIME_FENCE_PATH_ENV} must be an absolute path"
        )));
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CliError::Auth(format!(
                "read managed Runtime fence {}: {error}",
                path.display()
            )));
        }
    };
    let mut bytes = Vec::new();
    file.take(RUNTIME_FENCE_FILE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CliError::Auth(format!(
                "read managed Runtime fence {}: {error}",
                path.display()
            ))
        })?;
    if bytes.len() as u64 > RUNTIME_FENCE_FILE_MAX_BYTES {
        return Err(CliError::Auth(format!(
            "managed Runtime fence {} exceeds {RUNTIME_FENCE_FILE_MAX_BYTES} bytes",
            path.display()
        )));
    }
    let runtime_fence: RuntimeFence = serde_json::from_slice(&bytes).map_err(|error| {
        CliError::Auth(format!(
            "invalid managed Runtime fence {}: {error}",
            path.display()
        ))
    })?;
    runtime_fence
        .validate()
        .map_err(|error| CliError::Auth(format!("invalid managed runtime fence: {error}")))?;
    Ok(Some(runtime_fence))
}

#[cfg(test)]
fn runtime_fence_from_legacy_env(
    runtime_id: Option<&str>,
    runtime_epoch: Option<&str>,
) -> Result<Option<RuntimeFence>, CliError> {
    match (runtime_id, runtime_epoch) {
        (None, None) => Ok(None),
        (Some(runtime_id), Some(runtime_epoch)) => {
            let runtime_fence = RuntimeFence {
                runtime_id: runtime_id
                    .parse()
                    .map_err(|error| CliError::Auth(format!("invalid BUZZ_RUNTIME_ID: {error}")))?,
                runtime_epoch: runtime_epoch.parse().map_err(|error| {
                    CliError::Auth(format!("invalid BUZZ_RUNTIME_EPOCH: {error}"))
                })?,
            };
            runtime_fence.validate().map_err(|error| {
                CliError::Auth(format!("invalid managed runtime fence: {error}"))
            })?;
            Ok(Some(runtime_fence))
        }
        _ => Err(CliError::Auth(
            "BUZZ_RUNTIME_ID and BUZZ_RUNTIME_EPOCH must be supplied together".to_owned(),
        )),
    }
}

async fn read_meta(
    client: &BuzzClient,
    identity: ProjectViewIdentity,
) -> Result<V2MetaProjection, CliError> {
    let filter = json!({
        "kinds": [KIND_PROJECT_VIEW_META],
        "authors": [identity.relay_pubkey.to_hex()],
        "#t": [PROJECT_VIEW_V3_META_TAG],
        "limit": 2,
        "buzz_project_view": {"scope": "v2_migration_meta"},
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
    parse_v2_membership_projection(&event, &identity.relay_pubkey)
        .map_err(|error| integrity_error(error.to_string()))
}

pub(crate) fn integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!(
        "Project View v2 integrity error: {}",
        message.into()
    ))
}

fn project_view_integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!("Project View integrity error: {}", message.into()))
}

pub(crate) fn v3_integrity_error(message: impl Into<String>) -> CliError {
    CliError::Other(format!(
        "Project View v3 integrity error: {}",
        message.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        identity_from_nip11, is_managed_runtime_value, runtime_fence_from_file,
        runtime_fence_from_legacy_env, v3_identity_from_nip11, Nip11Document, ProjectViewSchema,
        PROJECT_VIEW_V3_EXTENSION,
    };
    use buzz_project_view::v2::RuntimeFence;
    use nostr::Keys;
    use uuid::Uuid;

    #[test]
    fn ordinary_identity_rejects_v2_only_but_closed_transition_identities_need_no_advertisement() {
        let relay_pubkey = Keys::generate().public_key().to_hex();
        let ordinary = v3_identity_from_nip11(Nip11Document {
            supported_extensions: vec!["buzz-project-view-v2".to_owned()],
            relay_self: Some(relay_pubkey.clone()),
        })
        .expect("parse v2-only NIP-11 identity");
        assert!(ordinary.is_none());

        let migration = identity_from_nip11(
            Nip11Document {
                supported_extensions: vec![PROJECT_VIEW_V3_EXTENSION.to_owned()],
                relay_self: Some(relay_pubkey.clone()),
            },
            ProjectViewSchema::V2,
        )
        .expect("migration identity must not depend on a v2 advertisement");
        assert_eq!(migration.schema, ProjectViewSchema::V2);
        assert_eq!(migration.relay_pubkey.to_hex(), relay_pubkey);

        let bootstrap = identity_from_nip11(
            Nip11Document {
                supported_extensions: Vec::new(),
                relay_self: Some(relay_pubkey.clone()),
            },
            ProjectViewSchema::V3,
        )
        .expect("v3 bootstrap readback needs the Relay signer, not runtime readiness");
        assert_eq!(bootstrap.schema, ProjectViewSchema::V3);
        assert_eq!(bootstrap.relay_pubkey.to_hex(), relay_pubkey);
    }

    #[test]
    fn runtime_fence_file_is_authoritative_and_fail_closed() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let path = directory.path().join("runtime.fence.json");
        let expected = RuntimeFence {
            runtime_id: Uuid::new_v4(),
            runtime_epoch: 7,
        };
        std::fs::write(
            &path,
            serde_json::to_vec(&expected).expect("serialize Runtime fence"),
        )
        .expect("write Runtime fence");
        assert_eq!(
            runtime_fence_from_file(&path).expect("read Runtime fence"),
            Some(expected)
        );

        std::fs::write(&path, br#"{"runtime_id":"not-a-uuid","runtime_epoch":7}"#)
            .expect("write malformed Runtime fence");
        assert!(runtime_fence_from_file(&path).is_err());
        std::fs::remove_file(&path).expect("remove Runtime fence");
        assert_eq!(
            runtime_fence_from_file(&path).expect("missing fence is unsupervised"),
            None
        );
    }

    #[test]
    fn legacy_runtime_fence_requires_one_valid_pair() {
        let runtime_id = Uuid::new_v4();
        let runtime_id_text = runtime_id.to_string();
        assert_eq!(
            runtime_fence_from_legacy_env(Some(&runtime_id_text), Some("3"))
                .expect("parse legacy Runtime fence"),
            Some(RuntimeFence {
                runtime_id,
                runtime_epoch: 3,
            })
        );
        assert!(runtime_fence_from_legacy_env(Some(&runtime_id_text), None).is_err());
        assert!(runtime_fence_from_legacy_env(Some("not-a-uuid"), Some("3")).is_err());
    }

    #[test]
    fn managed_runtime_requires_exact_new_mode_marker() {
        assert!(is_managed_runtime_value(Some("1")));
        assert!(!is_managed_runtime_value(None));
        assert!(!is_managed_runtime_value(Some("")));
        assert!(!is_managed_runtime_value(Some("0")));
        assert!(!is_managed_runtime_value(Some("true")));
        assert!(!is_managed_runtime_value(Some("xyz.block.buzz.app.dev")));
    }
}
