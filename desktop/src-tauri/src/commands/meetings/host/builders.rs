use buzz_sdk_pkg::{
    MeetingV2ActionsEndParams, MeetingV2BoardActionParams, MeetingV2EndOutcome, MeetingV2EndParams,
};
use uuid::Uuid;

use super::MeetingSnapshot;

pub(super) fn build_board_action(
    snapshot: &MeetingSnapshot,
    params: MeetingV2BoardActionParams<'_>,
) -> Result<nostr::EventBuilder, buzz_sdk_pkg::SdkError> {
    if snapshot.policy == buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY {
        buzz_sdk_pkg::build_meeting_v2_actions_board_action(params)
    } else {
        buzz_sdk_pkg::build_meeting_v2_board_action(params)
    }
}

pub(super) fn build_end(
    snapshot: &MeetingSnapshot,
    session_id: Uuid,
    outcome: MeetingV2EndOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
) -> Result<nostr::EventBuilder, buzz_sdk_pkg::SdkError> {
    if snapshot.policy == buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY {
        buzz_sdk_pkg::build_meeting_v2_actions_end(MeetingV2ActionsEndParams {
            session_id,
            create_event_id: &snapshot.create_event_id,
            outcome,
            reason_code,
            reason,
            action_fence: None,
        })
    } else {
        buzz_sdk_pkg::build_meeting_v2_end(MeetingV2EndParams {
            session_id,
            create_event_id: &snapshot.create_event_id,
            outcome,
            reason_code,
            reason,
        })
    }
}
