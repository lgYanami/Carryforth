-- Migration 0039.
-- Meeting V0 stage 3: Agent Ready/Pass/Yield signals, a durable decision
-- cohort, early arbitration, and explicit yielded rounds.

ALTER TABLE meeting_rounds
    ADD COLUMN settle_not_before TIMESTAMPTZ;

-- Preserve already-open Stage 2 competitions across a rolling upgrade.  They
-- settle at their original deadline; only competitions started after this
-- migration receive the new three-second early-settlement boundary.
UPDATE meeting_rounds
SET settle_not_before = claim_deadline
WHERE phase IN ('claiming', 'granted');

ALTER TABLE meeting_rounds
    DROP CONSTRAINT chk_meeting_round_phase_shape,
    DROP CONSTRAINT meeting_rounds_outcome_check,
    ADD CONSTRAINT meeting_rounds_outcome_check
        CHECK (outcome IN ('spoken', 'yielded', 'expired', 'ended')),
    ADD CONSTRAINT chk_meeting_round_phase_shape CHECK (
        (phase = 'open'
            AND settle_not_before IS NULL
            AND claim_deadline IS NULL
            AND holder_pubkey IS NULL
            AND grant_event_id IS NULL
            AND lease_expires_at IS NULL
            AND outcome IS NULL
            AND speech_event_id IS NULL)
        OR
        (phase = 'claiming'
            AND settle_not_before IS NOT NULL
            AND claim_deadline IS NOT NULL
            AND settle_not_before <= claim_deadline
            AND holder_pubkey IS NULL
            AND grant_event_id IS NULL
            AND lease_expires_at IS NULL
            AND outcome IS NULL
            AND speech_event_id IS NULL)
        OR
        (phase = 'granted'
            AND settle_not_before IS NOT NULL
            AND claim_deadline IS NOT NULL
            AND settle_not_before <= claim_deadline
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
                (outcome IN ('yielded', 'expired', 'ended')
                    AND speech_event_id IS NULL)
            ))
    );

DROP INDEX idx_meeting_rounds_due;
CREATE INDEX idx_meeting_rounds_due
    ON meeting_rounds (
        community_id,
        phase,
        settle_not_before,
        claim_deadline,
        lease_expires_at
    )
    WHERE phase IN ('claiming', 'granted');

CREATE TABLE meeting_floor_signals (
    community_id       UUID NOT NULL REFERENCES communities(id),
    session_id         UUID NOT NULL,
    round_number       BIGINT NOT NULL,
    participant_pubkey BYTEA NOT NULL,
    action             TEXT NOT NULL CHECK (action IN ('ready', 'pass', 'yield')),
    intent_basis        TEXT,
    grant_event_id      BYTEA,
    signal_event_id     BYTEA NOT NULL,
    floor_revision      BIGINT NOT NULL,
    received_at         TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (community_id, signal_event_id),
    FOREIGN KEY (community_id, session_id, round_number)
        REFERENCES meeting_rounds (community_id, session_id, round_number),
    CONSTRAINT chk_meeting_signal_pubkey_len
        CHECK (LENGTH(participant_pubkey) = 32),
    CONSTRAINT chk_meeting_signal_event_id_len
        CHECK (LENGTH(signal_event_id) = 32),
    CONSTRAINT chk_meeting_signal_grant_id_len
        CHECK (grant_event_id IS NULL OR LENGTH(grant_event_id) = 32),
    CONSTRAINT chk_meeting_signal_revision_positive CHECK (floor_revision > 0),
    CONSTRAINT chk_meeting_signal_shape CHECK (
        (action IN ('ready', 'pass')
            AND intent_basis IS NOT NULL
            AND LENGTH(intent_basis) BETWEEN 1 AND 512
            AND grant_event_id IS NULL)
        OR
        (action = 'yield'
            AND intent_basis IS NULL
            AND grant_event_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX uq_meeting_floor_intent_signal
    ON meeting_floor_signals (
        community_id,
        session_id,
        round_number,
        participant_pubkey,
        action,
        intent_basis
    )
    WHERE action IN ('ready', 'pass');

CREATE UNIQUE INDEX uq_meeting_floor_yield_signal
    ON meeting_floor_signals (community_id, grant_event_id)
    WHERE action = 'yield';

CREATE INDEX idx_meeting_floor_signals_round
    ON meeting_floor_signals (
        community_id,
        session_id,
        round_number,
        action,
        participant_pubkey
    );

CREATE TABLE meeting_round_decision_cohort (
    community_id       UUID NOT NULL REFERENCES communities(id),
    session_id         UUID NOT NULL,
    round_number       BIGINT NOT NULL,
    participant_pubkey BYTEA NOT NULL,
    ready_event_id     BYTEA NOT NULL,
    frozen_at          TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (community_id, session_id, round_number, participant_pubkey),
    FOREIGN KEY (community_id, session_id, round_number)
        REFERENCES meeting_rounds (community_id, session_id, round_number),
    CONSTRAINT chk_meeting_cohort_pubkey_len
        CHECK (LENGTH(participant_pubkey) = 32),
    CONSTRAINT chk_meeting_cohort_ready_event_id_len
        CHECK (LENGTH(ready_event_id) = 32)
);
