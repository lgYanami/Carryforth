use super::*;

pub(super) fn list_item_from_load(
    meeting_id: String,
    loaded: Result<MeetingLoadResult, MeetingReadError>,
    viewer_pubkey: &str,
) -> MeetingListItem {
    match loaded {
        Ok(MeetingLoadResult::Ready { snapshot }) => {
            let attention_reason = meeting_attention_reason(&snapshot, viewer_pubkey);
            let viewer_role = if snapshot.host_pubkey == viewer_pubkey {
                MeetingViewerRole::Host
            } else if snapshot
                .participants
                .iter()
                .any(|participant| participant.pubkey == viewer_pubkey)
            {
                MeetingViewerRole::Participant
            } else {
                MeetingViewerRole::Observer
            };
            MeetingListItem {
                meeting_id,
                title: snapshot.title.clone(),
                description: snapshot.description.clone(),
                lifecycle: Some(snapshot.lifecycle),
                phase: Some(snapshot.phase.clone()),
                current_speaker_pubkey: snapshot.current_speaker_pubkey.clone(),
                current_offer_pubkey: snapshot.current_offer_pubkey.clone(),
                needs_attention: attention_reason.is_some(),
                attention_reason,
                moderator_pubkey: Some(snapshot.moderator_pubkey.clone()),
                host_pubkey: Some(snapshot.host_pubkey.clone()),
                participant_count: Some(snapshot.participants.len()),
                participant_preview: snapshot.participants.iter().take(3).cloned().collect(),
                viewer_role: Some(viewer_role),
                policy: Some(snapshot.policy.clone()),
                created_at: Some(snapshot.created_at),
                updated_at: Some(snapshot.authoritative_updated_at),
                ended_at: snapshot.end.as_ref().map(|end| end.ended_at),
                latest_speech_at: snapshot.latest_speech_at,
                compatibility: MeetingListCompatibility::Ready,
            }
        }
        Ok(MeetingLoadResult::UnsupportedRelay) => {
            empty_list_item(meeting_id, MeetingListCompatibility::UnsupportedRelay)
        }
        Ok(MeetingLoadResult::UnsupportedProtocol { .. }) => {
            empty_list_item(meeting_id, MeetingListCompatibility::UnsupportedProtocol)
        }
        Ok(MeetingLoadResult::Forbidden) | Err(MeetingReadError::Forbidden) => {
            empty_list_item(meeting_id, MeetingListCompatibility::Forbidden)
        }
        Ok(MeetingLoadResult::NotFound) => {
            empty_list_item(meeting_id, MeetingListCompatibility::NotFound)
        }
        Err(MeetingReadError::Other(_)) => {
            empty_list_item(meeting_id, MeetingListCompatibility::UnsupportedProtocol)
        }
    }
}

pub(super) fn meeting_attention_reason(
    snapshot: &MeetingSnapshot,
    viewer_pubkey: &str,
) -> Option<MeetingAttentionReason> {
    let viewer_is_human = snapshot.participants.iter().any(|participant| {
        participant.pubkey == viewer_pubkey
            && participant.participant_type == MeetingParticipantType::Human
    });
    if !viewer_is_human {
        return None;
    }
    if matches!(snapshot.lifecycle, MeetingLifecycle::Aborted) {
        return Some(MeetingAttentionReason::MeetingAborted);
    }
    if snapshot.current_offer_pubkey.as_deref() == Some(viewer_pubkey) {
        return Some(MeetingAttentionReason::FloorOffer);
    }
    if snapshot.current_speaker_pubkey.as_deref() == Some(viewer_pubkey) {
        return Some(MeetingAttentionReason::FloorGrant);
    }
    if snapshot.moderator_pubkey != viewer_pubkey {
        return None;
    }
    if matches!(snapshot.lifecycle, MeetingLifecycle::FinalizingActions) {
        return snapshot.action.as_ref().and_then(|action| {
            if action.condition == "blocked" {
                Some(MeetingAttentionReason::HostActionBlocked)
            } else if action.condition == "runnable" {
                Some(MeetingAttentionReason::HostAction)
            } else {
                None
            }
        });
    }
    let host = snapshot.host.as_ref()?;
    if host.board_control.phase == "board_pending" {
        return Some(MeetingAttentionReason::HostBoard);
    }
    host.can_select.then_some(MeetingAttentionReason::HostFloor)
}

fn empty_list_item(meeting_id: String, compatibility: MeetingListCompatibility) -> MeetingListItem {
    MeetingListItem {
        title: meeting_id.clone(),
        meeting_id,
        description: None,
        lifecycle: None,
        phase: None,
        current_speaker_pubkey: None,
        current_offer_pubkey: None,
        needs_attention: false,
        attention_reason: None,
        moderator_pubkey: None,
        host_pubkey: None,
        participant_count: None,
        participant_preview: Vec::new(),
        viewer_role: None,
        policy: None,
        created_at: None,
        updated_at: None,
        ended_at: None,
        latest_speech_at: None,
        compatibility,
    }
}
