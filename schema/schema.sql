-- Buzz initial Postgres schema — multi-tenant.
--
-- Source of truth for fresh database setup. This is a clean, from-scratch
-- schema in which `community_id` is a first-class, server-resolved key on
-- every tenant-scoped row. It is NOT additive over the single-community
-- schema; the rewrite replaces it. Existing single-community deployments
-- migrate via the documented backfill migration (0002), which assigns all
-- pre-existing rows to one default community.
--
-- The governing contract is docs/multi-tenant-conformance.md. Every table
-- below cites the conformance surface it implements. The invariant behind the
-- whole schema (conformance "row zero"): a request's community is resolved
-- from the connection host by the server, never supplied by the client, and
-- every scoped row carries that immutable `community_id`.
--
-- Migration-lint obligations enforced by the Lane 0 lint harness:
--   1. Every tenant-scoped table has `community_id NOT NULL`.
--   2. No UNIQUE / PRIMARY KEY / FK on a scoped table is observable across
--      communities: each leads with `community_id` (or, for child rows whose
--      parent already pins the community, joins carry the community tuple).
--   3. `channels.community_id` is immutable (trigger below; no UPDATE path).
--   4. Operator-global tables are named in the explicit allowlist, not implied.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- ── Custom types ──────────────────────────────────────────────────────────────

CREATE TYPE channel_type AS ENUM ('stream', 'forum', 'dm', 'workflow');
CREATE TYPE channel_visibility AS ENUM ('open', 'private');
CREATE TYPE member_role AS ENUM ('owner', 'admin', 'member', 'guest', 'bot');
CREATE TYPE workflow_status AS ENUM ('active', 'disabled', 'archived');
CREATE TYPE run_status AS ENUM ('pending', 'running', 'waiting_approval', 'completed', 'failed', 'cancelled');
CREATE TYPE approval_status AS ENUM ('pending', 'granted', 'denied', 'expired');
CREATE TYPE delivery_method AS ENUM ('webhook', 'websocket');
CREATE TYPE subscription_status AS ENUM ('active', 'paused', 'deleted');
CREATE TYPE pause_reason AS ENUM ('user', 'system', 'rate_limit');
CREATE TYPE channel_add_policy AS ENUM ('anyone', 'owner_only', 'nobody');

-- ── Communities ───────────────────────────────────────────────────────────────
-- Conformance: row zero (host binding). The host map. `resolve_host(host)`
-- reads exactly one row here to mint the request's TenantContext. This table
-- is OPERATOR-GLOBAL: it is the registry of tenants, not itself tenant-scoped,
-- so it carries no `community_id` of its own (its `id` IS the community key).
-- Listed in the lint allowlist as operator-global.
--
-- Host normalization (Lane 0 contract): `host` is stored already-normalized —
-- ASCII-lowercased, trailing dot stripped, default port omitted. The UNIQUE is
-- on `lower(host)` belt-and-suspenders so `Relay.Example` and `relay.example`
-- can never become two tenants even if a writer forgets to normalize.
-- `resolve_host()` (buzz-core) applies the identical normalization before
-- lookup, so resolution and storage agree by construction.

CREATE TABLE communities (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    host            VARCHAR(255) NOT NULL,
    signing_key     BYTEA,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    archived_at     TIMESTAMPTZ,
    CONSTRAINT chk_communities_id_not_nil CHECK (id <> '00000000-0000-0000-0000-000000000000'::uuid)
);

CREATE UNIQUE INDEX idx_communities_host ON communities (lower(host));

