//! Managed Agent control-profile capability reconciliation.

use crate::app_state::AppState;

use super::{post_managed_agent_event, query_relay_at_with_keys, relay_http_base_url};

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentControlProfileHead {
    event_id: String,
    created_at: u64,
    channel_add_policy: String,
    capabilities: Vec<String>,
    display_name: Option<String>,
}

fn parse_agent_control_profile_head(
    events: &[nostr::Event],
) -> Result<Option<AgentControlProfileHead>, String> {
    let Some(event) = events.iter().max_by(|left, right| {
        (left.created_at.as_secs(), left.id.to_hex())
            .cmp(&(right.created_at.as_secs(), right.id.to_hex()))
    }) else {
        return Ok(None);
    };
    let content: serde_json::Value = serde_json::from_str(&event.content)
        .map_err(|error| format!("invalid Agent control profile JSON: {error}"))?;
    let channel_add_policy = content
        .get("channel_add_policy")
        .and_then(serde_json::Value::as_str)
        .filter(|policy| matches!(*policy, "anyone" | "owner_only" | "nobody"))
        .unwrap_or("anyone")
        .to_string();
    let mut capabilities = content
        .get("capabilities")
        .map(|value| {
            serde_json::from_value::<Vec<String>>(value.clone())
                .map_err(|error| format!("invalid Agent capability list: {error}"))
        })
        .transpose()?
        .unwrap_or_default();
    capabilities.sort();
    capabilities.dedup();
    let display_name = content
        .get("display_name")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    Ok(Some(AgentControlProfileHead {
        event_id: event.id.to_hex(),
        created_at: event.created_at.as_secs(),
        channel_add_policy,
        capabilities,
        display_name,
    }))
}

