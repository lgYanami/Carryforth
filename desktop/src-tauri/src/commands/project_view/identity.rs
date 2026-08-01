//! NIP-11 Project View capability and Relay identity selection.

use buzz_core_pkg::PublicKey;
use serde::Deserialize;

use crate::app_state::AppState;
use crate::relay::{
    classify_request_error, parse_json_response, relay_api_base_url_with_override,
    relay_error_message,
};

use super::{
    integrity_error, ProjectViewIdentity, ProjectViewSchema, PROJECT_VIEW_V1_EXTENSION,
    PROJECT_VIEW_V2_EXTENSION, PROJECT_VIEW_V3_EXTENSION,
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
    let info: Nip11Document = parse_json_response(response).await?;
    let has_v3 = info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V3_EXTENSION);
    let has_v2 = info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V2_EXTENSION);
    let has_v1 = info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_V1_EXTENSION);
    let schema = if has_v3 {
        ProjectViewSchema::V3
    } else if has_v2 {
        ProjectViewSchema::V2
    } else if has_v1 {
        ProjectViewSchema::V1
    } else {
        return Ok(None);
    };

    let relay_self = info.relay_self.ok_or_else(|| {
        integrity_error("NIP-11 advertises Project View without a Relay `self` key")
    })?;
    let relay_pubkey = PublicKey::from_hex(&relay_self)
        .map_err(|error| integrity_error(format!("invalid NIP-11 Relay `self`: {error}")))?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(integrity_error(
            "NIP-11 Relay `self` is not canonical lowercase hex",
        ));
    }
    Ok(Some(ProjectViewIdentity {
        relay_pubkey,
        schema,
        project_document_supported: info
            .supported_extensions
            .iter()
            .any(|extension| extension == "buzz-project-document-v1"),
    }))
}