-- ── Channels ──────────────────────────────────────────────────────────────────
-- Conformance: "Channels and channel membership". `community_id` immutable.
-- Channel UUIDs stay valid wire identifiers, but they are NOT globally unique:
-- the PK is `(community_id, id)`, so the same UUID may legitimately exist in two
-- communities (conformance lists "same channel UUID collision in two
-- communities" as a required isolation test). Handlers always carry `ctx`, so
-- `(ctx.community, h)` names exactly one channel; a client-supplied `h` can
-- never reach another community's channel.

CREATE TABLE channels (
    id              UUID NOT NULL DEFAULT gen_random_uuid(),
    community_id    UUID NOT NULL REFERENCES communities(id),
    name            VARCHAR(255) NOT NULL,
    channel_type    channel_type NOT NULL DEFAULT 'stream',
    visibility      channel_visibility NOT NULL DEFAULT 'open',
    description     TEXT,
    canvas          TEXT,
    created_by      BYTEA NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    archived_at     TIMESTAMPTZ,
    deleted_at      TIMESTAMPTZ,
    nip29_group_id  VARCHAR(255),
    topic_required  BOOLEAN NOT NULL DEFAULT FALSE,
    max_members     INT,
    topic           TEXT,
    topic_set_by    BYTEA,
    topic_set_at    TIMESTAMPTZ,
    purpose         TEXT,
    purpose_set_by  BYTEA,
    purpose_set_at  TIMESTAMPTZ,
    participant_hash BYTEA,
    ttl_seconds     INT,
    ttl_deadline    TIMESTAMPTZ,
    room_kind       TEXT NOT NULL DEFAULT 'standard'
        CHECK (room_kind IN ('standard', 'meeting')),
    PRIMARY KEY (community_id, id),
    CONSTRAINT chk_channels_id_not_nil CHECK (id <> '00000000-0000-0000-0000-000000000000'::uuid)
);

-- nip29 group id and DM participant hash are unique WITHIN a community, not globally.
CREATE UNIQUE INDEX idx_channels_nip29_group ON channels (community_id, nip29_group_id)
    WHERE nip29_group_id IS NOT NULL;
CREATE UNIQUE INDEX idx_channels_dm_hash ON channels (community_id, participant_hash)
    WHERE participant_hash IS NOT NULL;
CREATE INDEX idx_channels_community_type ON channels (community_id, channel_type);
CREATE INDEX idx_channels_community_visibility ON channels (community_id, visibility);
CREATE INDEX idx_channels_created_by ON channels (community_id, created_by);
CREATE INDEX idx_channels_ttl_expiry ON channels (ttl_deadline)
    WHERE ttl_seconds IS NOT NULL AND archived_at IS NULL AND deleted_at IS NULL;
CREATE INDEX idx_channels_community_room_kind ON channels (community_id, room_kind)
    WHERE deleted_at IS NULL;

-- channels.community_id is immutable: a channel can never be re-tenanted.
-- (Conformance: "Migration lint forbids channel re-tenanting except through an
-- explicitly modeled admission path." We have no such path, so: hard block.)
CREATE FUNCTION channels_community_id_immutable() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.community_id IS DISTINCT FROM OLD.community_id THEN
        RAISE EXCEPTION 'channels.community_id is immutable (channel % cannot be re-tenanted)', OLD.id
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_channels_community_id_immutable
    BEFORE UPDATE ON channels
    FOR EACH ROW EXECUTE FUNCTION channels_community_id_immutable();

-- ── Channel members ───────────────────────────────────────────────────────────
-- Conformance: "Channels and channel membership". PK leads with community_id.

CREATE TABLE channel_members (
    community_id UUID NOT NULL REFERENCES communities(id),
    channel_id  UUID NOT NULL,
    pubkey      BYTEA NOT NULL,
    role        member_role NOT NULL DEFAULT 'member',
    joined_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    invited_by  BYTEA,
    removed_at  TIMESTAMPTZ,
    removed_by  BYTEA,
    hidden_at   TIMESTAMPTZ,
    PRIMARY KEY (community_id, channel_id, pubkey),
    FOREIGN KEY (community_id, channel_id)
        REFERENCES channels (community_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_channel_members_pubkey ON channel_members (community_id, pubkey)
    WHERE removed_at IS NULL;

-- ── Meeting sessions ─────────────────────────────────────────────────────────
-- Meeting V0 reuses a private stream channel for messages and membership. This
-- table is the durable lifecycle projection; session_id is the channel UUID.

CREATE SEQUENCE meeting_security_order_seq AS BIGINT START WITH 1;

CREATE TABLE meeting_sessions (
    community_id      UUID NOT NULL REFERENCES communities(id),
    session_id        UUID NOT NULL,
    create_event_id   BYTEA NOT NULL,
    host_pubkey       BYTEA NOT NULL,
    source_channel_id UUID,
    schema_version    INT NOT NULL DEFAULT 1,
    status            TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'ended')),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    security_order    BIGINT NOT NULL DEFAULT nextval('meeting_security_order_seq')
        CHECK (security_order > 0),
    ended_at          TIMESTAMPTZ,
    ended_by          BYTEA,
    end_event_id      BYTEA,
    current_round     BIGINT NOT NULL DEFAULT 1,
    floor_revision    BIGINT NOT NULL DEFAULT 0,
    floor_policy_version TEXT NOT NULL DEFAULT 'uniform-v0',
    moderator_pubkey  BYTEA,
    PRIMARY KEY (community_id, session_id),
    UNIQUE (community_id, create_event_id),
    UNIQUE (community_id, end_event_id),
    FOREIGN KEY (community_id, session_id)
        REFERENCES channels (community_id, id),
    FOREIGN KEY (community_id, source_channel_id)
        REFERENCES channels (community_id, id),
    CONSTRAINT chk_meeting_create_event_id_len CHECK (LENGTH(create_event_id) = 32),
    CONSTRAINT chk_meeting_host_pubkey_len CHECK (LENGTH(host_pubkey) = 32),
    CONSTRAINT chk_meeting_end_event_id_len
        CHECK (end_event_id IS NULL OR LENGTH(end_event_id) = 32),
    CONSTRAINT chk_meeting_ended_by_len
        CHECK (ended_by IS NULL OR LENGTH(ended_by) = 32),
    CONSTRAINT chk_meeting_schema_version CHECK (schema_version IN (1, 2)),
    CONSTRAINT chk_meeting_moderator_pubkey_len
        CHECK (moderator_pubkey IS NULL OR LENGTH(moderator_pubkey) = 32),
    CONSTRAINT chk_meeting_current_round_positive CHECK (current_round > 0),
    CONSTRAINT chk_meeting_floor_revision_nonnegative CHECK (floor_revision >= 0),
    CONSTRAINT chk_meeting_floor_policy
        CHECK (floor_policy_version IN ('uniform-v0', 'moderated-baton-v1')),
    CONSTRAINT chk_meeting_protocol_shape
        CHECK (
            (schema_version = 1
                AND floor_policy_version = 'uniform-v0'
                AND moderator_pubkey IS NULL)
            OR
            (schema_version = 2
                AND floor_policy_version = 'moderated-baton-v1'
                AND moderator_pubkey IS NOT NULL)
        ),
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

CREATE TABLE meeting_rounds (
    community_id      UUID NOT NULL REFERENCES communities(id),
    session_id        UUID NOT NULL,
    round_number      BIGINT NOT NULL,
    floor_revision    BIGINT NOT NULL,
    phase             TEXT NOT NULL
        CHECK (phase IN ('open', 'claiming', 'granted', 'closed')),
    state_event_id    BYTEA NOT NULL,
    settle_not_before TIMESTAMPTZ,
    claim_deadline    TIMESTAMPTZ,
    holder_pubkey     BYTEA,
    grant_event_id    BYTEA,
    lease_expires_at  TIMESTAMPTZ,
    outcome           TEXT CHECK (outcome IN ('spoken', 'yielded', 'expired', 'ended')),
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
                (outcome IN ('yielded', 'expired', 'ended') AND speech_event_id IS NULL)
            ))
    )
);

CREATE INDEX idx_meeting_rounds_due
    ON meeting_rounds (
        community_id,
        phase,
        settle_not_before,
        claim_deadline,
        lease_expires_at
    )
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

CREATE INDEX idx_meeting_event_outbox_pending_session_sequence
    ON meeting_event_outbox (community_id, session_id, sequence)
    WHERE delivered_at IS NULL;

-- Meeting V1 freezes identity and configuration independently of channel roles
-- and persists every moderated-baton State snapshot.

