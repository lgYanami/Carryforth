//! Agent-authored runtime capability reconciliation.
//!
//! Meeting capability is durable Agent profile state, not presence. The
//! harness advertises it after authenticating to the exact Community so Relay
//! roster validation never has to infer support from an online process.

use std::collections::BTreeSet;

use nostr::{Event, Kind, Timestamp};
use serde_json::Value;

use crate::relay::{RelayError, RestClient};

const DEFAULT_CHANNEL_ADD_POLICY: &str = "anyone";

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentControlHead {
    event_id: String,
    created_at: u64,
    channel_add_policy: String,
    capabilities: Vec<String>,
    display_name: Option<String>,
}

fn parse_control_head(value: &Value) -> Option<AgentControlHead> {
    let events = value.as_array()?;
    events
        .iter()
        .filter_map(|value| serde_json::from_value::<Event>(value.clone()).ok())
        .filter(|event| event.kind.as_u16() as u32 == buzz_core::kind::KIND_AGENT_PROFILE)
        .filter_map(|event| {
            let content = serde_json::from_str::<Value>(&event.content).ok()?;
            let channel_add_policy = content
                .get("channel_add_policy")
                .and_then(Value::as_str)
                .filter(|policy| matches!(*policy, "anyone" | "owner_only" | "nobody"))?
                .to_string();
            let capabilities = content
                .get("capabilities")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let display_name = content
                .get("display_name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string);
            Some(AgentControlHead {
                event_id: event.id.to_hex(),
                created_at: event.created_at.as_secs(),
                channel_add_policy,
                capabilities,
                display_name,
            })
        })
        .max_by(|left, right| {
            (left.created_at, left.event_id.as_str())
                .cmp(&(right.created_at, right.event_id.as_str()))
        })
}

fn desired_capabilities(existing: &[String]) -> Vec<String> {
    let mut capabilities = existing
        .iter()
        .filter(|capability| {
            !matches!(
                capability.as_str(),
                buzz_sdk::MEETING_V2_ACTIONS_V2_CAPABILITY
                    | buzz_sdk::MEETING_V2_ACTIONS_V3_CAPABILITY
            )
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    capabilities.insert(buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY.to_string());
    capabilities.into_iter().collect()
}

async fn read_control_head(rest: &RestClient) -> Result<Option<AgentControlHead>, RelayError> {
    let filter = nostr::Filter::new()
        .author(rest.keys.public_key())
        .kind(Kind::Custom(buzz_core::kind::KIND_AGENT_PROFILE as u16))
        .limit(1);
    let value = rest.query(&[filter]).await?;
    Ok(parse_control_head(&value))
}

async fn read_metadata_display_name(rest: &RestClient) -> Result<Option<String>, RelayError> {
    let filter = nostr::Filter::new()
        .author(rest.keys.public_key())
        .kind(Kind::Custom(0))
        .limit(1);
    let value = rest.query(&[filter]).await?;
    Ok(value.as_array().and_then(|events| {
        events
            .iter()
            .filter_map(|value| serde_json::from_value::<Event>(value.clone()).ok())
            .max_by(|left, right| {
                (left.created_at.as_secs(), left.id.to_hex())
                    .cmp(&(right.created_at.as_secs(), right.id.to_hex()))
            })
            .and_then(|event| serde_json::from_str::<Value>(&event.content).ok())
            .and_then(|content| {
                content
                    .get("display_name")
                    .or_else(|| content.get("name"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
            })
    }))
}

/// Ensure this exact harness identity advertises Meeting V2 direct-action
/// support in its complete kind 10100 control profile.
///
/// Existing non-Meeting capabilities and channel-add policy are preserved.
/// The operation is idempotent and verifies the canonical replaceable event
/// after submission.
pub(crate) async fn reconcile_meeting_capability(rest: &RestClient) -> Result<bool, RelayError> {
    let current = read_control_head(rest).await?;
    let channel_add_policy = current
        .as_ref()
        .map(|head| head.channel_add_policy.as_str())
        .unwrap_or(DEFAULT_CHANNEL_ADD_POLICY);
    let capabilities = desired_capabilities(
        current
            .as_ref()
            .map(|head| head.capabilities.as_slice())
            .unwrap_or_default(),
    );
    let display_name = match current
        .as_ref()
        .and_then(|head| head.display_name.as_deref())
    {
        Some(display_name) => Some(display_name.to_string()),
        None => read_metadata_display_name(rest).await.unwrap_or(None),
    };

    if current.as_ref().is_some_and(|head| {
        head.capabilities == capabilities
            && head.channel_add_policy == channel_add_policy
            && (display_name.is_none() || head.display_name == display_name)
    }) {
        return Ok(false);
    }

    let capability_refs = capabilities.iter().map(String::as_str).collect::<Vec<_>>();
    let created_at = Timestamp::now().as_secs().max(
        current
            .as_ref()
            .map_or(0, |head| head.created_at.saturating_add(1)),
    );
    let event = buzz_sdk::build_agent_profile_controls(
        channel_add_policy,
        &capability_refs,
        display_name.as_deref(),
    )
    .map_err(|error| RelayError::Http(format!("build Agent capability profile: {error}")))?
    .custom_created_at(Timestamp::from(created_at))
    .sign_with_keys(&rest.keys)
    .map_err(|error| RelayError::Http(format!("sign Agent capability profile: {error}")))?;
    let event_id = event.id.to_hex();
    rest.submit_event(&event).await?;

    let canonical = read_control_head(rest).await?;
    if canonical.as_ref().map(|head| head.event_id.as_str()) != Some(event_id.as_str()) {
        return Err(RelayError::Http(
            "Agent capability profile canonical read-back did not match the submitted event"
                .to_string(),
        ));
    }
    Ok(true)
}

/// Reconcile in the background with bounded retry.
///
/// Capability publication is independent from ordinary chat availability: a
/// temporary HTTP bridge failure degrades Meeting compatibility but must not
/// terminate the harness. Startup and reconnect both invoke this helper.
pub(crate) fn spawn_meeting_capability_reconciliation(rest: RestClient, trigger: &'static str) {
    let task = tokio::spawn(async move {
        const RETRY_DELAYS: [std::time::Duration; 4] = [
            std::time::Duration::ZERO,
            std::time::Duration::from_millis(250),
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(4),
        ];
        for (index, delay) in RETRY_DELAYS.into_iter().enumerate() {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            match reconcile_meeting_capability(&rest).await {
                Ok(true) => {
                    tracing::info!(
                        capability = buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY,
                        trigger,
                        attempt = index + 1,
                        "published Agent Meeting capability"
                    );
                    return;
                }
                Ok(false) => {
                    tracing::debug!(
                        capability = buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY,
                        trigger,
                        attempt = index + 1,
                        "Agent Meeting capability already current"
                    );
                    return;
                }
                Err(error) if index + 1 < RETRY_DELAYS.len() => {
                    tracing::warn!(
                        capability = buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY,
                        trigger,
                        attempt = index + 1,
                        "Agent Meeting capability reconciliation failed; retrying: {error}"
                    );
                }
                Err(error) => {
                    tracing::error!(
                        capability = buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY,
                        trigger,
                        attempt = index + 1,
                        "Agent Meeting capability reconciliation remains degraded: {error}"
                    );
                }
            }
        }
    });
    drop(task);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys};

    fn event(content: Value, created_at: u64) -> Event {
        EventBuilder::new(
            Kind::Custom(buzz_core::kind::KIND_AGENT_PROFILE as u16),
            content.to_string(),
        )
        .custom_created_at(Timestamp::from(created_at))
        .sign_with_keys(&Keys::generate())
        .expect("sign profile fixture")
    }

    #[test]
    fn desired_capability_replaces_retired_generations_and_sorts() {
        assert_eq!(
            desired_capabilities(&[
                "z-capability".to_string(),
                buzz_sdk::MEETING_V2_ACTIONS_V2_CAPABILITY.to_string(),
                buzz_sdk::MEETING_V2_ACTIONS_V3_CAPABILITY.to_string(),
                buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY.to_string(),
            ]),
            vec![
                buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY.to_string(),
                "z-capability".to_string(),
            ]
        );
    }

    #[test]
    fn parse_control_head_treats_legacy_missing_capabilities_as_empty() {
        let event = event(serde_json::json!({"channel_add_policy": "owner_only"}), 10);
        let head = parse_control_head(&serde_json::json!([event])).expect("control head");
        assert_eq!(head.channel_add_policy, "owner_only");
        assert!(head.capabilities.is_empty());
    }

    #[test]
    fn parse_control_head_selects_latest_replaceable_event() {
        let old = event(
            serde_json::json!({
                "channel_add_policy": "anyone",
                "capabilities": []
            }),
            10,
        );
        let new = event(
            serde_json::json!({
                "channel_add_policy": "nobody",
                "capabilities": [buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY]
            }),
            11,
        );
        let head = parse_control_head(&serde_json::json!([old, new])).expect("control head");
        assert_eq!(head.created_at, 11);
        assert_eq!(head.channel_add_policy, "nobody");
    }
}
