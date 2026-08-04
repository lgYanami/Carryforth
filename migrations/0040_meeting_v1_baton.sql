-- Migration 0040.
-- Meeting V1 stage 1: moderated baton policy, frozen participant identities,
-- durable protocol configuration, and the complete additive projection
-- skeleton used by later baton commands.

ALTER TABLE meeting_sessions
    DROP CONSTRAINT chk_meeting_floor_policy,
    ADD COLUMN moderator_pubkey BYTEA,
    ADD CONSTRAINT chk_meeting_schema_version
        CHECK (schema_version IN (1, 2)),
    ADD CONSTRAINT chk_meeting_floor_policy
        CHECK (floor_policy_version IN ('uniform-v0', 'moderated-baton-v1')),
    ADD CONSTRAINT chk_meeting_moderator_pubkey_len
        CHECK (moderator_pubkey IS NULL OR LENGTH(moderator_pubkey) = 32),
    ADD CONSTRAINT chk_meeting_protocol_shape
        CHECK (
            (schema_version = 1
                AND floor_policy_version = 'uniform-v0'
                AND moderator_pubkey IS NULL)
            OR
            (schema_version = 2
                AND floor_policy_version = 'moderated-baton-v1'
                AND moderator_pubkey IS NOT NULL)
        );

CREATE FUNCTION meeting_session_protocol_immutable() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.schema_version IS DISTINCT FROM OLD.schema_version
       OR NEW.floor_policy_version IS DISTINCT FROM OLD.floor_policy_version
       OR NEW.moderator_pubkey IS DISTINCT FROM OLD.moderator_pubkey
    THEN
        RAISE EXCEPTION
            'meeting protocol, policy, and moderator are immutable for session %',
            OLD.session_id
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_meeting_session_protocol_immutable
    BEFORE UPDATE OF schema_version, floor_policy_version, moderator_pubkey
    ON meeting_sessions
    FOR EACH ROW
    EXECUTE FUNCTION meeting_session_protocol_immutable();

CREATE TABLE meeting_participants (
    community_id    UUID NOT NULL REFERENCES communities(id),
    session_id      UUID NOT NULL,
    pubkey          BYTEA NOT NULL,
    participant_type TEXT NOT NULL
        CHECK (participant_type IN ('human', 'agent')),
    channel_role    TEXT NOT NULL
        CHECK (channel_role IN ('owner', 'member', 'bot')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, session_id, pubkey),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_participant_pubkey_len CHECK (LENGTH(pubkey) = 32)
);

CREATE INDEX idx_meeting_participants_identity
    ON meeting_participants (community_id, pubkey, session_id);