CREATE TABLE meeting_participants (
    community_id     UUID NOT NULL REFERENCES communities(id),
    session_id       UUID NOT NULL,
    pubkey           BYTEA NOT NULL,
    participant_type TEXT NOT NULL
        CHECK (participant_type IN ('human', 'agent')),
    channel_role     TEXT NOT NULL
        CHECK (channel_role IN ('owner', 'member', 'bot')),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
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
    recovery_retry_at               TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    recovery_attempts               INT NOT NULL DEFAULT 0,
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
    CONSTRAINT chk_meeting_baton_recovery_attempts
        CHECK (recovery_attempts >= 0),
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

CREATE INDEX idx_meeting_baton_state_recovery_due
    ON meeting_baton_state (
        next_action_at,
        recovery_retry_at,
        community_id,
        session_id
    )
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
        (state IN ('pending', 'selected') AND terminal_at IS NULL)
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
    stage                 TEXT NOT NULL,
    CONSTRAINT chk_meeting_progress_stage
        CHECK (stage IN (
            'context_sync',
            'tool_use',
            'generating',
            'composing',
            'submitting'
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
    security_order      BIGINT NOT NULL DEFAULT nextval('meeting_security_order_seq')
        CHECK (security_order > 0),
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

CREATE INDEX idx_meeting_revocation_jobs_reader_fence
    ON meeting_revocation_jobs (community_id, revoked_pubkey, security_order);

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

-- ── Users ─────────────────────────────────────────────────────────────────────
-- Conformance: "Users, profiles, NIP-05, and user search". One profile per
-- (community, pubkey): the same key reposts kind:0 in each community it joins.

CREATE TABLE users (
    community_id        UUID NOT NULL REFERENCES communities(id),
    pubkey              BYTEA NOT NULL,
    nip05_handle        VARCHAR(255),
    display_name        VARCHAR(255),
    avatar_url          TEXT,
    about               TEXT,
    agent_type          VARCHAR(255),
    capabilities        JSONB,
    okta_user_id        VARCHAR(255),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deactivated_at      TIMESTAMPTZ,
    metadata_event_id   BYTEA,
    agent_owner_pubkey  BYTEA,
    channel_add_policy  channel_add_policy NOT NULL DEFAULT 'anyone',
    PRIMARY KEY (community_id, pubkey),
    CONSTRAINT chk_users_pubkey_len CHECK (LENGTH(pubkey) = 32),
    -- agent owner is a user in the SAME community.
    FOREIGN KEY (community_id, agent_owner_pubkey)
        REFERENCES users (community_id, pubkey) ON DELETE SET NULL
);

-- NIP-05 handle and Okta id unique within a community, not globally.
CREATE UNIQUE INDEX idx_users_nip05 ON users (community_id, lower(nip05_handle))
    WHERE nip05_handle IS NOT NULL;
CREATE UNIQUE INDEX idx_users_okta ON users (community_id, okta_user_id)
    WHERE okta_user_id IS NOT NULL;

-- ── Events (partitioned by month on created_at) ──────────────────────────────
-- Conformance: "Channel-less global events and DMs". `community_id` leads the
-- PK and every hot-path index. Partition stays BY RANGE (created_at) — the
-- monthly partition manager is unchanged (Max's call, plan §5/Lane0 contract).
-- Cross-community dedup: same signed event may exist in two communities;
-- (community_id, created_at, id) dedupes within one, allows across.

CREATE TABLE events (
    community_id UUID NOT NULL REFERENCES communities(id),
    id          BYTEA NOT NULL,
    pubkey      BYTEA NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL,
    kind        INT NOT NULL,
    tags        JSONB NOT NULL,
    content     TEXT NOT NULL,
    -- Full-text search vector (Typesense → Postgres FTS). Generated/STORED so
    -- it is a single source of truth — no sidecar indexer to keep coherent
    -- (Quinn option A, Lane-0 call). 'simple' config = no stemming/stopwords,
    -- matching the existing substring-ish search semantics; the search lane can
    -- revisit the config behind evidence. Tenant scoping is by the
    -- community-leading btree filters BitmapAnd-ed with the GIN probe, so the
    -- GIN index itself stays the minimal `GIN (search_tsv)` (Max's caveat:
    -- avoid btree_gin unless EXPLAIN proves it buys something).
    -- Privacy: encrypted/private routing wrappers and p-gated membership notices
    -- must never be discoverable through NIP-50 full-text search. NULL tsvector
    -- never matches `@@`.
    -- Keep in sync with migrations (final state: 0001 + 0005 + 0009).
    search_tsv  TSVECTOR GENERATED ALWAYS AS (
        CASE WHEN kind IN (1059, 30300, 30350, 30622, 44100, 44101, 44200) THEN NULL::tsvector
             ELSE to_tsvector('simple', content)
        END
    ) STORED,
    sig         BYTEA NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    channel_id  UUID,
    deleted_at  TIMESTAMPTZ,
    d_tag       TEXT,
    not_before  BIGINT,
    delivered_at BIGINT,
    PRIMARY KEY (community_id, created_at, id)
) PARTITION BY RANGE (created_at);

CREATE TABLE events_p_past PARTITION OF events
    FOR VALUES FROM (MINVALUE) TO ('2026-01-01');
CREATE TABLE events_p2026_01 PARTITION OF events
    FOR VALUES FROM ('2026-01-01') TO ('2026-02-01');
CREATE TABLE events_p2026_02 PARTITION OF events
    FOR VALUES FROM ('2026-02-01') TO ('2026-03-01');
CREATE TABLE events_p2026_03 PARTITION OF events
    FOR VALUES FROM ('2026-03-01') TO ('2026-04-01');
CREATE TABLE events_p2026_04 PARTITION OF events
    FOR VALUES FROM ('2026-04-01') TO ('2026-05-01');
CREATE TABLE events_p2026_05 PARTITION OF events
    FOR VALUES FROM ('2026-05-01') TO ('2026-06-01');
CREATE TABLE events_p2026_06 PARTITION OF events
    FOR VALUES FROM ('2026-06-01') TO ('2026-07-01');
CREATE TABLE events_p_future PARTITION OF events
    FOR VALUES FROM ('2026-07-01') TO (MAXVALUE);

-- Direct id lookup: the PK can't serve `WHERE id=$1` because created_at sits
-- between community_id and id. This index makes the scoped form
-- `WHERE community_id=$ AND id=$` index-served, not a partition scan.
CREATE INDEX idx_events_community_id ON events (community_id, id, created_at DESC);
-- Hot-path indexes, all community-leading.
CREATE INDEX idx_events_community_channel_created
    ON events (community_id, channel_id, created_at DESC, id);
CREATE INDEX idx_events_community_pubkey_kind_created
    ON events (community_id, pubkey, kind, created_at DESC, id);
CREATE INDEX idx_events_community_kind_created
    ON events (community_id, kind, created_at DESC, id);
CREATE INDEX idx_events_community_deleted ON events (community_id, deleted_at);
-- Addressable (replaceable) and NIP-33 parameterized lookups.
CREATE INDEX idx_events_addressable
    ON events (community_id, kind, pubkey, channel_id, deleted_at);
CREATE INDEX idx_events_parameterized
    ON events (community_id, kind, pubkey, d_tag, created_at DESC, id)
    WHERE d_tag IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX idx_events_not_before ON events (community_id, not_before)
    WHERE not_before IS NOT NULL AND deleted_at IS NULL AND delivered_at IS NULL;
-- Full-text search. Minimal GIN over the generated tsvector; community scoping
-- is supplied by the community-leading btree filters above (BitmapAnd), so this
-- stays a single-column GIN. The search lane confirms the final spelling with
-- EXPLAIN before its work lands (Quinn option A; Max's index-spelling caveat).
CREATE INDEX idx_events_search_tsv ON events USING GIN (search_tsv);

-- ── Event mentions ────────────────────────────────────────────────────────────
-- Conformance: "Channel-less global events and DMs" (#p fan-out). The join to
-- events MUST carry the community tuple (e.community_id = m.community_id AND
-- e.id = m.event_id) — bare e.id = m.event_id would leak cross-community
-- mentions (Max, verified at event.rs:222).

CREATE TABLE event_mentions (
    community_id        UUID NOT NULL REFERENCES communities(id),
    pubkey_hex          VARCHAR(64) NOT NULL,
    event_id            BYTEA NOT NULL,
    event_created_at    TIMESTAMPTZ NOT NULL,
    channel_id          UUID,
    event_kind          INT,
    PRIMARY KEY (community_id, pubkey_hex, event_id)
);

CREATE INDEX idx_event_mentions_pubkey_created
    ON event_mentions (community_id, pubkey_hex, event_created_at DESC);
CREATE INDEX idx_event_mentions_pubkey_kind_created
    ON event_mentions (community_id, pubkey_hex, event_kind, event_created_at DESC);

-- ── Subscriptions ─────────────────────────────────────────────────────────────
-- Conformance: "Mesh, agents, ACP/MCP, and CLI" (persisted subscriptions).

CREATE TABLE subscriptions (
    community_id        UUID NOT NULL REFERENCES communities(id),
    id                  VARCHAR(255) NOT NULL,
    owner_pubkey        BYTEA NOT NULL,
    filter_kinds        JSONB,
    filter_authors      JSONB,
    filter_channel_ids  JSONB,
    filter_since        TIMESTAMPTZ,
    filter_until        TIMESTAMPTZ,
    delivery_method     delivery_method NOT NULL DEFAULT 'webhook',
    delivery_url        TEXT,
    status              subscription_status NOT NULL DEFAULT 'active',
    pause_reason        pause_reason,
    delivered_count     BIGINT NOT NULL DEFAULT 0,
    error_count         BIGINT NOT NULL DEFAULT 0,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, owner_pubkey) REFERENCES users (community_id, pubkey)
);

-- ── Delivery log (partitioned by month on delivered_at) ──────────────────────
-- Conformance: subscription delivery audit. community_id carried for tenant
-- attribution; child of subscriptions.

CREATE TABLE delivery_log (
    community_id    UUID NOT NULL REFERENCES communities(id),
    id              BIGINT GENERATED ALWAYS AS IDENTITY,
    subscription_id VARCHAR(255),
    event_id        BYTEA,
    method          delivery_method,
    delivered_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    success         BOOLEAN,
    http_status     INT,
    error_message   TEXT,
    attempt_number  INT DEFAULT 1,
    PRIMARY KEY (delivered_at, id)
) PARTITION BY RANGE (delivered_at);

CREATE TABLE delivery_log_p_past PARTITION OF delivery_log
    FOR VALUES FROM (MINVALUE) TO ('2026-03-01');
CREATE TABLE delivery_log_p2026_03 PARTITION OF delivery_log
    FOR VALUES FROM ('2026-03-01') TO ('2026-04-01');
CREATE TABLE delivery_log_p2026_04 PARTITION OF delivery_log
    FOR VALUES FROM ('2026-04-01') TO ('2026-05-01');
CREATE TABLE delivery_log_p2026_05 PARTITION OF delivery_log
    FOR VALUES FROM ('2026-05-01') TO ('2026-06-01');
CREATE TABLE delivery_log_p2026_06 PARTITION OF delivery_log
    FOR VALUES FROM ('2026-06-01') TO ('2026-07-01');
CREATE TABLE delivery_log_p_future PARTITION OF delivery_log
    FOR VALUES FROM ('2026-07-01') TO (MAXVALUE);

CREATE INDEX idx_delivery_log_community_sub ON delivery_log (community_id, subscription_id);

-- ── Workflows ─────────────────────────────────────────────────────────────────
-- Conformance: "Workflows, runs, approvals, webhooks, schedules". Definition's
-- community fixed at create from req.community; runs/approvals inherit it.

CREATE TABLE workflows (
    community_id    UUID NOT NULL REFERENCES communities(id),
    id              UUID NOT NULL DEFAULT gen_random_uuid(),
    name            VARCHAR(255) NOT NULL,
    owner_pubkey    BYTEA NOT NULL,
    channel_id      UUID,
    definition      JSONB NOT NULL,
    definition_hash BYTEA NOT NULL,
    status          workflow_status NOT NULL DEFAULT 'active',
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, owner_pubkey) REFERENCES users (community_id, pubkey),
    FOREIGN KEY (community_id, channel_id) REFERENCES channels (community_id, id)
);

CREATE INDEX idx_workflows_channel_active ON workflows (community_id, channel_id, status, enabled);
-- Scheduler scans enabled schedule workflows; community_id returned per row so
-- side effects run under the owning tenant's context (Lane0 contract §4a.5).
CREATE INDEX idx_workflows_enabled ON workflows (enabled, status) WHERE enabled;

-- ── Workflow runs ─────────────────────────────────────────────────────────────

CREATE TABLE workflow_runs (
    community_id        UUID NOT NULL REFERENCES communities(id),
    id                  UUID NOT NULL DEFAULT gen_random_uuid(),
    workflow_id         UUID NOT NULL,
    status              run_status NOT NULL DEFAULT 'pending',
    trigger_event_id    BYTEA,
    current_step        INT NOT NULL DEFAULT 0,
    execution_trace     JSONB NOT NULL DEFAULT '[]',
    trigger_context     JSONB,
    started_at          TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,
    error_message       TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, workflow_id)
        REFERENCES workflows (community_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_workflow_runs_workflow ON workflow_runs (community_id, workflow_id);
CREATE INDEX idx_workflow_runs_status ON workflow_runs (community_id, status);

-- ── Workflow approvals ────────────────────────────────────────────────────────
-- token-hash lookup scoped: approval token grants cannot act on another
-- community's same hash (conformance).

CREATE TABLE workflow_approvals (
    community_id    UUID NOT NULL REFERENCES communities(id),
    token           BYTEA NOT NULL,
    workflow_id     UUID NOT NULL,
    run_id          UUID NOT NULL,
    step_id         VARCHAR(64) NOT NULL,
    step_index      INT NOT NULL,
    approver_spec   TEXT NOT NULL,
    status          approval_status NOT NULL DEFAULT 'pending',
    approver_pubkey BYTEA,
    note            TEXT,
    granted_at      TIMESTAMPTZ,
    denied_at       TIMESTAMPTZ,
    expires_at      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, token),
    FOREIGN KEY (community_id, workflow_id)
        REFERENCES workflows (community_id, id) ON DELETE CASCADE,
    FOREIGN KEY (community_id, run_id)
        REFERENCES workflow_runs (community_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_workflow_approvals_workflow ON workflow_approvals (community_id, workflow_id);
CREATE INDEX idx_workflow_approvals_run ON workflow_approvals (community_id, run_id);
CREATE INDEX idx_workflow_approvals_status ON workflow_approvals (community_id, status);

-- ── Scheduled workflow fires (cron claim) ─────────────────────────────────────
-- Plan §5: the at-most-once cron fire claim. UNIQUE (community_id, workflow_id,
-- scheduled_for) — only the pod that wins the claim insert creates the run.
-- Restart-safe (DB-durable). community is server provenance: the scheduler passes
-- workflow.community_id from list_all_enabled_workflows(), never a client input.
-- workflow_id is NOT globally unique under the (community_id, id) workflow key, so
-- the claim binds both community and id explicitly rather than resolving from id.
-- workflow_run_id links the won claim to the run it created (audit; NULL until the
-- post-insert attach, and stays NULL if run creation failed after a won claim).
-- The FK to workflow_runs uses NO ACTION (not SET NULL): community_id is shared
-- with the claim PK and is NOT NULL, so SET NULL is unimplementable here; a future
-- delete of a still-linked run is blocked rather than orphaning the at-most-once
-- claim row. workflow_runs are not pruned today, so this is a guardrail, not a path.

CREATE TABLE scheduled_workflow_fires (
    community_id    UUID NOT NULL REFERENCES communities(id),
    workflow_id     UUID NOT NULL,
    scheduled_for   TIMESTAMPTZ NOT NULL,
    claimed_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    workflow_run_id UUID,
    PRIMARY KEY (community_id, workflow_id, scheduled_for),
    FOREIGN KEY (community_id, workflow_id)
        REFERENCES workflows (community_id, id) ON DELETE CASCADE,
    FOREIGN KEY (community_id, workflow_run_id)
        REFERENCES workflow_runs (community_id, id) ON DELETE NO ACTION
);

-- The interval anchor reads MAX(scheduled_for) per workflow; the janitor prunes
-- by claimed_at globally (operator concern). See plan §5 retention coupling.
CREATE INDEX idx_scheduled_fires_claimed_at ON scheduled_workflow_fires (claimed_at);

-- ── API tokens ────────────────────────────────────────────────────────────────
-- Conformance: "API tokens and NIP-98 replay". token_hash uniqueness scoped to
-- (community_id, token_hash); channel claims reference channels in same community.

CREATE TABLE api_tokens (
    community_id        UUID NOT NULL REFERENCES communities(id),
    id                  UUID NOT NULL DEFAULT gen_random_uuid(),
    token_hash          BYTEA NOT NULL,
    owner_pubkey        BYTEA NOT NULL,
    name                VARCHAR(255) NOT NULL,
    scopes              JSONB NOT NULL,
    channel_ids         JSONB,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at          TIMESTAMPTZ,
    last_used_at        TIMESTAMPTZ,
    revoked_at          TIMESTAMPTZ,
    revoked_by          BYTEA,
    created_by_self_mint BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, owner_pubkey) REFERENCES users (community_id, pubkey),
    CONSTRAINT chk_api_tokens_hash_len CHECK (LENGTH(token_hash) = 32)
);

CREATE UNIQUE INDEX idx_api_tokens_hash ON api_tokens (community_id, token_hash);

-- ── Rate limit violations ─────────────────────────────────────────────────────
-- OPERATOR-GLOBAL: a deployment-health / abuse table, never tenant-observable.
-- Listed in the lint allowlist. Carries community_id as an attribution label
-- only (nullable, no uniqueness over it).

CREATE TABLE rate_limit_violations (
    id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    community_id    UUID,
    pubkey          BYTEA,
    violation_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    limit_type      VARCHAR(64),
    limit_value     INT,
    actual_value    INT,
    action_taken    VARCHAR(64)
);

-- ── Thread metadata ───────────────────────────────────────────────────────────
-- Conformance: thread lookups filter by community before event matching.

CREATE TABLE thread_metadata (
    community_id            UUID NOT NULL REFERENCES communities(id),
    event_created_at        TIMESTAMPTZ NOT NULL,
    event_id                BYTEA NOT NULL,
    channel_id              UUID NOT NULL,
    parent_event_id         BYTEA,
    parent_event_created_at TIMESTAMPTZ,
    root_event_id           BYTEA,
    root_event_created_at   TIMESTAMPTZ,
    depth                   INT NOT NULL DEFAULT 0,
    reply_count             INT NOT NULL DEFAULT 0,
    descendant_count        INT NOT NULL DEFAULT 0,
    last_reply_at           TIMESTAMPTZ,
    broadcast               BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (community_id, event_created_at, event_id),
    FOREIGN KEY (community_id, channel_id) REFERENCES channels (community_id, id)
);

CREATE INDEX idx_thread_metadata_parent ON thread_metadata (community_id, parent_event_id);
CREATE INDEX idx_thread_metadata_root ON thread_metadata (community_id, root_event_id);
CREATE INDEX idx_thread_metadata_channel_depth
    ON thread_metadata (community_id, channel_id, depth, event_created_at);
CREATE INDEX idx_thread_metadata_event_id ON thread_metadata (community_id, event_id);

-- ── Reactions ─────────────────────────────────────────────────────────────────
-- Conformance: reactions filter by community before event/pubkey matching.

CREATE TABLE reactions (
    community_id        UUID NOT NULL REFERENCES communities(id),
    event_created_at    TIMESTAMPTZ NOT NULL,
    event_id            BYTEA NOT NULL,
    pubkey              BYTEA NOT NULL,
    emoji               VARCHAR(64) NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    removed_at          TIMESTAMPTZ,
    reaction_event_id   BYTEA,
    PRIMARY KEY (community_id, event_created_at, event_id, pubkey, emoji)
);

CREATE INDEX idx_reactions_event ON reactions (community_id, event_id, event_created_at);
CREATE INDEX idx_reactions_pubkey ON reactions (community_id, pubkey);
-- A reaction's source event id is unique within a community.
CREATE UNIQUE INDEX idx_reactions_source_event ON reactions (community_id, reaction_event_id)
    WHERE reaction_event_id IS NOT NULL;

-- ── Pubkey allowlist ──────────────────────────────────────────────────────────
-- Conformance: "Relay membership, pubkey allowlist, archived identities".
-- PK becomes (community_id, pubkey).

CREATE TABLE pubkey_allowlist (
    community_id UUID NOT NULL REFERENCES communities(id),
    pubkey      BYTEA NOT NULL,
    added_by    BYTEA,
    added_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    note        TEXT,
    PRIMARY KEY (community_id, pubkey)
);

-- ── Relay members (NIP-43) ────────────────────────────────────────────────────
-- Conformance: membership gate, community-scoped. pubkey stored as hex TEXT
-- (unchanged wire form). PK (community_id, pubkey).

CREATE TABLE relay_members (
    community_id UUID NOT NULL REFERENCES communities(id),
    pubkey      TEXT NOT NULL,
    role        TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    added_by    TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, pubkey)
);

CREATE INDEX idx_relay_members_role ON relay_members (community_id, role);

-- ── Archived identities (NIP-IA) ──────────────────────────────────────────────
-- Conformance: archive cannot hide a key in another community. PK scoped.

CREATE TABLE archived_identities (
    community_id      UUID NOT NULL REFERENCES communities(id),
    pubkey            TEXT NOT NULL,
    consent_path      TEXT NOT NULL CHECK (consent_path IN ('self', 'owner', 'admin')),
    actor             TEXT NOT NULL,
    reason            TEXT,
    replaced_by       TEXT,
    request_event_id  TEXT NOT NULL,
    archived_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, pubkey)
);

-- ── Audit log ─────────────────────────────────────────────────────────────────
-- Conformance: "Audit log and observability". Per-community hash chain:
-- uniqueness (community_id, seq) and (community_id, hash). One chain per tenant.
-- (Lane Audit/Dawn builds the chain logic; Lane 0 fixes the scoped schema.)

CREATE TABLE audit_log (
    community_id    UUID NOT NULL REFERENCES communities(id),
    seq             BIGINT NOT NULL,
    hash            BYTEA NOT NULL,
    prev_hash       BYTEA,
    action          VARCHAR(64) NOT NULL,
    actor_pubkey    BYTEA,
    object_id       TEXT,
    detail          JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, seq)
);

CREATE UNIQUE INDEX idx_audit_log_hash ON audit_log (community_id, hash);

-- ── NIP-56 reports (kind:1984 ingest) ─────────────────────────────────────────
-- One row per accepted report event. Reports are signals, never triggers:
-- nothing auto-actions on them (NIP-56). Reporter identity is visible to
-- moderators in the queue but never revealed to the reported author.

CREATE TABLE moderation_reports (
    community_id        UUID NOT NULL REFERENCES communities(id),
    id                  UUID NOT NULL DEFAULT gen_random_uuid(),
    -- The signed kind:1984 event id (stored for audit/idempotency).
    report_event_id     BYTEA NOT NULL CHECK (length(report_event_id) = 32),
    reporter_pubkey     BYTEA NOT NULL CHECK (length(reporter_pubkey) = 32),
    -- What was reported. Exactly one target class per row (CHECK-enforced below).
    target_kind         TEXT NOT NULL CHECK (target_kind IN ('event', 'pubkey', 'blob')),
    target_event_id     BYTEA CHECK (target_event_id IS NULL OR length(target_event_id) = 32),
    target_pubkey       BYTEA CHECK (target_pubkey IS NULL OR length(target_pubkey) = 32),
    target_blob_sha256  BYTEA CHECK (target_blob_sha256 IS NULL OR length(target_blob_sha256) = 32),
    -- Channel inferred from an in-tenant target event row, when resolvable.
    channel_id          UUID,
    -- NIP-56 report type: illegal|nudity|malware|spam|impersonation|profanity|other.
    report_type         TEXT NOT NULL,
    -- Reporter's optional free-text context (mod-queue-only; never public).
    note                TEXT,
    status              TEXT NOT NULL DEFAULT 'open'
                        CHECK (status IN ('open', 'resolved', 'dismissed', 'escalated')),
    resolved_by         BYTEA,
    resolved_at         TIMESTAMPTZ,
    -- moderation_actions row that resolved this report, if any.
    action_id           UUID,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, id),
    -- Exactly one target class per row: target_kind is authoritative and the
    -- matching column (only) is populated. Queue/action code never guesses.
    CHECK (
        (target_kind = 'event'  AND target_event_id IS NOT NULL AND target_pubkey IS NULL     AND target_blob_sha256 IS NULL) OR
        (target_kind = 'pubkey' AND target_event_id IS NULL     AND target_pubkey IS NOT NULL AND target_blob_sha256 IS NULL) OR
        (target_kind = 'blob'   AND target_event_id IS NULL     AND target_pubkey IS NULL     AND target_blob_sha256 IS NOT NULL)
    ),
    -- Same-community channel provenance (channels are soft-deleted, never
    -- hard-deleted, so this FK cannot dangle).
    FOREIGN KEY (community_id, channel_id) REFERENCES channels (community_id, id)
);

-- Queue reads: open reports, newest first, per community.
CREATE INDEX idx_moderation_reports_status
    ON moderation_reports (community_id, status, created_at DESC);
-- Group-by-target for triage aggregation.
CREATE INDEX idx_moderation_reports_target_event
    ON moderation_reports (community_id, target_event_id)
    WHERE target_event_id IS NOT NULL;
CREATE INDEX idx_moderation_reports_target_pubkey
    ON moderation_reports (community_id, target_pubkey)
    WHERE target_pubkey IS NOT NULL;
-- Idempotency: one row per report event per community.
CREATE UNIQUE INDEX idx_moderation_reports_event
    ON moderation_reports (community_id, report_event_id);

-- ── Bans + timeouts (one restriction row per member) ──────────────────────────
-- Ban = connection block, enforced at the NIP-42 auth seam
-- ("blocked: you are banned from this community") + join/ingest surfaces.
-- Timeout = write-block only ("restricted: you are timed out until <ts>").
-- A row may be ban-only, timeout-only, or both over its lifetime.

CREATE TABLE community_bans (
    community_id    UUID NOT NULL REFERENCES communities(id),
    pubkey          BYTEA NOT NULL CHECK (length(pubkey) = 32),
    banned          BOOLEAN NOT NULL DEFAULT false,
    -- NULL + banned=true ⇒ permanent.
    ban_expires_at  TIMESTAMPTZ,
    ban_reason      TEXT,
    -- Write-block until this timestamp; NULL or past ⇒ not timed out.
    muted_until     TIMESTAMPTZ,
    mute_reason     TEXT,
    -- Moderator who last modified this row.
    actor_pubkey    BYTEA NOT NULL CHECK (length(actor_pubkey) = 32),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, pubkey)
);

