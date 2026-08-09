//! Metadata-first Meeting hydration for Project Context coordinates.

use std::collections::{BTreeMap, BTreeSet};

use buzz_project_context_pkg::ProjectContextCoordinate;
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use super::{
    coordinate_dto, coordinate_key, unavailable_coordinate, ProjectContextCoordinateDetail,
    ProjectContextDetailState, ProjectContextMeetingActionSummary, ProjectContextMeetingDetail,
    ProjectContextMeetingObservation, ProjectContextMeetingObservationState,
    ProjectContextMeetingParticipant, ProjectContextReadContext,
};
use crate::app_state::AppState;
use crate::commands::meetings::{
    read_meetings_for_project_context_at, MeetingContextRead, MeetingContextRecord,
};

pub(super) struct MeetingHydration {
    pub(super) observations: Vec<ProjectContextMeetingObservation>,
    pub(super) coordinates: BTreeMap<ProjectContextCoordinate, ProjectContextCoordinateDetail>,
}

pub(super) async fn hydrate_meetings(
    state: &AppState,
    context: &ProjectContextReadContext,
    requested: &BTreeSet<ProjectContextCoordinate>,
    required_by_edge: &BTreeSet<ProjectContextCoordinate>,
) -> MeetingHydration {
    let meeting_ids = requested
        .iter()
        .filter_map(|coordinate| match coordinate {
            ProjectContextCoordinate::Meeting { meeting_id } => Some(*meeting_id),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let reads = read_meetings_for_project_context_at(
        state,
        &context.api_base_url,
        &context.keys,
        &context.identity.relay_pubkey,
        &meeting_ids,
    )
    .await;

    let mut observations = Vec::with_capacity(meeting_ids.len());
    let mut coordinates = BTreeMap::new();
    for coordinate in requested {
        let ProjectContextCoordinate::Meeting { meeting_id } = coordinate else {
            continue;
        };
        let read = reads
            .get(meeting_id)
            .cloned()
            .unwrap_or(MeetingContextRead::Unavailable);
        let required = required_by_edge.contains(coordinate);
        let (detail, observation) = meeting_detail(coordinate, *meeting_id, read, required);
        coordinates.insert(coordinate.clone(), detail);
        observations.push(observation);
    }
    observations.sort_by_key(|observation| observation.meeting_id);
    MeetingHydration {
        observations,
        coordinates,
    }
}

fn meeting_detail(
    coordinate: &ProjectContextCoordinate,
    meeting_id: Uuid,
    read: MeetingContextRead,
    required_by_edge: bool,
) -> (
    ProjectContextCoordinateDetail,
    ProjectContextMeetingObservation,
) {
    match read {
        MeetingContextRead::Observed(record) => observed_detail(coordinate, meeting_id, *record)
            .unwrap_or_else(|| failed_detail(coordinate, meeting_id, "invalid_meeting_timestamp")),
        MeetingContextRead::NotAttachable => failed_or_unavailable_detail(
            coordinate,
            meeting_id,
            required_by_edge,
            "meeting_not_attachable",
        ),
        MeetingContextRead::NotFound => failed_or_unavailable_detail(
            coordinate,
            meeting_id,
            required_by_edge,
            "meeting_not_found",
        ),
        MeetingContextRead::Unavailable => {
            unavailable_detail(coordinate, meeting_id, "meeting_metadata_unavailable")
        }
        MeetingContextRead::VerificationFailed => {
            failed_detail(coordinate, meeting_id, "meeting_verification_failed")
        }
    }
}

fn observed_detail(
    coordinate: &ProjectContextCoordinate,
    meeting_id: Uuid,
    record: MeetingContextRecord,
) -> Option<(
    ProjectContextCoordinateDetail,
    ProjectContextMeetingObservation,
)> {
    let created_at = timestamp(record.created_at)?;
    let ended_at = match record.ended_at {
        Some(ended_at) => Some(timestamp(ended_at)?),
        None => None,
    };
    let updated_at = timestamp(record.updated_at)?;
    let observation = ProjectContextMeetingObservation {
        meeting_id,
        state: ProjectContextMeetingObservationState::Observed,
        state_revision: Some(record.state_revision),
        create_event_id: Some(record.create_event_id),
        state_event_id: Some(record.state_event_id),
        end_event_id: record.end_event_id,
        updated_at: Some(updated_at),
    };
    let detail = ProjectContextCoordinateDetail {
        coordinate_key: coordinate_key(coordinate),
        coordinate: coordinate_dto(coordinate),
        state: if record.terminal_outcome.is_some() {
            ProjectContextDetailState::Terminal
        } else {
            ProjectContextDetailState::Active
        },
        title: Some(record.title),
        summary: record.summary,
        status: Some(json!(record.lifecycle)),
        object_revision: None,
        document_revision: None,
        meeting: Some(ProjectContextMeetingDetail {
            discussion_goal: record.discussion_goal,
            lifecycle: record.lifecycle.to_string(),
            terminal_outcome: record.terminal_outcome,
            host_pubkey: record.host_pubkey,
            participant_count: record.participant_count,
            participant_preview: record
                .participant_preview
                .into_iter()
                .map(|participant| ProjectContextMeetingParticipant {
                    pubkey: participant.pubkey,
                    participant_type: participant.participant_type.to_owned(),
                })
                .collect(),
            created_at,
            ended_at,
            action_finalization: record.action_finalization.map(|action| {
                ProjectContextMeetingActionSummary {
                    condition: action.condition,
                    terminal_status: action.terminal_status,
                    actions_attested: action.actions_attested,
                }
            }),
        }),
        updated_at: Some(updated_at),
        updated_by: None,
        unavailable_reason: None,
    };
    Some((detail, observation))
}

fn failed_or_unavailable_detail(
    coordinate: &ProjectContextCoordinate,
    meeting_id: Uuid,
    required_by_edge: bool,
    reason: &'static str,
) -> (
    ProjectContextCoordinateDetail,
    ProjectContextMeetingObservation,
) {
    if required_by_edge {
        failed_detail(coordinate, meeting_id, reason)
    } else {
        unavailable_detail(coordinate, meeting_id, reason)
    }
}

fn failed_detail(
    coordinate: &ProjectContextCoordinate,
    meeting_id: Uuid,
    reason: &'static str,
) -> (
    ProjectContextCoordinateDetail,
    ProjectContextMeetingObservation,
) {
    unavailable_with_observation(
        coordinate,
        meeting_id,
        reason,
        ProjectContextMeetingObservationState::VerificationFailed,
    )
}

fn unavailable_detail(
    coordinate: &ProjectContextCoordinate,
    meeting_id: Uuid,
    reason: &'static str,
) -> (
    ProjectContextCoordinateDetail,
    ProjectContextMeetingObservation,
) {
    unavailable_with_observation(
        coordinate,
        meeting_id,
        reason,
        ProjectContextMeetingObservationState::Unavailable,
    )
}

fn unavailable_with_observation(
    coordinate: &ProjectContextCoordinate,
    meeting_id: Uuid,
    reason: &'static str,
    state: ProjectContextMeetingObservationState,
) -> (
    ProjectContextCoordinateDetail,
    ProjectContextMeetingObservation,
) {
    let mut detail = unavailable_coordinate(coordinate);
    detail.unavailable_reason = Some(reason);
    (
        detail,
        ProjectContextMeetingObservation {
            meeting_id,
            state,
            state_revision: None,
            create_event_id: None,
            state_event_id: None,
            end_event_id: None,
            updated_at: None,
        },
    )
}

fn timestamp(seconds: u64) -> Option<DateTime<Utc>> {
    i64::try_from(seconds)
        .ok()
        .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::meetings::{
        MeetingContextActionSummary as ReadActionSummary, MeetingContextParticipant,
    };

    fn coordinate(meeting_id: Uuid) -> ProjectContextCoordinate {
        ProjectContextCoordinate::Meeting { meeting_id }
    }

    fn terminal_record() -> MeetingContextRecord {
        MeetingContextRecord {
            title: "Memory boundary review".to_string(),
            discussion_goal: Some("Agree the first durable memory slice".to_string()),
            summary: Some("Meeting decisions about the durable memory boundary.".to_string()),
            lifecycle: "closed",
            terminal_outcome: Some("closed".to_string()),
            host_pubkey: "a".repeat(64),
            participant_count: 4,
            participant_preview: vec![MeetingContextParticipant {
                pubkey: "b".repeat(64),
                participant_type: "agent",
            }],
            created_at: 1_786_054_800,
            ended_at: Some(1_786_055_400),
            action_finalization: Some(ReadActionSummary {
                condition: "recorded".to_string(),
                terminal_status: Some("completed_closed".to_string()),
                actions_attested: true,
            }),
            state_revision: 64,
            create_event_id: "c".repeat(64),
            state_event_id: "d".repeat(64),
            end_event_id: Some("e".repeat(64)),
            updated_at: 1_786_055_400,
        }
    }

    #[test]
    fn terminal_meeting_emits_body_free_detail_and_evidence() {
        let meeting_id = Uuid::parse_str("60000000-0000-4000-8000-000000000001")
            .expect("fixture UUID must be valid");
        let (detail, observation) = meeting_detail(
            &coordinate(meeting_id),
            meeting_id,
            MeetingContextRead::Observed(Box::new(terminal_record())),
            true,
        );

        assert_eq!(detail.state, ProjectContextDetailState::Terminal);
        assert_eq!(detail.title.as_deref(), Some("Memory boundary review"));
        let meeting = detail.meeting.expect("terminal detail must be present");
        assert_eq!(meeting.participant_count, 4);
        assert_eq!(meeting.participant_preview.len(), 1);
        assert_eq!(meeting.lifecycle, "closed");
        assert_eq!(meeting.terminal_outcome.as_deref(), Some("closed"));
        assert_eq!(
            observation.state,
            ProjectContextMeetingObservationState::Observed
        );
        assert_eq!(observation.state_revision, Some(64));
        assert_eq!(observation.end_event_id, Some("e".repeat(64)));
    }

    #[test]
    fn required_nonterminal_meeting_is_a_verification_failure() {
        let meeting_id = Uuid::parse_str("60000000-0000-4000-8000-000000000001")
            .expect("fixture UUID must be valid");
        let (detail, observation) = meeting_detail(
            &coordinate(meeting_id),
            meeting_id,
            MeetingContextRead::NotAttachable,
            true,
        );

        assert_eq!(detail.state, ProjectContextDetailState::Unavailable);
        assert_eq!(detail.unavailable_reason, Some("meeting_not_attachable"));
        assert_eq!(
            observation.state,
            ProjectContextMeetingObservationState::VerificationFailed
        );
    }

    #[test]
    fn finalizing_meeting_emits_active_detail_with_frozen_action_metadata() {
        let meeting_id = Uuid::parse_str("60000000-0000-4000-8000-000000000002")
            .expect("fixture UUID must be valid");
        let mut record = terminal_record();
        record.lifecycle = "finalizing_actions";
        record.terminal_outcome = None;
        record.ended_at = None;
        record.end_event_id = None;
        record.action_finalization = Some(ReadActionSummary {
            condition: "runnable".to_string(),
            terminal_status: None,
            actions_attested: false,
        });

        let (detail, observation) = meeting_detail(
            &coordinate(meeting_id),
            meeting_id,
            MeetingContextRead::Observed(Box::new(record)),
            true,
        );

        assert_eq!(detail.state, ProjectContextDetailState::Active);
        assert_eq!(detail.status, Some(json!("finalizing_actions")));
        let meeting = detail.meeting.expect("finalizing detail must be present");
        assert_eq!(meeting.lifecycle, "finalizing_actions");
        assert!(meeting.terminal_outcome.is_none());
        assert!(meeting.ended_at.is_none());
        assert_eq!(
            meeting
                .action_finalization
                .expect("action summary")
                .condition,
            "runnable"
        );
        assert_eq!(
            observation.state,
            ProjectContextMeetingObservationState::Observed
        );
        assert!(observation.end_event_id.is_none());
    }

    #[test]
    fn query_only_missing_meeting_degrades_without_claiming_integrity_failure() {
        let meeting_id = Uuid::parse_str("60000000-0000-4000-8000-000000000001")
            .expect("fixture UUID must be valid");
        let (detail, observation) = meeting_detail(
            &coordinate(meeting_id),
            meeting_id,
            MeetingContextRead::NotFound,
            false,
        );

        assert_eq!(detail.state, ProjectContextDetailState::Unavailable);
        assert_eq!(
            observation.state,
            ProjectContextMeetingObservationState::Unavailable
        );
    }
}
