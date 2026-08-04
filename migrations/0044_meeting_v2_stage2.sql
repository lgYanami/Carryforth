-- Migration 0044.
-- Meeting V2 stage 2: durable Board/Floor gate, independent Board timing,
-- command idempotency, and closed/aborted terminal classification.

ALTER TABLE meeting_sessions
    ADD COLUMN terminal_outcome TEXT,
    ADD COLUMN terminal_reason_code TEXT,
    DROP CONSTRAINT chk_meeting_terminal_shape,
    ADD CONSTRAINT chk_meeting_terminal_reason_code CHECK (
        terminal_reason_code IS NULL
        OR OCTET_LENGTH(terminal_reason_code) BETWEEN 1 AND 128
    ),
    ADD CONSTRAINT chk_meeting_terminal_shape CHECK (
        (
            status = 'active'
            AND ended_at IS NULL
            AND ended_by IS NULL
            AND end_event_id IS NULL
            AND terminal_outcome IS NULL
            AND terminal_reason_code IS NULL
        )
        OR
        (
            status = 'ended'
            AND ended_at IS NOT NULL
            AND ended_by IS NOT NULL
            AND end_event_id IS NOT NULL
            AND (
                (
                    schema_version IN (1, 2)
                    AND terminal_outcome IS NULL
                    AND terminal_reason_code IS NULL
                )
                OR
                (
                    schema_version = 3
                    AND (
                        (terminal_outcome = 'closed' AND terminal_reason_code IS NULL)
                        OR
                        (terminal_outcome = 'aborted' AND terminal_reason_code IS NOT NULL)
                    )
                )
            )
        )
    );

