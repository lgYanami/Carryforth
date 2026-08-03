//! Strict moderated-baton Meeting command ingestion for V1 and V2.
//!
//! Wire parsing belongs at the Relay boundary. The database receives a closed,
//! typed command and remains authoritative for participant roles, revisions,
//! deadlines, idempotency, lazy recovery, and state transitions.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use buzz_core::kind::{
    KIND_MEETING_GRANT_SIGNAL, KIND_MEETING_HUMAN_FLOOR_REQUEST, KIND_MEETING_MODERATOR_COMMAND,
    KIND_MEETING_OFFER_RESPONSE, KIND_MEETING_SPEECH_INTENT,
};
use buzz_core::tenant::TenantContext;
use buzz_db::meeting::MAX_MEETING_PARTICIPANTS;
use buzz_db::meeting_baton::{
    BatonCommand, BatonCommandOutcome, BatonCommandTxParams, BatonDecisionAttemptFinishOutcome,
    BatonHandoffInput, BatonIntentDeferral, BatonProgressStage, BatonSelectionSource,
};
use buzz_db::meeting_v2::{BoardAction, BoardActionOutcome, BoardActionTxParams};
use buzz_db::DbError;
use nostr::Event;
use serde::Deserialize;
use uuid::Uuid;

use crate::state::AppState;

use super::command_executor::{
    decode_event_id, map_meeting_db_error, optional_single_tag, parse_single_uuid_tag,
    require_single_tag, validate_meeting_tag_schema, MeetingProtocol,
};
use super::ingest::{IngestAuth, IngestError, IngestResult};

const MAX_INTENT_SUMMARY_BYTES: usize = 512;
const MAX_SELECTION_REASON_BYTES: usize = 512;
const MAX_CONTROL_REASON_BYTES: usize = 1_024;
const MAX_RESPONSE_REASON_BYTES: usize = 512;
const MAX_HANDOFF_REASON_BYTES: usize = 1_024;

const REJECTION_REASON_CODES: &[&str] = &[
    "off_topic",
    "duplicate",
    "superseded",
    "unsupported",
    "agenda_mismatch",
];
const HANDOFF_DISMISS_REASON_CODES: &[&str] = &[
    "superseded",
    "answered_elsewhere",
    "out_of_scope",
    "no_longer_needed",
];
const YIELD_REASON_CODES: &[&str] = &[
    "no_longer_needed",
    "unable_to_answer",
    "insufficient_context",
    "tool_failure",
    "cancelled",
];
const HANDOFF_TYPES: &[&str] = &[
    "question",
    "information_request",
    "clarification",
    "review",
    "response_requested",
];
const ATTEMPT_COMPLETED_REASON_CODES: &[&str] = &["no_action", "idle_wait_fallback"];
const ATTEMPT_DISCARDED_REASON_CODES: &[&str] = &[
    "human_priority",
    "control_changed",
    "speech_changed",
    "meeting_ended",
    "moderator_changed",
    "cas_churn",
    "source_changed",
    "runtime_replaced",
];

/// Parse and execute one participant-authored moderated control command.
pub(crate) async fn handle_command(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    // Resolve only the room boundary before authorization. Full command
    // parsing can disclose protocol details (for example which action tags are
    // required), so non-participants must be rejected before that validation.
    let session_id = parse_single_uuid_tag(event, "h", "meeting session id")?;
    let protocol = authorize_participant_command(tenant, state, session_id, auth).await?;
    let (parsed_session_id, command) = parse_control_command(event, protocol)?;
    debug_assert_eq!(parsed_session_id, session_id);
    execute(tenant, state, session_id, event, protocol, command).await
}

/// Parse and execute one moderator-authored Meeting V2 Board Maintenance result.
///
/// Board commands are receipt-backed commands rather than durable Meeting
/// history. The database atomically replaces the pull-only current Board,
/// completes the Board window, and emits the next canonical State.
pub(crate) async fn handle_board_action(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let session_id = parse_single_uuid_tag(event, "h", "meeting session id")?;
    let protocol = authorize_participant_command(tenant, state, session_id, auth).await?;
    if !protocol.is_v2() {
        return Err(IngestError::Rejected(
            "invalid: Board Maintenance command targets a non-V2 Meeting".into(),
        ));
    }
    let (parsed_session_id, expected_control_epoch, board_window, action) =
        parse_board_action(event, protocol)?;
    debug_assert_eq!(parsed_session_id, session_id);
    let action_label = match &action {
        BoardAction::Update(_) => "update",
        BoardAction::Unchanged => "unchanged",
    };
    let started_at = Instant::now();
    let commit = match buzz_db::meeting_v2::execute_board_action(
        &state.db,
        BoardActionTxParams {
            community_id: tenant.community(),
            session_id,
            event,
            relay_keys: &state.relay_keypair,
            expected_control_epoch,
            board_window,
            action,
        },
    )
    .await
    {
        Ok(commit) => commit,
        Err(error) => {
            record_board_command_metrics(
                action_label,
                "error",
                false,
                0,
                started_at.elapsed().as_secs_f64(),
            );
            return Err(map_baton_db_error(error));
        }
    };
    let recovery = usize::from(commit.recovery_transition.is_some());
    match commit.outcome {
        BoardActionOutcome::Accepted {
            state_revision,
            board_event_id,
        } => {
            record_board_command_metrics(
                action_label,
                "accepted",
                false,
                recovery,
                started_at.elapsed().as_secs_f64(),
            );
            Ok(board_success_result(
                event,
                session_id,
                false,
                "accepted",
                Some(state_revision),
                Some(&board_event_id),
                recovery,
            ))
        }
        BoardActionOutcome::Duplicate {
            accepted: true,
            outcome_code,
            state_revision,
            board_event_id,
            ..
        } => {
            record_board_command_metrics(
                action_label,
                "accepted",
                true,
                recovery,
                started_at.elapsed().as_secs_f64(),
            );
            Ok(board_success_result(
                event,
                session_id,
                true,
                &outcome_code,
                state_revision,
                board_event_id.as_deref(),
                recovery,
            ))
        }
        BoardActionOutcome::Duplicate {
            accepted: false,
            outcome_class,
            outcome_code,
            ..
        } => {
            let outcome = if outcome_class == "rejected_after_recovery" {
                "expired"
            } else {
                "conflict"
            };
            record_board_command_metrics(
                action_label,
                outcome,
                true,
                recovery,
                started_at.elapsed().as_secs_f64(),
            );
            Err(board_rejection(outcome, &outcome_code, recovery))
        }
        BoardActionOutcome::Rejected {
            code,
            after_recovery,
        } => {
            let outcome = if after_recovery {
                "expired"
            } else {
                "conflict"
            };
            record_board_command_metrics(
                action_label,
                outcome,
                false,
                recovery,
                started_at.elapsed().as_secs_f64(),
            );
            Err(board_rejection(outcome, &code, recovery))
        }
    }
}

/// Parse and execute one moderator-authored Meeting V2 action-finalization command.
pub(crate) async fn handle_action_command(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let session_id = parse_single_uuid_tag(event, "h", "meeting session id")?;
    let protocol = authorize_participant_command(tenant, state, session_id, auth).await?;
    if !protocol.has_action_finalization() {
        return Err(IngestError::Rejected(
            "invalid: action command targets a Meeting without action finalization".into(),
        ));
    }
    let (parsed_session_id, command) = parse_action_command(event)?;
    debug_assert_eq!(parsed_session_id, session_id);
    let commit = buzz_db::meeting_v2_actions::execute_action_command(
        &state.db,
        buzz_db::meeting_v2_actions::ActionCommandTxParams {
            community_id: tenant.community(),
            session_id,
            event,
            command,
            relay_keys: &state.relay_keypair,
        },
    )
    .await
    .map_err(map_baton_db_error)?;
    let mut response = commit.response;
    if let Some(object) = response.as_object_mut() {
        object.insert(
            "duplicate".to_string(),
            serde_json::Value::Bool(commit.duplicate),
        );
    }
    if !commit.accepted {
        return Err(IngestError::Rejected(format!("conflict: {response}")));
    }
    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!("response:{response}"),
    })
}

