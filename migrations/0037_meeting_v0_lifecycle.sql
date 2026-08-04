-- Migration 0037.
-- Meeting V0 stage 1: distinguish meeting rooms from ordinary channels and
-- persist the lifecycle projection used for atomic create/end commands.

ALTER TABLE channels
    ADD COLUMN room_kind TEXT NOT NULL DEFAULT 'standard',
    ADD CONSTRAINT chk_channels_room_kind
        CHECK (room_kind IN ('standard', 'meeting'));

CREATE INDEX idx_channels_community_room_kind
    ON channels (community_id, room_kind)
    WHERE deleted_at IS NULL;

CREATE TABLE meeting_sessions (
    community_id      UUID NOT NULL REFERENCES communities(id),
    session_id        UUID NOT NULL,
    create_event_id   BYTEA NOT NULL,
    host_pubkey       BYTEA NOT NULL,
    source_channel_id UUID,
    schema_version    INT NOT NULL DEFAULT 1,
    status            TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'ended')),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at          TIMESTAMPTZ,
    ended_by          BYTEA,
    end_event_id      BYTEA,
    PRIMARY KEY (community_id, session_id),
    UNIQUE (community_id, create_event_id),
    UNIQUE (community_id, end_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES channels (community_id, id),
    FOREIGN KEY (community_id, source_channel_id)
        REFERENCES channels (community_id, id),
    CONSTRAINT chk_meeting_create_event_id_len
        CHECK (LENGTH(create_event_id) = 32),
    CONSTRAINT chk_meeting_host_pubkey_len
        CHECK (LENGTH(host_pubkey) = 32),
    CONSTRAINT chk_meeting_end_event_id_len
        CHECK (end_event_id IS NULL OR LENGTH(end_event_id) = 32),
    CONSTRAINT chk_meeting_ended_by_len
        CHECK (ended_by IS NULL OR LENGTH(ended_by) = 32),
    CONSTRAINT chk_meeting_terminal_shape
        CHECK (
            (status = 'active'
                AND ended_at IS NULL
                AND ended_by IS NULL
                AND end_event_id IS NULL)
            OR
            (status = 'ended'
                AND ended_at IS NOT NULL
                AND ended_by IS NOT NULL
                AND end_event_id IS NOT NULL)
        )
);

CREATE INDEX idx_meeting_sessions_status
    ON meeting_sessions (community_id, status, created_at DESC);
