//! Agent metadata and runtime-capability profile reconciliation — split from
//! `agents.rs` (file-size guard). Owns the reconcile data carrier, the
//! legacy-avatar backfill, and the needs-sync predicate.

use tauri::{AppHandle, Emitter};

use crate::app_state::AppState;
use crate::managed_agents::{managed_agent_avatar_url, AgentDefinition};

use super::*;

pub(crate) struct ProfileReconcileData {
    pub(crate) private_key_nsec: String,
    pub(crate) name: String,
    pub(crate) relay_url: String,
    /// Workspace Relay authority captured before this deferred reconcile was
    /// scheduled. A later Community switch must never retarget the task.
    pub(crate) workspace_relay_url: String,
    /// Exact ACP command configured for this Agent. Local capability
    /// reconciliation probes this command, never a guessed runtime binary.
    pub(crate) acp_command: String,
    /// Provider runtimes must self-advertise from the remote harness. Desktop
    /// only probes and signs on behalf of locally managed runtimes.
    pub(crate) backend: BackendKind,
    /// Expected avatar URL for the published profile. `None` for legacy records
    /// that predate the `avatar_url` field — these will be backfilled from the
    /// relay's existing kind:0 profile on first reconciliation.
    pub(crate) avatar_url: Option<String>,
    pub(crate) auth_tag: Option<String>,
    /// The agent's pubkey (hex). Needed to update the persisted record during
    /// avatar backfill migration.
    pub(crate) pubkey: String,
    /// The agent's command (e.g. "goose"). Used as fallback when no profile
    /// exists on the relay during avatar backfill.
    pub(crate) agent_command: String,
    /// Persona ID if this agent was created from a persona. Used during avatar
    /// backfill to recover the correct avatar from the persona record when the
    /// relay profile has been corrupted.
    pub(crate) persona_id: Option<String>,
}

impl ProfileReconcileData {
    pub(crate) fn from_record(
        record: &ManagedAgentRecord,
        personas: &[AgentDefinition],
        workspace_relay_url: String,
    ) -> Self {
        Self {
            private_key_nsec: record.private_key_nsec.clone(),
            name: record.name.clone(),
            relay_url: record.relay_url.clone(),
            workspace_relay_url,
            acp_command: record.acp_command.clone(),
            backend: record.backend.clone(),
            avatar_url: record.avatar_url.clone(),
            auth_tag: record.auth_tag.clone(),
            pubkey: record.pubkey.clone(),
            agent_command: crate::managed_agents::record_agent_command(record, personas),
            persona_id: record.persona_id.clone(),
        }
    }
}

/// Resolve the avatar to backfill for a legacy agent record (pre-PR-921, no
/// stored `avatar_url`).
///
/// Priority: the persona's avatar wins, because the old reconciliation code
/// could have overwritten the relay's kind:0 `picture` with the command default
/// — making the relay an unreliable source for persona-backed agents. Only fall
/// back to the relay's `picture`, then the command icon, for agents with no
/// persona avatar to recover from.
pub(super) fn resolve_legacy_avatar(
    persona_avatar: Option<String>,
    relay_picture: Option<String>,
    agent_command: &str,
) -> String {
    persona_avatar
        .or(relay_picture)
        .or_else(|| managed_agent_avatar_url(agent_command))
        .unwrap_or_default()
}

