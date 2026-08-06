-- Migration 0047.
-- One-shot cutover from fixed Meeting action deadlines to renewable leases.
-- Ended v2 action Meetings remain queryable history; no active v2 runtime is
-- converted in place.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM meeting_sessions
        WHERE status = 'active'
          AND floor_policy_version IN (
              'moderated-board-actions-v1',
              'moderated-board-actions-v2',
              'moderated-board-actions-v3'
          )
    ) OR EXISTS (
        SELECT 1
        FROM meeting_v2_action_runs
        WHERE terminal_status IS NULL
    ) THEN
        RAISE EXCEPTION
            'cannot enable renewable Meeting action leases while an action-capable Meeting or action run is active';
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
            'moderated-board-actions-v2',
            'moderated-board-actions-v3'
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
                'moderated-board-actions-v2',
                'moderated-board-actions-v3'
            )
            AND moderator_pubkey = host_pubkey
        )
    );

ALTER TABLE meeting_v2_config
    ALTER COLUMN action_finalization_ms SET DEFAULT 90000,
    ADD COLUMN action_operator_hard_cap_ms BIGINT DEFAULT 3600000,
    ADD CONSTRAINT chk_meeting_v2_action_operator_hard_cap_ms CHECK (
        action_operator_hard_cap_ms IS NULL
        OR action_operator_hard_cap_ms BETWEEN 300000 AND 86400000
    );

ALTER TABLE meeting_v2_action_runs
    ADD COLUMN progress_seq BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN last_progress_stage TEXT,
    ADD COLUMN last_progress_at TIMESTAMPTZ,
    ADD COLUMN operator_hard_deadline TIMESTAMPTZ,
    ADD CONSTRAINT chk_meeting_v2_action_progress_seq CHECK (progress_seq >= 0),
    ADD CONSTRAINT chk_meeting_v2_action_progress_stage CHECK (
        last_progress_stage IS NULL
        OR last_progress_stage IN (
            'reasoning',
            'tool_call',
            'tool_result',
            'finalizing',
            'waiting_human'
        )
    ),
    ADD CONSTRAINT chk_meeting_v2_action_progress_shape CHECK (
        (progress_seq = 0 AND last_progress_stage IS NULL AND last_progress_at IS NULL)
        OR (progress_seq > 0 AND last_progress_stage IS NOT NULL AND last_progress_at IS NOT NULL)
    ),
    ADD CONSTRAINT chk_meeting_v2_action_operator_deadline CHECK (
        operator_hard_deadline IS NULL OR operator_hard_deadline > created_at
    );

ALTER TABLE meeting_v2_action_command_receipts
    DROP CONSTRAINT chk_meeting_v2_action_receipt_action,
    ADD CONSTRAINT chk_meeting_v2_action_receipt_action CHECK (
        action IN ('begin', 'renew', 'block', 'retry', 'return-to-board')
    );

CREATE TABLE meeting_v2_action_lease_renewals (
    community_id        UUID NOT NULL REFERENCES communities(id),
    session_id          UUID NOT NULL,
    action_run_id       UUID NOT NULL,
    action_window_epoch BIGINT NOT NULL,
    progress_seq        BIGINT NOT NULL,
    renewal_event_id    BYTEA NOT NULL,
    stage               TEXT NOT NULL,
    last_activity_seq   BIGINT NOT NULL,
    accepted_at         TIMESTAMPTZ NOT NULL,
    lease_expires_at    TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (
        community_id,
        session_id,
        action_run_id,
        action_window_epoch,
        progress_seq
    ),
    UNIQUE (community_id, renewal_event_id),
    FOREIGN KEY (community_id, session_id, action_run_id)
        REFERENCES meeting_v2_action_runs (community_id, session_id, action_run_id),
    CONSTRAINT chk_meeting_v2_action_renewal_window CHECK (action_window_epoch > 0),
    CONSTRAINT chk_meeting_v2_action_renewal_progress CHECK (progress_seq > 0),
    CONSTRAINT chk_meeting_v2_action_renewal_event CHECK (LENGTH(renewal_event_id) = 32),
    CONSTRAINT chk_meeting_v2_action_renewal_stage CHECK (
        stage IN ('reasoning', 'tool_call', 'tool_result', 'finalizing', 'waiting_human')
    ),
    CONSTRAINT chk_meeting_v2_action_renewal_activity CHECK (last_activity_seq >= 0),
    CONSTRAINT chk_meeting_v2_action_renewal_expiry CHECK (lease_expires_at > accepted_at)
);

CREATE INDEX idx_meeting_v2_action_renewals_session
    ON meeting_v2_action_lease_renewals (
        community_id,
        session_id,
        accepted_at,
        progress_seq
    );