CREATE TABLE meeting_v2_config (
    community_id        UUID NOT NULL REFERENCES communities(id),
    session_id          UUID NOT NULL,
    timing_profile_version TEXT NOT NULL,
    board_maintenance_ms BIGINT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, session_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_v2_timing_profile
        CHECK (OCTET_LENGTH(timing_profile_version) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_v2_board_maintenance_ms
        CHECK (board_maintenance_ms BETWEEN 1 AND 86400000)
);

ALTER TABLE meeting_v2_bootstrap_state
    DROP CONSTRAINT meeting_v2_bootstrap_state_runtime_phase_check,
    DROP CONSTRAINT meeting_v2_bootstrap_state_control_epoch_check,
    ALTER COLUMN control_epoch DROP DEFAULT,
    ALTER COLUMN control_epoch SET DEFAULT 1,
    ADD COLUMN board_window BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN board_started_at TIMESTAMPTZ,
    ADD COLUMN board_deadline_at TIMESTAMPTZ,
    ADD COLUMN board_completed_at TIMESTAMPTZ,
    ADD COLUMN board_outcome TEXT,
    ADD COLUMN terminal_outcome TEXT,
    ADD COLUMN terminal_reason_code TEXT,
    ADD COLUMN terminal_at TIMESTAMPTZ,
    ADD CONSTRAINT chk_meeting_v2_runtime_phase CHECK (
        runtime_phase IN (
            'bootstrap_locked',
            'board_pending',
            'floor_ready',
            'ended'
        )
    ),
    ADD CONSTRAINT chk_meeting_v2_control_epoch CHECK (control_epoch > 0),
    ADD CONSTRAINT chk_meeting_v2_board_window CHECK (board_window >= 0),
    ADD CONSTRAINT chk_meeting_v2_board_outcome CHECK (
        board_outcome IS NULL
        OR board_outcome IN ('updated', 'unchanged', 'timed_out', 'preempted')
    ),
    ADD CONSTRAINT chk_meeting_v2_terminal_outcome CHECK (
        terminal_outcome IS NULL OR terminal_outcome IN ('closed', 'aborted')
    ),
    ADD CONSTRAINT chk_meeting_v2_terminal_reason CHECK (
        terminal_reason_code IS NULL
        OR OCTET_LENGTH(terminal_reason_code) BETWEEN 1 AND 128
    ),
    ADD CONSTRAINT chk_meeting_v2_runtime_shape CHECK (
        (
            runtime_phase = 'bootstrap_locked'
            AND board_window = 0
            AND board_started_at IS NULL
            AND board_deadline_at IS NULL
            AND board_completed_at IS NULL
            AND board_outcome IS NULL
            AND terminal_outcome IS NULL
            AND terminal_reason_code IS NULL
            AND terminal_at IS NULL
        )
        OR
        (
            runtime_phase = 'board_pending'
            AND board_window > 0
            AND board_started_at IS NOT NULL
            AND board_deadline_at > board_started_at
            AND board_completed_at IS NULL
            AND board_outcome IS NULL
            AND terminal_outcome IS NULL
            AND terminal_reason_code IS NULL
            AND terminal_at IS NULL
        )
        OR
        (
            runtime_phase = 'floor_ready'
            AND board_window > 0
            AND board_started_at IS NOT NULL
            AND board_deadline_at IS NULL
            AND board_completed_at IS NOT NULL
            AND board_outcome IS NOT NULL
            AND terminal_outcome IS NULL
            AND terminal_reason_code IS NULL
            AND terminal_at IS NULL
        )
        OR
        (
            runtime_phase = 'ended'
            AND board_deadline_at IS NULL
            AND terminal_outcome IS NOT NULL
            AND terminal_at IS NOT NULL
            AND (
                (terminal_outcome = 'closed' AND terminal_reason_code IS NULL)
                OR
                (terminal_outcome = 'aborted' AND terminal_reason_code IS NOT NULL)
            )
        )
    );

CREATE INDEX idx_meeting_v2_board_due
    ON meeting_v2_bootstrap_state (
        board_deadline_at,
        community_id,
        session_id
    )
    WHERE runtime_phase = 'board_pending';

CREATE TABLE meeting_v2_board_command_receipts (
    community_id        UUID NOT NULL REFERENCES communities(id),
    session_id          UUID NOT NULL,
    command_event_id    BYTEA NOT NULL,
    author_pubkey       BYTEA NOT NULL,
    action              TEXT NOT NULL CHECK (action IN ('update', 'unchanged')),
    accepted            BOOLEAN NOT NULL,
    outcome_class       TEXT NOT NULL CHECK (
        outcome_class IN ('accepted', 'rejected_terminal', 'rejected_after_recovery')
    ),
    outcome_code        TEXT NOT NULL,
    control_epoch       BIGINT,
    board_window        BIGINT,
    state_revision      BIGINT,
    board_event_id      BYTEA,
    response_json       JSONB NOT NULL,
    recorded_at         TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, command_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_v2_board_receipt_event_id
        CHECK (LENGTH(command_event_id) = 32),
    CONSTRAINT chk_meeting_v2_board_receipt_author
        CHECK (LENGTH(author_pubkey) = 32),
    CONSTRAINT chk_meeting_v2_board_receipt_outcome
        CHECK (OCTET_LENGTH(outcome_code) BETWEEN 1 AND 128),
    CONSTRAINT chk_meeting_v2_board_receipt_epoch
        CHECK (control_epoch IS NULL OR control_epoch > 0),
    CONSTRAINT chk_meeting_v2_board_receipt_window
        CHECK (board_window IS NULL OR board_window > 0),
    CONSTRAINT chk_meeting_v2_board_receipt_state
        CHECK (state_revision IS NULL OR state_revision > 0),
    CONSTRAINT chk_meeting_v2_board_receipt_board_event
        CHECK (board_event_id IS NULL OR LENGTH(board_event_id) = 32),
    CONSTRAINT chk_meeting_v2_board_receipt_response
        CHECK (jsonb_typeof(response_json) = 'object')
);

CREATE INDEX idx_meeting_v2_board_receipts_session
    ON meeting_v2_board_command_receipts (
        community_id,
        session_id,
        recorded_at,
        command_event_id
    );