/// Reconcile an Agent's kind:0 metadata and kind:10100 runtime controls.
///
/// Queries the relay for the Agent's existing metadata and re-publishes if
/// missing or stale. For a local Agent, it then probes the exact configured ACP
/// command and adds or withdraws the Meeting capability only after a positive
/// result. Provider runtimes self-advertise from their remote harness.
///
/// For legacy records (pre-PR-921) where `avatar_url` is `None`, this function
/// backfills via `resolve_legacy_avatar` — preferring the persona record's avatar
/// over the relay's `picture`, since the old code may have corrupted the relay
/// profile — and persists the updated record. After backfill, normal
/// reconciliation proceeds.
///
/// Query and publish target the relay returned by `effective_agent_relay_url`
/// for every agent regardless of backend: an explicit per-agent `relay_url`
/// wins, and a blank one falls back to the active workspace relay. This keeps
/// reconciliation following the session's relay for never-pinned agents while
/// honoring a deliberate pin wherever it points.
pub(crate) async fn reconcile_agent_profile(
    state: &AppState,
    app: &AppHandle,
    agent_pubkey: &str,
    data: &ProfileReconcileData,
) -> Result<(), String> {
    use crate::relay::{query_agent_profile, sync_managed_agent_profile};

    // An explicit per-agent relay wins; an empty one falls back to the active
    // workspace relay. Resolved once and used for both the read and write-back.
    let relay_url =
        crate::relay::effective_agent_relay_url(&data.relay_url, &data.workspace_relay_url);

    if !state
        .managed_agent_profile_reconcile_enabled
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Ok(());
    }

    // Query the relay for the agent's existing kind:0 profile.
    let existing = query_agent_profile(state, &relay_url, agent_pubkey).await?;

    // Resolve the expected avatar — backfilling for legacy records that have no
    // stored avatar_url yet.
    let expected_avatar = match data.avatar_url.as_deref() {
        Some(url) => url.to_string(),
        None => {
            // Legacy record: the relay profile may have been corrupted by the
            // old reconciliation code (it overwrote the persona avatar with the
            // command default), so the persona record is the authoritative source.
            let persona_avatar = data.persona_id.as_ref().and_then(|pid| {
                load_personas(app)
                    .ok()?
                    .into_iter()
                    .find(|p| p.id == *pid)?
                    .avatar_url
            });

            let backfilled = resolve_legacy_avatar(
                persona_avatar,
                existing.as_ref().and_then(|info| info.picture.clone()),
                &data.agent_command,
            );

            // Persist the backfilled avatar so this migration only runs once.
            if !backfilled.is_empty() {
                let _store_guard = state
                    .managed_agents_store_lock
                    .lock()
                    .map_err(|e| e.to_string())?;
                let mut records = load_managed_agents(app)?;
                if let Some(record) = records.iter_mut().find(|r| r.pubkey == data.pubkey) {
                    record.avatar_url = Some(backfilled.clone());
                    save_managed_agents(app, &records)?;
                }
            }

            backfilled
        }
    };

    let expected_avatar = if expected_avatar.is_empty() {
        None
    } else {
        Some(expected_avatar)
    };

    let agent_keys = Keys::parse(&data.private_key_nsec)
        .map_err(|e| format!("failed to parse agent keys: {e}"))?;

    if !state
        .managed_agent_profile_reconcile_enabled
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Ok(());
    }

    if profile_needs_sync(existing.as_ref(), &data.name, expected_avatar.as_deref()) {
        sync_managed_agent_profile(
            state,
            &relay_url,
            &agent_keys,
            &data.name,
            expected_avatar.as_deref(),
            data.auth_tag.as_deref(),
        )
        .await?;
    }

    if data.backend != BackendKind::Local {
        // The local Desktop cannot attest what a provider actually launched.
        // Its remote trusted buzz-acp process performs self-advertisement.
        return Ok(());
    }

    let acp_command = data.acp_command.clone();
    let probe = tokio::task::spawn_blocking(move || {
        crate::managed_agents::probe_meeting_capability(&acp_command)
    })
    .await
    .map_err(|error| format!("ACP capability probe task failed: {error}"))?;
    let supports_meeting_actions = match probe {
        crate::managed_agents::MeetingCapabilityProbe::Supported => true,
        crate::managed_agents::MeetingCapabilityProbe::Unsupported => false,
        crate::managed_agents::MeetingCapabilityProbe::Unknown(error) => {
            return Err(format!(
                "Meeting capability remains unchanged because the exact ACP harness could not be verified: {error}"
            ));
        }
    };

    let result = crate::relay::sync_managed_agent_capabilities(
        state,
        &relay_url,
        &agent_keys,
        Some(&data.name),
        data.auth_tag.as_deref(),
        supports_meeting_actions,
    )
    .await;
    if result.is_ok() {
        // Refresh the kind:10100-backed Agent directory immediately. The
        // frontend coalesces fleet bursts into one query invalidation.
        let _ = app.emit("agents-data-changed", ());
    }
    result
}

/// Decide whether a published profile is missing or stale relative to the
/// expected name and avatar. A missing profile always needs sync; a present
/// one is stale when either the display name or picture diverges.
pub(super) fn profile_needs_sync(
    existing: Option<&crate::relay::AgentProfileInfo>,
    expected_name: &str,
    expected_avatar: Option<&str>,
) -> bool {
    match existing {
        None => true,
        Some(info) => {
            let name_matches = info.display_name.as_deref() == Some(expected_name);
            let picture_matches = info.picture.as_deref() == expected_avatar;
            !name_matches || !picture_matches
        }
    }
}

// Async so the blocking body (disk reads/writes + process termination) runs off
// the main UI thread via spawn_blocking. State is re-derived from the owned
// AppHandle inside the closure (`State<'_, _>` is borrowed, MutexGuard is !Send).
