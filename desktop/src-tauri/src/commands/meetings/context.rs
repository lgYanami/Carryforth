//! Body-free Meeting reads used by Project Context and its Inspector.

use std::collections::{BTreeMap, BTreeSet};

use futures_util::{stream, StreamExt as _};
use nostr::{Keys, PublicKey};
use tauri::State;
use uuid::Uuid;

use super::{
    canonical_meeting_id, load_meeting_snapshot, load_meeting_snapshot_at, read_error_message,
    read_meeting_identity, read_meeting_identity_at, MeetingReadError,
};
use crate::app_state::AppState;

use super::model::{
    MeetingContextInspectorActionSummary, MeetingContextInspectorDetail,
    MeetingContextInspectorLoadResult, MeetingLifecycle, MeetingLoadResult, MeetingParticipantType,
};

/// Bounded participant identity carried by Project Context metadata-first reads.
#[derive(Debug, Clone)]
pub(crate) struct MeetingContextParticipant {
    pub(crate) pubkey: String,
    pub(crate) participant_type: &'static str,
}

/// Action-finalization facts safe to expose without embedding Board or Speech.
#[derive(Debug, Clone)]
pub(crate) struct MeetingContextActionSummary {
    pub(crate) condition: String,
    pub(crate) terminal_status: Option<String>,
    pub(crate) actions_attested: bool,
}

/// Verified, body-free Meeting metadata for a Context coordinate.
#[derive(Debug, Clone)]
pub(crate) struct MeetingContextRecord {
    pub(crate) title: String,
    pub(crate) discussion_goal: Option<String>,
    pub(crate) lifecycle: &'static str,
    pub(crate) terminal_outcome: Option<String>,
    pub(crate) host_pubkey: String,
    pub(crate) participant_count: usize,
    pub(crate) participant_preview: Vec<MeetingContextParticipant>,
    pub(crate) created_at: u64,
    pub(crate) ended_at: Option<u64>,
    pub(crate) action_finalization: Option<MeetingContextActionSummary>,
    pub(crate) state_revision: u64,
    pub(crate) create_event_id: String,
    pub(crate) state_event_id: String,
    pub(crate) end_event_id: Option<String>,
    pub(crate) updated_at: u64,
}

/// Per-Meeting result retained by Project Context so one failed hydration does
/// not erase otherwise verified Edge topology.
#[derive(Debug, Clone)]
pub(crate) enum MeetingContextRead {
    Observed(Box<MeetingContextRecord>),
    NotAttachable,
    NotFound,
    Unavailable,
    VerificationFailed,
}

const PROJECT_CONTEXT_MEETING_CONCURRENCY: usize = 4;

/// Hydrate a bounded set of Meeting coordinates against a Relay URL and signer
/// captured before any await. Board and Speech bodies never cross this boundary.
pub(crate) async fn read_meetings_for_project_context_at(
    state: &AppState,
    api_base_url: &str,
    keys: &Keys,
    expected_relay_pubkey: &PublicKey,
    meeting_ids: &BTreeSet<Uuid>,
) -> BTreeMap<Uuid, MeetingContextRead> {
    if meeting_ids.is_empty() {
        return BTreeMap::new();
    }
    let identity = match read_meeting_identity_at(state, api_base_url).await {
        Ok(Some(identity)) if identity.relay_pubkey == *expected_relay_pubkey => identity,
        Ok(Some(_)) | Ok(None) => {
            return meeting_ids
                .iter()
                .copied()
                .map(|meeting_id| (meeting_id, MeetingContextRead::VerificationFailed))
                .collect();
        }
        Err(message) => {
            let state = if transient_meeting_read_error(&message) {
                MeetingContextRead::Unavailable
            } else {
                MeetingContextRead::VerificationFailed
            };
            return meeting_ids
                .iter()
                .copied()
                .map(|meeting_id| (meeting_id, state.clone()))
                .collect();
        }
    };

    stream::iter(meeting_ids.iter().copied().map(|meeting_id| {
        let identity = &identity;
        async move {
            let loaded = load_meeting_snapshot_at(
                state,
                identity,
                &meeting_id.to_string(),
                api_base_url,
                keys,
            )
            .await;
            (meeting_id, context_read_from_load(loaded))
        }
    }))
    .buffer_unordered(PROJECT_CONTEXT_MEETING_CONCURRENCY)
    .collect()
    .await
}

