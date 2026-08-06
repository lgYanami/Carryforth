//! Strict parsers for signed Meeting Create and Relay-authored projections.

use super::*;

pub(super) fn parse_create(
    event: &Event,
    meeting_id: &str,
) -> Result<Option<CreateProjection>, MeetingReadError> {
    if event.kind.as_u16() as u32 != KIND_MEETING_CREATE
        || single_tag(event, "h") != Some(meeting_id)
    {
        return Ok(None);
    }
    verify_event(event, "Meeting Create")?;
    let version = required_tag(event, "v", "Meeting Create")?;
    let policy = required_tag(event, "policy", "Meeting Create")?;
    if version != buzz_sdk_pkg::MEETING_V2_SCHEMA_VERSION
        || !matches!(
            policy,
            buzz_sdk_pkg::MEETING_V2_POLICY
                | buzz_sdk_pkg::MEETING_V2_ACTIONS_V2_POLICY
                | buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY
        )
    {
        return Ok(None);
    }
    let title = required_tag(event, "name", "Meeting Create")?.trim();
    if title.is_empty() {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting Create has an empty title",
        )));
    }
    let host_pubkey = event.pubkey.to_hex();
    let mut participants = BTreeSet::from([host_pubkey.clone()]);
    for tag in tags_named(event, "p") {
        let Some(pubkey) = tag.get(1) else {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting Create has an invalid participant tag",
            )));
        };
        require_hex64(pubkey, "Meeting participant").map_err(MeetingReadError::Other)?;
        if !participants.insert(pubkey.clone()) {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting Create has a duplicate participant",
            )));
        }
    }
    if participants.len() < 2 || participants.len() > 12 {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting Create roster is outside the supported bounds",
        )));
    }
    let initial_board = buzz_sdk_pkg::parse_meeting_v2_board_content(&event.content)
        .map_err(|error| MeetingReadError::Other(integrity_error(error.to_string())))?;
    let source_channel_id = optional_tag(event, "source", "Meeting Create")?
        .map(|source| {
            let parsed = Uuid::parse_str(source).map_err(|_| {
                MeetingReadError::Other(integrity_error(
                    "Meeting Create source is not a canonical Channel UUID",
                ))
            })?;
            if parsed.is_nil() || parsed.to_string() != source || source == meeting_id {
                return Err(MeetingReadError::Other(integrity_error(
                    "Meeting Create source is not a distinct canonical Channel UUID",
                )));
            }
            Ok(source.to_string())
        })
        .transpose()?;
    Ok(Some(CreateProjection {
        meeting_id: meeting_id.to_string(),
        title: title.to_string(),
        description: optional_tag(event, "about", "Meeting Create")?.map(str::to_string),
        source_channel_id,
        policy: policy.to_string(),
        host_pubkey,
        participant_pubkeys: participants,
        event_id: event.id.to_hex(),
        created_at: event.created_at.as_secs(),
        initial_board,
    }))
}

pub(super) fn parse_state(
    event: &Event,
    identity: &MeetingIdentity,
    create: &CreateProjection,
) -> Result<Option<StateProjection>, MeetingReadError> {
    if event.kind.as_u16() as u32 != KIND_MEETING_STATE
        || single_tag(event, "h") != Some(create.meeting_id.as_str())
    {
        return Ok(None);
    }
    verify_relay_event(event, identity, "Meeting State")?;
    if required_tag(event, "v", "Meeting State")? != buzz_sdk_pkg::MEETING_V2_SCHEMA_VERSION
        || required_tag(event, "policy", "Meeting State")? != create.policy
        || required_tag(event, "moderator", "Meeting State")? != create.host_pubkey
    {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting State protocol does not match Create",
        )));
    }
    let state: StateWire = serde_json::from_str(&event.content).map_err(|error| {
        MeetingReadError::Other(integrity_error(format!(
            "invalid Meeting State content: {error}"
        )))
    })?;
    let tag_state_revision = parse_revision_tag(event, "state-revision", "Meeting State")?;
    let tag_floor_revision = parse_revision_tag(event, "floor-revision", "Meeting State")?;
    let tag_intent_revision = parse_revision_tag(event, "intent-revision", "Meeting State")?;
    let tag_speech_revision = parse_revision_tag(event, "speech-revision", "Meeting State")?;
    if required_tag(event, "phase", "Meeting State")? != state.phase
        || state.moderator_pubkey != create.host_pubkey
        || state.state_revision != tag_state_revision
        || state.floor_revision != tag_floor_revision
        || state.intent_revision != tag_intent_revision
        || state.speech_revision != tag_speech_revision
    {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting State content does not match its signed tags",
        )));
    }
    if !matches!(
        state.phase.as_str(),
        "moderator_idle" | "moderator_control" | "offered" | "granted" | "ended"
    ) {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting State has an unsupported phase",
        )));
    }
    validate_board_control(state.board_control.as_ref(), &state, create)?;
    validate_floor_state(&state, create)?;
    validate_host_projection(&state, create)?;
    Ok(Some(StateProjection {
        event_id: event.id.to_hex(),
        created_at: event.created_at.as_secs(),
        state,
    }))
}