-- ── Moderation audit ──────────────────────────────────────────────────────────
-- One row per accepted moderation action. Full detail (reporter identities,
-- private reasons, matched NIP-OA principal) stays mod/audit-only; the public
-- tombstone carries only action_id + reason_code + sanitized public_reason.

CREATE TABLE moderation_actions (
    community_id    UUID NOT NULL REFERENCES communities(id),
    id              UUID NOT NULL DEFAULT gen_random_uuid(),
    actor_pubkey    BYTEA NOT NULL CHECK (length(actor_pubkey) = 32),
    action          TEXT NOT NULL CHECK (action IN (
                        'delete_message', 'kick', 'ban', 'unban',
                        'timeout', 'untimeout', 'dismiss_report', 'escalate',
                        'resolve:delete', 'resolve:kick', 'resolve:ban',
                        'resolve:timeout')),
    target_pubkey   BYTEA CHECK (target_pubkey IS NULL OR length(target_pubkey) = 32),
    target_event_id BYTEA CHECK (target_event_id IS NULL OR length(target_event_id) = 32),
    channel_id      UUID,
    -- Machine-readable rule/reason code (e.g. "spam", "community_rule_3").
    reason_code     TEXT,
    -- Sanitized, safe for the public tombstone.
    public_reason   TEXT,
    -- Mod-only context; never leaves the audit surface.
    private_reason  TEXT,
    -- NIP-OA: which principal matched a ban ('self' | 'owner'); audit-only,
    -- the client never learns which.
    matched_principal TEXT CHECK (matched_principal IS NULL OR matched_principal IN ('self', 'owner')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, channel_id) REFERENCES channels (community_id, id)
);

