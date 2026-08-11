use crate::app_state::AppState;

pub(crate) const LOCAL_RELAY_WS_URL: &str = "ws://localhost:3000";
pub(crate) const LOCAL_ONLY_UNAVAILABLE: &str = "unavailable:desktop:local_only";

/// Validate a relay chosen by the Desktop workspace surface. Local-only
/// binaries accept exactly the canonical local relay spelling; aliases,
/// alternate ports, trailing slashes, and remote hosts are rejected.
pub(crate) fn validate_workspace_relay_url(relay_url: &str) -> Result<String, String> {
    if relay_url == LOCAL_RELAY_WS_URL {
        Ok(relay_url.to_owned())
    } else {
        Err(LOCAL_ONLY_UNAVAILABLE.to_owned())
    }
}

pub fn relay_ws_url() -> String {
    LOCAL_RELAY_WS_URL.to_owned()
}

#[cfg(test)]
fn relay_url_override_for_test(state: &AppState) -> Option<String> {
    state
        .relay_url_override
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
}

/// Return the only Relay coordinate supported by Carryforth Desktop.
pub fn relay_ws_url_with_override(state: &AppState) -> String {
    #[cfg(test)]
    if let Some(relay_url) = relay_url_override_for_test(state) {
        return relay_url;
    }

    #[cfg(not(test))]
    let _ = state;
    LOCAL_RELAY_WS_URL.to_owned()
}

/// Return the HTTP origin corresponding to the fixed local Relay.
pub fn relay_api_base_url_with_override(state: &AppState) -> String {
    #[cfg(test)]
    if let Some(relay_url) = relay_url_override_for_test(state) {
        return relay_http_base_url(&relay_url);
    }

    #[cfg(not(test))]
    let _ = state;
    relay_http_base_url(LOCAL_RELAY_WS_URL)
}

/// Selects the relay a managed Agent should use for a Relay operation.
///
/// Every managed Agent targets the active local workspace. The legacy stored
/// Relay coordinate remains untouched solely for local data compatibility.
pub fn effective_agent_relay_url(_record_relay: &str, workspace_relay: &str) -> String {
    workspace_relay.to_string()
}

pub fn relay_http_base_url(relay_url: &str) -> String {
    let trimmed = relay_url.trim().trim_end_matches('/');
    if let Some(suffix) = trimmed.strip_prefix("wss://") {
        return format!("https://{suffix}");
    }
    if let Some(suffix) = trimmed.strip_prefix("ws://") {
        return format!("http://{suffix}");
    }
    trimmed.to_string()
}

pub fn relay_api_base_url() -> String {
    relay_http_base_url(LOCAL_RELAY_WS_URL)
}