fn validate_floor_state(
    state: &StateWire,
    create: &CreateProjection,
) -> Result<(), MeetingReadError> {
    let participant_types = state
        .participants
        .iter()
        .map(|participant| (participant.pubkey.as_str(), participant.participant_type))
        .collect::<BTreeMap<_, _>>();
    let mut request_ids = BTreeSet::new();
    let mut requesters = BTreeSet::new();
    let mut positions = BTreeSet::new();
    for request in &state.human_queue {
        require_hex64(&request.request_id, "Meeting Human Floor Request")
            .map_err(MeetingReadError::Other)?;
        require_hex64(&request.requester_pubkey, "Meeting Human Floor requester")
            .map_err(MeetingReadError::Other)?;
        if request.queue_position <= 0
            || !matches!(request.state.as_str(), "queued" | "offered")
            || participant_types.get(request.requester_pubkey.as_str())
                != Some(&FrozenParticipantType::Human)
            || request.requester_pubkey == create.host_pubkey
            || !request_ids.insert(request.request_id.as_str())
            || !requesters.insert(request.requester_pubkey.as_str())
            || !positions.insert(request.queue_position)
        {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting State has an invalid Human Floor queue",
            )));
        }
    }

    match state.phase.as_str() {
        "offered" if state.offer.is_some() && state.grant.is_none() => {}
        "granted" if state.grant.is_some() && state.offer.is_none() => {}
        "moderator_idle" | "moderator_control" | "ended"
            if state.offer.is_none() && state.grant.is_none() => {}
        _ => {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting State phase does not match its active Offer or Grant",
            )));
        }
    }

    if let Some(offer) = &state.offer {
        validate_floor_id(&offer.offer_id, "Meeting Offer")?;
        validate_floor_actor(
            &offer.target_pubkey,
            offer.target_participant_type,
            &participant_types,
            "Meeting Offer target",
        )?;
        validate_allocation(&offer.allocation_source, &offer.turn_role)?;
        validate_source_ids([
            offer.source_intent_id.as_deref(),
            offer.source_request_id.as_deref(),
            offer.source_handoff_id.as_deref(),
            offer.source_speech_event_id.as_deref(),
        ])?;
        validate_handoff(offer.handoff_context.as_ref(), &participant_types)?;
        if offer.basis_speech_revision != state.speech_revision
            || offer.created_at_ms < 0
            || offer.ack_deadline_ms < offer.created_at_ms
        {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting Offer has an invalid revision or deadline",
            )));
        }
    }
    if let Some(grant) = &state.grant {
        validate_floor_id(&grant.grant_id, "Meeting Grant")?;
        validate_floor_actor(
            &grant.holder_pubkey,
            *participant_types
                .get(grant.holder_pubkey.as_str())
                .ok_or_else(|| {
                    MeetingReadError::Other(integrity_error(
                        "Meeting Grant holder is outside the frozen roster",
                    ))
                })?,
            &participant_types,
            "Meeting Grant holder",
        )?;
        validate_allocation(&grant.allocation_source, &grant.turn_role)?;
        validate_floor_id(&grant.source_offer_id, "Meeting Grant source Offer")?;
        validate_source_ids([
            grant.source_intent_id.as_deref(),
            grant.source_request_id.as_deref(),
            grant.source_handoff_id.as_deref(),
            grant.source_speech_event_id.as_deref(),
        ])?;
        validate_handoff(grant.handoff_context.as_ref(), &participant_types)?;
        if grant.basis_speech_revision != state.speech_revision
            || grant.created_at_ms < 0
            || grant.soft_lease_expires_at_ms < grant.created_at_ms
            || grant.hard_deadline_ms < grant.soft_lease_expires_at_ms
            || grant.progress_seq < 0
        {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting Grant has an invalid revision, deadline, or progress sequence",
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_host_projection(
    state: &StateWire,
    create: &CreateProjection,
) -> Result<(), MeetingReadError> {
    if state.control_epoch == 0
        || u32::try_from(state.consecutive_moderator_speeches).is_err()
        || state
            .moderator_decision_deadline_ms
            .is_some_and(|deadline| deadline < 0)
        || state.next_action_at_ms.is_some_and(|deadline| deadline < 0)
    {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting State has invalid moderator control metadata",
        )));
    }
    let roster = state
        .participants
        .iter()
        .map(|participant| participant.pubkey.as_str())
        .collect::<BTreeSet<_>>();
    let mut intent_ids = BTreeSet::new();
    let mut intent_events = BTreeSet::new();
    let mut intent_authors = BTreeSet::new();
    for intent in &state.pending_intents {
        require_hex64(&intent.intent_id, "Meeting Intent").map_err(MeetingReadError::Other)?;
        require_hex64(&intent.current_event_id, "Meeting Intent event")
            .map_err(MeetingReadError::Other)?;
        require_hex64(&intent.author_pubkey, "Meeting Intent author")
            .map_err(MeetingReadError::Other)?;
        if let Some(addressed_to) = &intent.addressed_to {
            require_hex64(addressed_to, "Meeting Intent addressee")
                .map_err(MeetingReadError::Other)?;
            if !roster.contains(addressed_to.as_str()) {
                return Err(MeetingReadError::Other(integrity_error(
                    "Meeting Intent addressee is outside the frozen roster",
                )));
            }
            if addressed_to == &intent.author_pubkey {
                return Err(MeetingReadError::Other(integrity_error(
                    "Meeting Intent cannot address its own author",
                )));
            }
        }
        if let Some(last_offer_id) = &intent.last_offer_id {
            require_hex64(last_offer_id, "Meeting Intent last Offer")
                .map_err(MeetingReadError::Other)?;
        }
        if !roster.contains(intent.author_pubkey.as_str())
            || intent.basis_speech_revision > state.speech_revision
            || intent.summary.trim().is_empty()
            || intent.summary.trim() != intent.summary
            || intent.summary.len() > 512
            || intent.summary.chars().any(char::is_control)
            || intent.created_at_ms < 0
            || u32::try_from(intent.selection_attempt_count).is_err()
            || !intent_ids.insert(intent.intent_id.as_str())
            || !intent_events.insert(intent.current_event_id.as_str())
            || !intent_authors.insert(intent.author_pubkey.as_str())
        {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting State has an invalid pending Intent pool",
            )));
        }
    }

    let active_handoff_id = state
        .offer
        .as_ref()
        .and_then(|offer| offer.source_handoff_id.as_deref())
        .or_else(|| {
            state
                .grant
                .as_ref()
                .and_then(|grant| grant.source_handoff_id.as_deref())
        });
    let mut handoff_ids = BTreeSet::new();
    for handoff in &state.unresolved_handoffs {
        for (value, context) in [
            (&handoff.handoff_id, "Meeting Handoff"),
            (
                &handoff.source_speech_event_id,
                "Meeting Handoff source Speech",
            ),
            (&handoff.from_pubkey, "Meeting Handoff source participant"),
            (&handoff.to_pubkey, "Meeting Handoff target participant"),
        ] {
            require_hex64(value, context).map_err(MeetingReadError::Other)?;
        }
        for value in [
            handoff.last_offer_id.as_ref(),
            handoff.last_grant_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            require_hex64(value, "Meeting Handoff attempt").map_err(MeetingReadError::Other)?;
        }
        if handoff.question_state != "open"
            || !roster.contains(handoff.from_pubkey.as_str())
            || !roster.contains(handoff.to_pubkey.as_str())
            || handoff.from_pubkey == handoff.to_pubkey
            || !matches!(
                handoff.reason_type.as_str(),
                "question"
                    | "information_request"
                    | "clarification"
                    | "review"
                    | "response_requested"
            )
            || handoff.reason_text.trim().is_empty()
            || handoff.reason_text.trim() != handoff.reason_text
            || handoff.reason_text.len() > 1024
            || handoff.reason_text.chars().any(char::is_control)
            || handoff.created_at_ms < 0
            || u32::try_from(handoff.attempt_count).is_err()
            || !handoff_ids.insert(handoff.handoff_id.as_str())
        {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting State has an invalid open Handoff pool",
            )));
        }
        if active_handoff_id == Some(handoff.handoff_id.as_str())
            && handoff.last_offer_id.is_none()
            && handoff.last_grant_id.is_none()
        {
            return Err(MeetingReadError::Other(integrity_error(
                "active Meeting Handoff has no attempt projection",
            )));
        }
    }
    if state
        .pending_intents
        .iter()
        .any(|intent| intent.author_pubkey == create.host_pubkey)
        && state
            .pending_intents
            .iter()
            .filter(|intent| intent.author_pubkey == create.host_pubkey)
            .count()
            > 1
    {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting State has multiple moderator self Intents",
        )));
    }
    Ok(())
}