fn parse_action_command(
    event: &Event,
) -> Result<(Uuid, buzz_db::meeting_v2_actions::ActionCommand), IngestError> {
    let action = require_single_tag(event, "action")?;
    let session_id = parse_single_uuid_tag(event, "h", "meeting session id")?;
    if require_single_tag(event, "v")? != buzz_sdk::MEETING_V2_SCHEMA_VERSION {
        return Err(IngestError::Rejected(
            "invalid: Meeting V2 action command must use schema version 3".into(),
        ));
    }
    if require_single_tag(event, "policy")? != buzz_sdk::MEETING_V2_ACTIONS_POLICY {
        return Err(IngestError::Rejected(format!(
            "invalid: Meeting V2 action command policy must be {}",
            buzz_sdk::MEETING_V2_ACTIONS_POLICY
        )));
    }
    let command = match action.as_str() {
        "begin" => {
            validate_meeting_tag_schema(
                event,
                &[
                    "h",
                    "v",
                    "policy",
                    "action",
                    "expected-control-epoch",
                    "board-window",
                    "expected-state",
                    "board",
                ],
                &["decision-attempt"],
                &[],
            )?;
            require_empty_content(event, "Meeting V2 action begin")?;
            buzz_db::meeting_v2_actions::ActionCommand::Begin {
                expected_control_epoch: parse_positive_i64_tag(event, "expected-control-epoch")?,
                board_window: parse_positive_i64_tag(event, "board-window")?,
                expected_state_event_id: decode_event_id(
                    &require_single_tag(event, "expected-state")?,
                    "Meeting expected State event id",
                )?,
                board_event_id: decode_event_id(
                    &require_single_tag(event, "board")?,
                    "Meeting final Board event id",
                )?,
                expected_decision_attempt_id: optional_single_tag(event, "decision-attempt")?
                    .map(|value| decode_event_id(&value, "Meeting decision attempt id"))
                    .transpose()?,
            }
        }
        "plan" => {
            validate_action_run_tag_schema(event, &[])?;
            let fence = parse_action_run_fence(event)?;
            if fence.plan_event_id.is_some() {
                return Err(IngestError::Rejected(
                    "invalid: Meeting action plan must use action-plan=none".into(),
                ));
            }
            let plan: buzz_sdk::MeetingV2ActionPlan = serde_json::from_str(&event.content)
                .map_err(|error| {
                    IngestError::Rejected(format!(
                        "invalid: malformed Meeting V2 action plan JSON: {error}"
                    ))
                })?;
            buzz_sdk::validate_meeting_v2_action_plan(&plan)
                .map_err(|error| IngestError::Rejected(format!("invalid: {error}")))?;
            buzz_db::meeting_v2_actions::ActionCommand::Plan { fence, plan }
        }
        "block" => {
            validate_action_run_tag_schema(event, &["reason-code"])?;
            validate_clean_action_text(&event.content, 1_024, true, "block reason")?;
            let reason_code = require_single_tag(event, "reason-code")?;
            validate_clean_action_text(&reason_code, 128, false, "block reason code")?;
            buzz_db::meeting_v2_actions::ActionCommand::Block {
                fence: parse_action_run_fence(event)?,
                reason_code,
            }
        }
        "retry" | "complete" | "return-to-board" => {
            validate_action_run_tag_schema(event, &[])?;
            require_empty_content(event, "Meeting V2 action command")?;
            let fence = parse_action_run_fence(event)?;
            match action.as_str() {
                "retry" => buzz_db::meeting_v2_actions::ActionCommand::Retry { fence },
                "complete" => buzz_db::meeting_v2_actions::ActionCommand::Complete { fence },
                _ => buzz_db::meeting_v2_actions::ActionCommand::ReturnToBoard { fence },
            }
        }
        "step-prepared" => {
            validate_action_run_tag_schema(
                event,
                &[
                    "step",
                    "attempt",
                    "project-event",
                    "expected-project-revision",
                ],
            )?;
            let fence = parse_action_run_fence(event)?;
            if fence.plan_event_id.is_none() {
                return Err(IngestError::Rejected(
                    "invalid: prepared action step requires a frozen plan".into(),
                ));
            }
            let step_id = parse_non_nil_uuid_tag(event, "step", "Meeting action step id")?;
            let attempt =
                i32::try_from(parse_positive_i64_tag(event, "attempt")?).map_err(|_| {
                    IngestError::Rejected("invalid: Meeting action attempt exceeds i32".into())
                })?;
            let project_event_id = decode_event_id(
                &require_single_tag(event, "project-event")?,
                "Meeting prepared Project event id",
            )?;
            let expected_project_revision =
                parse_positive_i64_tag(event, "expected-project-revision")?;
            let signed_project_event: Event =
                serde_json::from_str(&event.content).map_err(|error| {
                    IngestError::Rejected(format!(
                        "invalid: malformed signed Project View event JSON: {error}"
                    ))
                })?;
            buzz_db::meeting_v2_actions::ActionCommand::StepPrepared {
                fence,
                step_id,
                attempt,
                project_event_id,
                expected_project_revision,
                signed_project_event,
            }
        }
        "step-applied" => {
            validate_action_run_tag_schema(
                event,
                &["step", "project-event", "accepted-project-revision"],
            )?;
            require_empty_content(event, "Meeting V2 applied action step")?;
            let fence = parse_action_run_fence(event)?;
            if fence.plan_event_id.is_none() {
                return Err(IngestError::Rejected(
                    "invalid: applied action step requires a frozen plan".into(),
                ));
            }
            buzz_db::meeting_v2_actions::ActionCommand::StepApplied {
                fence,
                step_id: parse_non_nil_uuid_tag(event, "step", "Meeting action step id")?,
                project_event_id: decode_event_id(
                    &require_single_tag(event, "project-event")?,
                    "Meeting applied Project event id",
                )?,
                accepted_project_revision: parse_positive_i64_tag(
                    event,
                    "accepted-project-revision",
                )?,
            }
        }
        _ => {
            return Err(IngestError::Rejected(format!(
                "invalid: unsupported Meeting V2 action command {action}"
            )));
        }
    };
    Ok((session_id, command))
}

fn validate_action_run_tag_schema(
    event: &Event,
    additional_required: &[&str],
) -> Result<(), IngestError> {
    let mut required = vec![
        "h",
        "v",
        "policy",
        "action",
        "action-run",
        "action-window",
        "action-plan",
    ];
    required.extend_from_slice(additional_required);
    validate_meeting_tag_schema(event, &required, &[], &[])
}

fn parse_action_run_fence(
    event: &Event,
) -> Result<buzz_db::meeting_v2_actions::ActionRunFence, IngestError> {
    let action_run_id = Uuid::parse_str(&require_single_tag(event, "action-run")?)
        .map_err(|_| IngestError::Rejected("invalid: bad Meeting action run id".into()))?;
    if action_run_id.is_nil() {
        return Err(IngestError::Rejected(
            "invalid: Meeting action run id must not be nil".into(),
        ));
    }
    let action_window_epoch = parse_positive_i64_tag(event, "action-window")?;
    let plan = require_single_tag(event, "action-plan")?;
    let plan_event_id = if plan == "none" {
        None
    } else {
        Some(decode_event_id(&plan, "Meeting action plan event id")?)
    };
    Ok(buzz_db::meeting_v2_actions::ActionRunFence {
        action_run_id,
        action_window_epoch,
        plan_event_id,
    })
}

fn parse_non_nil_uuid_tag(event: &Event, tag_name: &str, field: &str) -> Result<Uuid, IngestError> {
    let value = Uuid::parse_str(&require_single_tag(event, tag_name)?)
        .map_err(|_| IngestError::Rejected(format!("invalid: bad {field}")))?;
    if value.is_nil() {
        return Err(IngestError::Rejected(format!(
            "invalid: {field} must not be nil"
        )));
    }
    Ok(value)
}

fn validate_clean_action_text(
    value: &str,
    max_bytes: usize,
    allow_empty: bool,
    field: &str,
) -> Result<(), IngestError> {
    if value.is_empty() && allow_empty {
        return Ok(());
    }
    if value.is_empty()
        || value.trim() != value
        || value.len() > max_bytes
        || value.chars().any(char::is_control)
    {
        return Err(IngestError::Rejected(format!(
            "invalid: Meeting action {field} must be clean and at most {max_bytes} bytes"
        )));
    }
    Ok(())
}

fn record_board_command_metrics(
    action: &'static str,
    outcome: &'static str,
    duplicate: bool,
    recovery_count: usize,
    latency_seconds: f64,
) {
    let duplicate = if duplicate { "true" } else { "false" };
    metrics::counter!(
        "meeting_v2_board_command_total",
        "action" => action,
        "outcome" => outcome,
        "duplicate" => duplicate
    )
    .increment(1);
    metrics::histogram!(
        "meeting_v2_board_command_latency_seconds",
        "action" => action,
        "outcome" => outcome
    )
    .record(latency_seconds);
    metrics::histogram!(
        "meeting_v2_board_command_recovery_transitions",
        "action" => action,
        "outcome" => outcome
    )
    .record(recovery_count as f64);
}

fn parse_board_action(
    event: &Event,
    protocol: MeetingProtocol,
) -> Result<(Uuid, i64, i64, BoardAction), IngestError> {
    validate_meeting_tag_schema(
        event,
        &[
            "h",
            "v",
            "policy",
            "action",
            "expected-control-epoch",
            "board-window",
        ],
        &[],
        &[],
    )?;
    if require_single_tag(event, "v")? != buzz_sdk::MEETING_V2_SCHEMA_VERSION {
        return Err(IngestError::Rejected(
            "invalid: Meeting V2 Board command must use schema version 3".into(),
        ));
    }
    if require_single_tag(event, "policy")? != protocol.policy() {
        return Err(IngestError::Rejected(format!(
            "invalid: Meeting V2 Board command policy must be {}",
            protocol.policy()
        )));
    }
    let action = match require_single_tag(event, "action")?.as_str() {
        "update" => BoardAction::Update(
            buzz_sdk::parse_meeting_v2_board_content(&event.content)
                .map_err(|error| IngestError::Rejected(error.to_string()))?,
        ),
        "unchanged" => {
            require_empty_content(event, "Meeting V2 Board unchanged")?;
            BoardAction::Unchanged
        }
        _ => {
            return Err(IngestError::Rejected(
                "invalid: Meeting V2 Board action must be update or unchanged".into(),
            ));
        }
    };
    let expected_control_epoch = parse_positive_i64_tag(event, "expected-control-epoch")?;
    let board_window = parse_positive_i64_tag(event, "board-window")?;
    let session_id = parse_single_uuid_tag(event, "h", "meeting session id")?;
    Ok((session_id, expected_control_epoch, board_window, action))
}

fn board_success_result(
    event: &Event,
    session_id: Uuid,
    duplicate: bool,
    outcome: &str,
    state_revision: Option<i64>,
    board_event_id: Option<&[u8]>,
    recovery_count: usize,
) -> IngestResult {
    IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!(
            "response:{}",
            serde_json::json!({
                "meeting_id": session_id,
                "outcome": outcome,
                "duplicate": duplicate,
                "state_revision": state_revision,
                "board_event_id": board_event_id.map(hex::encode),
                "recovery_transitions": recovery_count,
            })
        ),
    }
}

fn board_rejection(prefix: &str, code: &str, recovery_count: usize) -> IngestError {
    IngestError::Rejected(format!(
        "{prefix}: {}",
        serde_json::json!({
            "code": code,
            "recovery_transitions": recovery_count,
        })
    ))
}

/// Parse and execute one Grant-bound moderated Meeting canonical speech.
///
/// The ordinary ingest path has already enforced channel token scope,
/// membership, archival state, and message-size limits before reaching here.
pub(crate) async fn handle_speech(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    protocol: MeetingProtocol,
) -> Result<IngestResult, IngestError> {
    let (session_id, command) = parse_speech(event, protocol)?;
    execute(tenant, state, session_id, event, protocol, command).await
}

async fn authorize_participant_command(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    session_id: Uuid,
    auth: &IngestAuth,
) -> Result<MeetingProtocol, IngestError> {
    if auth
        .channel_ids()
        .is_some_and(|ids| !ids.contains(&session_id))
    {
        return Err(IngestError::AuthFailed(
            "restricted: token not authorized for this meeting".into(),
        ));
    }
    let actor_pubkey = auth.pubkey().to_bytes();
    let is_participant = state
        .is_member_cached(tenant.community(), session_id, &actor_pubkey)
        .await
        .map_err(|error| {
            IngestError::Internal(format!(
                "error: checking Meeting V1 participant access: {error}"
            ))
        })?;
    if !is_participant {
        return Err(IngestError::AuthFailed(
            "restricted: not a participant in this meeting".into(),
        ));
    }
    let persisted = buzz_db::meeting::get_meeting_policy(&state.db, tenant.community(), session_id)
        .await
        .map_err(map_meeting_db_error)?;
    let protocol =
        MeetingProtocol::from_persisted(persisted.schema_version, &persisted.floor_policy_version)?;
    if !matches!(
        protocol,
        MeetingProtocol::ModeratedBatonV1
            | MeetingProtocol::ModeratedBoardV2
            | MeetingProtocol::ModeratedBoardActionsV2
    ) {
        return Err(IngestError::Rejected(
            "invalid: moderated Baton command targets an unsupported Session".into(),
        ));
    }
    let restriction = state
        .db
        .moderation_restriction_state(tenant.community(), auth.pubkey().as_bytes())
        .await
        .map_err(|error| {
            IngestError::Internal(format!(
                "error: checking moderated Meeting author restriction state: {error}"
            ))
        })?;
    if restriction.banned {
        return Err(IngestError::AuthFailed(
            "blocked: you are banned from this community".into(),
        ));
    }
    if restriction
        .muted_until
        .is_some_and(|until| until > chrono::Utc::now())
    {
        return Err(IngestError::AuthFailed(
            "restricted: you are timed out from writing".into(),
        ));
    }
    Ok(protocol)
}

