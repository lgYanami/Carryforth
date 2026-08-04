-- Migration 0043.
-- Meeting V2 stage 1: additive protocol identity, one relay-managed current
-- board, and an intentionally locked bootstrap runtime. No V2 floor mutation
-- is enabled by this migration.

ALTER TABLE meeting_sessions
    DROP CONSTRAINT chk_meeting_schema_version,
    DROP CONSTRAINT chk_meeting_floor_policy,
    DROP CONSTRAINT chk_meeting_protocol_shape,
    ADD CONSTRAINT chk_meeting_schema_version
        CHECK (schema_version IN (1, 2, 3)),
    ADD CONSTRAINT chk_meeting_floor_policy
        CHECK (floor_policy_version IN (
            'uniform-v0',
            'moderated-baton-v1',
            'moderated-board-v1'
        )),
    ADD CONSTRAINT chk_meeting_protocol_shape
        CHECK (
            (schema_version = 1
                AND floor_policy_version = 'uniform-v0'
                AND moderator_pubkey IS NULL)
            OR
            (schema_version = 2
                AND floor_policy_version = 'moderated-baton-v1'
                AND moderator_pubkey IS NOT NULL)
            OR
            (schema_version = 3
                AND floor_policy_version = 'moderated-board-v1'
                AND moderator_pubkey = host_pubkey)
        );

CREATE TABLE meeting_current_boards (
    community_id    UUID NOT NULL REFERENCES communities(id),
    session_id      UUID NOT NULL,
    board_event_id  BYTEA NOT NULL,
    board_format    TEXT NOT NULL,
    board_content   TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, session_id),
    UNIQUE (community_id, board_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_current_board_event_id_len
        CHECK (LENGTH(board_event_id) = 32),
    CONSTRAINT chk_meeting_current_board_format
        CHECK (board_format = 'markdown'),
    CONSTRAINT chk_meeting_current_board_content
        CHECK (
            OCTET_LENGTH(BTRIM(board_content)) > 0
            AND OCTET_LENGTH(board_content) <= 65536
        )
);

-- Stage one persists a fail-closed marker instead of pretending that the V2
-- Board/Floor runtime already exists. Stage two expands this projection and
-- owns every transition out of bootstrap_locked.
CREATE TABLE meeting_v2_bootstrap_state (
    community_id   UUID NOT NULL REFERENCES communities(id),
    session_id     UUID NOT NULL,
    runtime_phase  TEXT NOT NULL DEFAULT 'bootstrap_locked'
        CHECK (runtime_phase = 'bootstrap_locked'),
    control_epoch  BIGINT NOT NULL DEFAULT 1
        CHECK (control_epoch = 1),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, session_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id)
);