fn validate_floor_id(value: &str, context: &str) -> Result<(), MeetingReadError> {
    require_hex64(value, context).map_err(MeetingReadError::Other)
}

fn validate_floor_actor(
    pubkey: &str,
    participant_type: FrozenParticipantType,
    participant_types: &BTreeMap<&str, FrozenParticipantType>,
    context: &str,
) -> Result<(), MeetingReadError> {
    require_hex64(pubkey, context).map_err(MeetingReadError::Other)?;
    if participant_types.get(pubkey) != Some(&participant_type) {
        return Err(MeetingReadError::Other(integrity_error(format!(
            "{context} does not match the frozen roster"
        ))));
    }
    Ok(())
}

fn validate_allocation(source: &str, turn_role: &str) -> Result<(), MeetingReadError> {
    if !matches!(
        source,
        "human_request" | "fallback" | "moderator_select" | "directed_handoff"
    ) || !matches!(turn_role, "participant" | "moderator_self")
    {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting Floor allocation is unsupported",
        )));
    }
    Ok(())
}

fn validate_source_ids<'a>(
    values: impl IntoIterator<Item = Option<&'a str>>,
) -> Result<(), MeetingReadError> {
    for value in values.into_iter().flatten() {
        validate_floor_id(value, "Meeting Floor source event")?;
    }
    Ok(())
}

