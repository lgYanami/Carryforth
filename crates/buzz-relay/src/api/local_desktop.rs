//! Loopback-only bootstrap for the Carryforth Desktop identity.
//!
//! The first authenticated Desktop identity may become the initial owner of an
//! otherwise greenfield local Community. This is not an owner rotation API:
//! [`buzz_db::Db::bootstrap_owner`] atomically closes the path as soon as an
//! owner, membership, preparation, or canonical Project View state exists.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::Json,
};
use serde_json::Value;
use tracing::{info, warn};

use buzz_db::relay_members::{LocalOwnerBootstrapOutcome, Nip43MembershipSnapshotReconciliation};

use crate::state::AppState;

use super::{api_error, bridge, internal_error};

/// Canonical Relay coordinate accepted by the local Desktop bootstrap.
pub const LOCAL_DESKTOP_RELAY_URL: &str = "ws://localhost:3000";
const LOCAL_DESKTOP_AUTHORITY: &str = "localhost:3000";
const LOCAL_OWNER_PATH: &str = "/api/local/owner";

/// Whether a Relay deployment is the exact Carryforth local coordinate.
pub fn is_local_desktop_relay_url(relay_url: &str) -> bool {
    relay_url == LOCAL_DESKTOP_RELAY_URL
}

fn local_owner_claim_surface_allowed(
    relay_url: &str,
    request_host: &str,
    peer: SocketAddr,
) -> bool {
    is_local_desktop_relay_url(relay_url)
        && request_host.eq_ignore_ascii_case(LOCAL_DESKTOP_AUTHORITY)
        && peer.ip().is_loopback()
}