CREATE INDEX idx_moderation_actions_created
    ON moderation_actions (community_id, created_at DESC);
CREATE INDEX idx_moderation_actions_target_pubkey
    ON moderation_actions (community_id, target_pubkey)
    WHERE target_pubkey IS NOT NULL;

-- Same-community resolution provenance: a report can only be resolved by an
-- action row in its own community. Added after moderation_actions exists.
ALTER TABLE moderation_reports
    ADD FOREIGN KEY (community_id, action_id)
    REFERENCES moderation_actions (community_id, id);

-- ── Lint allowlist registry ───────────────────────────────────────────────────
-- The explicit registry of tables that are deliberately operator-global (NOT
-- tenant-scoped). The migration-lint harness reads this: any table NOT listed
-- here MUST carry a NOT NULL community_id and lead its uniques with it. Making
-- the allowlist a DB table (not a hard-coded list in the linter) keeps the
-- registry next to the schema it governs and reviewable in one migration diff.

CREATE TABLE _operator_global_tables (
    table_name  TEXT PRIMARY KEY,
    reason      TEXT NOT NULL
);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('communities',           'the tenant registry itself; id IS the community key'),
    ('rate_limit_violations', 'deployment abuse/health; never tenant-observable; community_id is an attribution label only'),
    ('_operator_global_tables', 'the registry table itself');