fn validate_handoff(
    context: Option<&HandoffContextWire>,
    participant_types: &BTreeMap<&str, FrozenParticipantType>,
) -> Result<(), MeetingReadError> {
    let Some(context) = context else {
        return Ok(());
    };
    require_hex64(&context.from_pubkey, "Meeting Handoff source")
        .map_err(MeetingReadError::Other)?;
    if !participant_types.contains_key(context.from_pubkey.as_str())
        || !matches!(
            context.reason_type.as_str(),
            "question" | "information_request" | "clarification" | "review" | "response_requested"
        )
        || context.reason_text.trim().is_empty()
        || context.reason_text.trim() != context.reason_text
        || context.reason_text.len() > 1024
        || context.reason_text.chars().any(char::is_control)
    {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting Handoff context is invalid",
        )));
    }
    Ok(())
}

fn validate_board_control(
    control: Option<&BoardControlWire>,
    state: &StateWire,
    create: &CreateProjection,
) -> Result<(), MeetingReadError> {
    let Some(control) = control else {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting V2 State has no Board control projection",
        )));
    };
    if !matches!(
        control.phase.as_str(),
        "bootstrap_locked" | "board_pending" | "floor_ready" | "finalizing_actions" | "ended"
    ) {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting State has an unsupported Board phase",
        )));
    }
    if control.control_epoch == 0
        || control.control_epoch != state.control_epoch
        || (control.phase != "bootstrap_locked" && control.board_window == 0)
        || control
            .board_started_at_ms
            .is_some_and(|timestamp| timestamp < 0)
        || control
            .board_deadline_at_ms
            .is_some_and(|timestamp| timestamp < 0)
        || control
            .board_completed_at_ms
            .is_some_and(|timestamp| timestamp < 0)
        || control.board_outcome.as_deref().is_some_and(|outcome| {
            !matches!(outcome, "updated" | "unchanged" | "timed_out" | "preempted")
        })
    {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting State has invalid Board control fences",
        )));
    }
    match control.phase.as_str() {
        "board_pending"
            if control.board_started_at_ms.is_some()
                && control.board_deadline_at_ms.is_some()
                && control.board_completed_at_ms.is_none()
                && control.board_outcome.is_none() => {}
        "floor_ready" | "finalizing_actions" | "ended"
            if control.board_completed_at_ms.is_some() && control.board_outcome.is_some() => {}
        "bootstrap_locked" => {}
        _ => {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting State Board phase does not match its window outcome",
            )));
        }
    }
    if !matches!(
        create.policy.as_str(),
        buzz_sdk_pkg::MEETING_V2_ACTIONS_V2_POLICY | buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY
    ) && control.action.is_some()
    {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting State exposes actions for a non-action policy",
        )));
    }
    if control.phase == "finalizing_actions" && control.action.is_none() {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting State is finalizing actions without an action run",
        )));
    }
    let Some(action) = &control.action else {
        return Ok(());
    };
    if action.action_run_id.is_nil() {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting action run ID is nil",
        )));
    }
    if action.action_window_epoch == 0 {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting action window is zero",
        )));
    }
    require_hex64(&action.board_event_id, "Meeting action Board event")
        .map_err(MeetingReadError::Other)?;
    if !matches!(action.condition.as_str(), "runnable" | "blocked") {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting action run has an unsupported condition",
        )));
    }
    if action.terminal_status.as_deref().is_some_and(|status| {
        !matches!(
            status,
            "returned_to_board" | "completed_closed" | "completed_aborted"
        )
    }) {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting action run has an unsupported terminal status",
        )));
    }
    if let Some(completion_event_id) = &action.completion_event_id {
        require_hex64(completion_event_id, "Meeting action completion event")
            .map_err(MeetingReadError::Other)?;
    }
    if action
        .action_deadline_at_ms
        .is_some_and(|deadline| deadline < 0)
    {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting action deadline is negative",
        )));
    }
    if action.progress_seq < 0
        || action
            .last_progress_at_ms
            .is_some_and(|timestamp| timestamp < 0)
        || action
            .operator_hard_deadline_ms
            .is_some_and(|timestamp| timestamp < 0)
        || action.created_at_ms.is_some_and(|timestamp| timestamp < 0)
        || action.last_progress_stage.as_deref().is_some_and(|stage| {
            !matches!(
                stage,
                "reasoning" | "tool_call" | "tool_result" | "finalizing" | "waiting_human"
            )
        })
        || (action.progress_seq == 0
            && (action.last_progress_stage.is_some() || action.last_progress_at_ms.is_some()))
        || (action.progress_seq > 0
            && (action.last_progress_stage.is_none() || action.last_progress_at_ms.is_none()))
        || action
            .operator_hard_deadline_ms
            .zip(action.action_deadline_at_ms)
            .is_some_and(|(operator, lease)| lease > operator)
    {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting action progress or renewable deadline metadata is invalid",
        )));
    }
    if action.last_error_code.as_deref().is_some_and(|code| {
        !matches!(
            code,
            "external_operation_failed"
                | "external_state_conflict"
                | "tool_unavailable"
                | "provider_failure"
                | "affinity_lost"
                | "action_deadline_exceeded"
                | "action_lease_expired"
                | "action_operator_deadline_exceeded"
        )
    }) {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting action run has an unsupported block reason",
        )));
    }
    match action.terminal_status.as_deref() {
        None if control.phase == "finalizing_actions"
            && action.completion_event_id.is_none()
            && ((action.condition == "runnable"
                && action.action_deadline_at_ms.is_some()
                && action.last_error_code.is_none())
                || (action.condition == "blocked"
                    && action.action_deadline_at_ms.is_none()
                    && action.last_error_code.is_some())) => {}
        Some("returned_to_board")
            if control.phase != "finalizing_actions"
                && action.completion_event_id.is_none()
                && action.action_deadline_at_ms.is_none() => {}
        Some("completed_closed")
            if control.phase == "ended"
                && action.completion_event_id.is_some()
                && action.action_deadline_at_ms.is_none() => {}
        Some("completed_aborted")
            if control.phase == "ended"
                && action.completion_event_id.is_none()
                && action.action_deadline_at_ms.is_none() => {}
        _ => {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting action condition does not match its lifecycle phase",
            )));
        }
    }
    Ok(())
}

