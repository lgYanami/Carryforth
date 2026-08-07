//! NIP-11 Project View capability and Relay identity selection.

use buzz_core_pkg::PublicKey;
use buzz_project_document_pkg::PROJECT_DOCUMENT_CAPABILITY;
use serde::Deserialize;

use crate::app_state::AppState;
use crate::relay::{
    classify_request_error, parse_json_response, relay_api_base_url_with_override,
    relay_error_message,
};

use super::{
    integrity_error, ProjectViewIdentity, ProjectViewSchema, PROJECT_CONTEXT_EXTENSION,
    PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION, PROJECT_VIEW_V3_EXTENSION,
};

#[derive(Debug, Deserialize)]
struct Nip11Document {
    #[serde(default)]
    supported_extensions: Vec<String>,
    #[serde(rename = "self")]
    relay_self: Option<String>,
}

pub(super) async fn read_identity(state: &AppState) -> Result<Option<ProjectViewIdentity>, String> {
    read_identity_at(state, &relay_api_base_url_with_override(state)).await
}

pub(crate) async fn read_identity_at(
    state: &AppState,
    api_base_url: &str,
) -> Result<Option<ProjectViewIdentity>, String> {
    let info = read_nip11_at(state, api_base_url).await?;
    let runtime_ready = info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V3_EXTENSION);
    let bootstrap_discoverable = info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION);
    if runtime_ready && bootstrap_discoverable {
        return Err(integrity_error(
            "NIP-11 advertises both Project View v3 runtime and bootstrap discovery",
        ));
    }
    if !runtime_ready && !bootstrap_discoverable {
        return Ok(None);
    }

    let relay_pubkey = parse_relay_self(&info, "Project View")?;
    Ok(Some(ProjectViewIdentity {
        relay_pubkey,
        schema: ProjectViewSchema::V3,
        runtime_ready,
        project_context_supported: info
            .supported_extensions
            .iter()
            .any(|extension| extension == PROJECT_CONTEXT_EXTENSION),
    }))
}

/// Resolve the independent Project Document capability and canonical Relay
/// signer without requiring Project View runtime or bootstrap advertisement.
pub(crate) async fn read_project_document_identity_at(
    state: &AppState,
    api_base_url: &str,
) -> Result<Option<PublicKey>, String> {
    let info = read_nip11_at(state, api_base_url).await?;
    if !info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_DOCUMENT_CAPABILITY)
    {
        return Ok(None);
    }
    parse_relay_self(&info, "Project Document").map(Some)
}

async fn read_nip11_at(state: &AppState, api_base_url: &str) -> Result<Nip11Document, String> {
    crate::relay_admission::wait_for_rate_limit().await;
    let url = format!("{}/info", api_base_url.trim_end_matches('/'));
    let response = state
        .http_client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/nostr+json")
        .send()
        .await
        .map_err(|error| classify_request_error(&error))?;
    if !response.status().is_success() {
        return Err(relay_error_message(response).await);
    }
    parse_json_response(response).await
}

fn parse_relay_self(info: &Nip11Document, surface: &str) -> Result<PublicKey, String> {
    let relay_self = info.relay_self.as_deref().ok_or_else(|| {
        integrity_error(format!(
            "NIP-11 advertises {surface} without a Relay `self` key"
        ))
    })?;
    let relay_pubkey = PublicKey::from_hex(relay_self)
        .map_err(|error| integrity_error(format!("invalid NIP-11 Relay `self`: {error}")))?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(integrity_error(
            "NIP-11 Relay `self` is not canonical lowercase hex",
        ));
    }
    Ok(relay_pubkey)
}