-- NIP-PL effective lease state and durable wake outbox. Every key is led by
-- community_id: client-provided origin is confirmation only, never routing.
CREATE TABLE push_leases (
    community_id UUID NOT NULL REFERENCES communities(id),
    author BYTEA NOT NULL CHECK (length(author) = 32),
    installation_id TEXT NOT NULL CHECK (octet_length(installation_id) BETWEEN 1 AND 64),
    source_event_id BYTEA NOT NULL CHECK (length(source_event_id) = 32),
    source_created_at BIGINT NOT NULL,
    generation BIGINT NOT NULL CHECK (generation > 0),
    active BOOLEAN NOT NULL,
    endpoint_enabled BOOLEAN NOT NULL DEFAULT true,
    app_profile TEXT,
    endpoint_hash BYTEA CHECK (endpoint_hash IS NULL OR length(endpoint_hash) = 32),
    endpoint_grant TEXT,
    max_class TEXT CHECK (max_class IS NULL OR max_class IN ('silent','default','time_sensitive','urgent')),
    subscriptions JSONB,
    expires_at BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, author, installation_id),
    UNIQUE (community_id, source_event_id),
    CHECK ((active AND app_profile IS NOT NULL AND endpoint_hash IS NOT NULL AND endpoint_grant IS NOT NULL AND max_class IS NOT NULL AND subscriptions IS NOT NULL)
        OR (NOT active AND app_profile IS NULL AND endpoint_hash IS NULL AND endpoint_grant IS NULL AND max_class IS NULL AND subscriptions IS NULL))
);
CREATE UNIQUE INDEX push_leases_endpoint_unique
    ON push_leases (community_id, author, app_profile, endpoint_hash)
    WHERE active;
