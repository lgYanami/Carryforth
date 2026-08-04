-- Replace the unreleased planned Meeting action materializer with a minimal,
-- moderator-directed action-finalization lifecycle.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM meeting_sessions
        WHERE status = 'active'
          AND floor_policy_version = 'moderated-board-actions-v1'
    ) THEN
        RAISE EXCEPTION
            'cannot remove Meeting planned actions while an active moderated-board-actions-v1 Session exists';
    END IF;
END
$$;

ALTER TABLE meeting_sessions
    DROP CONSTRAINT chk_meeting_floor_policy,
    DROP CONSTRAINT chk_meeting_protocol_shape,
    ADD CONSTRAINT chk_meeting_floor_policy CHECK (
        floor_policy_version IN (
            'uniform-v0',
            'moderated-baton-v1',
            'moderated-board-v1',
            'moderated-board-actions-v1',
            'moderated-board-actions-v2'
        )
    ),
    ADD CONSTRAINT chk_meeting_protocol_shape CHECK (
        (
            schema_version = 1
            AND floor_policy_version = 'uniform-v0'
            AND moderator_pubkey IS NULL
        )
        OR
        (
            schema_version = 2
            AND floor_policy_version = 'moderated-baton-v1'
            AND moderator_pubkey IS NOT NULL
        )
        OR
        (
            schema_version = 3
            AND floor_policy_version IN (
                'moderated-board-v1',
                'moderated-board-actions-v1',
                'moderated-board-actions-v2'
            )
            AND moderator_pubkey = host_pubkey
        )
    );

DROP TABLE meeting_v2_action_step_attempts;
DROP TABLE meeting_v2_action_steps;
DROP TABLE meeting_v2_action_command_receipts;
DROP TABLE meeting_v2_action_runs;

CREATE TABLE meeting_v2_action_runs (
    community_id        UUID NOT NULL REFERENCES communities(id),
    session_id          UUID NOT NULL,
    action_run_id       UUID NOT NULL,
    begin_event_id      BYTEA NOT NULL,
    board_event_id      BYTEA NOT NULL,
    control_epoch       BIGINT NOT NULL,
    board_window        BIGINT NOT NULL,
    action_window_epoch BIGINT NOT NULL DEFAULT 1,
    action_condition    TEXT NOT NULL,
    terminal_status     TEXT,
    completion_event_id BYTEA,
    action_deadline_at  TIMESTAMPTZ,
    last_error_code     TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    terminal_at         TIMESTAMPTZ,
    PRIMARY KEY (community_id, session_id, action_run_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT uq_meeting_v2_action_begin
        UNIQUE (community_id, begin_event_id),
    CONSTRAINT chk_meeting_v2_action_begin_event
        CHECK (LENGTH(begin_event_id) = 32),
    CONSTRAINT chk_meeting_v2_action_board_event
        CHECK (LENGTH(board_event_id) = 32),
    CONSTRAINT chk_meeting_v2_action_control_epoch
        CHECK (control_epoch > 0),
    CONSTRAINT chk_meeting_v2_action_board_window
        CHECK (board_window > 0),
    CONSTRAINT chk_meeting_v2_action_window
        CHECK (action_window_epoch > 0),
    CONSTRAINT chk_meeting_v2_action_condition
        CHECK (action_condition IN ('runnable', 'blocked')),
    CONSTRAINT chk_meeting_v2_action_terminal
        CHECK (
            terminal_status IS NULL
            OR terminal_status IN (
                'completed_closed',
                'completed_aborted',
                'returned_to_board'
            )
        ),
    CONSTRAINT chk_meeting_v2_action_completion_event
        CHECK (completion_event_id IS NULL OR LENGTH(completion_event_id) = 32),
    CONSTRAINT chk_meeting_v2_action_error
        CHECK (
            last_error_code IS NULL
            OR OCTET_LENGTH(last_error_code) BETWEEN 1 AND 128
        ),
    CONSTRAINT chk_meeting_v2_action_live_shape CHECK (
        (
            terminal_status IS NULL
            AND terminal_at IS NULL
            AND completion_event_id IS NULL
            AND (
                (
                    action_condition = 'runnable'
                    AND action_deadline_at IS NOT NULL
                )
                OR
                (
                    action_condition = 'blocked'
                    AND action_deadline_at IS NULL
                )
            )
        )
        OR
        (
            terminal_status IS NOT NULL
            AND terminal_at IS NOT NULL
            AND action_deadline_at IS NULL
            AND (
                (
                    terminal_status = 'completed_closed'
                    AND completion_event_id IS NOT NULL
                )
                OR
                (
                    terminal_status IN ('completed_aborted', 'returned_to_board')
                    AND completion_event_id IS NULL
                )
            )
        )
    )
);

CREATE UNIQUE INDEX uq_meeting_v2_active_action_run
    ON meeting_v2_action_runs (community_id, session_id)
    WHERE terminal_status IS NULL;

CREATE INDEX idx_meeting_v2_action_deadline
    ON meeting_v2_action_runs (
        action_deadline_at,
        community_id,
        session_id,
        action_run_id
    )
    WHERE terminal_status IS NULL
      AND action_condition = 'runnable';

CREATE TABLE meeting_v2_action_command_receipts (
    community_id        UUID NOT NULL REFERENCES communities(id),
    session_id          UUID NOT NULL,
    command_event_id    BYTEA NOT NULL,
    author_pubkey       BYTEA NOT NULL,
    action              TEXT NOT NULL,
    action_run_id       UUID,
    action_window_epoch BIGINT,
    accepted            BOOLEAN NOT NULL,
    outcome_code        TEXT NOT NULL,
    response_json       JSONB NOT NULL,
    recorded_at         TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, command_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_v2_action_receipt_event
        CHECK (LENGTH(command_event_id) = 32),
    CONSTRAINT chk_meeting_v2_action_receipt_author
        CHECK (LENGTH(author_pubkey) = 32),
    CONSTRAINT chk_meeting_v2_action_receipt_action CHECK (
        action IN ('begin', 'block', 'retry', 'return-to-board')
    ),
    CONSTRAINT chk_meeting_v2_action_receipt_window
        CHECK (action_window_epoch IS NULL OR action_window_epoch > 0),
    CONSTRAINT chk_meeting_v2_action_receipt_outcome
        CHECK (OCTET_LENGTH(outcome_code) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_v2_action_receipt_response
        CHECK (jsonb_typeof(response_json) = 'object')
);

CREATE INDEX idx_meeting_v2_action_receipts_session
    ON meeting_v2_action_command_receipts (
        community_id,
        session_id,
        recorded_at,
        command_event_id
    );