pub(super) fn select_current_state(
    mut states: Vec<StateProjection>,
    create: &CreateProjection,
) -> Result<Option<StateProjection>, MeetingReadError> {
    states.sort_by(|left, right| {
        left.state
            .state_revision
            .cmp(&right.state.state_revision)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });

    let mut previous_revisions: Option<(u64, u64, u64, u64)> = None;
    let mut frozen_participants: Option<Vec<MeetingParticipant>> = None;
    let mut previous_state_revision: Option<u64> = None;
    let mut previous_event_id: Option<&str> = None;
    for state in &states {
        if previous_state_revision == Some(state.state.state_revision)
            && previous_event_id != Some(state.event_id.as_str())
        {
            return Err(MeetingReadError::Other(integrity_error(
                "conflicting Relay State events share a state revision",
            )));
        }

        let revisions = (
            state.state.state_revision,
            state.state.floor_revision,
            state.state.intent_revision,
            state.state.speech_revision,
        );
        if let Some(previous) = previous_revisions {
            if revisions.0 < previous.0
                || revisions.1 < previous.1
                || revisions.2 < previous.2
                || revisions.3 < previous.3
            {
                return Err(MeetingReadError::Other(integrity_error(
                    "Meeting State revisions moved backwards",
                )));
            }
        }

        let participants = validate_participants(&state.state, create)?;
        if frozen_participants
            .as_ref()
            .is_some_and(|frozen| frozen != &participants)
        {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting State changed a frozen participant classification",
            )));
        }
        frozen_participants = Some(participants);
        previous_revisions = Some(revisions);
        previous_state_revision = Some(state.state.state_revision);
        previous_event_id = Some(state.event_id.as_str());
    }

    Ok(states.pop())
}