fn context_read_from_load(
    loaded: Result<MeetingLoadResult, MeetingReadError>,
) -> MeetingContextRead {
    let snapshot = match loaded {
        Ok(MeetingLoadResult::Ready { snapshot }) => snapshot,
        Ok(MeetingLoadResult::NotFound) => return MeetingContextRead::NotFound,
        Ok(MeetingLoadResult::Forbidden) | Err(MeetingReadError::Forbidden) => {
            return MeetingContextRead::VerificationFailed;
        }
        Ok(MeetingLoadResult::UnsupportedRelay)
        | Ok(MeetingLoadResult::UnsupportedProtocol { .. }) => {
            return MeetingContextRead::VerificationFailed;
        }
        Err(MeetingReadError::Other(message)) => {
            return if transient_meeting_read_error(&message) {
                MeetingContextRead::Unavailable
            } else {
                MeetingContextRead::VerificationFailed
            };
        }
    };
    if snapshot.lifecycle == MeetingLifecycle::Initializing {
        return MeetingContextRead::NotAttachable;
    }
    let terminal = matches!(
        snapshot.lifecycle,
        MeetingLifecycle::Closed | MeetingLifecycle::Aborted
    );
    let end = snapshot.end.as_ref();
    if terminal != end.is_some() {
        return MeetingContextRead::VerificationFailed;
    }
    let state_event_id = snapshot
        .floor
        .as_ref()
        .map(|floor| floor.state_event_id.clone())
        .or_else(|| {
            snapshot
                .host
                .as_ref()
                .map(|host| host.state_event_id.clone())
        });
    let Some(state_event_id) = state_event_id else {
        return MeetingContextRead::VerificationFailed;
    };
    let action_finalization = snapshot
        .action
        .as_ref()
        .map(|action| MeetingContextActionSummary {
            condition: action.condition.clone(),
            terminal_status: action.terminal_status.clone(),
            actions_attested: end.is_some_and(|end| end.actions_attested),
        });
    let participant_preview = snapshot
        .participants
        .iter()
        .take(3)
        .map(|participant| MeetingContextParticipant {
            pubkey: participant.pubkey.clone(),
            participant_type: match participant.participant_type {
                MeetingParticipantType::Human => "human",
                MeetingParticipantType::Agent => "agent",
                MeetingParticipantType::Unknown => "unknown",
            },
        })
        .collect();
    MeetingContextRead::Observed(Box::new(MeetingContextRecord {
        title: snapshot.title.clone(),
        discussion_goal: snapshot.description.clone(),
        lifecycle: meeting_lifecycle_label(snapshot.lifecycle),
        terminal_outcome: end.map(|end| end.outcome.clone()),
        host_pubkey: snapshot.host_pubkey.clone(),
        participant_count: snapshot.participants.len(),
        participant_preview,
        created_at: snapshot.created_at,
        ended_at: end.map(|end| end.ended_at),
        action_finalization,
        state_revision: snapshot.state_revision,
        create_event_id: snapshot.create_event_id.clone(),
        state_event_id,
        end_event_id: end.map(|end| end.event_id.clone()),
        updated_at: snapshot.authoritative_updated_at,
    }))
}

fn transient_meeting_read_error(message: &str) -> bool {
    message.starts_with("relay unreachable:")
        || message.starts_with("relay rate-limited:")
        || message.starts_with("relay returned 409")
        || message.starts_with("relay returned 5")
}

fn meeting_lifecycle_label(lifecycle: MeetingLifecycle) -> &'static str {
    match lifecycle {
        MeetingLifecycle::Initializing => "initializing",
        MeetingLifecycle::Active => "active",
        MeetingLifecycle::FinalizingActions => "finalizing_actions",
        MeetingLifecycle::Closed => "closed",
        MeetingLifecycle::Aborted => "aborted",
    }
}

/// Load verified Meeting metadata without exposing Board or Speech.
#[tauri::command]
pub async fn get_meeting_context_detail(
    meeting_id: String,
    state: State<'_, AppState>,
) -> Result<MeetingContextInspectorLoadResult, String> {
    let meeting_id = canonical_meeting_id(&meeting_id)?;
    let Some(identity) = read_meeting_identity(&state).await? else {
        return Ok(MeetingContextInspectorLoadResult::UnsupportedRelay);
    };
    let loaded = match load_meeting_snapshot(&state, &identity, &meeting_id).await {
        Ok(result) => result,
        Err(MeetingReadError::Forbidden) => {
            return Ok(MeetingContextInspectorLoadResult::Forbidden);
        }
        Err(error) => return Err(read_error_message(error)),
    };
    let snapshot = match loaded {
        MeetingLoadResult::Ready { snapshot } => snapshot,
        MeetingLoadResult::UnsupportedRelay => {
            return Ok(MeetingContextInspectorLoadResult::UnsupportedRelay);
        }
        MeetingLoadResult::Forbidden => {
            return Ok(MeetingContextInspectorLoadResult::Forbidden);
        }
        MeetingLoadResult::NotFound => return Ok(MeetingContextInspectorLoadResult::NotFound),
        MeetingLoadResult::UnsupportedProtocol { .. } => {
            return Ok(MeetingContextInspectorLoadResult::UnsupportedProtocol);
        }
    };
    if snapshot.lifecycle == MeetingLifecycle::Initializing {
        return Ok(MeetingContextInspectorLoadResult::NotAttachable);
    }
    let terminal = matches!(
        snapshot.lifecycle,
        MeetingLifecycle::Closed | MeetingLifecycle::Aborted
    );
    let end = snapshot.end.as_ref();
    if terminal != end.is_some() {
        return Ok(MeetingContextInspectorLoadResult::UnsupportedProtocol);
    }
    let action_finalization =
        snapshot
            .action
            .as_ref()
            .map(|action| MeetingContextInspectorActionSummary {
                condition: action.condition.clone(),
                terminal_status: action.terminal_status.clone(),
                actions_attested: end.is_some_and(|end| end.actions_attested),
            });
    Ok(MeetingContextInspectorLoadResult::Ready {
        detail: Box::new(MeetingContextInspectorDetail {
            meeting_id: snapshot.meeting_id.clone(),
            title: snapshot.title.clone(),
            description: snapshot.description.clone(),
            host_pubkey: snapshot.host_pubkey.clone(),
            participants: snapshot.participants.clone(),
            lifecycle: snapshot.lifecycle,
            terminal_outcome: end.map(|end| end.outcome.clone()),
            created_at: snapshot.created_at,
            ended_at: end.map(|end| end.ended_at),
            action_finalization,
        }),
    })
}
