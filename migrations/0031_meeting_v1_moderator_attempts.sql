-- Meeting V1 moderator optimistic-decision foundation.
--
-- Candidate eligibility, authoritative decision attempts, and one-use retry
-- evidence are persisted so an ACP process can restart without becoming the
-- source of truth. This migration is additive; existing Meeting V1 sessions
-- keep their current candidate set by binding live sources to their current
-- decision epoch.

ALTER TABLE meeting_baton_config
    ADD COLUMN moderator_max_rejudgments INT NOT NULL DEFAULT 2,
    ADD COLUMN moderator_max_cas_rebases_per_attempt INT NOT NULL DEFAULT 8,
    ADD CONSTRAINT chk_meeting_baton_moderator_rejudgments
        CHECK (moderator_max_rejudgments BETWEEN 0 AND 8),
    ADD CONSTRAINT chk_meeting_baton_moderator_cas_rebases
        CHECK (moderator_max_cas_rebases_per_attempt BETWEEN 1 AND 64);

ALTER TABLE meeting_speech_intents
    ADD COLUMN eligible_decision_epoch BIGINT NOT NULL DEFAULT 0,
    ADD CONSTRAINT chk_meeting_intent_eligible_decision_epoch
        CHECK (eligible_decision_epoch >= 0);

UPDATE meeting_speech_intents intents
SET eligible_decision_epoch = state.decision_epoch
FROM meeting_baton_state state
WHERE state.community_id = intents.community_id
  AND state.session_id = intents.session_id
  AND intents.state IN ('pending', 'selected');

CREATE INDEX idx_meeting_intent_decision_cohort
    ON meeting_speech_intents (
        community_id,
        session_id,
        eligible_decision_epoch,
        created_at,
        intent_id
    )
    WHERE state = 'pending';

ALTER TABLE meeting_directed_handoffs
    ADD COLUMN eligible_decision_epoch BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN moderator_retry_blocked_fingerprint BYTEA,
    ADD COLUMN moderator_retry_not_before TIMESTAMPTZ,
    ADD CONSTRAINT chk_meeting_handoff_eligible_decision_epoch
        CHECK (eligible_decision_epoch >= 0),
    ADD CONSTRAINT chk_meeting_handoff_retry_fingerprint_len
        CHECK (
            moderator_retry_blocked_fingerprint IS NULL
            OR LENGTH(moderator_retry_blocked_fingerprint) = 32
        ),
    ADD CONSTRAINT chk_meeting_handoff_retry_suppression_shape
        CHECK (
            (moderator_retry_blocked_fingerprint IS NULL
                AND moderator_retry_not_before IS NULL)
            OR
            (moderator_retry_blocked_fingerprint IS NOT NULL
                AND moderator_retry_not_before IS NOT NULL)
        );

UPDATE meeting_directed_handoffs handoffs
SET eligible_decision_epoch = state.decision_epoch
FROM meeting_baton_state state
WHERE state.community_id = handoffs.community_id
  AND state.session_id = handoffs.session_id
  AND handoffs.question_state = 'open';

CREATE INDEX idx_meeting_handoff_decision_cohort
    ON meeting_directed_handoffs (
        community_id,
        session_id,
        eligible_decision_epoch,
        created_at,
        handoff_id
    )
    WHERE question_state = 'open';

ALTER TABLE meeting_baton_state
    ADD COLUMN decision_attempt INT NOT NULL DEFAULT 0,
    ADD COLUMN active_decision_attempt_id BYTEA,
    ADD CONSTRAINT chk_meeting_baton_decision_attempt
        CHECK (decision_attempt >= 0),
    ADD CONSTRAINT chk_meeting_baton_active_decision_attempt_id_len
        CHECK (
            active_decision_attempt_id IS NULL
            OR LENGTH(active_decision_attempt_id) = 32
        );

