-- Migration 0038.
-- Meeting V0 stage 2: relay-authoritative speech rounds, canonical claims,
-- single-use grants, and a transactional fan-out outbox.

ALTER TABLE meeting_sessions
    ADD COLUMN current_round BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN floor_revision BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN floor_policy_version TEXT NOT NULL DEFAULT 'uniform-v0',
    ADD CONSTRAINT chk_meeting_current_round_positive CHECK (current_round > 0),
    ADD CONSTRAINT chk_meeting_floor_revision_nonnegative CHECK (floor_revision >= 0),
    ADD CONSTRAINT chk_meeting_floor_policy
        CHECK (floor_policy_version = 'uniform-v0');

CREATE TABLE meeting_rounds (
    community_id      UUID NOT NULL REFERENCES communities(id),
    session_id        UUID NOT NULL,
    round_number      BIGINT NOT NULL,
    floor_revision    BIGINT NOT NULL,
    phase             TEXT NOT NULL
        CHECK (phase IN ('open', 'claiming', 'granted', 'closed')),
    state_event_id    BYTEA NOT NULL,
    claim_deadline    TIMESTAMPTZ,
    holder_pubkey     BYTEA,
    grant_event_id    BYTEA,
    lease_expires_at  TIMESTAMPTZ,
    outcome           TEXT CHECK (outcome IN ('spoken', 'expired', 'ended')),
    speech_event_id   BYTEA,
    policy_version    TEXT NOT NULL DEFAULT 'uniform-v0'
        CHECK (policy_version = 'uniform-v0'),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, session_id, round_number),
    UNIQUE (community_id, state_event_id),
    UNIQUE (community_id, grant_event_id),
    UNIQUE (community_id, speech_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_round_number_positive CHECK (round_number > 0),
    CONSTRAINT chk_meeting_round_revision_positive CHECK (floor_revision > 0),
    CONSTRAINT chk_meeting_round_state_event_id_len
        CHECK (LENGTH(state_event_id) = 32),
    CONSTRAINT chk_meeting_round_holder_len
        CHECK (holder_pubkey IS NULL OR LENGTH(holder_pubkey) = 32),
    CONSTRAINT chk_meeting_round_grant_event_id_len
        CHECK (grant_event_id IS NULL OR LENGTH(grant_event_id) = 32),
    CONSTRAINT chk_meeting_round_speech_event_id_len
        CHECK (speech_event_id IS NULL OR LENGTH(speech_event_id) = 32),
    CONSTRAINT chk_meeting_round_phase_shape CHECK (
        (phase = 'open'
            AND claim_deadline IS NULL
            AND holder_pubkey IS NULL
            AND grant_event_id IS NULL
            AND lease_expires_at IS NULL
            AND outcome IS NULL
            AND speech_event_id IS NULL)
        OR
        (phase = 'claiming'
            AND claim_deadline IS NOT NULL
            AND holder_pubkey IS NULL
            AND grant_event_id IS NULL
            AND lease_expires_at IS NULL
            AND outcome IS NULL
            AND speech_event_id IS NULL)
        OR
        (phase = 'granted'
            AND claim_deadline IS NOT NULL
            AND holder_pubkey IS NOT NULL
            AND grant_event_id IS NOT NULL
            AND lease_expires_at IS NOT NULL
            AND outcome IS NULL
            AND speech_event_id IS NULL)
        OR
        (phase = 'closed'
            AND outcome IS NOT NULL
            AND (
                (outcome = 'spoken' AND speech_event_id IS NOT NULL)
                OR
                (outcome IN ('expired', 'ended') AND speech_event_id IS NULL)
            ))
    )
);

CREATE INDEX idx_meeting_rounds_due
    ON meeting_rounds (community_id, phase, claim_deadline, lease_expires_at)
    WHERE phase IN ('claiming', 'granted');

CREATE TABLE meeting_floor_claims (
    community_id      UUID NOT NULL REFERENCES communities(id),
    session_id        UUID NOT NULL,
    round_number      BIGINT NOT NULL,
    claimant_pubkey   BYTEA NOT NULL,
    claim_event_id    BYTEA NOT NULL,
    floor_revision    BIGINT NOT NULL,
    received_at       TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (community_id, session_id, round_number, claimant_pubkey),
    UNIQUE (community_id, claim_event_id),
    FOREIGN KEY (community_id, session_id, round_number)
        REFERENCES meeting_rounds (community_id, session_id, round_number),
    CONSTRAINT chk_meeting_claim_claimant_len CHECK (LENGTH(claimant_pubkey) = 32),
    CONSTRAINT chk_meeting_claim_event_id_len CHECK (LENGTH(claim_event_id) = 32),
    CONSTRAINT chk_meeting_claim_revision_positive CHECK (floor_revision > 0)
);

CREATE INDEX idx_meeting_floor_claims_round
    ON meeting_floor_claims (community_id, session_id, round_number, claim_event_id);

CREATE TABLE meeting_event_outbox (
    community_id      UUID NOT NULL REFERENCES communities(id),
    sequence          BIGINT GENERATED ALWAYS AS IDENTITY,
    session_id        UUID NOT NULL,
    event_id          BYTEA NOT NULL,
    available_at      TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    claimed_by        UUID,
    claimed_until     TIMESTAMPTZ,
    attempts          INT NOT NULL DEFAULT 0,
    delivered_at      TIMESTAMPTZ,
    last_error        TEXT,
    PRIMARY KEY (community_id, sequence),
    UNIQUE (community_id, event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES meeting_sessions (community_id, session_id),
    CONSTRAINT chk_meeting_outbox_event_id_len CHECK (LENGTH(event_id) = 32),
    CONSTRAINT chk_meeting_outbox_attempts_nonnegative CHECK (attempts >= 0)
);

CREATE INDEX idx_meeting_event_outbox_pending
    ON meeting_event_outbox (available_at, sequence)
    WHERE delivered_at IS NULL;
