-- Migration 0045.
-- Meeting V2 action finalization: an optional, policy-discriminated lifecycle
-- stage between the final Floor decision and normal close.

ALTER TABLE meeting_sessions
    DROP CONSTRAINT chk_meeting_floor_policy,
    DROP CONSTRAINT chk_meeting_protocol_shape,
    ADD CONSTRAINT chk_meeting_floor_policy CHECK (
        floor_policy_version IN (
            'uniform-v0',
            'moderated-baton-v1',
            'moderated-board-v1',
            'moderated-board-actions-v1'
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
                'moderated-board-actions-v1'
            )
            AND moderator_pubkey = host_pubkey
        )
    );

ALTER TABLE meeting_v2_config
    ADD COLUMN action_finalization_ms BIGINT NOT NULL DEFAULT 300000,
    ADD CONSTRAINT chk_meeting_v2_action_finalization_ms
        CHECK (action_finalization_ms BETWEEN 1 AND 86400000);

ALTER TABLE meeting_v2_bootstrap_state
    DROP CONSTRAINT chk_meeting_v2_runtime_phase,
    DROP CONSTRAINT chk_meeting_v2_runtime_shape,
    ADD CONSTRAINT chk_meeting_v2_runtime_phase CHECK (
        runtime_phase IN (
            'bootstrap_locked',
            'board_pending',
            'floor_ready',
            'finalizing_actions',
            'ended'
        )
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
            runtime_phase = 'finalizing_actions'
            AND board_window > 0
            AND board_started_at IS NOT NULL
            AND board_deadline_at IS NULL
            AND board_completed_at IS NOT NULL
            AND board_outcome IN ('updated', 'unchanged')
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

CREATE TABLE meeting_v2_action_runs (
    community_id        UUID NOT NULL REFERENCES communities(id),
    session_id          UUID NOT NULL,
    action_run_id       UUID NOT NULL,
    begin_event_id      BYTEA NOT NULL,
    plan_event_id       BYTEA,
    board_event_id      BYTEA NOT NULL,
    control_epoch       BIGINT NOT NULL,
    board_window        BIGINT NOT NULL,
    action_window_epoch BIGINT NOT NULL DEFAULT 1,
    action_phase        TEXT NOT NULL,
    action_condition    TEXT NOT NULL,
    terminal_status     TEXT,
    completion_project_revision BIGINT,
    action_deadline_at  TIMESTAMPTZ,
    last_error_code     TEXT,
    plan_json           JSONB,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    terminal_at         TIMESTAMPTZ,
    PRIMARY KEY (community_id, session_id, action_run_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT uq_meeting_v2_action_begin
        UNIQUE (community_id, begin_event_id),
    CONSTRAINT uq_meeting_v2_action_plan
        UNIQUE (community_id, plan_event_id),
    CONSTRAINT chk_meeting_v2_action_begin_event
        CHECK (LENGTH(begin_event_id) = 32),
    CONSTRAINT chk_meeting_v2_action_plan_event
        CHECK (plan_event_id IS NULL OR LENGTH(plan_event_id) = 32),
    CONSTRAINT chk_meeting_v2_action_board_event
        CHECK (LENGTH(board_event_id) = 32),
    CONSTRAINT chk_meeting_v2_action_control_epoch
        CHECK (control_epoch > 0),
    CONSTRAINT chk_meeting_v2_action_board_window
        CHECK (board_window > 0),
    CONSTRAINT chk_meeting_v2_action_window
        CHECK (action_window_epoch > 0),
    CONSTRAINT chk_meeting_v2_action_phase
        CHECK (action_phase IN ('planning', 'applying', 'ready_to_close')),
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
    CONSTRAINT chk_meeting_v2_action_completion_revision
        CHECK (
            completion_project_revision IS NULL
            OR completion_project_revision > 0
        ),
    CONSTRAINT chk_meeting_v2_action_error
        CHECK (
            last_error_code IS NULL
            OR OCTET_LENGTH(last_error_code) BETWEEN 1 AND 128
        ),
    CONSTRAINT chk_meeting_v2_action_plan_json
        CHECK (plan_json IS NULL OR jsonb_typeof(plan_json) = 'object'),
    CONSTRAINT chk_meeting_v2_action_plan_phase CHECK (
        (action_phase = 'planning' AND plan_event_id IS NULL AND plan_json IS NULL)
        OR
        (action_phase IN ('applying', 'ready_to_close')
            AND plan_event_id IS NOT NULL AND plan_json IS NOT NULL)
    ),
    CONSTRAINT chk_meeting_v2_action_live_shape CHECK (
        (
            terminal_status IS NULL
            AND terminal_at IS NULL
            AND (
                (
                    action_phase IN ('planning', 'applying')
                    AND action_condition = 'runnable'
                    AND action_deadline_at IS NOT NULL
                )
                OR
                (
                    action_phase = 'ready_to_close'
                    AND action_condition = 'runnable'
                    AND action_deadline_at IS NULL
                )
                OR
                (action_condition = 'blocked' AND action_deadline_at IS NULL)
            )
        )
        OR
        (
            terminal_status IS NOT NULL
            AND terminal_at IS NOT NULL
            AND action_deadline_at IS NULL
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

CREATE TABLE meeting_v2_action_steps (
    community_id        UUID NOT NULL REFERENCES communities(id),
    session_id          UUID NOT NULL,
    action_run_id       UUID NOT NULL,
    action_id           UUID,
    step_id             UUID NOT NULL,
    step_order          INTEGER NOT NULL,
    step_kind           TEXT NOT NULL,
    desired_payload     JSONB NOT NULL,
    assignee_pubkey     BYTEA,
    resolved_role_id    UUID,
    resolved_assignment_id UUID,
    target_object_type  TEXT NOT NULL,
    target_object_id    UUID NOT NULL,
    accepted_project_revision BIGINT,
    status              TEXT NOT NULL DEFAULT 'pending',
    last_error_code     TEXT,
    attempt_count       INTEGER NOT NULL DEFAULT 0,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, session_id, action_run_id, step_id),
    FOREIGN KEY (community_id, session_id, action_run_id)
        REFERENCES meeting_v2_action_runs (
            community_id,
            session_id,
            action_run_id
        ),
    CONSTRAINT uq_meeting_v2_action_step_order
        UNIQUE (community_id, session_id, action_run_id, step_order),
    CONSTRAINT chk_meeting_v2_action_step_order
        CHECK (step_order > 0),
    CONSTRAINT chk_meeting_v2_action_step_kind CHECK (
        step_kind IN (
            'project_view.create_requirement',
            'project_view.create_work',
            'project_view.set_work_responsibility'
        )
    ),
    CONSTRAINT chk_meeting_v2_action_step_payload
        CHECK (jsonb_typeof(desired_payload) = 'object'),
    CONSTRAINT chk_meeting_v2_action_step_assignee
        CHECK (assignee_pubkey IS NULL OR LENGTH(assignee_pubkey) = 32),
    CONSTRAINT chk_meeting_v2_action_step_revision
        CHECK (
            accepted_project_revision IS NULL
            OR accepted_project_revision > 0
        ),
    CONSTRAINT chk_meeting_v2_action_step_status
        CHECK (status IN ('pending', 'prepared', 'applied', 'failed', 'abandoned')),
    CONSTRAINT chk_meeting_v2_action_step_error
        CHECK (
            last_error_code IS NULL
            OR OCTET_LENGTH(last_error_code) BETWEEN 1 AND 128
        ),
    CONSTRAINT chk_meeting_v2_action_step_attempt_count
        CHECK (attempt_count >= 0)
);

CREATE TABLE meeting_v2_action_step_attempts (
    community_id        UUID NOT NULL REFERENCES communities(id),
    session_id          UUID NOT NULL,
    action_run_id       UUID NOT NULL,
    step_id             UUID NOT NULL,
    action_window_epoch BIGINT NOT NULL,
    attempt_number      INTEGER NOT NULL,
    project_command_event_id BYTEA,
    signed_project_event JSONB,
    expected_project_revision BIGINT,
    accepted_project_revision BIGINT,
    status              TEXT NOT NULL,
    error_code          TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (
        community_id,
        session_id,
        action_run_id,
        step_id,
        attempt_number
    ),
    FOREIGN KEY (community_id, session_id, action_run_id, step_id)
        REFERENCES meeting_v2_action_steps (
            community_id,
            session_id,
            action_run_id,
            step_id
        ),
    CONSTRAINT chk_meeting_v2_action_attempt_window
        CHECK (action_window_epoch > 0),
    CONSTRAINT chk_meeting_v2_action_attempt_number
        CHECK (attempt_number > 0),
    CONSTRAINT chk_meeting_v2_action_project_event
        CHECK (
            project_command_event_id IS NULL
            OR LENGTH(project_command_event_id) = 32
        ),
    CONSTRAINT chk_meeting_v2_action_signed_event
        CHECK (
            signed_project_event IS NULL
            OR jsonb_typeof(signed_project_event) = 'object'
        ),
    CONSTRAINT chk_meeting_v2_action_attempt_revisions CHECK (
        (expected_project_revision IS NULL OR expected_project_revision >= 0)
        AND
        (accepted_project_revision IS NULL OR accepted_project_revision > 0)
    ),
    CONSTRAINT chk_meeting_v2_action_attempt_status CHECK (
        status IN (
            'prepared',
            'published',
            'accepted',
            'rejected',
            'indeterminate',
            'abandoned'
        )
    ),
    CONSTRAINT chk_meeting_v2_action_attempt_error
        CHECK (error_code IS NULL OR OCTET_LENGTH(error_code) BETWEEN 1 AND 128)
);

CREATE UNIQUE INDEX uq_meeting_v2_action_project_event
    ON meeting_v2_action_step_attempts (community_id, project_command_event_id)
    WHERE project_command_event_id IS NOT NULL;

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
        action IN (
            'begin',
            'plan',
            'step-prepared',
            'step-applied',
            'block',
            'complete',
            'retry',
            'return-to-board'
        )
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