async fn execute(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    session_id: Uuid,
    event: &Event,
    protocol: MeetingProtocol,
    command: BatonCommand,
) -> Result<IngestResult, IngestError> {
    let action = command_metric_action(&command);
    let started_at = Instant::now();
    let result = buzz_db::meeting_baton::execute_baton_command(
        &state.db,
        BatonCommandTxParams {
            community_id: tenant.community(),
            session_id,
            event,
            relay_keys: &state.relay_keypair,
            command,
        },
    )
    .await;
    let result = match result {
        Ok(result) => {
            record_command_metrics(
                protocol,
                action,
                classify_command_outcome(&result.command_outcome),
                result.recovery_transitions.len(),
                started_at.elapsed().as_secs_f64(),
            );
            result
        }
        Err(error) => {
            record_command_metrics(
                protocol,
                action,
                CommandMetricOutcome {
                    outcome: "error",
                    duplicate: false,
                },
                0,
                started_at.elapsed().as_secs_f64(),
            );
            return Err(map_baton_db_error(error));
        }
    };

    let recovery_count = result.recovery_transitions.len();
    match result.command_outcome {
        BatonCommandOutcome::Accepted {
            canonical_object_id,
            state_revision,
        } => Ok(success_result(
            event,
            session_id,
            canonical_object_id,
            Some(state_revision),
            recovery_count,
            false,
            "accepted",
        )),
        BatonCommandOutcome::Duplicate {
            accepted: true,
            canonical_object_id,
            state_revision,
            outcome_code,
            ..
        } => Ok(success_result(
            event,
            session_id,
            canonical_object_id,
            state_revision,
            recovery_count,
            true,
            &outcome_code,
        )),
        BatonCommandOutcome::Duplicate {
            accepted: false,
            outcome_class,
            canonical_object_id,
            outcome_code,
            retry_ticket_id,
            ..
        } => Err(command_rejection(
            if outcome_class == "rejected_after_recovery" {
                "expired"
            } else {
                "conflict"
            },
            &outcome_code,
            canonical_object_id.as_deref(),
            retry_ticket_id.as_deref(),
            recovery_count,
        )),
        BatonCommandOutcome::RejectedTerminal {
            code,
            canonical_object_id,
            retry_ticket_id,
        } => Err(command_rejection(
            "conflict",
            &code,
            canonical_object_id.as_deref(),
            retry_ticket_id.as_deref(),
            recovery_count,
        )),
        BatonCommandOutcome::RejectedAfterRecovery {
            code,
            canonical_object_id,
            retry_ticket_id,
        } => Err(command_rejection(
            "expired",
            &code,
            canonical_object_id.as_deref(),
            retry_ticket_id.as_deref(),
            recovery_count,
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommandMetricOutcome {
    outcome: &'static str,
    duplicate: bool,
}

fn command_metric_action(command: &BatonCommand) -> &'static str {
    match command {
        BatonCommand::IntentSubmit { .. } => "intent_submit",
        BatonCommand::IntentRefresh { .. } => "intent_refresh",
        BatonCommand::IntentWithdraw { .. } => "intent_withdraw",
        BatonCommand::ModeratorSelect { .. } => "moderator_select",
        BatonCommand::ModeratorReject { .. } => "moderator_reject",
        BatonCommand::ModeratorDismissHandoff { .. } => "moderator_dismiss_handoff",
        BatonCommand::ModeratorWithdrawSelf { .. } => "moderator_withdraw_self",
        BatonCommand::ModeratorDecisionAttemptStart { .. } => "decision_attempt_start",
        BatonCommand::ModeratorDecisionAttemptFinish { .. } => "decision_attempt_finish",
        BatonCommand::ModeratorDecisionRetry { .. } => "decision_retry",
        BatonCommand::ModeratorCompleteCohort { .. } => "complete_cohort",
        BatonCommand::ModeratorDecisionAttemptAbandon { .. } => "decision_attempt_abandon",
        BatonCommand::ModeratorRecall { .. } => "moderator_recall",
        BatonCommand::HumanRequest => "human_request",
        BatonCommand::HumanWithdraw { .. } => "human_withdraw",
        BatonCommand::OfferAck { .. } => "offer_ack",
        BatonCommand::OfferDecline { .. } => "offer_decline",
        BatonCommand::GrantProgress { .. } => "grant_progress",
        BatonCommand::GrantYield { .. } => "grant_yield",
        BatonCommand::Speech { .. } => "speech",
    }
}

fn classify_command_outcome(outcome: &BatonCommandOutcome) -> CommandMetricOutcome {
    match outcome {
        BatonCommandOutcome::Accepted { .. } => CommandMetricOutcome {
            outcome: "accepted",
            duplicate: false,
        },
        BatonCommandOutcome::Duplicate { accepted: true, .. } => CommandMetricOutcome {
            outcome: "accepted",
            duplicate: true,
        },
        BatonCommandOutcome::Duplicate {
            accepted: false,
            outcome_class,
            ..
        } => CommandMetricOutcome {
            outcome: match outcome_class.as_str() {
                "rejected_terminal" => "rejected_terminal",
                "rejected_after_recovery" => "rejected_after_recovery",
                _ => "unknown",
            },
            duplicate: true,
        },
        BatonCommandOutcome::RejectedTerminal { .. } => CommandMetricOutcome {
            outcome: "rejected_terminal",
            duplicate: false,
        },
        BatonCommandOutcome::RejectedAfterRecovery { .. } => CommandMetricOutcome {
            outcome: "rejected_after_recovery",
            duplicate: false,
        },
    }
}

fn record_command_metrics(
    protocol: MeetingProtocol,
    action: &'static str,
    classified: CommandMetricOutcome,
    recovery_count: usize,
    latency_seconds: f64,
) {
    let duplicate = if classified.duplicate {
        "true"
    } else {
        "false"
    };
    let protocol = match protocol {
        MeetingProtocol::ModeratedBatonV1 => "v1",
        MeetingProtocol::ModeratedBoardV2 => "v2",
        MeetingProtocol::ModeratedBoardActionsV2 => "v2-actions",
        MeetingProtocol::UniformV0 => "v0",
    };
    let labels = [
        ("protocol", protocol),
        ("action", action),
        ("outcome", classified.outcome),
        ("duplicate", duplicate),
    ];
    metrics::counter!("meeting_baton_command_total", &labels).increment(1);
    metrics::histogram!("meeting_baton_command_latency_seconds", &labels).record(latency_seconds);
    metrics::histogram!("meeting_baton_command_recovery_transitions", &labels)
        .record(recovery_count as f64);
    if protocol == "v1" {
        let legacy_labels = [
            ("action", action),
            ("outcome", classified.outcome),
            ("duplicate", duplicate),
        ];
        metrics::counter!("meeting_v1_command_total", &legacy_labels).increment(1);
        metrics::histogram!("meeting_v1_command_latency_seconds", &legacy_labels)
            .record(latency_seconds);
        metrics::histogram!("meeting_v1_command_recovery_transitions", &legacy_labels)
            .record(recovery_count as f64);
    }
}

fn map_baton_db_error(error: DbError) -> IngestError {
    match error {
        DbError::AccessDenied(_) => IngestError::AuthFailed(
            "restricted: not authorized for this moderated Meeting operation".into(),
        ),
        other => map_meeting_db_error(other),
    }
}

fn success_result(
    event: &Event,
    session_id: Uuid,
    canonical_object_id: Option<Vec<u8>>,
    state_revision: Option<i64>,
    recovery_count: usize,
    duplicate: bool,
    outcome_code: &str,
) -> IngestResult {
    IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!(
            "response:{}",
            serde_json::json!({
                "meeting_id": session_id,
                "canonical_object_id": canonical_object_id.as_deref().map(hex::encode),
                "state_revision": state_revision,
                "recovery_transitions": recovery_count,
                "duplicate": duplicate,
                "outcome": outcome_code,
            })
        ),
    }
}

fn command_rejection(
    prefix: &str,
    code: &str,
    canonical_object_id: Option<&[u8]>,
    retry_ticket_id: Option<&[u8]>,
    recovery_count: usize,
) -> IngestError {
    let details = serde_json::json!({
        "code": code,
        "canonical_object_id": canonical_object_id.map(hex::encode),
        "retry_ticket_id": retry_ticket_id.map(hex::encode),
        "recovery_transitions": recovery_count,
    });
    IngestError::Rejected(format!("{prefix}: {details}"))
}

fn parse_control_command(
    event: &Event,
    protocol: MeetingProtocol,
) -> Result<(Uuid, BatonCommand), IngestError> {
    match event.kind.as_u16() as u32 {
        KIND_MEETING_SPEECH_INTENT => parse_intent(event, protocol),
        KIND_MEETING_MODERATOR_COMMAND => parse_moderator_command(event, protocol),
        KIND_MEETING_HUMAN_FLOOR_REQUEST => parse_human_request(event, protocol),
        KIND_MEETING_OFFER_RESPONSE => parse_offer_response(event, protocol),
        KIND_MEETING_GRANT_SIGNAL => parse_grant_signal(event, protocol),
        kind => Err(IngestError::Rejected(format!(
            "invalid: kind {kind} is not a moderated Meeting baton command"
        ))),
    }
}

fn parse_intent(
    event: &Event,
    protocol: MeetingProtocol,
) -> Result<(Uuid, BatonCommand), IngestError> {
    let action = require_single_tag(event, "action")?;
    let command = match action.as_str() {
        "submit" => {
            validate_meeting_tag_schema(
                event,
                &["h", "v", "action", "basis-speech-revision"],
                &["addressed-to"],
                &[],
            )?;
            BatonCommand::IntentSubmit {
                basis_speech_revision: parse_nonnegative_i64_tag(event, "basis-speech-revision")?,
                summary: required_text(&event.content, MAX_INTENT_SUMMARY_BYTES, "Intent summary")?,
                addressed_to: optional_pubkey_tag(event, "addressed-to")?,
            }
        }
        "refresh" => {
            validate_meeting_tag_schema(
                event,
                &[
                    "h",
                    "v",
                    "action",
                    "intent",
                    "prev",
                    "basis-speech-revision",
                ],
                &["addressed-to"],
                &[],
            )?;
            BatonCommand::IntentRefresh {
                intent_id: event_id_tag(event, "intent")?,
                previous_event_id: event_id_tag(event, "prev")?,
                basis_speech_revision: parse_nonnegative_i64_tag(event, "basis-speech-revision")?,
                summary: required_text(&event.content, MAX_INTENT_SUMMARY_BYTES, "Intent summary")?,
                addressed_to: optional_pubkey_tag(event, "addressed-to")?,
            }
        }
        "withdraw" => {
            validate_meeting_tag_schema(event, &["h", "v", "action", "intent", "prev"], &[], &[])?;
            require_empty_content(event, "Intent withdraw")?;
            BatonCommand::IntentWithdraw {
                intent_id: event_id_tag(event, "intent")?,
                previous_event_id: event_id_tag(event, "prev")?,
            }
        }
        _ => {
            return Err(IngestError::Rejected(
                "invalid: Intent action must be submit, refresh, or withdraw".into(),
            ));
        }
    };
    Ok((parse_baton_session(event, protocol)?, command))
}

fn parse_moderator_command(
    event: &Event,
    protocol: MeetingProtocol,
) -> Result<(Uuid, BatonCommand), IngestError> {
    let action = require_single_tag(event, "action")?;
    let command = match action.as_str() {
        "select" => parse_moderator_select(event)?,
        "reject" => {
            validate_meeting_tag_schema(
                event,
                &["h", "v", "action", "intent", "prev", "reason-code", "p"],
                &["decision-attempt"],
                &[],
            )?;
            BatonCommand::ModeratorReject {
                intent_id: event_id_tag(event, "intent")?,
                previous_event_id: event_id_tag(event, "prev")?,
                author_pubkey: pubkey_tag(event, "p")?,
                reason_code: closed_value_tag(
                    event,
                    "reason-code",
                    REJECTION_REASON_CODES,
                    "Intent rejection reason code",
                )?,
                reason_text: required_text(
                    &event.content,
                    MAX_CONTROL_REASON_BYTES,
                    "Intent rejection reason",
                )?,
                attempt_id: optional_event_id_tag(event, "decision-attempt")?,
            }
        }
        "dismiss-handoff" => {
            validate_meeting_tag_schema(
                event,
                &[
                    "h",
                    "v",
                    "action",
                    "handoff",
                    "expected-speech-revision",
                    "expected-handoff-attempt-count",
                    "reason-code",
                ],
                &["decision-attempt"],
                &[],
            )?;
            BatonCommand::ModeratorDismissHandoff {
                handoff_id: event_id_tag(event, "handoff")?,
                expected_speech_revision: parse_nonnegative_i64_tag(
                    event,
                    "expected-speech-revision",
                )?,
                expected_attempt_count: parse_nonnegative_i32_tag(
                    event,
                    "expected-handoff-attempt-count",
                )?,
                reason_code: closed_value_tag(
                    event,
                    "reason-code",
                    HANDOFF_DISMISS_REASON_CODES,
                    "Handoff dismissal reason code",
                )?,
                reason_text: required_text(
                    &event.content,
                    MAX_CONTROL_REASON_BYTES,
                    "Handoff dismissal reason",
                )?,
                attempt_id: optional_event_id_tag(event, "decision-attempt")?,
            }
        }
        "withdraw-self" => {
            validate_meeting_tag_schema(
                event,
                &["h", "v", "action", "decision-attempt", "intent", "prev"],
                &[],
                &[],
            )?;
            require_empty_content(event, "moderator self-Intent withdrawal")?;
            BatonCommand::ModeratorWithdrawSelf {
                attempt_id: event_id_tag(event, "decision-attempt")?,
                intent_id: event_id_tag(event, "intent")?,
                previous_event_id: event_id_tag(event, "prev")?,
            }
        }
        "decision-attempt-start" => {
            validate_meeting_tag_schema(
                event,
                &[
                    "h",
                    "v",
                    "action",
                    "expected-control-epoch",
                    "expected-decision-epoch",
                    "expected-intent-revision",
                    "expected-speech-revision",
                    "expected-state",
                ],
                &["replacement-attempt"],
                &[],
            )?;
            require_empty_content(event, "DecisionAttempt Start")?;
            BatonCommand::ModeratorDecisionAttemptStart {
                expected_control_epoch: parse_positive_i64_tag(event, "expected-control-epoch")?,
                expected_decision_epoch: parse_nonnegative_i64_tag(
                    event,
                    "expected-decision-epoch",
                )?,
                expected_intent_revision: parse_nonnegative_i64_tag(
                    event,
                    "expected-intent-revision",
                )?,
                expected_speech_revision: parse_nonnegative_i64_tag(
                    event,
                    "expected-speech-revision",
                )?,
                expected_state_event_id: event_id_tag(event, "expected-state")?,
                replacement_of_attempt_id: optional_event_id_tag(event, "replacement-attempt")?,
            }
        }
        "decision-attempt-finish" => {
            validate_meeting_tag_schema(
                event,
                &[
                    "h",
                    "v",
                    "action",
                    "decision-attempt",
                    "outcome",
                    "reason-code",
                ],
                &[],
                &[],
            )?;
            require_empty_content(event, "DecisionAttempt Finish")?;
            let outcome = closed_value_tag(
                event,
                "outcome",
                &["completed", "discarded"],
                "DecisionAttempt finish outcome",
            )?;
            let (outcome, reasons) = if outcome == "completed" {
                (
                    BatonDecisionAttemptFinishOutcome::Completed,
                    ATTEMPT_COMPLETED_REASON_CODES,
                )
            } else {
                (
                    BatonDecisionAttemptFinishOutcome::Discarded,
                    ATTEMPT_DISCARDED_REASON_CODES,
                )
            };
            BatonCommand::ModeratorDecisionAttemptFinish {
                attempt_id: event_id_tag(event, "decision-attempt")?,
                outcome,
                reason_code: closed_value_tag(
                    event,
                    "reason-code",
                    reasons,
                    "DecisionAttempt finish reason",
                )?,
            }
        }
        "decision-retry" => {
            validate_meeting_tag_schema(
                event,
                &[
                    "h",
                    "v",
                    "action",
                    "decision-attempt",
                    "retry-ticket",
                    "failed-action",
                    "expected-control-epoch",
                    "expected-decision-epoch",
                    "expected-attempt-number",
                ],
                &[],
                &[],
            )?;
            require_empty_content(event, "DecisionRetry")?;
            BatonCommand::ModeratorDecisionRetry {
                attempt_id: event_id_tag(event, "decision-attempt")?,
                retry_ticket_id: event_id_tag(event, "retry-ticket")?,
                failed_action_event_id: event_id_tag(event, "failed-action")?,
                expected_control_epoch: parse_positive_i64_tag(event, "expected-control-epoch")?,
                expected_decision_epoch: parse_positive_i64_tag(event, "expected-decision-epoch")?,
                expected_attempt_number: parse_positive_i32_tag(event, "expected-attempt-number")?,
            }
        }
        "complete-cohort" => {
            validate_meeting_tag_schema(
                event,
                &[
                    "h",
                    "v",
                    "action",
                    "decision-attempt",
                    "expected-control-epoch",
                    "expected-decision-epoch",
                ],
                &[],
                &[],
            )?;
            require_empty_content(event, "CompleteCohort")?;
            BatonCommand::ModeratorCompleteCohort {
                attempt_id: event_id_tag(event, "decision-attempt")?,
                expected_control_epoch: parse_positive_i64_tag(event, "expected-control-epoch")?,
                expected_decision_epoch: parse_positive_i64_tag(event, "expected-decision-epoch")?,
            }
        }
        "decision-attempt-abandon" => {
            validate_meeting_tag_schema(
                event,
                &["h", "v", "action", "decision-attempt"],
                &[],
                &[],
            )?;
            require_empty_content(event, "DecisionAttempt Abandon")?;
            BatonCommand::ModeratorDecisionAttemptAbandon {
                attempt_id: event_id_tag(event, "decision-attempt")?,
            }
        }
        "recall" => {
            validate_meeting_tag_schema(event, &["h", "v", "action", "control-epoch"], &[], &[])?;
            BatonCommand::ModeratorRecall {
                control_epoch: parse_positive_i64_tag(event, "control-epoch")?,
                reason: optional_text(&event.content, MAX_CONTROL_REASON_BYTES, "Recall reason")?,
            }
        }
        _ => {
            return Err(IngestError::Rejected(
                "invalid: unsupported Meeting V1 moderator action".into(),
            ));
        }
    };
    Ok((parse_baton_session(event, protocol)?, command))
}

fn parse_moderator_select(event: &Event) -> Result<BatonCommand, IngestError> {
    validate_meeting_tag_schema(
        event,
        &[
            "h",
            "v",
            "action",
            "expected-control-epoch",
            "expected-decision-epoch",
            "expected-intent-revision",
            "expected-speech-revision",
        ],
        &[
            "intent",
            "handoff",
            "expected-handoff-attempt-count",
            "decision-attempt",
            "expected-source-event",
        ],
        &[],
    )?;
    let intent_id = optional_event_id_tag(event, "intent")?;
    let handoff_id = optional_event_id_tag(event, "handoff")?;
    let handoff_attempt = optional_nonnegative_i32_tag(event, "expected-handoff-attempt-count")?;
    let source = match (intent_id, handoff_id, handoff_attempt) {
        (Some(intent_id), None, None) => BatonSelectionSource::Intent { intent_id },
        (None, Some(handoff_id), Some(expected_attempt_count)) => BatonSelectionSource::Handoff {
            handoff_id,
            expected_attempt_count,
        },
        _ => {
            return Err(IngestError::Rejected(
                "invalid: Select must reference exactly one Intent, or one Handoff with its expected attempt count"
                    .into(),
            ));
        }
    };
    let content: SelectContent = serde_json::from_str(&event.content).map_err(|error| {
        IngestError::Rejected(format!("invalid: malformed Select content: {error}"))
    })?;
    if matches!(&source, BatonSelectionSource::Handoff { .. }) && !content.deferrals.is_empty() {
        return Err(IngestError::Rejected(
            "invalid: Handoff Select cannot carry Intent deferrals".into(),
        ));
    }
    let selection_reason = content
        .selection_reason
        .map(|reason| required_text(&reason, MAX_SELECTION_REASON_BYTES, "selection reason"))
        .transpose()?;
    if content.deferrals.len() > MAX_MEETING_PARTICIPANTS {
        return Err(IngestError::Rejected(format!(
            "invalid: Select has too many deferrals (max {MAX_MEETING_PARTICIPANTS})"
        )));
    }
    let mut seen = HashSet::with_capacity(content.deferrals.len());
    let mut deferrals = Vec::with_capacity(content.deferrals.len());
    for deferral in content.deferrals {
        let intent_id = decode_event_id(&deferral.intent_id, "deferred Intent id")?;
        if matches!(
            &source,
            BatonSelectionSource::Intent {
                intent_id: selected
            } if selected == &intent_id
        ) {
            return Err(IngestError::Rejected(
                "invalid: selected Intent cannot defer itself".into(),
            ));
        }
        if !seen.insert(intent_id.clone()) {
            return Err(IngestError::Rejected(
                "invalid: Select contains a duplicate deferred Intent".into(),
            ));
        }
        deferrals.push(BatonIntentDeferral {
            intent_id,
            previous_event_id: decode_event_id(&deferral.prev, "deferred Intent current event id")?,
            reason: required_text(
                &deferral.reason,
                MAX_CONTROL_REASON_BYTES,
                "deferral reason",
            )?,
        });
    }
    Ok(BatonCommand::ModeratorSelect {
        source,
        expected_control_epoch: parse_positive_i64_tag(event, "expected-control-epoch")?,
        expected_decision_epoch: parse_nonnegative_i64_tag(event, "expected-decision-epoch")?,
        expected_intent_revision: parse_nonnegative_i64_tag(event, "expected-intent-revision")?,
        expected_speech_revision: parse_nonnegative_i64_tag(event, "expected-speech-revision")?,
        selection_reason,
        deferrals,
        attempt_id: optional_event_id_tag(event, "decision-attempt")?,
        expected_source_event_id: optional_event_id_tag(event, "expected-source-event")?,
    })
}

fn parse_human_request(
    event: &Event,
    protocol: MeetingProtocol,
) -> Result<(Uuid, BatonCommand), IngestError> {
    let action = require_single_tag(event, "action")?;
    let command = match action.as_str() {
        "request" => {
            validate_meeting_tag_schema(event, &["h", "v", "action"], &[], &[])?;
            require_empty_content(event, "Human request")?;
            BatonCommand::HumanRequest
        }
        "withdraw" => {
            validate_meeting_tag_schema(event, &["h", "v", "action", "request"], &[], &[])?;
            require_empty_content(event, "Human request withdrawal")?;
            BatonCommand::HumanWithdraw {
                request_id: event_id_tag(event, "request")?,
            }
        }
        _ => {
            return Err(IngestError::Rejected(
                "invalid: Human floor action must be request or withdraw".into(),
            ));
        }
    };
    Ok((parse_baton_session(event, protocol)?, command))
}

fn parse_offer_response(
    event: &Event,
    protocol: MeetingProtocol,
) -> Result<(Uuid, BatonCommand), IngestError> {
    validate_meeting_tag_schema(event, &["h", "v", "action", "meeting-offer"], &[], &[])?;
    let offer_id = event_id_tag(event, "meeting-offer")?;
    let command = match require_single_tag(event, "action")?.as_str() {
        "ack" => {
            require_empty_content(event, "Offer ACK")?;
            BatonCommand::OfferAck { offer_id }
        }
        "decline" => BatonCommand::OfferDecline {
            offer_id,
            reason: optional_text(
                &event.content,
                MAX_RESPONSE_REASON_BYTES,
                "Offer decline reason",
            )?,
        },
        _ => {
            return Err(IngestError::Rejected(
                "invalid: Offer response action must be ack or decline".into(),
            ));
        }
    };
    Ok((parse_baton_session(event, protocol)?, command))
}

fn parse_grant_signal(
    event: &Event,
    protocol: MeetingProtocol,
) -> Result<(Uuid, BatonCommand), IngestError> {
    let action = require_single_tag(event, "action")?;
    let command = match action.as_str() {
        "progress" => {
            validate_meeting_tag_schema(
                event,
                &["h", "v", "action", "meeting-grant", "progress-seq", "stage"],
                &[],
                &[],
            )?;
            require_empty_content(event, "Grant Progress")?;
            BatonCommand::GrantProgress {
                grant_id: event_id_tag(event, "meeting-grant")?,
                progress_seq: parse_positive_i64_tag(event, "progress-seq")?,
                stage: parse_progress_stage(event)?,
            }
        }
        "yield" => {
            validate_meeting_tag_schema(
                event,
                &["h", "v", "action", "meeting-grant"],
                &["reason-code"],
                &[],
            )?;
            BatonCommand::GrantYield {
                grant_id: event_id_tag(event, "meeting-grant")?,
                reason_code: optional_closed_value_tag(
                    event,
                    "reason-code",
                    YIELD_REASON_CODES,
                    "Yield reason code",
                )?,
                reason: optional_text(&event.content, MAX_RESPONSE_REASON_BYTES, "Yield reason")?,
            }
        }
        _ => {
            return Err(IngestError::Rejected(
                "invalid: Grant signal action must be progress or yield".into(),
            ));
        }
    };
    Ok((parse_baton_session(event, protocol)?, command))
}

fn parse_speech(
    event: &Event,
    protocol: MeetingProtocol,
) -> Result<(Uuid, BatonCommand), IngestError> {
    validate_meeting_tag_schema(
        event,
        &["h", "v", "meeting-grant", "speech-revision"],
        &["handoff-to", "handoff-type", "handoff-reason"],
        &["p"],
    )?;
    validate_speech_mentions(event)?;
    if event.content.trim().is_empty() {
        return Err(IngestError::Rejected(
            "invalid: meeting speech content is required".into(),
        ));
    }
    let handoff_to = optional_pubkey_tag(event, "handoff-to")?;
    let handoff_type = optional_single_tag(event, "handoff-type")?;
    let handoff_reason = optional_single_tag(event, "handoff-reason")?;
    let handoff = match (handoff_to, handoff_type, handoff_reason) {
        (None, None, None) => None,
        (Some(to_pubkey), Some(reason_type), Some(reason_text)) => {
            if !HANDOFF_TYPES.contains(&reason_type.as_str()) {
                return Err(IngestError::Rejected(format!(
                    "invalid: unsupported Handoff type {reason_type}"
                )));
            }
            Some(BatonHandoffInput {
                to_pubkey,
                reason_type,
                reason_text: required_text(
                    &reason_text,
                    MAX_HANDOFF_REASON_BYTES,
                    "Handoff reason",
                )?,
            })
        }
        _ => {
            return Err(IngestError::Rejected(
                "invalid: handoff-to, handoff-type, and handoff-reason must appear together".into(),
            ));
        }
    };
    Ok((
        parse_baton_session(event, protocol)?,
        BatonCommand::Speech {
            grant_id: event_id_tag(event, "meeting-grant")?,
            speech_revision: parse_positive_i64_tag(event, "speech-revision")?,
            handoff,
        },
    ))
}

fn parse_baton_session(event: &Event, protocol: MeetingProtocol) -> Result<Uuid, IngestError> {
    let expected_version = match protocol {
        MeetingProtocol::ModeratedBatonV1 => buzz_sdk::MEETING_V1_SCHEMA_VERSION,
        MeetingProtocol::ModeratedBoardV2 | MeetingProtocol::ModeratedBoardActionsV2 => {
            buzz_sdk::MEETING_V2_SCHEMA_VERSION
        }
        MeetingProtocol::UniformV0 => {
            return Err(IngestError::Rejected(
                "invalid: uniform Meeting does not accept Baton commands".into(),
            ));
        }
    };
    if require_single_tag(event, "v")? != expected_version {
        return Err(IngestError::Rejected(
            "invalid: Meeting Baton command schema does not match the persisted Session".into(),
        ));
    }
    let session_id = parse_single_uuid_tag(event, "h", "meeting session id")?;
    if session_id.is_nil() {
        return Err(IngestError::Rejected(
            "invalid: Meeting Baton session id must not be nil".into(),
        ));
    }
    Ok(session_id)
}

fn validate_speech_mentions(event: &Event) -> Result<(), IngestError> {
    let mut mentions = HashSet::new();
    for tag in event.tags.iter() {
        let parts = tag.as_slice();
        if parts.first().map(String::as_str) != Some("p") {
            continue;
        }
        let Some(value) = parts.get(1) else {
            return Err(IngestError::Rejected(
                "invalid: meeting p tag must contain a participant pubkey".into(),
            ));
        };
        let mention = decode_pubkey(value, "meeting mention")?;
        if !mentions.insert(mention) {
            return Err(IngestError::Rejected(
                "invalid: Meeting V1 speech cannot mention the same participant twice".into(),
            ));
        }
    }
    if mentions.len() > MAX_MEETING_PARTICIPANTS {
        return Err(IngestError::Rejected(format!(
            "invalid: Meeting V1 speech cannot mention more than {MAX_MEETING_PARTICIPANTS} participants"
        )));
    }
    Ok(())
}

fn event_id_tag(event: &Event, tag_name: &str) -> Result<Vec<u8>, IngestError> {
    decode_event_id(
        &require_single_tag(event, tag_name)?,
        &format!("{tag_name} event id"),
    )
}

fn optional_event_id_tag(event: &Event, tag_name: &str) -> Result<Option<Vec<u8>>, IngestError> {
    optional_single_tag(event, tag_name)?
        .map(|value| decode_event_id(&value, &format!("{tag_name} event id")))
        .transpose()
}

fn pubkey_tag(event: &Event, tag_name: &str) -> Result<Vec<u8>, IngestError> {
    decode_pubkey(&require_single_tag(event, tag_name)?, tag_name)
}

fn optional_pubkey_tag(event: &Event, tag_name: &str) -> Result<Option<Vec<u8>>, IngestError> {
    optional_single_tag(event, tag_name)?
        .map(|value| decode_pubkey(&value, tag_name))
        .transpose()
}

fn decode_pubkey(value: &str, field_name: &str) -> Result<Vec<u8>, IngestError> {
    if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(IngestError::Rejected(format!(
            "invalid: {field_name} must be a 64-character pubkey"
        )));
    }
    hex::decode(value).map_err(|_| IngestError::Rejected(format!("invalid: bad {field_name} hex")))
}

fn parse_nonnegative_i64_tag(event: &Event, tag_name: &str) -> Result<i64, IngestError> {
    let value = require_single_tag(event, tag_name)?;
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(IngestError::Rejected(format!(
            "invalid: {tag_name} must be a non-negative decimal integer"
        )));
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
        .ok_or_else(|| {
            IngestError::Rejected(format!(
                "invalid: {tag_name} must be a non-negative integer"
            ))
        })
}

fn parse_positive_i64_tag(event: &Event, tag_name: &str) -> Result<i64, IngestError> {
    let value = require_single_tag(event, tag_name)?;
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(IngestError::Rejected(format!(
            "invalid: {tag_name} must be a positive decimal integer"
        )));
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            IngestError::Rejected(format!("invalid: {tag_name} must be a positive integer"))
        })
}