CREATE TABLE meeting_moderator_decision_attempts (
    community_id                 UUID NOT NULL REFERENCES communities(id),
    session_id                   UUID NOT NULL,
    attempt_id                   BYTEA NOT NULL,
    moderator_pubkey             BYTEA NOT NULL,
    control_epoch                BIGINT NOT NULL,
    decision_epoch               BIGINT NOT NULL,
    attempt_number               INT NOT NULL,
    speech_revision              BIGINT NOT NULL,
    snapshot_intent_revision     BIGINT NOT NULL,
    snapshot_state_event_id      BYTEA NOT NULL,
    candidate_snapshot_json      JSONB NOT NULL,
    candidate_snapshot_hash      BYTEA NOT NULL,
    state                        TEXT NOT NULL
        CHECK (state IN (
            'running',
            'completed',
            'committed',
            'retry_required',
            'discarded',
            'timed_out',
            'abandoned'
        )),
    replacement_of_attempt_id    BYTEA,
    started_by_event_id          BYTEA NOT NULL,
    terminal_event_id            BYTEA,
    terminal_reason              TEXT,
    started_at                   TIMESTAMPTZ NOT NULL,
    deadline_at                  TIMESTAMPTZ NOT NULL,
    terminal_at                  TIMESTAMPTZ,
    PRIMARY KEY (community_id, session_id, attempt_id),
    UNIQUE (
        community_id,
        session_id,
        control_epoch,
        decision_epoch,
        attempt_number
    ),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    FOREIGN KEY (community_id, session_id, replacement_of_attempt_id)
        REFERENCES meeting_moderator_decision_attempts (
            community_id,
            session_id,
            attempt_id
        ),
    CONSTRAINT chk_meeting_moderator_attempt_id_len
        CHECK (LENGTH(attempt_id) = 32),
    CONSTRAINT chk_meeting_moderator_attempt_pubkey_len
        CHECK (LENGTH(moderator_pubkey) = 32),
    CONSTRAINT chk_meeting_moderator_attempt_snapshot_state_len
        CHECK (LENGTH(snapshot_state_event_id) = 32),
    CONSTRAINT chk_meeting_moderator_attempt_snapshot_hash_len
        CHECK (LENGTH(candidate_snapshot_hash) = 32),
    CONSTRAINT chk_meeting_moderator_attempt_replacement_len
        CHECK (
            replacement_of_attempt_id IS NULL
            OR LENGTH(replacement_of_attempt_id) = 32
        ),
    CONSTRAINT chk_meeting_moderator_attempt_start_event_len
        CHECK (LENGTH(started_by_event_id) = 32),
    CONSTRAINT chk_meeting_moderator_attempt_terminal_event_len
        CHECK (
            terminal_event_id IS NULL
            OR LENGTH(terminal_event_id) = 32
        ),
    CONSTRAINT chk_meeting_moderator_attempt_epochs CHECK (
        control_epoch > 0
        AND decision_epoch > 0
        AND attempt_number > 0
        AND speech_revision >= 0
        AND snapshot_intent_revision >= 0
    ),
    CONSTRAINT chk_meeting_moderator_attempt_snapshot
        CHECK (jsonb_typeof(candidate_snapshot_json) = 'object'),
    CONSTRAINT chk_meeting_moderator_attempt_reason
        CHECK (
            terminal_reason IS NULL
            OR OCTET_LENGTH(terminal_reason) BETWEEN 1 AND 128
        ),
    CONSTRAINT chk_meeting_moderator_attempt_terminal_shape CHECK (
        (state = 'running'
            AND terminal_at IS NULL
            AND terminal_event_id IS NULL
            AND terminal_reason IS NULL)
        OR
        (state <> 'running' AND terminal_at IS NOT NULL)
    ),
    CONSTRAINT chk_meeting_moderator_attempt_time
        CHECK (deadline_at > started_at)
);

CREATE UNIQUE INDEX uq_meeting_running_moderator_attempt
    ON meeting_moderator_decision_attempts (community_id, session_id)
    WHERE state = 'running';

CREATE INDEX idx_meeting_moderator_attempt_epoch
    ON meeting_moderator_decision_attempts (
        community_id,
        session_id,
        decision_epoch,
        attempt_number
    );

ALTER TABLE meeting_baton_state
    ADD CONSTRAINT fk_meeting_baton_active_decision_attempt
    FOREIGN KEY (community_id, session_id, active_decision_attempt_id)
    REFERENCES meeting_moderator_decision_attempts (
        community_id,
        session_id,
        attempt_id
    );