CREATE INDEX push_leases_expiry ON push_leases (community_id, expires_at) WHERE active;

CREATE TABLE push_wake_outbox (
    community_id UUID NOT NULL REFERENCES communities(id),
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    author BYTEA NOT NULL CHECK (length(author) = 32),
    installation_id TEXT NOT NULL,
    lease_generation BIGINT NOT NULL CHECK (lease_generation > 0),
    endpoint_hash BYTEA NOT NULL CHECK (length(endpoint_hash) = 32),
    event_id BYTEA NOT NULL CHECK (length(event_id) = 32),
    class TEXT NOT NULL CHECK (class IN ('silent','default','time_sensitive','urgent')),
    expires_at BIGINT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','sending','delivered','failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until TIMESTAMPTZ,
    claim_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, author, installation_id)
        REFERENCES push_leases (community_id, author, installation_id),
    UNIQUE (community_id, endpoint_hash, event_id)
);
CREATE INDEX push_wake_outbox_due
    ON push_wake_outbox (community_id, next_attempt_at) WHERE state = 'pending';
CREATE INDEX push_wake_outbox_recovery
    ON push_wake_outbox (community_id, lease_until) WHERE state = 'sending';
-- Durable event-to-push matching follower. The trigger runs in the event insert
-- transaction, so every accepted persistent event has a crash-safe match job and
-- rejected/rolled-back events never do. Processing is idempotent through the
-- push_wake_outbox endpoint/event unique key.
CREATE TABLE push_match_queue (
    community_id UUID NOT NULL REFERENCES communities(id),
    event_id BYTEA NOT NULL CHECK (length(event_id) = 32),
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','matching')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until TIMESTAMPTZ,
    claim_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, event_id)
);
CREATE INDEX push_match_queue_due
    ON push_match_queue (next_attempt_at, created_at) WHERE state = 'pending';
CREATE INDEX push_match_queue_recovery
    ON push_match_queue (lease_until) WHERE state = 'matching';

-- T1b push gate (keep in sync with migrations/0023). Enqueue only when the
-- community has an active, endpoint-enabled, unexpired lease; the shared
-- advisory lock pairs with the exclusive lock taken by lease activations
-- (crates/buzz-db/src/push.rs) to close the lost-wake race.
CREATE FUNCTION enqueue_push_match_job() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    -- Keep this allowlist identical to the relay's validated NIP-PL descriptor.
    -- Centralizing it on the events table covers every durable producer,
    -- including internal paths that bypass live dispatch.
    IF NEW.kind IN (7, 9, 1059, 40007, 46010) THEN
        PERFORM pg_advisory_xact_lock_shared(
            hashtextextended('buzz_push_gate:' || NEW.community_id::text, 0));
        IF EXISTS (
            SELECT 1 FROM push_leases
            WHERE community_id = NEW.community_id
              AND active
              AND endpoint_enabled
              AND expires_at > EXTRACT(EPOCH FROM now())::bigint
        ) THEN
            INSERT INTO push_match_queue (community_id, event_id)
            VALUES (NEW.community_id, NEW.id)
            ON CONFLICT DO NOTHING;
        END IF;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER events_enqueue_push_match
AFTER INSERT ON events
FOR EACH ROW EXECUTE FUNCTION enqueue_push_match_job();

-- Channel TTL refresh (keep in sync with migrations/0024). Runs deferred, in
-- the transaction that makes a channel-scoped event durable, so a TTL
-- transition committed while ingest was in flight is never missed. The
-- per-channel advisory lock is SHARED here — permanent-channel commits admit
-- each other — and taken EXCLUSIVE by TTL transitions (update_channel in
-- crates/buzz-db/src/channel.rs), which forces the same total order the
-- 0022 row lock provided without serializing the hot path.
CREATE FUNCTION refresh_channel_ttl_after_event_insert() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    channel_ttl INTEGER;
BEGIN
    -- Kind 9007 creates the channel and initializes its deadline itself.
    IF NEW.channel_id IS NOT NULL AND NEW.kind <> 9007 THEN
        BEGIN
            PERFORM pg_advisory_xact_lock_shared(hashtextextended(
                'buzz_channel_ttl:' || NEW.community_id::text || ':' || NEW.channel_id::text, 0));

            SELECT ttl_seconds INTO channel_ttl
            FROM channels
            WHERE community_id = NEW.community_id AND id = NEW.channel_id;

            IF channel_ttl IS NOT NULL THEN
                UPDATE channels
                SET ttl_deadline = clock_timestamp() + make_interval(secs => ttl_seconds)
                WHERE community_id = NEW.community_id
                  AND id = NEW.channel_id
                  AND ttl_seconds IS NOT NULL
                  AND archived_at IS NULL
                  AND deleted_at IS NULL;
            END IF;
        EXCEPTION WHEN OTHERS THEN
            -- Preserve the existing best-effort contract: a TTL refresh failure
            -- must not reject an otherwise valid durable event.
            RAISE WARNING 'channel TTL refresh failed for community %, channel %: %',
                NEW.community_id, NEW.channel_id, SQLERRM;
        END;
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER events_refresh_channel_ttl
AFTER INSERT ON events
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION refresh_channel_ttl_after_event_insert();