fn parse_nonnegative_i32_tag(event: &Event, tag_name: &str) -> Result<i32, IngestError> {
    let value = require_single_tag(event, tag_name)?;
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(IngestError::Rejected(format!(
            "invalid: {tag_name} must be a non-negative decimal integer"
        )));
    }
    value
        .parse::<i32>()
        .ok()
        .filter(|value| *value >= 0)
        .ok_or_else(|| {
            IngestError::Rejected(format!(
                "invalid: {tag_name} must be a non-negative integer"
            ))
        })
}

fn parse_positive_i32_tag(event: &Event, tag_name: &str) -> Result<i32, IngestError> {
    let value = parse_nonnegative_i32_tag(event, tag_name)?;
    if value == 0 {
        return Err(IngestError::Rejected(format!(
            "invalid: {tag_name} must be a positive integer"
        )));
    }
    Ok(value)
}

fn optional_nonnegative_i32_tag(event: &Event, tag_name: &str) -> Result<Option<i32>, IngestError> {
    optional_single_tag(event, tag_name)?
        .map(|value| {
            if !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(IngestError::Rejected(format!(
                    "invalid: {tag_name} must be a non-negative decimal integer"
                )));
            }
            value
                .parse::<i32>()
                .ok()
                .filter(|number| *number >= 0)
                .ok_or_else(|| {
                    IngestError::Rejected(format!(
                        "invalid: {tag_name} must be a non-negative integer"
                    ))
                })
        })
        .transpose()
}