pub(super) fn parse_current_board(
    events: &[Event],
    identity: &MeetingIdentity,
    create: &CreateProjection,
) -> Result<MeetingBoard, MeetingReadError> {
    let mut boards = Vec::new();
    for event in events {
        if event.kind.as_u16() as u32 != KIND_MEETING_BOARD
            || single_tag(event, "h") != Some(create.meeting_id.as_str())
        {
            continue;
        }
        verify_relay_event(event, identity, "Meeting Board")?;
        if required_tag(event, "v", "Meeting Board")? != buzz_sdk_pkg::MEETING_V2_SCHEMA_VERSION
            || required_tag(event, "policy", "Meeting Board")? != create.policy
            || required_tag(event, "format", "Meeting Board")?
                != buzz_sdk_pkg::MEETING_V2_BOARD_FORMAT
            || required_tag(event, "moderator", "Meeting Board")? != create.host_pubkey
        {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting Board protocol does not match Create",
            )));
        }
        let content = buzz_sdk_pkg::parse_meeting_v2_board_content(&event.content)
            .map_err(|error| MeetingReadError::Other(integrity_error(error.to_string())))?;
        boards.push(MeetingBoard {
            event_id: event.id.to_hex(),
            format: content.format,
            body: content.body,
            moderator_pubkey: create.host_pubkey.clone(),
            updated_at: event.created_at.as_secs(),
            source: MeetingBoardSource::Projection,
        });
    }
    boards.sort_by(|left, right| {
        left.updated_at
            .cmp(&right.updated_at)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });
    Ok(boards.pop().unwrap_or_else(|| MeetingBoard {
        event_id: create.event_id.clone(),
        format: create.initial_board.format.clone(),
        body: create.initial_board.body.clone(),
        moderator_pubkey: create.host_pubkey.clone(),
        updated_at: create.created_at,
        source: MeetingBoardSource::Create,
    }))
}

