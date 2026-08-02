use super::*;

/// Response from `POST /events`.
#[derive(Debug, Deserialize, serde::Serialize)]
pub struct SubmitEventResponse {
    pub event_id: String,
    pub accepted: bool,
    pub message: String,
}

/// Sign with an explicit identity and POST the event to an explicit relay.
///
/// The caller owns the signer lifetime. This is important for deferred work:
/// an in-process identity swap cannot retarget the event or its NIP-98 auth
/// after the caller has validated which identity the operation belongs to.
pub async fn submit_event_at_with_keys(
    builder: nostr::EventBuilder,
    state: &AppState,
    api_base_url: &str,
    keys: &nostr::Keys,
) -> Result<SubmitEventResponse, String> {
    let event = builder
        .sign_with_keys(keys)
        .map_err(|e| format!("failed to sign event: {e}"))?;
    submit_signed_event_at_with_keys(&event, state, api_base_url, keys).await
}

/// POST an already-signed event to an explicit Relay using the same identity
/// for NIP-98 authentication.
pub async fn submit_signed_event_at_with_keys(
    event: &nostr::Event,
    state: &AppState,
    api_base_url: &str,
    keys: &nostr::Keys,
) -> Result<SubmitEventResponse, String> {
    submit_signed_event_at_with_keys_typed(event, state, api_base_url, keys)
        .await
        .map_err(|error| error.message)
}

/// Submit an already-signed event while retaining whether a failure may have
/// happened after the Relay accepted the request.
pub(crate) async fn submit_signed_event_at_with_keys_typed(
    event: &nostr::Event,
    state: &AppState,
    api_base_url: &str,
    keys: &nostr::Keys,
) -> Result<SubmitEventResponse, RelayHttpError> {
    if event.pubkey != keys.public_key() {
        return Err(RelayHttpError::internal(
            "signed event does not match the publishing identity",
        ));
    }
    crate::relay_admission::wait_for_rate_limit().await;
    let url = format!("{}/events", api_base_url.trim_end_matches('/'));
    let body_bytes = event.as_json().into_bytes();
    let auth_header = build_nip98_auth_header_for_keys(keys, &Method::POST, &url, &body_bytes)
        .map_err(RelayHttpError::internal)?;

    let response = state
        .http_client
        .post(&url)
        .header("Authorization", auth_header)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .map_err(|error| typed_request_error(&error, true))?;

    if !response.status().is_success() {
        return Err(typed_response_error(response, true).await);
    }

    let result: SubmitEventResponse = parse_json_response_typed(response, true).await?;
    if !result.accepted {
        return Err(RelayHttpError {
            status: None,
            category: RelayHttpErrorCategory::Http,
            message: format!("relay rejected event: {}", result.message),
            retry_after_seconds: None,
            request_may_have_reached_relay: false,
        });
    }

    Ok(result)
}

/// Build and submit an event to the currently active workspace relay.
pub async fn submit_event(
    builder: nostr::EventBuilder,
    state: &AppState,
) -> Result<SubmitEventResponse, String> {
    let api_base_url = relay_api_base_url_with_override(state);
    let keys = state.signing_keys()?;
    submit_event_at_with_keys(builder, state, &api_base_url, &keys).await
}