fn parse_progress_stage(event: &Event) -> Result<BatonProgressStage, IngestError> {
    match require_single_tag(event, "stage")?.as_str() {
        "context_sync" => Ok(BatonProgressStage::ContextSync),
        "tool_use" => Ok(BatonProgressStage::ToolUse),
        "generating" => Ok(BatonProgressStage::Generating),
        "composing" => Ok(BatonProgressStage::Composing),
        "submitting" => Ok(BatonProgressStage::Submitting),
        stage => Err(IngestError::Rejected(format!(
            "invalid: unsupported Grant Progress stage {stage}"
        ))),
    }
}

fn closed_value_tag(
    event: &Event,
    tag_name: &str,
    allowed: &[&str],
    field_name: &str,
) -> Result<String, IngestError> {
    let value = require_single_tag(event, tag_name)?;
    if !allowed.contains(&value.as_str()) {
        return Err(IngestError::Rejected(format!(
            "invalid: unsupported {field_name} {value}"
        )));
    }
    Ok(value)
}

fn optional_closed_value_tag(
    event: &Event,
    tag_name: &str,
    allowed: &[&str],
    field_name: &str,
) -> Result<Option<String>, IngestError> {
    optional_single_tag(event, tag_name)?
        .map(|value| {
            if allowed.contains(&value.as_str()) {
                Ok(value)
            } else {
                Err(IngestError::Rejected(format!(
                    "invalid: unsupported {field_name} {value}"
                )))
            }
        })
        .transpose()
}

