//! Byte-exact NIP-98 request authorization helpers.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use nostr::{EventBuilder, EventId, JsonUtil, Keys, Kind, Tag};
use reqwest::Method;
use sha2::{Digest, Sha256};

use crate::app_state::AppState;

pub fn build_nip98_auth_header(
    method: &Method,
    url: &str,
    body: &[u8],
    state: &AppState,
) -> Result<String, String> {
    let keys = state.keys.lock().map_err(|error| error.to_string())?;
    build_nip98_auth_header_for_keys(&keys, method, url, body)
}

pub fn build_nip98_auth_header_for_keys(
    keys: &Keys,
    method: &Method,
    url: &str,
    body: &[u8],
) -> Result<String, String> {
    build_nip98_auth_observation_for_keys(keys, method, url, body)
        .map(|observation| observation.authorization_header)
}

/// Exact NIP-98 signing observation for one authenticated HTTP attempt.
pub(crate) struct Nip98AuthorizationObservation {
    pub authorization_header: String,
    pub auth_event_id: EventId,
}

impl std::fmt::Debug for Nip98AuthorizationObservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Nip98AuthorizationObservation")
            .field("authorization_header", &"<redacted>")
            .field("auth_event_id", &self.auth_event_id)
            .finish()
    }
}

/// Build one fresh NIP-98 Event and retain its identity for request-aware
/// response verification. The exact `body` bytes supplied here must be sent
/// unchanged in this same attempt.
pub(crate) fn build_nip98_auth_observation_for_keys(
    keys: &Keys,
    method: &Method,
    url: &str,
    body: &[u8],
) -> Result<Nip98AuthorizationObservation, String> {
    let payload_hash = hex::encode(Sha256::digest(body));

    // A nonce keeps identical same-second requests from sharing an Event id
    // and tripping Relay replay detection.
    let nonce_hex = uuid::Uuid::new_v4().to_string();
    let tags = vec![
        Tag::parse(vec!["u", url]).map_err(|error| format!("url tag failed: {error}"))?,
        Tag::parse(vec!["method", method.as_str()])
            .map_err(|error| format!("method tag failed: {error}"))?,
        Tag::parse(vec!["payload", &payload_hash])
            .map_err(|error| format!("payload tag failed: {error}"))?,
        Tag::parse(vec!["nonce", &nonce_hex])
            .map_err(|error| format!("nonce tag failed: {error}"))?,
    ];
    let event = EventBuilder::new(Kind::HttpAuth, "")
        .tags(tags)
        .sign_with_keys(keys)
        .map_err(|error| format!("sign failed: {error}"))?;
    Ok(Nip98AuthorizationObservation {
        authorization_header: format!("Nostr {}", BASE64.encode(event.as_json().as_bytes())),
        auth_event_id: event.id,
    })
}

#[cfg(test)]
mod tests {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use nostr::{Event, JsonUtil, Keys};
    use sha2::{Digest as _, Sha256};

    use super::build_nip98_auth_observation_for_keys;

    #[test]
    fn observed_nip98_retains_the_exact_signed_event_identity() {
        let keys = Keys::generate();
        let url = "http://localhost:3000/query";
        let body = br#"[{"problem":"sensitive"}]"#;
        let observation =
            build_nip98_auth_observation_for_keys(&keys, &reqwest::Method::POST, url, body)
                .expect("observed NIP-98");
        let encoded = observation
            .authorization_header
            .strip_prefix("Nostr ")
            .expect("Nostr scheme");
        let event =
            Event::from_json(BASE64.decode(encoded).expect("base64 Event")).expect("NIP-98 Event");
        event.verify().expect("NIP-98 signature");
        assert_eq!(event.id, observation.auth_event_id);
        let tags = event
            .tags
            .iter()
            .map(|tag| tag.as_slice())
            .collect::<Vec<_>>();
        assert!(tags
            .iter()
            .any(|tag| tag.len() == 2 && tag[0] == "u" && tag[1] == url));
        assert!(tags
            .iter()
            .any(|tag| tag.len() == 2 && tag[0] == "method" && tag[1] == "POST"));
        let expected_hash = hex::encode(Sha256::digest(body));
        assert!(tags.iter().any(|tag| {
            tag.len() == 2 && tag[0] == "payload" && tag[1] == expected_hash.as_str()
        }));

        let rendered = format!("{observation:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("sensitive"));
        assert!(!rendered.contains(&observation.authorization_header));
    }
}