CREATE TABLE meeting_moderator_retry_tickets (
    community_id                  UUID NOT NULL REFERENCES communities(id),
    session_id                    UUID NOT NULL,
    retry_ticket_id               BYTEA NOT NULL,
    attempt_id                    BYTEA NOT NULL,
    failed_action_event_id        BYTEA NOT NULL,
    source_type                   TEXT NOT NULL
        CHECK (source_type IN ('intent', 'handoff')),
    source_id                     BYTEA NOT NULL,
    snapshot_source_event_id      BYTEA,
    snapshot_handoff_attempt_count INT,
    conflict_code                 TEXT NOT NULL,
    control_epoch                 BIGINT NOT NULL,
    decision_epoch                BIGINT NOT NULL,
    deadline_at                   TIMESTAMPTZ NOT NULL,
    created_at                    TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    consumed_at                   TIMESTAMPTZ,
    consumed_by_event_id          BYTEA,
    PRIMARY KEY (community_id, session_id, retry_ticket_id),
    UNIQUE (community_id, failed_action_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    FOREIGN KEY (community_id, session_id, attempt_id)
        REFERENCES meeting_moderator_decision_attempts (
            community_id,
            session_id,
            attempt_id
        ),
    CONSTRAINT chk_meeting_retry_ticket_id_len
        CHECK (LENGTH(retry_ticket_id) = 32),
    CONSTRAINT chk_meeting_retry_ticket_attempt_id_len
        CHECK (LENGTH(attempt_id) = 32),
    CONSTRAINT chk_meeting_retry_ticket_failed_action_len
        CHECK (LENGTH(failed_action_event_id) = 32),
    CONSTRAINT chk_meeting_retry_ticket_source_id_len
        CHECK (LENGTH(source_id) = 32),
    CONSTRAINT chk_meeting_retry_ticket_source_event_len
        CHECK (
            snapshot_source_event_id IS NULL
            OR LENGTH(snapshot_source_event_id) = 32
        ),
    CONSTRAINT chk_meeting_retry_ticket_consumed_event_len
        CHECK (
            consumed_by_event_id IS NULL
            OR LENGTH(consumed_by_event_id) = 32
        ),
    CONSTRAINT chk_meeting_retry_ticket_source_version CHECK (
        (source_type = 'intent'
            AND snapshot_source_event_id IS NOT NULL
            AND snapshot_handoff_attempt_count IS NULL)
        OR
        (source_type = 'handoff'
            AND snapshot_source_event_id IS NULL
            AND snapshot_handoff_attempt_count IS NOT NULL
            AND snapshot_handoff_attempt_count >= 0)
    ),
    CONSTRAINT chk_meeting_retry_ticket_conflict_code
        CHECK (OCTET_LENGTH(conflict_code) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_retry_ticket_epochs CHECK (
        control_epoch > 0 AND decision_epoch > 0
    ),
    CONSTRAINT chk_meeting_retry_ticket_consumed_shape CHECK (
        (consumed_at IS NULL AND consumed_by_event_id IS NULL)
        OR
        (consumed_at IS NOT NULL AND consumed_by_event_id IS NOT NULL)
    ),
    CONSTRAINT chk_meeting_retry_ticket_deadline
        CHECK (deadline_at > created_at)
);

CREATE INDEX idx_meeting_retry_ticket_attempt
    ON meeting_moderator_retry_tickets (
        community_id,
        session_id,
        attempt_id,
        created_at
    );

ALTER TABLE meeting_v1_command_receipts
    ADD COLUMN retry_ticket_id BYTEA;

ALTER TABLE meeting_v1_command_receipts
    ADD CONSTRAINT chk_meeting_v1_receipt_retry_ticket_len
        CHECK (retry_ticket_id IS NULL OR LENGTH(retry_ticket_id) = 32);

ALTER TABLE meeting_v1_command_receipts
    ADD CONSTRAINT fk_meeting_v1_receipt_retry_ticket
        FOREIGN KEY (community_id, session_id, retry_ticket_id)
        REFERENCES meeting_moderator_retry_tickets (
            community_id,
            session_id,
            retry_ticket_id
        );

ALTER TABLE meeting_baton_state
    DROP CONSTRAINT chk_meeting_baton_state_phase_shape,
    ADD CONSTRAINT chk_meeting_baton_state_phase_shape CHECK (
        (phase = 'moderator_idle'
            AND active_offer_id IS NULL
            AND active_grant_id IS NULL
            AND (
                (active_decision_attempt_id IS NULL
                    AND moderator_decision_started_at IS NULL
                    AND moderator_decision_deadline IS NULL
                    AND next_action_at IS NULL)
                OR
                (moderator_decision_started_at IS NOT NULL
                    AND moderator_decision_deadline IS NOT NULL
                    AND next_action_at = moderator_decision_deadline)
            ))
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
            AND active_decision_attempt_id IS NULL
            AND moderator_decision_started_at IS NULL
            AND moderator_decision_deadline IS NULL
            AND next_action_at IS NULL)
    );