CREATE TABLE meeting_baton_config (
    community_id              UUID NOT NULL REFERENCES communities(id),
    session_id                UUID NOT NULL,
    timing_profile_version    TEXT NOT NULL,
    agent_offer_ack_ms        BIGINT NOT NULL,
    human_offer_ack_ms        BIGINT NOT NULL,
    moderator_decision_ms     BIGINT NOT NULL,
    grant_soft_lease_ms       BIGINT NOT NULL,
    progress_interval_ms      BIGINT NOT NULL,
    grant_hard_deadline_ms    BIGINT NOT NULL,
    agent_safety_margin_ms    BIGINT NOT NULL,
    max_handoff_depth         INT NOT NULL,
    max_open_handoffs         INT NOT NULL,
    fallback_policy_version   TEXT NOT NULL,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, session_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_baton_timing_profile
        CHECK (OCTET_LENGTH(timing_profile_version) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_baton_fallback_policy
        CHECK (OCTET_LENGTH(fallback_policy_version) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_baton_positive_durations CHECK (
        agent_offer_ack_ms > 0
        AND human_offer_ack_ms > 0
        AND moderator_decision_ms > 0
        AND grant_soft_lease_ms > 0
        AND progress_interval_ms > 0
        AND grant_hard_deadline_ms > 0
        AND agent_safety_margin_ms > 0
    ),
    CONSTRAINT chk_meeting_baton_duration_order CHECK (
        progress_interval_ms <= grant_soft_lease_ms
        AND grant_soft_lease_ms <= grant_hard_deadline_ms
        AND agent_safety_margin_ms < grant_hard_deadline_ms
    ),
    CONSTRAINT chk_meeting_baton_handoff_depth
        CHECK (max_handoff_depth BETWEEN 0 AND 255),
    CONSTRAINT chk_meeting_baton_open_handoffs
        CHECK (max_open_handoffs BETWEEN 1 AND 32)
);

CREATE TABLE meeting_baton_state_history (
    community_id             UUID NOT NULL REFERENCES communities(id),
    session_id               UUID NOT NULL,
    state_revision           BIGINT NOT NULL,
    state_event_id           BYTEA NOT NULL,
    floor_revision           BIGINT NOT NULL,
    intent_revision          BIGINT NOT NULL,
    speech_revision          BIGINT NOT NULL,
    control_epoch            BIGINT NOT NULL,
    decision_epoch           BIGINT NOT NULL,
    transition_primary_type  TEXT NOT NULL,
    transition_effects_json  JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, session_id, state_revision),
    UNIQUE (community_id, state_event_id),
    UNIQUE (community_id, session_id, state_revision, state_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_baton_history_state_event_id_len
        CHECK (LENGTH(state_event_id) = 32),
    CONSTRAINT chk_meeting_baton_history_revisions CHECK (
        state_revision > 0
        AND floor_revision >= 0
        AND intent_revision >= 0
        AND speech_revision >= 0
        AND control_epoch > 0
        AND decision_epoch >= 0
    ),
    CONSTRAINT chk_meeting_baton_history_transition
        CHECK (LENGTH(transition_primary_type) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_baton_history_effects
        CHECK (jsonb_typeof(transition_effects_json) = 'array')
);

CREATE TABLE meeting_baton_state (
    community_id                    UUID NOT NULL REFERENCES communities(id),
    session_id                      UUID NOT NULL,
    phase                           TEXT NOT NULL
        CHECK (phase IN (
            'moderator_idle',
            'moderator_control',
            'offered',
            'granted',
            'ended'
        )),
    floor_revision                  BIGINT NOT NULL DEFAULT 0,
    intent_revision                 BIGINT NOT NULL DEFAULT 0,
    speech_revision                 BIGINT NOT NULL DEFAULT 0,
    state_revision                  BIGINT NOT NULL,
    control_epoch                   BIGINT NOT NULL,
    decision_epoch                  BIGINT NOT NULL DEFAULT 0,
    state_event_id                  BYTEA NOT NULL,
    active_offer_id                 BYTEA,
    active_grant_id                 BYTEA,
    handoff_depth                   INT NOT NULL DEFAULT 0,
    consecutive_moderator_speeches  INT NOT NULL DEFAULT 0,
    forced_return_to_moderator      BOOLEAN NOT NULL DEFAULT FALSE,
    recall_event_id                 BYTEA,
    moderator_decision_started_at   TIMESTAMPTZ,
    moderator_decision_deadline     TIMESTAMPTZ,
    next_action_at                  TIMESTAMPTZ,
    created_at                      TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at                      TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, session_id),
    UNIQUE (community_id, state_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    FOREIGN KEY (community_id, session_id, state_revision, state_event_id)
        REFERENCES meeting_baton_state_history (
            community_id,
            session_id,
            state_revision,
            state_event_id
        ),
    CONSTRAINT chk_meeting_baton_state_event_id_len
        CHECK (LENGTH(state_event_id) = 32),
    CONSTRAINT chk_meeting_baton_active_offer_id_len
        CHECK (active_offer_id IS NULL OR LENGTH(active_offer_id) = 32),
    CONSTRAINT chk_meeting_baton_active_grant_id_len
        CHECK (active_grant_id IS NULL OR LENGTH(active_grant_id) = 32),
    CONSTRAINT chk_meeting_baton_recall_event_id_len
        CHECK (recall_event_id IS NULL OR LENGTH(recall_event_id) = 32),
    CONSTRAINT chk_meeting_baton_state_revisions CHECK (
        state_revision > 0
        AND floor_revision >= 0
        AND intent_revision >= 0
        AND speech_revision >= 0
        AND control_epoch > 0
        AND decision_epoch >= 0
    ),
    CONSTRAINT chk_meeting_baton_state_depth
        CHECK (handoff_depth BETWEEN 0 AND 255),
    CONSTRAINT chk_meeting_baton_moderator_speeches
        CHECK (consecutive_moderator_speeches >= 0),
    CONSTRAINT chk_meeting_baton_state_phase_shape CHECK (
        (phase = 'moderator_idle'
            AND active_offer_id IS NULL
            AND active_grant_id IS NULL
            AND moderator_decision_started_at IS NULL
            AND moderator_decision_deadline IS NULL
            AND next_action_at IS NULL)
        OR
        (phase = 'moderator_control'
            AND active_offer_id IS NULL
            AND active_grant_id IS NULL
            AND moderator_decision_started_at IS NOT NULL
            AND moderator_decision_deadline IS NOT NULL
            AND next_action_at = moderator_decision_deadline)
        OR
        (phase = 'offered'
            AND active_offer_id IS NOT NULL
            AND active_grant_id IS NULL
            AND moderator_decision_started_at IS NULL
            AND moderator_decision_deadline IS NULL
            AND next_action_at IS NOT NULL)
        OR
        (phase = 'granted'
            AND active_offer_id IS NULL
            AND active_grant_id IS NOT NULL
            AND moderator_decision_started_at IS NULL
            AND moderator_decision_deadline IS NULL
            AND next_action_at IS NOT NULL)
        OR
        (phase = 'ended'
            AND active_offer_id IS NULL
            AND active_grant_id IS NULL
            AND moderator_decision_started_at IS NULL
            AND moderator_decision_deadline IS NULL
            AND next_action_at IS NULL)
    )
);

CREATE INDEX idx_meeting_baton_state_due
    ON meeting_baton_state (next_action_at, community_id, session_id)
    WHERE next_action_at IS NOT NULL;

CREATE TABLE meeting_speech_intents (
    community_id             UUID NOT NULL REFERENCES communities(id),
    session_id               UUID NOT NULL,
    intent_id                BYTEA NOT NULL,
    author_pubkey            BYTEA NOT NULL,
    current_event_id         BYTEA NOT NULL,
    basis_speech_revision    BIGINT NOT NULL,
    summary                  TEXT NOT NULL,
    addressed_to             BYTEA,
    state                    TEXT NOT NULL
        CHECK (state IN (
            'pending',
            'selected',
            'rejected',
            'withdrawn',
            'stale',
            'consumed',
            'ended'
        )),
    selected_grant_id        BYTEA,
    reason_code              TEXT,
    reason_text              TEXT,
    terminal_event_id        BYTEA,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    terminal_at              TIMESTAMPTZ,
    selection_attempt_count  INT NOT NULL DEFAULT 0,
    last_offer_id            BYTEA,
    last_attempt_outcome     TEXT,
    deferred_by_offer_id     BYTEA,
    defer_event_id           BYTEA,
    defer_reason             TEXT,
    PRIMARY KEY (community_id, session_id, intent_id),
    UNIQUE (community_id, current_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_intent_id_len CHECK (LENGTH(intent_id) = 32),
    CONSTRAINT chk_meeting_intent_author_len CHECK (LENGTH(author_pubkey) = 32),
    CONSTRAINT chk_meeting_intent_current_event_id_len
        CHECK (LENGTH(current_event_id) = 32),
    CONSTRAINT chk_meeting_intent_addressed_to_len
        CHECK (addressed_to IS NULL OR LENGTH(addressed_to) = 32),
    CONSTRAINT chk_meeting_intent_selected_grant_id_len
        CHECK (selected_grant_id IS NULL OR LENGTH(selected_grant_id) = 32),
    CONSTRAINT chk_meeting_intent_terminal_event_id_len
        CHECK (terminal_event_id IS NULL OR LENGTH(terminal_event_id) = 32),
    CONSTRAINT chk_meeting_intent_last_offer_id_len
        CHECK (last_offer_id IS NULL OR LENGTH(last_offer_id) = 32),
    CONSTRAINT chk_meeting_intent_deferred_offer_id_len
        CHECK (deferred_by_offer_id IS NULL OR LENGTH(deferred_by_offer_id) = 32),
    CONSTRAINT chk_meeting_intent_defer_event_id_len
        CHECK (defer_event_id IS NULL OR LENGTH(defer_event_id) = 32),
    CONSTRAINT chk_meeting_intent_basis_revision
        CHECK (basis_speech_revision >= 0),
    CONSTRAINT chk_meeting_intent_summary
        CHECK (OCTET_LENGTH(summary) BETWEEN 1 AND 512),
    CONSTRAINT chk_meeting_intent_reason_code
        CHECK (reason_code IS NULL OR OCTET_LENGTH(reason_code) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_intent_reason_text
        CHECK (reason_text IS NULL OR OCTET_LENGTH(reason_text) BETWEEN 1 AND 1024),
    CONSTRAINT chk_meeting_intent_defer_reason
        CHECK (defer_reason IS NULL OR OCTET_LENGTH(defer_reason) BETWEEN 1 AND 1024),
    CONSTRAINT chk_meeting_intent_last_attempt_outcome CHECK (
        last_attempt_outcome IS NULL
        OR last_attempt_outcome IN (
            'offered',
            'granted',
            'declined',
            'timed_out',
            'preempted',
            'recalled',
            'source_changed',
            'source_withdrawn',
            'spoken',
            'yielded',
            'soft_expired',
            'hard_expired',
            'ended'
        )
    ),
    CONSTRAINT chk_meeting_intent_attempts
        CHECK (selection_attempt_count >= 0),
    CONSTRAINT chk_meeting_intent_terminal_shape CHECK (
        (state IN ('pending', 'selected')
            AND terminal_at IS NULL)
        OR
        (state IN ('rejected', 'withdrawn', 'stale', 'consumed', 'ended')
            AND terminal_at IS NOT NULL)
    ),
    CONSTRAINT chk_meeting_intent_rejected_shape CHECK (
        state <> 'rejected'
        OR (
            reason_code IS NOT NULL
            AND reason_text IS NOT NULL
            AND terminal_event_id IS NOT NULL
        )
    ),
    CONSTRAINT chk_meeting_intent_defer_shape CHECK (
        (deferred_by_offer_id IS NULL
            AND defer_event_id IS NULL
            AND defer_reason IS NULL)
        OR
        (state = 'pending'
            AND deferred_by_offer_id IS NOT NULL
            AND defer_event_id IS NOT NULL
            AND defer_reason IS NOT NULL)
    )
);

CREATE UNIQUE INDEX uq_meeting_pending_intent_per_author
    ON meeting_speech_intents (community_id, session_id, author_pubkey)
    WHERE state = 'pending';

CREATE UNIQUE INDEX uq_meeting_selected_intent_grant
    ON meeting_speech_intents (community_id, session_id, selected_grant_id)
    WHERE selected_grant_id IS NOT NULL;

CREATE TABLE meeting_human_floor_requests (
    community_id       UUID NOT NULL REFERENCES communities(id),
    session_id         UUID NOT NULL,
    request_id         BYTEA NOT NULL,
    requester_pubkey   BYTEA NOT NULL,
    queue_position     BIGINT GENERATED ALWAYS AS IDENTITY,
    state              TEXT NOT NULL
        CHECK (state IN (
            'queued',
            'offered',
            'granted',
            'withdrawn',
            'declined',
            'timed_out',
            'ended'
        )),
    offer_id           BYTEA,
    grant_id           BYTEA,
    request_event_id   BYTEA NOT NULL,
    terminal_event_id  BYTEA,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    terminal_at        TIMESTAMPTZ,
    PRIMARY KEY (community_id, session_id, request_id),
    UNIQUE (community_id, request_event_id),
    UNIQUE (community_id, session_id, queue_position),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_request_id_len CHECK (LENGTH(request_id) = 32),
    CONSTRAINT chk_meeting_requester_pubkey_len
        CHECK (LENGTH(requester_pubkey) = 32),
    CONSTRAINT chk_meeting_request_offer_id_len
        CHECK (offer_id IS NULL OR LENGTH(offer_id) = 32),
    CONSTRAINT chk_meeting_request_grant_id_len
        CHECK (grant_id IS NULL OR LENGTH(grant_id) = 32),
    CONSTRAINT chk_meeting_request_event_id_len
        CHECK (LENGTH(request_event_id) = 32),
    CONSTRAINT chk_meeting_request_terminal_event_id_len
        CHECK (terminal_event_id IS NULL OR LENGTH(terminal_event_id) = 32),
    CONSTRAINT chk_meeting_request_terminal_shape CHECK (
        (state IN ('queued', 'offered') AND terminal_at IS NULL)
        OR
        (state IN ('granted', 'withdrawn', 'declined', 'timed_out', 'ended')
            AND terminal_at IS NOT NULL)
    )
);

CREATE UNIQUE INDEX uq_meeting_active_request_per_human
    ON meeting_human_floor_requests (
        community_id,
        session_id,
        requester_pubkey
    )
    WHERE state IN ('queued', 'offered');

CREATE INDEX idx_meeting_human_request_fifo
    ON meeting_human_floor_requests (
        community_id,
        session_id,
        queue_position
    )
    WHERE state = 'queued';

CREATE TABLE meeting_baton_offers (
    community_id             UUID NOT NULL REFERENCES communities(id),
    session_id               UUID NOT NULL,
    offer_id                 BYTEA NOT NULL,
    target_pubkey            BYTEA NOT NULL,
    allocation_source        TEXT NOT NULL
        CHECK (allocation_source IN (
            'moderator_select',
            'directed_handoff',
            'human_request',
            'fallback'
        )),
    turn_role                TEXT NOT NULL
        CHECK (turn_role IN ('participant', 'moderator_self')),
    allocation_event_id      BYTEA,
    selection_reason         TEXT,
    source_intent_id         BYTEA,
    source_request_id        BYTEA,
    source_handoff_id        BYTEA,
    source_speech_event_id   BYTEA,
    reason_type              TEXT,
    reason_text              TEXT,
    basis_speech_revision    BIGINT NOT NULL,
    depth_mode               TEXT NOT NULL
        CHECK (depth_mode IN ('reset', 'preserve', 'increment_provisional')),
    previous_handoff_depth   INT NOT NULL,
    requested_handoff_depth  INT NOT NULL,
    ack_deadline             TIMESTAMPTZ NOT NULL,
    state                    TEXT NOT NULL
        CHECK (state IN (
            'pending',
            'acked',
            'declined',
            'timed_out',
            'preempted',
            'recalled',
            'source_changed',
            'source_withdrawn',
            'ended'
        )),
    response_event_id        BYTEA,
    response_reason          TEXT,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    resolved_at              TIMESTAMPTZ,
    PRIMARY KEY (community_id, session_id, offer_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_offer_id_len CHECK (LENGTH(offer_id) = 32),
    CONSTRAINT chk_meeting_offer_target_len CHECK (LENGTH(target_pubkey) = 32),
    CONSTRAINT chk_meeting_offer_allocation_event_id_len
        CHECK (allocation_event_id IS NULL OR LENGTH(allocation_event_id) = 32),
    CONSTRAINT chk_meeting_offer_source_intent_id_len
        CHECK (source_intent_id IS NULL OR LENGTH(source_intent_id) = 32),
    CONSTRAINT chk_meeting_offer_source_request_id_len
        CHECK (source_request_id IS NULL OR LENGTH(source_request_id) = 32),
    CONSTRAINT chk_meeting_offer_source_handoff_id_len
        CHECK (source_handoff_id IS NULL OR LENGTH(source_handoff_id) = 32),
    CONSTRAINT chk_meeting_offer_source_speech_id_len
        CHECK (source_speech_event_id IS NULL OR LENGTH(source_speech_event_id) = 32),
    CONSTRAINT chk_meeting_offer_response_event_id_len
        CHECK (response_event_id IS NULL OR LENGTH(response_event_id) = 32),
    CONSTRAINT chk_meeting_offer_response_reason
        CHECK (
            response_reason IS NULL
            OR OCTET_LENGTH(response_reason) BETWEEN 1 AND 512
        ),
    CONSTRAINT chk_meeting_offer_basis_revision
        CHECK (basis_speech_revision >= 0),
    CONSTRAINT chk_meeting_offer_selection_reason
        CHECK (
            selection_reason IS NULL
            OR OCTET_LENGTH(selection_reason) BETWEEN 1 AND 512
        ),
    CONSTRAINT chk_meeting_offer_reason_type
        CHECK (reason_type IS NULL OR OCTET_LENGTH(reason_type) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_offer_reason_text
        CHECK (reason_text IS NULL OR OCTET_LENGTH(reason_text) BETWEEN 1 AND 1024),
    CONSTRAINT chk_meeting_offer_depths CHECK (
        previous_handoff_depth BETWEEN 0 AND 255
        AND requested_handoff_depth BETWEEN 0 AND 255
    ),
    CONSTRAINT chk_meeting_offer_resolution_shape CHECK (
        (state = 'pending'
            AND response_event_id IS NULL
            AND response_reason IS NULL
            AND resolved_at IS NULL)
        OR
        (state <> 'pending'
            AND resolved_at IS NOT NULL
            AND (response_reason IS NULL OR response_event_id IS NOT NULL))
    ),
    CONSTRAINT chk_meeting_offer_source_shape CHECK (
        (
            allocation_source IN ('moderator_select', 'fallback')
            AND (
                (source_intent_id IS NOT NULL
                    AND source_request_id IS NULL
                    AND source_handoff_id IS NULL)
                OR
                (allocation_source = 'moderator_select'
                    AND source_intent_id IS NULL
                    AND source_request_id IS NULL
                    AND source_handoff_id IS NOT NULL)
            )
        )
        OR
        (allocation_source = 'directed_handoff'
            AND source_intent_id IS NULL
            AND source_request_id IS NULL
            AND source_handoff_id IS NOT NULL)
        OR
        (allocation_source = 'human_request'
            AND source_intent_id IS NULL
            AND source_request_id IS NOT NULL
            AND source_handoff_id IS NULL)
    ),
    CONSTRAINT chk_meeting_offer_allocation_shape CHECK (
        (allocation_source = 'moderator_select'
            AND allocation_event_id IS NOT NULL
            AND depth_mode = 'reset'
            AND requested_handoff_depth = 0
            AND (
                (source_intent_id IS NOT NULL
                    AND source_speech_event_id IS NULL
                    AND reason_type IS NULL
                    AND reason_text IS NULL)
                OR
                (source_handoff_id IS NOT NULL
                    AND turn_role = 'participant'
                    AND source_speech_event_id IS NOT NULL
                    AND source_speech_event_id = source_handoff_id
                    AND reason_type IS NOT NULL
                    AND reason_text IS NOT NULL)
            ))
        OR
        (allocation_source = 'fallback'
            AND allocation_event_id IS NULL
            AND source_intent_id IS NOT NULL
            AND source_speech_event_id IS NULL
            AND reason_type IS NULL
            AND reason_text IS NULL
            AND depth_mode = 'reset'
            AND requested_handoff_depth = 0)
        OR
        (allocation_source = 'directed_handoff'
            AND allocation_event_id IS NOT NULL
            AND turn_role = 'participant'
            AND source_handoff_id IS NOT NULL
            AND source_speech_event_id IS NOT NULL
            AND allocation_event_id = source_speech_event_id
            AND source_speech_event_id = source_handoff_id
            AND reason_type IS NOT NULL
            AND reason_text IS NOT NULL
            AND (
                (depth_mode = 'reset' AND requested_handoff_depth = 0)
                OR
                (depth_mode = 'increment_provisional'
                    AND previous_handoff_depth < 255
                    AND requested_handoff_depth = previous_handoff_depth + 1)
            ))
        OR
        (allocation_source = 'human_request'
            AND allocation_event_id IS NOT NULL
            AND turn_role = 'participant'
            AND source_request_id IS NOT NULL
            AND source_speech_event_id IS NULL
            AND reason_type IS NULL
            AND reason_text IS NULL
            AND depth_mode = 'preserve'
            AND requested_handoff_depth = previous_handoff_depth)
    )
);

CREATE UNIQUE INDEX uq_meeting_active_offer
    ON meeting_baton_offers (community_id, session_id)
    WHERE state = 'pending';

CREATE INDEX idx_meeting_offer_source_handoff
    ON meeting_baton_offers (
        community_id,
        session_id,
        source_handoff_id,
        created_at
    )
    WHERE source_handoff_id IS NOT NULL;

CREATE INDEX idx_meeting_offer_deadline
    ON meeting_baton_offers (ack_deadline, community_id, session_id)
    WHERE state = 'pending';

CREATE TABLE meeting_baton_grants (
    community_id             UUID NOT NULL REFERENCES communities(id),
    session_id               UUID NOT NULL,
    grant_id                 BYTEA NOT NULL,
    holder_pubkey            BYTEA NOT NULL,
    allocation_source        TEXT NOT NULL
        CHECK (allocation_source IN (
            'moderator_select',
            'directed_handoff',
            'human_request',
            'fallback'
        )),
    turn_role                TEXT NOT NULL
        CHECK (turn_role IN ('participant', 'moderator_self')),
    source_offer_id          BYTEA NOT NULL,
    allocation_event_id      BYTEA,
    selection_reason         TEXT,
    source_intent_id         BYTEA,
    source_request_id        BYTEA,
    source_handoff_id        BYTEA,
    source_speech_event_id   BYTEA,
    basis_speech_revision    BIGINT NOT NULL,
    depth_mode               TEXT NOT NULL
        CHECK (depth_mode IN ('reset', 'preserve', 'increment_provisional')),
    previous_handoff_depth   INT NOT NULL,
    handoff_depth            INT NOT NULL,
    soft_lease_expires_at    TIMESTAMPTZ NOT NULL,
    hard_deadline            TIMESTAMPTZ NOT NULL,
    progress_seq             BIGINT NOT NULL DEFAULT 0,
    state                    TEXT NOT NULL
        CHECK (state IN (
            'active',
            'spoken',
            'yielded',
            'soft_expired',
            'hard_expired',
            'ended'
        )),
    speech_event_id          BYTEA,
    terminal_event_id        BYTEA,
    terminal_reason          TEXT,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    terminal_at              TIMESTAMPTZ,
    PRIMARY KEY (community_id, session_id, grant_id),
    UNIQUE (community_id, speech_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    FOREIGN KEY (community_id, session_id, source_offer_id)
        REFERENCES meeting_baton_offers (community_id, session_id, offer_id),
    CONSTRAINT chk_meeting_grant_id_len CHECK (LENGTH(grant_id) = 32),
    CONSTRAINT chk_meeting_grant_holder_len CHECK (LENGTH(holder_pubkey) = 32),
    CONSTRAINT chk_meeting_grant_source_offer_id_len
        CHECK (LENGTH(source_offer_id) = 32),
    CONSTRAINT chk_meeting_grant_allocation_event_id_len
        CHECK (allocation_event_id IS NULL OR LENGTH(allocation_event_id) = 32),
    CONSTRAINT chk_meeting_grant_source_intent_id_len
        CHECK (source_intent_id IS NULL OR LENGTH(source_intent_id) = 32),
    CONSTRAINT chk_meeting_grant_source_request_id_len
        CHECK (source_request_id IS NULL OR LENGTH(source_request_id) = 32),
    CONSTRAINT chk_meeting_grant_source_handoff_id_len
        CHECK (source_handoff_id IS NULL OR LENGTH(source_handoff_id) = 32),
    CONSTRAINT chk_meeting_grant_source_speech_id_len
        CHECK (source_speech_event_id IS NULL OR LENGTH(source_speech_event_id) = 32),
    CONSTRAINT chk_meeting_grant_speech_event_id_len
        CHECK (speech_event_id IS NULL OR LENGTH(speech_event_id) = 32),
    CONSTRAINT chk_meeting_grant_terminal_event_id_len
        CHECK (terminal_event_id IS NULL OR LENGTH(terminal_event_id) = 32),
    CONSTRAINT chk_meeting_grant_terminal_reason
        CHECK (
            terminal_reason IS NULL
            OR (
                terminal_event_id IS NOT NULL
                AND OCTET_LENGTH(terminal_reason) BETWEEN 1 AND 512
            )
        ),
    CONSTRAINT chk_meeting_grant_basis_revision
        CHECK (basis_speech_revision >= 0),
    CONSTRAINT chk_meeting_grant_selection_reason
        CHECK (
            selection_reason IS NULL
            OR OCTET_LENGTH(selection_reason) BETWEEN 1 AND 512
        ),
    CONSTRAINT chk_meeting_grant_depths CHECK (
        previous_handoff_depth BETWEEN 0 AND 255
        AND handoff_depth BETWEEN 0 AND 255
    ),
    CONSTRAINT chk_meeting_grant_progress_seq CHECK (progress_seq >= 0),
    CONSTRAINT chk_meeting_grant_deadline_order
        CHECK (soft_lease_expires_at <= hard_deadline),
    CONSTRAINT chk_meeting_grant_terminal_shape CHECK (
        (state = 'active'
            AND speech_event_id IS NULL
            AND terminal_at IS NULL)
        OR
        (state = 'spoken'
            AND speech_event_id IS NOT NULL
            AND terminal_at IS NOT NULL)
        OR
        (state IN ('yielded', 'soft_expired', 'hard_expired', 'ended')
            AND speech_event_id IS NULL
            AND terminal_at IS NOT NULL)
    ),
    CONSTRAINT chk_meeting_grant_allocation_shape CHECK (
        (allocation_source = 'moderator_select'
            AND allocation_event_id IS NOT NULL
            AND depth_mode = 'reset'
            AND handoff_depth = 0
            AND (
                (source_intent_id IS NOT NULL
                    AND source_request_id IS NULL
                    AND source_handoff_id IS NULL
                    AND source_speech_event_id IS NULL)
                OR
                (source_intent_id IS NULL
                    AND source_request_id IS NULL
                    AND source_handoff_id IS NOT NULL
                    AND source_speech_event_id IS NOT NULL
                    AND source_speech_event_id = source_handoff_id
                    AND turn_role = 'participant')
            ))
        OR
        (allocation_source = 'fallback'
            AND allocation_event_id IS NULL
            AND source_intent_id IS NOT NULL
            AND source_request_id IS NULL
            AND source_handoff_id IS NULL
            AND source_speech_event_id IS NULL
            AND depth_mode = 'reset'
            AND handoff_depth = 0)
        OR
        (allocation_source = 'directed_handoff'
            AND allocation_event_id IS NOT NULL
            AND turn_role = 'participant'
            AND source_intent_id IS NULL
            AND source_request_id IS NULL
            AND source_handoff_id IS NOT NULL
            AND source_speech_event_id IS NOT NULL
            AND allocation_event_id = source_speech_event_id
            AND source_speech_event_id = source_handoff_id
            AND (
                (depth_mode = 'reset' AND handoff_depth = 0)
                OR
                (depth_mode = 'increment_provisional'
                    AND previous_handoff_depth < 255
                    AND handoff_depth = previous_handoff_depth + 1)
            ))
        OR
        (allocation_source = 'human_request'
            AND allocation_event_id IS NOT NULL
            AND turn_role = 'participant'
            AND source_intent_id IS NULL
            AND source_request_id IS NOT NULL
            AND source_handoff_id IS NULL
            AND source_speech_event_id IS NULL
            AND depth_mode = 'preserve'
            AND handoff_depth = previous_handoff_depth)
    )
);

CREATE UNIQUE INDEX uq_meeting_active_grant
    ON meeting_baton_grants (community_id, session_id)
    WHERE state = 'active';

CREATE INDEX idx_meeting_grant_source_handoff
    ON meeting_baton_grants (
        community_id,
        session_id,
        source_handoff_id,
        created_at
    )
    WHERE source_handoff_id IS NOT NULL;

CREATE INDEX idx_meeting_grant_deadline
    ON meeting_baton_grants (
        soft_lease_expires_at,
        hard_deadline,
        community_id,
        session_id
    )
    WHERE state = 'active';

CREATE TABLE meeting_grant_progress (
    community_id          UUID NOT NULL REFERENCES communities(id),
    session_id            UUID NOT NULL,
    grant_id              BYTEA NOT NULL,
    progress_seq          BIGINT NOT NULL,
    progress_event_id     BYTEA NOT NULL,
    stage                 TEXT NOT NULL
        CHECK (stage IN (
            'planning',
            'tooling',
            'drafting',
            'composing',
            'finalizing'
        )),
    soft_lease_expires_at TIMESTAMPTZ NOT NULL,
    accepted_at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, session_id, grant_id, progress_seq),
    UNIQUE (community_id, progress_event_id),
    FOREIGN KEY (community_id, session_id, grant_id)
        REFERENCES meeting_baton_grants (community_id, session_id, grant_id),
    CONSTRAINT chk_meeting_progress_grant_id_len CHECK (LENGTH(grant_id) = 32),
    CONSTRAINT chk_meeting_progress_event_id_len
        CHECK (LENGTH(progress_event_id) = 32),
    CONSTRAINT chk_meeting_progress_seq CHECK (progress_seq > 0)
);

CREATE TABLE meeting_directed_handoffs (
    community_id                UUID NOT NULL REFERENCES communities(id),
    session_id                  UUID NOT NULL,
    handoff_id                  BYTEA NOT NULL,
    source_speech_event_id      BYTEA NOT NULL,
    from_pubkey                 BYTEA NOT NULL,
    to_pubkey                   BYTEA NOT NULL,
    reason_type                 TEXT NOT NULL,
    reason_text                 TEXT NOT NULL,
    requested_depth             INT NOT NULL,
    question_state              TEXT NOT NULL
        CHECK (question_state IN ('open', 'answered', 'dismissed', 'blocked', 'ended')),
    initial_disposition         TEXT NOT NULL
        CHECK (initial_disposition IN ('offered', 'blocked')),
    blocked_by                  TEXT
        CHECK (blocked_by IN (
            'human_request',
            'recall',
            'max_depth',
            'open_question_limit'
        )),
    last_offer_id               BYTEA,
    last_grant_id               BYTEA,
    last_attempt_outcome        TEXT,
    attempt_count               INT NOT NULL DEFAULT 0,
    answered_by_speech_event_id BYTEA,
    dismiss_event_id            BYTEA,
    dismiss_reason_code         TEXT,
    dismiss_reason_text         TEXT,
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    answered_at                 TIMESTAMPTZ,
    dismissed_at                TIMESTAMPTZ,
    terminal_at                 TIMESTAMPTZ,
    PRIMARY KEY (community_id, session_id, handoff_id),
    UNIQUE (community_id, source_speech_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_handoff_id_len CHECK (LENGTH(handoff_id) = 32),
    CONSTRAINT chk_meeting_handoff_source_speech_id_len
        CHECK (LENGTH(source_speech_event_id) = 32),
    CONSTRAINT chk_meeting_handoff_identity
        CHECK (handoff_id = source_speech_event_id),
    CONSTRAINT chk_meeting_handoff_from_len CHECK (LENGTH(from_pubkey) = 32),
    CONSTRAINT chk_meeting_handoff_to_len CHECK (LENGTH(to_pubkey) = 32),
    CONSTRAINT chk_meeting_handoff_last_offer_len
        CHECK (last_offer_id IS NULL OR LENGTH(last_offer_id) = 32),
    CONSTRAINT chk_meeting_handoff_last_grant_len
        CHECK (last_grant_id IS NULL OR LENGTH(last_grant_id) = 32),
    CONSTRAINT chk_meeting_handoff_answer_speech_len
        CHECK (
            answered_by_speech_event_id IS NULL
            OR LENGTH(answered_by_speech_event_id) = 32
        ),
    CONSTRAINT chk_meeting_handoff_dismiss_event_len
        CHECK (dismiss_event_id IS NULL OR LENGTH(dismiss_event_id) = 32),
    CONSTRAINT chk_meeting_handoff_reason
        CHECK (OCTET_LENGTH(reason_text) BETWEEN 1 AND 1024),
    CONSTRAINT chk_meeting_handoff_reason_type
        CHECK (OCTET_LENGTH(reason_type) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_handoff_dismiss_reason_code
        CHECK (
            dismiss_reason_code IS NULL
            OR OCTET_LENGTH(dismiss_reason_code) BETWEEN 1 AND 128
        ),
    CONSTRAINT chk_meeting_handoff_dismiss_reason_text
        CHECK (
            dismiss_reason_text IS NULL
            OR OCTET_LENGTH(dismiss_reason_text) BETWEEN 1 AND 1024
        ),
    CONSTRAINT chk_meeting_handoff_last_attempt_outcome CHECK (
        last_attempt_outcome IS NULL
        OR last_attempt_outcome IN (
            'offered',
            'granted',
            'declined',
            'timed_out',
            'preempted',
            'recalled',
            'source_changed',
            'source_withdrawn',
            'spoken',
            'yielded',
            'soft_expired',
            'hard_expired',
            'ended'
        )
    ),
    CONSTRAINT chk_meeting_handoff_requested_depth
        CHECK (requested_depth BETWEEN 0 AND 255),
    CONSTRAINT chk_meeting_handoff_attempt_count CHECK (attempt_count >= 0),
    CONSTRAINT chk_meeting_handoff_terminal_shape CHECK (
        (question_state = 'open'
            AND (
                blocked_by IS NULL
                OR blocked_by IN ('human_request', 'recall', 'max_depth')
            )
            AND terminal_at IS NULL)
        OR
        (question_state = 'answered'
            AND answered_by_speech_event_id IS NOT NULL
            AND answered_at IS NOT NULL
            AND terminal_at IS NOT NULL)
        OR
        (question_state = 'dismissed'
            AND blocked_by IS NULL
            AND dismiss_event_id IS NOT NULL
            AND dismiss_reason_code IS NOT NULL
            AND dismiss_reason_text IS NOT NULL
            AND dismissed_at IS NOT NULL
            AND terminal_at IS NOT NULL)
        OR
        (question_state = 'blocked'
            AND blocked_by = 'open_question_limit'
            AND terminal_at IS NOT NULL)
        OR
        (question_state = 'ended' AND terminal_at IS NOT NULL)
    )
);

CREATE INDEX idx_meeting_open_handoffs
    ON meeting_directed_handoffs (community_id, session_id, created_at)
    WHERE question_state = 'open';

CREATE TABLE meeting_baton_fallback_attempts (
    community_id            UUID NOT NULL REFERENCES communities(id),
    session_id              UUID NOT NULL,
    intent_id               BYTEA NOT NULL,
    current_intent_event_id BYTEA NOT NULL,
    speech_revision         BIGINT NOT NULL,
    offer_id                BYTEA NOT NULL,
    attempted_at            TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (
        community_id,
        session_id,
        intent_id,
        current_intent_event_id,
        speech_revision
    ),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_fallback_intent_id_len CHECK (LENGTH(intent_id) = 32),
    CONSTRAINT chk_meeting_fallback_intent_event_id_len
        CHECK (LENGTH(current_intent_event_id) = 32),
    CONSTRAINT chk_meeting_fallback_offer_id_len CHECK (LENGTH(offer_id) = 32),
    CONSTRAINT chk_meeting_fallback_speech_revision CHECK (speech_revision >= 0)
);

CREATE TABLE meeting_v1_command_receipts (
    community_id        UUID NOT NULL REFERENCES communities(id),
    session_id          UUID NOT NULL,
    command_event_id    BYTEA NOT NULL,
    author_pubkey       BYTEA NOT NULL,
    kind                INT NOT NULL,
    action              TEXT NOT NULL,
    accepted            BOOLEAN NOT NULL,
    outcome_code        TEXT NOT NULL,
    canonical_object_id BYTEA,
    state_revision      BIGINT,
    response_json       JSONB NOT NULL,
    recorded_at         TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, command_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_receipt_command_event_id_len
        CHECK (LENGTH(command_event_id) = 32),
    CONSTRAINT chk_meeting_receipt_author_pubkey_len
        CHECK (LENGTH(author_pubkey) = 32),
    CONSTRAINT chk_meeting_receipt_canonical_object_id_len
        CHECK (canonical_object_id IS NULL OR LENGTH(canonical_object_id) = 32),
    CONSTRAINT chk_meeting_receipt_state_revision
        CHECK (state_revision IS NULL OR state_revision > 0),
    CONSTRAINT chk_meeting_receipt_action
        CHECK (LENGTH(action) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_receipt_outcome
        CHECK (LENGTH(outcome_code) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_receipt_response
        CHECK (jsonb_typeof(response_json) = 'object')
);

CREATE INDEX idx_meeting_v1_receipts_session
    ON meeting_v1_command_receipts (
        community_id,
        session_id,
        recorded_at,
        command_event_id
    );

CREATE TABLE meeting_revocation_jobs (
    community_id        UUID NOT NULL REFERENCES communities(id),
    job_id              UUID NOT NULL,
    revoked_pubkey      BYTEA NOT NULL,
    revocation_event_id BYTEA NOT NULL,
    state               TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'running', 'completed')),
    cursor_session_id   UUID,
    attempts            INT NOT NULL DEFAULT 0,
    next_attempt_at     TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    last_error          TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    completed_at        TIMESTAMPTZ,
    PRIMARY KEY (community_id, job_id),
    UNIQUE (community_id, revocation_event_id),
    CONSTRAINT chk_meeting_revocation_pubkey_len
        CHECK (LENGTH(revoked_pubkey) = 32),
    CONSTRAINT chk_meeting_revocation_event_id_len
        CHECK (LENGTH(revocation_event_id) = 32),
    CONSTRAINT chk_meeting_revocation_attempts CHECK (attempts >= 0),
    CONSTRAINT chk_meeting_revocation_terminal_shape CHECK (
        (state IN ('pending', 'running') AND completed_at IS NULL)
        OR
        (state = 'completed' AND completed_at IS NOT NULL)
    )
);

CREATE INDEX idx_meeting_revocation_jobs_due
    ON meeting_revocation_jobs (
        next_attempt_at,
        community_id,
        job_id
    )
    WHERE state IN ('pending', 'running');

ALTER TABLE meeting_baton_state
    ADD CONSTRAINT fk_meeting_baton_active_offer
        FOREIGN KEY (community_id, session_id, active_offer_id)
        REFERENCES meeting_baton_offers (community_id, session_id, offer_id),
    ADD CONSTRAINT fk_meeting_baton_active_grant
        FOREIGN KEY (community_id, session_id, active_grant_id)
        REFERENCES meeting_baton_grants (community_id, session_id, grant_id);

ALTER TABLE meeting_speech_intents
    ADD CONSTRAINT fk_meeting_intent_selected_grant
        FOREIGN KEY (community_id, session_id, selected_grant_id)
        REFERENCES meeting_baton_grants (community_id, session_id, grant_id),
    ADD CONSTRAINT fk_meeting_intent_last_offer
        FOREIGN KEY (community_id, session_id, last_offer_id)
        REFERENCES meeting_baton_offers (community_id, session_id, offer_id),
    ADD CONSTRAINT fk_meeting_intent_deferred_offer
        FOREIGN KEY (community_id, session_id, deferred_by_offer_id)
        REFERENCES meeting_baton_offers (community_id, session_id, offer_id);

ALTER TABLE meeting_human_floor_requests
    ADD CONSTRAINT fk_meeting_request_offer
        FOREIGN KEY (community_id, session_id, offer_id)
        REFERENCES meeting_baton_offers (community_id, session_id, offer_id),
    ADD CONSTRAINT fk_meeting_request_grant
        FOREIGN KEY (community_id, session_id, grant_id)
        REFERENCES meeting_baton_grants (community_id, session_id, grant_id);

ALTER TABLE meeting_directed_handoffs
    ADD CONSTRAINT fk_meeting_handoff_last_offer
        FOREIGN KEY (community_id, session_id, last_offer_id)
        REFERENCES meeting_baton_offers (community_id, session_id, offer_id),
    ADD CONSTRAINT fk_meeting_handoff_last_grant
        FOREIGN KEY (community_id, session_id, last_grant_id)
        REFERENCES meeting_baton_grants (community_id, session_id, grant_id);

ALTER TABLE meeting_baton_fallback_attempts
    ADD CONSTRAINT fk_meeting_fallback_intent
        FOREIGN KEY (community_id, session_id, intent_id)
        REFERENCES meeting_speech_intents (community_id, session_id, intent_id),
    ADD CONSTRAINT fk_meeting_fallback_offer
        FOREIGN KEY (community_id, session_id, offer_id)
        REFERENCES meeting_baton_offers (community_id, session_id, offer_id);