/// Claim the first owner of the exact local Community.
///
/// Authentication is always NIP-98; the development `X-Pubkey` shortcut is
/// deliberately disabled. A different existing owner produces an idempotent
/// `already_initialized` response and is never replaced.
pub async fn claim_initial_owner(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let raw_host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !local_owner_claim_surface_allowed(&state.config.relay_url, raw_host, peer) {
        return Err(api_error(StatusCode::NOT_FOUND, "not found"));
    }

    let tenant = crate::tenant::bind_community(&state.db, raw_host)
        .await
        .map_err(|_| api_error(StatusCode::NOT_FOUND, "not found"))?;
    if tenant.host() != LOCAL_DESKTOP_AUTHORITY {
        return Err(api_error(StatusCode::NOT_FOUND, "not found"));
    }

    let expected_url =
        bridge::nip98_expected_url(&state.config.relay_url, &tenant, LOCAL_OWNER_PATH);
    let (pubkey, event_id) = bridge::verify_bridge_auth_with_options(
        &headers,
        "POST",
        &expected_url,
        Some(&body),
        true,
        true,
    )?;
    bridge::check_nip98_replay(&state, &tenant, event_id).await?;

    let pubkey_hex = pubkey.to_hex();
    let bootstrap_outcome = state
        .db
        .claim_local_owner(tenant.community(), &pubkey_hex)
        .await
        .map_err(|error| internal_error(&format!("claim local owner: {error}")))?;
    let members = state
        .db
        .list_relay_members(tenant.community())
        .await
        .map_err(|error| internal_error(&format!("read local owner state: {error}")))?;
    let caller_role = members
        .iter()
        .find(|member| member.pubkey == pubkey_hex)
        .map(|member| member.role.as_str());
    let owner_exists = members.iter().any(|member| member.role == "owner");

    if matches!(
        bootstrap_outcome,
        LocalOwnerBootstrapOutcome::Created | LocalOwnerBootstrapOutcome::AlreadyOwner
    ) && caller_role == Some("owner")
    {
        let snapshot_status = state
            .db
            .nip43_membership_snapshot_reconciliation_status(
                tenant.community(),
                &state.relay_keypair.public_key(),
            )
            .await
            .map_err(|error| {
                warn!(
                    community = %tenant.community(),
                    pubkey = %pubkey_hex,
                    outcome = ?bootstrap_outcome,
                    snapshot_action = "failed",
                    error = %error,
                    "local Desktop membership snapshot inspection failed"
                );
                internal_error(&format!("inspect local membership snapshot: {error}"))
            })?;
        let snapshot_action = match snapshot_status {
            Nip43MembershipSnapshotReconciliation::Needed => {
                crate::handlers::side_effects::publish_nip43_membership_list(&tenant, &state)
                    .await
                    .map_err(|error| {
                        warn!(
                            community = %tenant.community(),
                            pubkey = %pubkey_hex,
                            outcome = ?bootstrap_outcome,
                            snapshot_action = "failed",
                            error = %error,
                            "local Desktop membership snapshot reconciliation failed"
                        );
                        internal_error(&format!("reconcile local membership snapshot: {error}"))
                    })?;
                if matches!(
                    state
                        .db
                        .nip43_membership_snapshot_reconciliation_status(
                            tenant.community(),
                            &state.relay_keypair.public_key(),
                        )
                        .await
                        .map_err(|error| {
                            warn!(
                                community = %tenant.community(),
                                pubkey = %pubkey_hex,
                                outcome = ?bootstrap_outcome,
                                snapshot_action = "failed",
                                error = %error,
                                "local Desktop membership snapshot verification failed"
                            );
                            internal_error(&format!("verify local membership snapshot: {error}"))
                        })?,
                    Nip43MembershipSnapshotReconciliation::Needed
                ) {
                    warn!(
                        community = %tenant.community(),
                        pubkey = %pubkey_hex,
                        outcome = ?bootstrap_outcome,
                        snapshot_action = "failed",
                        "local Desktop membership snapshot remained inconsistent"
                    );
                    return Err(internal_error(
                        "local membership snapshot remained inconsistent after reconciliation",
                    ));
                }
                "published"
            }
            Nip43MembershipSnapshotReconciliation::Current => "already_current",
            Nip43MembershipSnapshotReconciliation::GovernedV3 => "governed_noop",
        };
        info!(
            community = %tenant.community(),
            pubkey = %pubkey_hex,
            outcome = ?bootstrap_outcome,
            snapshot_action,
            "local Desktop owner bootstrap ready"
        );
        return Ok(Json(serde_json::json!({
            "status": "ready",
            "role": "owner",
            "pubkey": pubkey_hex,
        })));
    }

    if owner_exists {
        return Ok(Json(serde_json::json!({
            "status": "already_initialized",
            "role": caller_role,
            "pubkey": pubkey_hex,
        })));
    }

    Err(internal_error(
        "local owner claim returned a writable outcome without an owner row",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_relay_coordinate_is_exact_and_has_no_enable_switch() {
        assert!(is_local_desktop_relay_url("ws://localhost:3000"));
        for rejected in [
            " ws://localhost:3000 ",
            "ws://localhost:3000/",
            "ws://127.0.0.1:3000",
            "ws://localhost:3001",
            "wss://localhost:3000",
            "wss://relay.example",
        ] {
            assert!(!is_local_desktop_relay_url(rejected), "{rejected}");
        }
    }

    #[test]
    fn owner_claim_requires_exact_host_and_loopback_peer() {
        assert!(local_owner_claim_surface_allowed(
            LOCAL_DESKTOP_RELAY_URL,
            LOCAL_DESKTOP_AUTHORITY,
            "127.0.0.1:41000".parse().unwrap(),
        ));
        assert!(local_owner_claim_surface_allowed(
            LOCAL_DESKTOP_RELAY_URL,
            "LOCALHOST:3000",
            "[::1]:41000".parse().unwrap(),
        ));
        assert!(!local_owner_claim_surface_allowed(
            LOCAL_DESKTOP_RELAY_URL,
            LOCAL_DESKTOP_AUTHORITY,
            "192.0.2.10:41000".parse().unwrap(),
        ));
        assert!(!local_owner_claim_surface_allowed(
            LOCAL_DESKTOP_RELAY_URL,
            "127.0.0.1:3000",
            "127.0.0.1:41000".parse().unwrap(),
        ));
    }
}