pub(super) fn parse_current_end(
    events: &[Event],
    identity: &MeetingIdentity,
    create: &CreateProjection,
) -> Result<Option<MeetingEndState>, MeetingReadError> {
    let mut ends = Vec::new();
    for event in events {
        if event.kind.as_u16() as u32 != KIND_MEETING_END
            || single_tag(event, "h") != Some(create.meeting_id.as_str())
        {
            continue;
        }
        verify_event(event, "Meeting End")?;
        let outcome = required_tag(event, "outcome", "Meeting End")?;
        if required_tag(event, "v", "Meeting End")? != buzz_sdk_pkg::MEETING_V2_SCHEMA_VERSION
            || required_tag(event, "policy", "Meeting End")? != create.policy
            || required_tag(event, "e", "Meeting End")? != create.event_id
            || !matches!(outcome, "closed" | "aborted")
        {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting End protocol does not match Create",
            )));
        }
        let reason_code = optional_tag(event, "reason-code", "Meeting End")?.map(str::to_string);
        if outcome == "aborted" && reason_code.is_none() {
            return Err(MeetingReadError::Other(integrity_error(
                "aborted Meeting End has no reason code",
            )));
        }
        let attestation = optional_tag(event, "attestation", "Meeting End")?;
        if attestation.is_some_and(|value| value != "actions-recorded") {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting End has an unsupported attestation",
            )));
        }
        let actions_attested = attestation == Some("actions-recorded");
        if actions_attested
            && (outcome != "closed"
                || !matches!(
                    create.policy.as_str(),
                    buzz_sdk_pkg::MEETING_V2_ACTIONS_V2_POLICY
                        | buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY
                ))
        {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting End action attestation does not match its outcome or policy",
            )));
        }
        let signer = event.pubkey.to_hex();
        let termination_source = if signer == create.host_pubkey {
            MeetingTerminationSource::Host
        } else if event.pubkey == identity.relay_pubkey {
            MeetingTerminationSource::Relay
        } else {
            MeetingTerminationSource::Unknown
        };
        ends.push(MeetingEndState {
            event_id: event.id.to_hex(),
            outcome: outcome.to_string(),
            reason_code,
            reason: (!event.content.trim().is_empty()).then(|| event.content.clone()),
            ended_by: signer,
            ended_at: event.created_at.as_secs(),
            actions_attested,
            termination_source,
        });
    }
    ends.sort_by(|left, right| {
        left.ended_at
            .cmp(&right.ended_at)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });
    if ends.len() > 1 {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting has conflicting End events",
        )));
    }
    Ok(ends.pop())
}

pub(super) fn validate_participants(
    state: &StateWire,
    create: &CreateProjection,
) -> Result<Vec<MeetingParticipant>, MeetingReadError> {
    let mut participants = BTreeMap::new();
    for participant in &state.participants {
        require_hex64(&participant.pubkey, "Meeting State participant")
            .map_err(MeetingReadError::Other)?;
        if participants
            .insert(
                participant.pubkey.clone(),
                MeetingParticipant {
                    pubkey: participant.pubkey.clone(),
                    participant_type: participant.participant_type.into(),
                    channel_role: participant.channel_role.clone(),
                },
            )
            .is_some()
        {
            return Err(MeetingReadError::Other(integrity_error(
                "Meeting State has a duplicate participant",
            )));
        }
    }
    if participants.keys().cloned().collect::<BTreeSet<_>>() != create.participant_pubkeys {
        return Err(MeetingReadError::Other(integrity_error(
            "Meeting State roster does not match Create",
        )));
    }
    Ok(participants.into_values().collect())
}

