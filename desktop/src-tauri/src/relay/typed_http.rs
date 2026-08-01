use serde::de::DeserializeOwned;

use super::{
    classify_request_error, extract_retry_in_hint, parse_json_response, relay_error_message,
};

/// Stable failure classes retained by native commands that need to distinguish
/// definitive protocol failures from writes whose delivery is uncertain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelayHttpErrorCategory {
    Connect,
    Timeout,
    RateLimited,
    Forbidden,
    Conflict,
    Unavailable,
    Http,
    Malformed,
    Internal,
}

/// Sanitized HTTP failure metadata. Raw response bodies and endpoint URLs are
/// deliberately not retained in this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelayHttpError {
    pub(crate) status: Option<u16>,
    pub(crate) category: RelayHttpErrorCategory,
    pub(crate) message: String,
    pub(crate) retry_after_seconds: Option<u64>,
    /// True only when a write may have crossed the network boundary before the
    /// caller observed the failure. Read callers ignore this field.
    pub(crate) request_may_have_reached_relay: bool,
}

impl RelayHttpError {
    pub(super) fn internal(message: impl Into<String>) -> Self {
        Self {
            status: None,
            category: RelayHttpErrorCategory::Internal,
            message: message.into(),
            retry_after_seconds: None,
            request_may_have_reached_relay: false,
        }
    }
}

pub(crate) fn typed_request_error(error: &reqwest::Error, write: bool) -> RelayHttpError {
    let category = if error.is_timeout() {
        RelayHttpErrorCategory::Timeout
    } else {
        RelayHttpErrorCategory::Connect
    };
    RelayHttpError {
        status: None,
        category,
        message: classify_request_error(error),
        retry_after_seconds: None,
        request_may_have_reached_relay: write && !error.is_connect(),
    }
}

pub(crate) async fn typed_response_error(
    response: reqwest::Response,
    write: bool,
) -> RelayHttpError {
    let status_code = response.status().as_u16();
    let message = relay_error_message(response).await;
    let retry_after_seconds = extract_retry_in_hint(&message);
    let category = match status_code {
        403 => RelayHttpErrorCategory::Forbidden,
        409 => RelayHttpErrorCategory::Conflict,
        429 => RelayHttpErrorCategory::RateLimited,
        502..=504 => RelayHttpErrorCategory::Unavailable,
        _ => RelayHttpErrorCategory::Http,
    };
    RelayHttpError {
        status: Some(status_code),
        category,
        message,
        retry_after_seconds,
        // A canonical Relay 503 is a definitive pre-commit unavailable result
        // for Project commands. Only gateway 502/504 responses lose that
        // provenance and must be treated as potentially post-ingest.
        request_may_have_reached_relay: write && matches!(status_code, 502 | 504),
    }
}

pub(crate) async fn parse_json_response_typed<T: DeserializeOwned>(
    response: reqwest::Response,
    write: bool,
) -> Result<T, RelayHttpError> {
    parse_json_response(response)
        .await
        .map_err(|message| RelayHttpError {
            status: None,
            category: RelayHttpErrorCategory::Malformed,
            message,
            retry_after_seconds: None,
            request_may_have_reached_relay: write,
        })
}