fn reconcile_meeting_capability_list(
    existing: &[String],
    supports_meeting_actions: bool,
) -> Vec<String> {
    let mut capabilities = existing
        .iter()
        .filter(|capability| {
            !matches!(
                capability.as_str(),
                buzz_sdk_pkg::MEETING_V2_ACTIONS_V2_CAPABILITY
                    | buzz_sdk_pkg::MEETING_V2_ACTIONS_V3_CAPABILITY
                    | buzz_sdk_pkg::MEETING_V2_ACTIONS_CAPABILITY
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    if supports_meeting_actions {
        capabilities.push(buzz_sdk_pkg::MEETING_V2_ACTIONS_CAPABILITY.to_string());
    }
    capabilities.sort();
    capabilities.dedup();
    capabilities
}

fn build_agent_control_profile_event(
    agent_keys: &nostr::Keys,
    channel_add_policy: &str,
    capabilities: &[String],
    display_name: Option<&str>,
    prior_created_at: Option<u64>,
    auth_tag_json: Option<&str>,
) -> Result<nostr::Event, String> {
    let capability_refs = capabilities.iter().map(String::as_str).collect::<Vec<_>>();
    let mut builder = buzz_sdk_pkg::build_agent_profile_controls(
        channel_add_policy,
        &capability_refs,
        display_name,
    )
    .map_err(|error| format!("failed to build Agent control profile: {error}"))?;
    let created_at = nostr::Timestamp::now()
        .as_secs()
        .max(prior_created_at.unwrap_or(0).saturating_add(1));
    builder = builder.custom_created_at(nostr::Timestamp::from(created_at));

    if let Some(tag_json) = auth_tag_json {
        let agent_pubkey_hex = agent_keys.public_key().to_hex();
        let compat_pubkey = nostr::PublicKey::from_hex(&agent_pubkey_hex)
            .map_err(|error| format!("failed to convert Agent pubkey: {error}"))?;
        buzz_sdk_pkg::nip_oa::verify_auth_tag(tag_json, &compat_pubkey)
            .map_err(|error| format!("auth tag verification failed for Agent controls: {error}"))?;
        let compat_tag = buzz_sdk_pkg::nip_oa::parse_auth_tag(tag_json)
            .map_err(|error| format!("failed to parse Agent controls auth tag: {error}"))?;
        let tag = nostr::Tag::parse(compat_tag.as_slice())
            .map_err(|error| format!("failed to convert Agent controls auth tag: {error}"))?;
        builder = builder.tags([tag]);
    }

    builder
        .sign_with_keys(agent_keys)
        .map_err(|error| format!("failed to sign Agent control profile: {error}"))
}

/// Reconcile one managed Agent's Meeting direct-action declaration in its
/// complete kind 10100 control profile.
///
/// A successful exact harness probe supplies `supports_meeting_actions`.
/// Unknown probe outcomes must not call this function: only an explicit
/// supported/unsupported result may add or withdraw the declaration.
pub async fn sync_managed_agent_capabilities(
    state: &AppState,
    relay_url: &str,
    agent_keys: &nostr::Keys,
    display_name: Option<&str>,
    auth_tag: Option<&str>,
    supports_meeting_actions: bool,
) -> Result<(), String> {
    let api_base_url = relay_http_base_url(relay_url);
    let filter = serde_json::json!({
        "authors": [agent_keys.public_key().to_hex()],
        "kinds": [buzz_core_pkg::kind::KIND_AGENT_PROFILE],
        "limit": 1
    });
    for attempt in 1..=3 {
        let current_events = query_relay_at_with_keys(
            state,
            &api_base_url,
            std::slice::from_ref(&filter),
            agent_keys,
            auth_tag,
        )
        .await?;
        let current = parse_agent_control_profile_head(&current_events)?;
        let capabilities = reconcile_meeting_capability_list(
            current
                .as_ref()
                .map(|head| head.capabilities.as_slice())
                .unwrap_or_default(),
            supports_meeting_actions,
        );
        let channel_add_policy = current
            .as_ref()
            .map(|head| head.channel_add_policy.as_str())
            .unwrap_or("anyone");

        if current.as_ref().is_some_and(|head| {
            head.channel_add_policy == channel_add_policy && head.capabilities == capabilities
        }) {
            return Ok(());
        }

        let event = build_agent_control_profile_event(
            agent_keys,
            channel_add_policy,
            &capabilities,
            display_name.or_else(|| {
                current
                    .as_ref()
                    .and_then(|head| head.display_name.as_deref())
            }),
            current.as_ref().map(|head| head.created_at),
            auth_tag,
        )?;
        post_managed_agent_event(
            state,
            relay_url,
            agent_keys,
            &event,
            auth_tag,
            "Could not sync the Agent's runtime capabilities",
        )
        .await?;

        let canonical_events = query_relay_at_with_keys(
            state,
            &api_base_url,
            std::slice::from_ref(&filter),
            agent_keys,
            auth_tag,
        )
        .await?;
        let canonical = parse_agent_control_profile_head(&canonical_events)?;
        let expected_event_id = event.id.to_hex();
        if canonical.as_ref().map(|head| head.event_id.as_str()) == Some(expected_event_id.as_str())
            || canonical.as_ref().is_some_and(|head| {
                head.channel_add_policy == channel_add_policy && head.capabilities == capabilities
            })
        {
            return Ok(());
        }
        if attempt == 3 {
            return Err(
                "Agent capability profile lost three concurrent canonical write races".to_string(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_agent_control_profile_event, parse_agent_control_profile_head,
        reconcile_meeting_capability_list,
    };

    #[test]
    fn agent_control_profile_preserves_policy_and_has_complete_capabilities() {
        let keys = nostr::Keys::generate();
        let event = build_agent_control_profile_event(
            &keys,
            "owner_only",
            &[
                "z-capability".to_string(),
                buzz_sdk_pkg::MEETING_V2_ACTIONS_CAPABILITY.to_string(),
            ],
            Some("TestBot"),
            Some(100),
            None,
        )
        .expect("build controls");
        let head = parse_agent_control_profile_head(&[event])
            .expect("parse controls")
            .expect("control head");
        assert_eq!(head.channel_add_policy, "owner_only");
        assert_eq!(head.display_name.as_deref(), Some("TestBot"));
        assert_eq!(
            head.capabilities,
            vec![
                buzz_sdk_pkg::MEETING_V2_ACTIONS_CAPABILITY.to_string(),
                "z-capability".to_string(),
            ]
        );
        assert!(head.created_at >= 101);
    }

    #[test]
    fn legacy_agent_control_profile_defaults_missing_capabilities() {
        let keys = nostr::Keys::generate();
        let event = nostr::EventBuilder::new(
            nostr::Kind::Custom(buzz_core_pkg::kind::KIND_AGENT_PROFILE as u16),
            serde_json::json!({"channel_add_policy": "nobody"}).to_string(),
        )
        .sign_with_keys(&keys)
        .expect("sign legacy controls");
        let head = parse_agent_control_profile_head(&[event])
            .expect("parse legacy controls")
            .expect("control head");
        assert_eq!(head.channel_add_policy, "nobody");
        assert!(head.capabilities.is_empty());
    }

    #[test]
    fn meeting_capability_reconciliation_preserves_unknowns_and_can_withdraw() {
        let existing = vec![
            "z-capability".to_string(),
            buzz_sdk_pkg::MEETING_V2_ACTIONS_V2_CAPABILITY.to_string(),
            buzz_sdk_pkg::MEETING_V2_ACTIONS_V3_CAPABILITY.to_string(),
            buzz_sdk_pkg::MEETING_V2_ACTIONS_CAPABILITY.to_string(),
            "a-capability".to_string(),
        ];
        assert_eq!(
            reconcile_meeting_capability_list(&existing, true),
            vec![
                "a-capability".to_string(),
                buzz_sdk_pkg::MEETING_V2_ACTIONS_CAPABILITY.to_string(),
                "z-capability".to_string(),
            ]
        );
        assert_eq!(
            reconcile_meeting_capability_list(&existing, false),
            vec!["a-capability".to_string(), "z-capability".to_string()]
        );
    }
}