-- Replica-fence floor guard (keep in sync with migrations/0021). A deferred
-- constraint trigger re-checks, inside COMMIT processing, that channel-bearing
-- event rows are no older than `buzz.created_at_floor` seconds before commit
-- time (clock_timestamp(), NOT the transaction-frozen now()). This turns the
-- relay's ingest-time created_at envelope into a commit-time storage
-- invariant, which is what lets keyset-cursor pages below the replica fence
-- be served by a read replica without holes. Enforcement is armed per session
-- via the GUC (set by the relay's writer pool on connect); sessions without
-- the GUC (pg_restore, manual backfills) bypass it and must hold the replica
-- fence closed for their duration. The only structural exemption is
-- channel_id IS NULL: those rows never appear in keyset-paged windows.
CREATE FUNCTION events_created_at_floor_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    floor_secs numeric := nullif(current_setting('buzz.created_at_floor', true), '')::numeric;
BEGIN
    IF floor_secs IS NOT NULL
       AND floor_secs > 0
       AND NEW.channel_id IS NOT NULL
       AND NEW.created_at < clock_timestamp() - make_interval(secs => floor_secs)
    THEN
        RAISE EXCEPTION
            'events.created_at % is more than % s before commit time %; below the replica-fence floor',
            NEW.created_at, floor_secs, clock_timestamp()
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NULL;
END
$$;

-- INSERT OR UPDATE OF: an UPDATE can move a previously exempt row into the
-- guarded set (channel_id NULL -> NOT NULL) or move a channel row's
-- created_at below the fence, so both mutation paths re-run the guard on the
-- NEW row. A created_at rewrite that crosses partition bounds runs as
-- DELETE + INSERT and hits the cloned AFTER INSERT guard on the destination
-- partition; an in-partition rewrite fires the UPDATE OF arm.
CREATE CONSTRAINT TRIGGER events_created_at_floor
    AFTER INSERT OR UPDATE OF created_at, channel_id ON events
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION events_created_at_floor_guard();

-- Durable, deployment-global authority for the public NIP-PL push gateway.
-- This state is intentionally outside relay community tenancy: installations
-- delegate to relay signing keys and may authorize multiple relay deployments.
CREATE TABLE push_gateway_challenges (
    id UUID PRIMARY KEY,
    challenge_hash BYTEA NOT NULL CHECK (length(challenge_hash) = 32),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX push_gateway_challenges_expiry ON push_gateway_challenges (expires_at);

CREATE TABLE push_gateway_installations (
    id UUID PRIMARY KEY,
    app_attest_key_id BYTEA NOT NULL UNIQUE CHECK (octet_length(app_attest_key_id) BETWEEN 1 AND 128),
    app_attest_public_key BYTEA NOT NULL CHECK (octet_length(app_attest_public_key) BETWEEN 33 AND 256),
    assertion_counter BIGINT NOT NULL CHECK (assertion_counter BETWEEN 0 AND 4294967295),
    app_profile TEXT NOT NULL CHECK (app_profile IN ('buzz-ios-production','buzz-ios-sandbox')),
    token_ciphertext BYTEA NOT NULL CHECK (octet_length(token_ciphertext) BETWEEN 1 AND 2048),
    token_fingerprint BYTEA NOT NULL CHECK (length(token_fingerprint) = 32),
    endpoint_epoch BIGINT NOT NULL CHECK (endpoint_epoch > 0),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (app_profile, token_fingerprint)
);
CREATE INDEX push_gateway_installations_expiry ON push_gateway_installations (expires_at) WHERE revoked_at IS NULL;

CREATE TABLE push_gateway_delegations (
    id UUID PRIMARY KEY,
    installation_id UUID NOT NULL REFERENCES push_gateway_installations(id),
    relay_pubkey BYTEA NOT NULL CHECK (length(relay_pubkey) = 32),
    endpoint_epoch BIGINT NOT NULL CHECK (endpoint_epoch > 0),
    generation BIGINT NOT NULL CHECK (generation > 0),
    not_before TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (installation_id, relay_pubkey),
    CHECK (not_before < expires_at)
);
CREATE INDEX push_gateway_delegations_expiry ON push_gateway_delegations (expires_at) WHERE revoked_at IS NULL;

CREATE TABLE push_gateway_endpoint_quotas (
    token_fingerprint BYTEA PRIMARY KEY CHECK (length(token_fingerprint) = 32),
    window_started_at TIMESTAMPTZ NOT NULL,
    admitted BIGINT NOT NULL CHECK (admitted >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX push_gateway_endpoint_quotas_updated ON push_gateway_endpoint_quotas (updated_at);

CREATE TABLE push_gateway_delivery_auth_replays (
    relay_pubkey BYTEA NOT NULL CHECK (length(relay_pubkey) = 32),
    auth_event_id BYTEA NOT NULL CHECK (length(auth_event_id) = 32),
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (relay_pubkey, auth_event_id)
);
CREATE INDEX push_gateway_delivery_auth_replays_expiry ON push_gateway_delivery_auth_replays (expires_at);

CREATE TABLE push_gateway_delivery_request_replays (
    relay_pubkey BYTEA NOT NULL CHECK (length(relay_pubkey) = 32),
    request_id UUID NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (relay_pubkey, request_id)
);
CREATE INDEX push_gateway_delivery_request_replays_expiry ON push_gateway_delivery_request_replays (expires_at);

INSERT INTO _operator_global_tables (table_name, reason) VALUES
    ('push_gateway_challenges', 'public gateway one-time challenges span relay communities'),
    ('push_gateway_installations', 'public gateway installation authority spans relay communities'),
    ('push_gateway_delegations', 'public gateway relay delegations span relay communities'),
    ('push_gateway_endpoint_quotas', 'public gateway endpoint abuse ceilings span relay communities'),
    ('push_gateway_delivery_auth_replays', 'public gateway signed-event replay admission spans relay communities'),
    ('push_gateway_delivery_request_replays', 'public gateway stable request-id admission spans relay communities');