fn required_text(value: &str, max_bytes: usize, field_name: &str) -> Result<String, IngestError> {
    if value.is_empty() {
        return Err(IngestError::Rejected(format!(
            "invalid: {field_name} is required"
        )));
    }
    validate_text(value, max_bytes, field_name)?;
    Ok(value.to_string())
}

fn optional_text(
    value: &str,
    max_bytes: usize,
    field_name: &str,
) -> Result<Option<String>, IngestError> {
    if value.is_empty() {
        return Ok(None);
    }
    validate_text(value, max_bytes, field_name)?;
    Ok(Some(value.to_string()))
}

fn validate_text(value: &str, max_bytes: usize, field_name: &str) -> Result<(), IngestError> {
    if value.trim() != value {
        return Err(IngestError::Rejected(format!(
            "invalid: {field_name} must not have leading or trailing whitespace"
        )));
    }
    if value.len() > max_bytes {
        return Err(IngestError::Rejected(format!(
            "invalid: {field_name} exceeds {max_bytes} UTF-8 bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(IngestError::Rejected(format!(
            "invalid: {field_name} must not contain control characters"
        )));
    }
    Ok(())
}

fn require_empty_content(event: &Event, field_name: &str) -> Result<(), IngestError> {
    if event.content.is_empty() {
        Ok(())
    } else {
        Err(IngestError::Rejected(format!(
            "invalid: {field_name} content must be empty"
        )))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectContent {
    selection_reason: Option<String>,
    deferrals: Vec<SelectDeferral>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectDeferral {
    intent_id: String,
    prev: String,
    reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    fn signed(kind: u32, content: &str, tags: Vec<Tag>) -> Event {
        EventBuilder::new(
            Kind::Custom(u16::try_from(kind).expect("test kind")),
            content,
        )
        .tags(tags)
        .sign_with_keys(&Keys::generate())
        .expect("sign test event")
    }

    fn common(session_id: Uuid, action: &str) -> Vec<Tag> {
        vec![
            Tag::parse(["h", &session_id.to_string()]).expect("h"),
            Tag::parse(["v", "2"]).expect("v"),
            Tag::parse(["action", action]).expect("action"),
        ]
    }

    #[test]
    fn intent_submit_parses_strict_shape() {
        let session_id = Uuid::new_v4();
        let mut tags = common(session_id, "submit");
        tags.push(Tag::parse(["basis-speech-revision", "0"]).expect("basis"));
        let event = signed(KIND_MEETING_SPEECH_INTENT, "I have evidence", tags);
        let (parsed_session, command) =
            parse_control_command(&event, MeetingProtocol::ModeratedBatonV1).expect("parse");
        assert_eq!(parsed_session, session_id);
        assert!(matches!(
            command,
            BatonCommand::IntentSubmit {
                basis_speech_revision: 0,
                ..
            }
        ));
    }

    #[test]
    fn v2_intent_and_board_actions_parse_with_v2_identity() {
        let session_id = Uuid::new_v4();
        let intent =
            buzz_sdk::build_meeting_v2_intent_submit(buzz_sdk::MeetingV1IntentSubmitParams {
                session_id,
                basis_speech_revision: 0,
                addressed_to: None,
                summary: "I have V2 evidence",
            })
            .expect("build V2 Intent")
            .sign_with_keys(&Keys::generate())
            .expect("sign V2 Intent");
        assert!(matches!(
            parse_control_command(&intent, MeetingProtocol::ModeratedBoardV2),
            Ok((parsed, BatonCommand::IntentSubmit { .. })) if parsed == session_id
        ));
        assert!(parse_control_command(&intent, MeetingProtocol::ModeratedBatonV1).is_err());

        let update =
            buzz_sdk::build_meeting_v2_board_action(buzz_sdk::MeetingV2BoardActionParams {
                session_id,
                expected_control_epoch: 3,
                board_window: 2,
                board: Some("# Goal\nShip safely."),
            })
            .expect("build Board update")
            .sign_with_keys(&Keys::generate())
            .expect("sign Board update");
        assert!(matches!(
            parse_board_action(&update, MeetingProtocol::ModeratedBoardV2),
            Ok((parsed, 3, 2, BoardAction::Update(_))) if parsed == session_id
        ));

        let unchanged =
            buzz_sdk::build_meeting_v2_board_action(buzz_sdk::MeetingV2BoardActionParams {
                session_id,
                expected_control_epoch: 3,
                board_window: 2,
                board: None,
            })
            .expect("build Board unchanged")
            .sign_with_keys(&Keys::generate())
            .expect("sign Board unchanged");
        assert!(matches!(
            parse_board_action(&unchanged, MeetingProtocol::ModeratedBoardV2),
            Ok((parsed, 3, 2, BoardAction::Unchanged)) if parsed == session_id
        ));
    }

    #[test]
    fn v2_action_commands_parse_with_the_actions_policy_only() {
        let session_id = Uuid::new_v4();
        let action_run_id = Uuid::new_v4();
        let state_event_id = "aa".repeat(32);
        let board_event_id = "bb".repeat(32);
        let moderator = Keys::generate();
        let assignee = Keys::generate().public_key().to_hex();

        let board =
            buzz_sdk::build_meeting_v2_actions_board_action(buzz_sdk::MeetingV2BoardActionParams {
                session_id,
                expected_control_epoch: 3,
                board_window: 2,
                board: None,
            })
            .expect("build actions Board command")
            .sign_with_keys(&moderator)
            .expect("sign actions Board command");
        assert!(matches!(
            parse_board_action(&board, MeetingProtocol::ModeratedBoardActionsV2),
            Ok((parsed, 3, 2, BoardAction::Unchanged)) if parsed == session_id
        ));
        assert!(parse_board_action(&board, MeetingProtocol::ModeratedBoardV2).is_err());

        let begin = buzz_sdk::build_meeting_v2_action_begin(buzz_sdk::MeetingV2ActionBeginParams {
            session_id,
            expected_control_epoch: 3,
            board_window: 2,
            expected_state_event_id: &state_event_id,
            board_event_id: &board_event_id,
            expected_decision_attempt_id: None,
        })
        .expect("build action begin")
        .sign_with_keys(&moderator)
        .expect("sign action begin");
        assert!(matches!(
            parse_action_command(&begin),
            Ok((parsed, buzz_db::meeting_v2_actions::ActionCommand::Begin {
                expected_control_epoch: 3,
                board_window: 2,
                expected_decision_attempt_id: None,
                ..
            })) if parsed == session_id
        ));

        let action_id = Uuid::new_v4();
        let plan = buzz_sdk::MeetingV2ActionPlan {
            version: buzz_sdk::MEETING_V2_ACTION_PLAN_VERSION,
            action_run_id,
            board_event_id,
            items: vec![buzz_sdk::MeetingV2ActionItem {
                action_id,
                summary: "Implement the accepted design".to_string(),
                assignee_pubkey: assignee,
            }],
            steps: vec![buzz_sdk::MeetingV2ActionStep {
                step_id: Uuid::new_v4(),
                action_id: Some(action_id),
                kind: buzz_sdk::MeetingV2ActionStepKind::ProjectViewCreateWork,
                target_object_id: Uuid::new_v4(),
                payload: serde_json::json!({"title": "Implement"}),
            }],
        };
        let plan_event =
            buzz_sdk::build_meeting_v2_action_plan(buzz_sdk::MeetingV2ActionPlanParams {
                session_id,
                fence: buzz_sdk::MeetingV2ActionRunFence {
                    action_run_id,
                    action_window: 1,
                    plan_event_id: None,
                },
                plan: &plan,
            })
            .expect("build action plan")
            .sign_with_keys(&moderator)
            .expect("sign action plan");
        assert!(matches!(
            parse_action_command(&plan_event),
            Ok((parsed, buzz_db::meeting_v2_actions::ActionCommand::Plan {
                fence,
                plan: parsed_plan,
            })) if parsed == session_id
                && fence.action_run_id == action_run_id
                && fence.action_window_epoch == 1
                && fence.plan_event_id.is_none()
                && parsed_plan == plan
        ));

        let project_event = buzz_sdk::project_view_v2::build_project_object_command(
            buzz_project_view::v2::ProjectObjectCommand::new(
                7,
                None,
                buzz_project_view::MutationRequest::Create(buzz_project_view::CreateMutation {
                    object: buzz_project_view::NewProjectViewObject::Work {
                        id: plan.steps[0].target_object_id,
                        title: "Implement".to_string(),
                        description: "Apply the frozen Meeting plan".to_string(),
                        status: buzz_project_view::WorkStatus::Pending,
                        priority: buzz_project_view::Priority::Normal,
                        handles: buzz_project_view::ObjectRef {
                            object_type: buzz_project_view::ProjectViewObjectType::Requirement,
                            object_id: Uuid::new_v4(),
                        },
                    },
                }),
            ),
        )
        .expect("build Project View command")
        .sign_with_keys(&moderator)
        .expect("sign Project View command");
        let project_event_json = serde_json::to_value(&project_event).expect("serialize event");
        let plan_event_id = plan_event.id.to_hex();
        let project_event_id = project_event.id.to_hex();
        let prepared = buzz_sdk::build_meeting_v2_action_step_prepared(
            buzz_sdk::MeetingV2ActionStepPreparedParams {
                session_id,
                fence: buzz_sdk::MeetingV2ActionRunFence {
                    action_run_id,
                    action_window: 1,
                    plan_event_id: Some(&plan_event_id),
                },
                step_id: plan.steps[0].step_id,
                attempt: 1,
                project_event_id: &project_event_id,
                expected_project_revision: 7,
                signed_project_event: &project_event_json,
            },
        )
        .expect("build prepared step")
        .sign_with_keys(&moderator)
        .expect("sign prepared step");
        assert!(matches!(
            parse_action_command(&prepared),
            Ok((parsed, buzz_db::meeting_v2_actions::ActionCommand::StepPrepared {
                fence,
                step_id,
                attempt: 1,
                expected_project_revision: 7,
                signed_project_event,
                ..
            })) if parsed == session_id
                && fence.action_run_id == action_run_id
                && fence.plan_event_id.as_deref() == Some(plan_event.id.as_bytes())
                && step_id == plan.steps[0].step_id
                && signed_project_event == project_event
        ));

        let applied = buzz_sdk::build_meeting_v2_action_step_applied(
            buzz_sdk::MeetingV2ActionStepAppliedParams {
                session_id,
                fence: buzz_sdk::MeetingV2ActionRunFence {
                    action_run_id,
                    action_window: 1,
                    plan_event_id: Some(&plan_event_id),
                },
                step_id: plan.steps[0].step_id,
                project_event_id: &project_event_id,
                accepted_project_revision: 8,
            },
        )
        .expect("build applied step")
        .sign_with_keys(&moderator)
        .expect("sign applied step");
        assert!(matches!(
            parse_action_command(&applied),
            Ok((parsed, buzz_db::meeting_v2_actions::ActionCommand::StepApplied {
                step_id,
                accepted_project_revision: 8,
                ..
            })) if parsed == session_id && step_id == plan.steps[0].step_id
        ));
    }

    #[test]
    fn sdk_baton_builders_parse_at_the_relay_boundary() {
        let session_id = Uuid::new_v4();
        let intent = "aa".repeat(32);
        let previous = "ab".repeat(32);
        let handoff = "ac".repeat(32);
        let offer = "ad".repeat(32);
        let grant = "ae".repeat(32);
        let attempt = "af".repeat(32);
        let retry_ticket = "b0".repeat(32);
        let failed_action = "b1".repeat(32);
        let state_event = "b2".repeat(32);
        let participant = "bb".repeat(32);
        let commands = vec![
            buzz_sdk::build_meeting_v1_intent_submit(buzz_sdk::MeetingV1IntentSubmitParams {
                session_id,
                basis_speech_revision: 0,
                addressed_to: Some(&participant),
                summary: "I can explain the risk.",
            }),
            buzz_sdk::build_meeting_v1_intent_refresh(buzz_sdk::MeetingV1IntentRefreshParams {
                session_id,
                intent_id: &intent,
                previous_event_id: &previous,
                basis_speech_revision: 1,
                addressed_to: None,
                summary: "I can explain the updated risk.",
            }),
            buzz_sdk::build_meeting_v1_intent_withdraw(buzz_sdk::MeetingV1IntentWithdrawParams {
                session_id,
                intent_id: &intent,
                previous_event_id: &previous,
            }),
            buzz_sdk::build_meeting_v1_moderator_select(buzz_sdk::MeetingV1ModeratorSelectParams {
                session_id,
                selection: buzz_sdk::MeetingV1Selection::Intent { intent_id: &intent },
                expected_control_epoch: 1,
                expected_decision_epoch: 0,
                expected_intent_revision: 0,
                expected_speech_revision: 0,
                selection_reason: Some("Relevant."),
                deferrals: &[],
                attempt_id: Some(&attempt),
                expected_source_event_id: Some(&previous),
            }),
            buzz_sdk::build_meeting_v1_moderator_select(buzz_sdk::MeetingV1ModeratorSelectParams {
                session_id,
                selection: buzz_sdk::MeetingV1Selection::Handoff {
                    handoff_id: &handoff,
                    expected_attempt_count: 0,
                },
                expected_control_epoch: 1,
                expected_decision_epoch: 0,
                expected_intent_revision: 0,
                expected_speech_revision: 0,
                selection_reason: None,
                deferrals: &[],
                attempt_id: None,
                expected_source_event_id: None,
            }),
            buzz_sdk::build_meeting_v1_moderator_reject(buzz_sdk::MeetingV1ModeratorRejectParams {
                session_id,
                intent_id: &intent,
                previous_event_id: &previous,
                intent_author_pubkey: &participant,
                reason_code: buzz_sdk::MeetingV1IntentRejectionReason::Duplicate,
                reason_text: "Already covered.",
                attempt_id: None,
            }),
            buzz_sdk::build_meeting_v1_moderator_dismiss_handoff(
                buzz_sdk::MeetingV1ModeratorDismissHandoffParams {
                    session_id,
                    handoff_id: &handoff,
                    expected_speech_revision: 0,
                    expected_attempt_count: 0,
                    reason_code: buzz_sdk::MeetingV1HandoffDismissReason::AnsweredElsewhere,
                    reason_text: "Already answered.",
                    attempt_id: None,
                },
            ),
            buzz_sdk::build_meeting_v1_decision_attempt_start(
                buzz_sdk::MeetingV1DecisionAttemptStartParams {
                    session_id,
                    expected_control_epoch: 1,
                    expected_decision_epoch: 1,
                    expected_intent_revision: 2,
                    expected_speech_revision: 0,
                    expected_state_event_id: &state_event,
                    replacement_of_attempt_id: None,
                },
            ),
            buzz_sdk::build_meeting_v1_decision_attempt_finish(
                buzz_sdk::MeetingV1DecisionAttemptFinishParams {
                    session_id,
                    attempt_id: &attempt,
                    outcome: buzz_sdk::MeetingV1DecisionAttemptFinishOutcome::Completed,
                    reason_code: "no_action",
                },
            ),
            buzz_sdk::build_meeting_v1_decision_retry(buzz_sdk::MeetingV1DecisionRetryParams {
                session_id,
                attempt_id: &attempt,
                retry_ticket_id: &retry_ticket,
                failed_action_event_id: &failed_action,
                expected_control_epoch: 1,
                expected_decision_epoch: 1,
                expected_attempt_number: 1,
            }),
            buzz_sdk::build_meeting_v1_complete_cohort(buzz_sdk::MeetingV1CompleteCohortParams {
                session_id,
                attempt_id: &attempt,
                expected_control_epoch: 1,
                expected_decision_epoch: 1,
            }),
            buzz_sdk::build_meeting_v1_decision_attempt_abandon(
                buzz_sdk::MeetingV1DecisionAttemptAbandonParams {
                    session_id,
                    attempt_id: &attempt,
                },
            ),
            buzz_sdk::build_meeting_v1_moderator_withdraw_self(
                buzz_sdk::MeetingV1ModeratorWithdrawSelfParams {
                    session_id,
                    attempt_id: &attempt,
                    intent_id: &intent,
                    previous_event_id: &previous,
                },
            ),
            buzz_sdk::build_meeting_v1_moderator_recall(buzz_sdk::MeetingV1ModeratorRecallParams {
                session_id,
                control_epoch: 1,
                reason: Some("Agenda check."),
            }),
            buzz_sdk::build_meeting_v1_human_floor_request(
                buzz_sdk::MeetingV1HumanFloorRequestParams { session_id },
            ),
            buzz_sdk::build_meeting_v1_human_floor_withdraw(
                buzz_sdk::MeetingV1HumanFloorWithdrawParams {
                    session_id,
                    request_id: &previous,
                },
            ),
            buzz_sdk::build_meeting_v1_offer_ack(buzz_sdk::MeetingV1OfferAckParams {
                session_id,
                offer_id: &offer,
            }),
            buzz_sdk::build_meeting_v1_offer_decline(buzz_sdk::MeetingV1OfferDeclineParams {
                session_id,
                offer_id: &offer,
                reason: Some("Unavailable."),
            }),
            buzz_sdk::build_meeting_v1_grant_progress(buzz_sdk::MeetingV1GrantProgressParams {
                session_id,
                grant_id: &grant,
                progress_seq: 1,
                stage: buzz_sdk::MeetingV1ProgressStage::ToolUse,
            }),
            buzz_sdk::build_meeting_v1_grant_yield(buzz_sdk::MeetingV1GrantYieldParams {
                session_id,
                grant_id: &grant,
                reason_code: Some(buzz_sdk::MeetingV1GrantYieldReason::InsufficientContext),
                reason: Some("Context unavailable."),
            }),
        ];
        let expected_actions = [
            "intent_submit",
            "intent_refresh",
            "intent_withdraw",
            "moderator_select",
            "moderator_select",
            "moderator_reject",
            "moderator_dismiss_handoff",
            "decision_attempt_start",
            "decision_attempt_finish",
            "decision_retry",
            "complete_cohort",
            "decision_attempt_abandon",
            "moderator_withdraw_self",
            "moderator_recall",
            "human_request",
            "human_withdraw",
            "offer_ack",
            "offer_decline",
            "grant_progress",
            "grant_yield",
        ];
        let keys = Keys::generate();
        for (builder, expected_action) in commands.into_iter().zip(expected_actions) {
            let event = builder
                .expect("valid SDK builder")
                .sign_with_keys(&keys)
                .expect("sign SDK event");
            let (_, command) = parse_control_command(&event, MeetingProtocol::ModeratedBatonV1)
                .expect("Relay must accept SDK command");
            assert_eq!(command_metric_action(&command), expected_action);
        }

        let speech = buzz_sdk::build_meeting_v1_speech(buzz_sdk::MeetingV1SpeechParams {
            session_id,
            grant_id: &grant,
            speech_revision: 1,
            content: "The risk is contained.",
            mentions: &[&participant],
            handoff: Some(buzz_sdk::MeetingV1DirectedHandoff {
                target_pubkey: &participant,
                handoff_type: buzz_sdk::MeetingV1HandoffType::Review,
                reason: "Please verify.",
            }),
        })
        .expect("valid SDK speech")
        .sign_with_keys(&keys)
        .expect("sign SDK speech");
        let (_, command) = parse_speech(&speech, MeetingProtocol::ModeratedBatonV1)
            .expect("Relay must accept SDK speech");
        assert_eq!(command_metric_action(&command), "speech");
    }

    #[test]
    fn command_metric_outcomes_are_closed_and_keep_duplicate_separate() {
        let cases = [
            (
                BatonCommandOutcome::Accepted {
                    canonical_object_id: None,
                    state_revision: 1,
                },
                CommandMetricOutcome {
                    outcome: "accepted",
                    duplicate: false,
                },
            ),
            (
                BatonCommandOutcome::Duplicate {
                    accepted: true,
                    outcome_class: "accepted".to_string(),
                    canonical_object_id: None,
                    state_revision: Some(1),
                    outcome_code: "accepted".to_string(),
                    retry_ticket_id: None,
                },
                CommandMetricOutcome {
                    outcome: "accepted",
                    duplicate: true,
                },
            ),
            (
                BatonCommandOutcome::Duplicate {
                    accepted: false,
                    outcome_class: "rejected_terminal".to_string(),
                    canonical_object_id: None,
                    state_revision: None,
                    outcome_code: "conflict".to_string(),
                    retry_ticket_id: None,
                },
                CommandMetricOutcome {
                    outcome: "rejected_terminal",
                    duplicate: true,
                },
            ),
            (
                BatonCommandOutcome::Duplicate {
                    accepted: false,
                    outcome_class: "rejected_after_recovery".to_string(),
                    canonical_object_id: None,
                    state_revision: Some(2),
                    outcome_code: "offer_expired".to_string(),
                    retry_ticket_id: None,
                },
                CommandMetricOutcome {
                    outcome: "rejected_after_recovery",
                    duplicate: true,
                },
            ),
            (
                BatonCommandOutcome::RejectedTerminal {
                    code: "conflict".to_string(),
                    canonical_object_id: None,
                    retry_ticket_id: None,
                },
                CommandMetricOutcome {
                    outcome: "rejected_terminal",
                    duplicate: false,
                },
            ),
            (
                BatonCommandOutcome::RejectedAfterRecovery {
                    code: "expired".to_string(),
                    canonical_object_id: None,
                    retry_ticket_id: None,
                },
                CommandMetricOutcome {
                    outcome: "rejected_after_recovery",
                    duplicate: false,
                },
            ),
        ];
        for (outcome, expected) in cases {
            assert_eq!(classify_command_outcome(&outcome), expected);
        }

        let private_text = BatonCommandOutcome::Duplicate {
            accepted: false,
            outcome_class: "session-or-error-shaped-private-text".to_string(),
            canonical_object_id: None,
            state_revision: None,
            outcome_code: "reason body".to_string(),
            retry_ticket_id: None,
        };
        assert_eq!(
            classify_command_outcome(&private_text),
            CommandMetricOutcome {
                outcome: "unknown",
                duplicate: true,
            }
        );
    }

    #[test]
    fn select_requires_exactly_one_source_and_deferrals_array() {
        let session_id = Uuid::new_v4();
        let mut tags = common(session_id, "select");
        for (name, value) in [
            ("expected-control-epoch", "0"),
            ("expected-decision-epoch", "0"),
            ("expected-intent-revision", "0"),
            ("expected-speech-revision", "0"),
        ] {
            tags.push(Tag::parse([name, value]).expect("revision"));
        }
        let event = signed(KIND_MEETING_MODERATOR_COMMAND, r#"{"deferrals":[]}"#, tags);
        assert!(parse_control_command(&event, MeetingProtocol::ModeratedBatonV1).is_err());
    }

    #[test]
    fn select_requires_positive_control_epoch() {
        let session_id = Uuid::new_v4();
        let mut tags = common(session_id, "select");
        tags.push(Tag::parse(["intent", &"55".repeat(32)]).expect("intent"));
        for (name, value) in [
            ("expected-control-epoch", "0"),
            ("expected-decision-epoch", "0"),
            ("expected-intent-revision", "0"),
            ("expected-speech-revision", "0"),
        ] {
            tags.push(Tag::parse([name, value]).expect("revision"));
        }
        let event = signed(KIND_MEETING_MODERATOR_COMMAND, r#"{"deferrals":[]}"#, tags);
        assert!(parse_control_command(&event, MeetingProtocol::ModeratedBatonV1).is_err());
    }

    #[test]
    fn select_rejects_deferrals_incompatible_with_its_source() {
        let session_id = Uuid::new_v4();
        let selected = "55".repeat(32);
        let previous = "66".repeat(32);

        let mut intent_tags = common(session_id, "select");
        intent_tags.push(Tag::parse(["intent", &selected]).expect("intent"));
        for (name, value) in [
            ("expected-control-epoch", "1"),
            ("expected-decision-epoch", "0"),
            ("expected-intent-revision", "0"),
            ("expected-speech-revision", "0"),
        ] {
            intent_tags.push(Tag::parse([name, value]).expect("revision"));
        }
        let self_deferral = serde_json::json!({
            "deferrals": [{
                "intent_id": selected,
                "prev": previous,
                "reason": "Later."
            }]
        })
        .to_string();
        let event = signed(KIND_MEETING_MODERATOR_COMMAND, &self_deferral, intent_tags);
        assert!(parse_control_command(&event, MeetingProtocol::ModeratedBatonV1).is_err());

        let mut handoff_tags = common(session_id, "select");
        handoff_tags.extend([
            Tag::parse(["handoff", &"77".repeat(32)]).expect("handoff"),
            Tag::parse(["expected-handoff-attempt-count", "0"]).expect("attempt"),
        ]);
        for (name, value) in [
            ("expected-control-epoch", "1"),
            ("expected-decision-epoch", "0"),
            ("expected-intent-revision", "0"),
            ("expected-speech-revision", "0"),
        ] {
            handoff_tags.push(Tag::parse([name, value]).expect("revision"));
        }
        let handoff_deferral = serde_json::json!({
            "deferrals": [{
                "intent_id": "88".repeat(32),
                "prev": "99".repeat(32),
                "reason": "Later."
            }]
        })
        .to_string();
        let event = signed(
            KIND_MEETING_MODERATOR_COMMAND,
            &handoff_deferral,
            handoff_tags,
        );
        assert!(parse_control_command(&event, MeetingProtocol::ModeratedBatonV1).is_err());
    }

    #[test]
    fn commands_reject_nil_session_ids() {
        let mut tags = common(Uuid::nil(), "submit");
        tags.push(Tag::parse(["basis-speech-revision", "0"]).expect("basis"));
        let event = signed(KIND_MEETING_SPEECH_INTENT, "I have evidence", tags);
        assert!(parse_control_command(&event, MeetingProtocol::ModeratedBatonV1).is_err());
    }

    #[test]
    fn progress_uses_frozen_stage_vocabulary() {
        let session_id = Uuid::new_v4();
        let grant = "11".repeat(32);
        let mut tags = common(session_id, "progress");
        tags.extend([
            Tag::parse(["meeting-grant", &grant]).expect("grant"),
            Tag::parse(["progress-seq", "1"]).expect("sequence"),
            Tag::parse(["stage", "planning"]).expect("old stage"),
        ]);
        let event = signed(KIND_MEETING_GRANT_SIGNAL, "", tags);
        assert!(parse_control_command(&event, MeetingProtocol::ModeratedBatonV1).is_err());
    }

    #[test]
    fn directed_handoff_is_all_or_none() {
        let session_id = Uuid::new_v4();
        let event = signed(
            9,
            "Can you verify this?",
            vec![
                Tag::parse(["h", &session_id.to_string()]).expect("h"),
                Tag::parse(["v", "2"]).expect("v"),
                Tag::parse(["meeting-grant", &"22".repeat(32)]).expect("grant"),
                Tag::parse(["speech-revision", "1"]).expect("revision"),
                Tag::parse(["handoff-to", &"33".repeat(32)]).expect("target"),
            ],
        );
        assert!(parse_speech(&event, MeetingProtocol::ModeratedBatonV1).is_err());
    }

    #[test]
    fn speech_mentions_are_unique_and_bounded_by_the_roster_limit() {
        let session_id = Uuid::new_v4();
        let base_tags = || {
            vec![
                Tag::parse(["h", &session_id.to_string()]).expect("h"),
                Tag::parse(["v", "2"]).expect("v"),
                Tag::parse(["meeting-grant", &"22".repeat(32)]).expect("grant"),
                Tag::parse(["speech-revision", "1"]).expect("revision"),
            ]
        };

        let duplicate = "33".repeat(32);
        let mut duplicate_tags = base_tags();
        duplicate_tags.extend([
            Tag::parse(["p", &duplicate]).expect("first mention"),
            Tag::parse(["p", &duplicate.to_ascii_uppercase()]).expect("duplicate mention"),
        ]);
        let event = signed(9, "Speech", duplicate_tags);
        assert!(parse_speech(&event, MeetingProtocol::ModeratedBatonV1).is_err());

        let mut excessive_tags = base_tags();
        for index in 1..=(MAX_MEETING_PARTICIPANTS + 1) {
            let mention = format!("{index:064x}");
            excessive_tags.push(Tag::parse(["p", mention.as_str()]).expect("mention"));
        }
        let event = signed(9, "Speech", excessive_tags);
        assert!(parse_speech(&event, MeetingProtocol::ModeratedBatonV1).is_err());
    }

    #[test]
    fn yield_reason_code_is_closed() {
        let session_id = Uuid::new_v4();
        let mut tags = common(session_id, "yield");
        tags.extend([
            Tag::parse(["meeting-grant", &"44".repeat(32)]).expect("grant"),
            Tag::parse(["reason-code", "anything"]).expect("reason code"),
        ]);
        let event = signed(KIND_MEETING_GRANT_SIGNAL, "", tags);
        assert!(parse_control_command(&event, MeetingProtocol::ModeratedBatonV1).is_err());
    }
}