pub(super) fn parse_speech(
    event: &Event,
    meeting_id: &str,
    roster: &BTreeMap<String, MeetingParticipantType>,
    moderator_pubkey: &str,
    current_speech_revision: u64,
) -> Result<Option<MeetingSpeech>, String> {
    if event.kind.as_u16() as u32 != KIND_STREAM_MESSAGE
        || single_tag(event, "h") != Some(meeting_id)
        || single_tag(event, "v") != Some(buzz_sdk_pkg::MEETING_V2_SCHEMA_VERSION)
    {
        return Ok(None);
    }
    event
        .verify()
        .map_err(|error| integrity_error(format!("invalid Meeting Speech signature: {error}")))?;
    let author_pubkey = event.pubkey.to_hex();
    let Some(author_participant_type) = roster.get(&author_pubkey).copied() else {
        return Ok(None);
    };
    let grant_event_id = required_tag_string(event, "meeting-grant", "Meeting Speech")?;
    require_hex64(&grant_event_id, "Meeting Speech grant")?;
    let speech_revision = required_tag_string(event, "speech-revision", "Meeting Speech")?
        .parse::<u64>()
        .map_err(|_| integrity_error("Meeting Speech has an invalid revision"))?;
    if speech_revision == 0 || speech_revision > current_speech_revision {
        return Ok(None);
    }
    if event.content.trim().is_empty() {
        return Err(integrity_error("Meeting Speech content is empty"));
    }
    if matches!(author_participant_type, MeetingParticipantType::Unknown) {
        return Err(integrity_error(
            "Meeting Speech author has no frozen participant type",
        ));
    }
    let handoff_to = unique_speech_tag(event, "handoff-to")?;
    let handoff_type = unique_speech_tag(event, "handoff-type")?;
    let handoff_reason = unique_speech_tag(event, "handoff-reason")?;
    let handoff = match (handoff_to, handoff_type, handoff_reason) {
        (None, None, None) => None,
        (Some(target_pubkey), Some(handoff_type), Some(reason)) => {
            require_hex64(target_pubkey, "Meeting Speech Handoff target")?;
            if target_pubkey == author_pubkey.as_str() {
                return Err(integrity_error(
                    "Meeting Speech Handoff target must be another participant",
                ));
            }
            if !roster.contains_key(target_pubkey) {
                return Err(integrity_error(
                    "Meeting Speech Handoff target is outside the frozen roster",
                ));
            }
            let handoff_type = match handoff_type {
                "question" => MeetingSpeechHandoffType::Question,
                "information_request" => MeetingSpeechHandoffType::InformationRequest,
                "clarification" => MeetingSpeechHandoffType::Clarification,
                "review" => MeetingSpeechHandoffType::Review,
                "response_requested" => MeetingSpeechHandoffType::ResponseRequested,
                _ => {
                    return Err(integrity_error(
                        "Meeting Speech has an unsupported Handoff type",
                    ));
                }
            };
            if reason.trim().is_empty()
                || reason.trim() != reason
                || reason.len() > 1024
                || reason.chars().any(char::is_control)
            {
                return Err(integrity_error(
                    "Meeting Speech has an invalid Handoff reason",
                ));
            }
            Some(MeetingSpeechHandoff {
                target_pubkey: target_pubkey.to_string(),
                handoff_type,
                reason: reason.to_string(),
            })
        }
        _ => {
            return Err(integrity_error(
                "Meeting Speech Handoff fields must appear together",
            ));
        }
    };
    let mentions = tags_named(event, "p")
        .filter_map(|tag| tag.get(1).cloned())
        .filter(|pubkey| roster.contains_key(pubkey))
        .collect();
    let author_is_moderator = author_pubkey == moderator_pubkey;
    Ok(Some(MeetingSpeech {
        event_id: event.id.to_hex(),
        author_pubkey,
        content: event.content.clone(),
        created_at: event.created_at.as_secs(),
        speech_revision,
        grant_event_id,
        mentions,
        author_participant_type,
        author_is_moderator,
        handoff,
    }))
}

fn unique_speech_tag<'a>(event: &'a Event, name: &str) -> Result<Option<&'a str>, String> {
    let mut tags = tags_named(event, name);
    let Some(tag) = tags.next() else {
        return Ok(None);
    };
    if tags.next().is_some() {
        return Err(integrity_error(format!(
            "Meeting Speech has duplicate {name} tags"
        )));
    }
    let value = tag
        .get(1)
        .ok_or_else(|| integrity_error(format!("Meeting Speech has an invalid {name} tag")))?;
    if value.is_empty() {
        return Err(integrity_error(format!(
            "Meeting Speech has an empty {name} tag"
        )));
    }
    Ok(Some(value))
}
