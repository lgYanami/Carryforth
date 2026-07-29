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
    project_view_enabled BOOLEAN NOT NULL DEFAULT FALSE,
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

-- ── Project View canonical state (keep in sync with migration 0025) ──────────

CREATE TABLE project_view_state (
    community_id             UUID        NOT NULL,
    project_revision         BIGINT      NOT NULL,
    active_object_count      INTEGER     NOT NULL DEFAULT 0,
    initialized_at           TIMESTAMPTZ NOT NULL,
    updated_at               TIMESTAMPTZ NOT NULL,
    last_event_id            BYTEA       NOT NULL,
    last_actor_pubkey         BYTEA       NOT NULL,
    meta_projection_event_id BYTEA       NOT NULL,
    projection_pubkey        BYTEA       NOT NULL,
    projection_generation    BIGINT      NOT NULL,

    PRIMARY KEY (community_id),
    CONSTRAINT project_view_state_community_fk
        FOREIGN KEY (community_id)
        REFERENCES communities (id)
        ON DELETE NO ACTION,
    CONSTRAINT project_view_state_revision_check
        CHECK (project_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_view_state_active_count_check
        CHECK (active_object_count >= 0),
    CONSTRAINT project_view_state_time_check
        CHECK (updated_at >= initialized_at),
    CONSTRAINT project_view_state_last_event_id_check
        CHECK (octet_length(last_event_id) = 32),
    CONSTRAINT project_view_state_last_actor_pubkey_check
        CHECK (octet_length(last_actor_pubkey) = 32),
    CONSTRAINT project_view_state_meta_event_id_check
        CHECK (octet_length(meta_projection_event_id) = 32),
    CONSTRAINT project_view_state_projection_pubkey_check
        CHECK (octet_length(projection_pubkey) = 32),
    CONSTRAINT project_view_state_generation_check
        CHECK (projection_generation BETWEEN 1 AND 9007199254740991)
);

CREATE TABLE project_view_objects (
    community_id             UUID        NOT NULL,
    object_id                UUID        NOT NULL,
    object_type              TEXT        NOT NULL,
    schema_version           SMALLINT    NOT NULL,
    object_revision          BIGINT      NOT NULL,
    project_revision         BIGINT      NOT NULL,
    body                     JSONB,

    under_goal_id            UUID,
    under_plan_id            UUID,
    planned_in_stage_id      UUID,
    about_object_id          UUID,
    about_object_type        TEXT,
    handles_object_id        UUID,
    handles_object_type      TEXT,

    created_at               TIMESTAMPTZ NOT NULL,
    updated_at               TIMESTAMPTZ NOT NULL,
    created_by               BYTEA       NOT NULL,
    updated_by               BYTEA       NOT NULL,
    source_event_id          BYTEA       NOT NULL,
    projection_event_id      BYTEA       NOT NULL,
    deleted_at               TIMESTAMPTZ,

    PRIMARY KEY (community_id, object_id),
    CONSTRAINT project_view_objects_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_view_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_objects_under_goal_fk
        FOREIGN KEY (community_id, under_goal_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_objects_under_plan_fk
        FOREIGN KEY (community_id, under_plan_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_objects_planned_stage_fk
        FOREIGN KEY (community_id, planned_in_stage_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_objects_about_fk
        FOREIGN KEY (community_id, about_object_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_objects_handles_fk
        FOREIGN KEY (community_id, handles_object_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_objects_type_check
        CHECK (object_type IN (
            'project_profile', 'goal', 'role', 'plan', 'stage',
            'requirement', 'issue', 'work', 'resource'
        )),
    CONSTRAINT project_view_objects_schema_check
        CHECK (schema_version = 1),
    CONSTRAINT project_view_objects_revision_check
        CHECK (
            object_revision BETWEEN 1 AND 9007199254740991
            AND project_revision BETWEEN 1 AND 9007199254740991
        ),
    CONSTRAINT project_view_objects_id_check
        CHECK (object_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_view_objects_profile_id_check
        CHECK (
            (object_type = 'project_profile' AND object_id = community_id)
            OR (object_type <> 'project_profile' AND object_id <> community_id)
        ),
    CONSTRAINT project_view_objects_body_check
        CHECK (
            (
                deleted_at IS NULL
                AND body IS NOT NULL
                AND jsonb_typeof(body) = 'object'
            )
            OR (
                deleted_at IS NOT NULL
                AND body IS NULL
                AND under_goal_id IS NULL
                AND under_plan_id IS NULL
                AND planned_in_stage_id IS NULL
                AND about_object_id IS NULL
                AND about_object_type IS NULL
                AND handles_object_id IS NULL
                AND handles_object_type IS NULL
            )
        ),
    CONSTRAINT project_view_objects_time_check
        CHECK (
            updated_at >= created_at
            AND (deleted_at IS NULL OR deleted_at = updated_at)
        ),
    CONSTRAINT project_view_objects_created_by_check
        CHECK (octet_length(created_by) = 32),
    CONSTRAINT project_view_objects_updated_by_check
        CHECK (octet_length(updated_by) = 32),
    CONSTRAINT project_view_objects_source_event_check
        CHECK (octet_length(source_event_id) = 32),
    CONSTRAINT project_view_objects_projection_event_check
        CHECK (octet_length(projection_event_id) = 32),
    CONSTRAINT project_view_objects_about_pair_check
        CHECK ((about_object_id IS NULL) = (about_object_type IS NULL)),
    CONSTRAINT project_view_objects_handles_pair_check
        CHECK ((handles_object_id IS NULL) = (handles_object_type IS NULL)),
    CONSTRAINT project_view_objects_reference_type_check
        CHECK (
            (about_object_type IS NULL OR about_object_type IN (
                'project_profile', 'goal', 'role', 'plan', 'stage',
                'requirement', 'issue', 'work', 'resource'
            ))
            AND
            (handles_object_type IS NULL OR handles_object_type IN ('requirement', 'issue'))
        ),
    CONSTRAINT project_view_objects_relation_shape_check
        CHECK (
            deleted_at IS NOT NULL
            OR (
                object_type IN ('project_profile', 'goal', 'role', 'resource')
                AND under_goal_id IS NULL
                AND under_plan_id IS NULL
                AND planned_in_stage_id IS NULL
                AND about_object_id IS NULL
                AND handles_object_id IS NULL
            )
            OR (
                object_type = 'plan'
                AND under_plan_id IS NULL
                AND planned_in_stage_id IS NULL
                AND about_object_id IS NULL
                AND handles_object_id IS NULL
            )
            OR (
                object_type = 'stage'
                AND under_goal_id IS NULL
                AND under_plan_id IS NOT NULL
                AND planned_in_stage_id IS NULL
                AND about_object_id IS NULL
                AND handles_object_id IS NULL
            )
            OR (
                object_type = 'requirement'
                AND under_goal_id IS NULL
                AND under_plan_id IS NULL
                AND about_object_id IS NULL
                AND handles_object_id IS NULL
            )
            OR (
                object_type = 'issue'
                AND under_goal_id IS NULL
                AND under_plan_id IS NULL
                AND handles_object_id IS NULL
            )
            OR (
                object_type = 'work'
                AND under_goal_id IS NULL
                AND under_plan_id IS NULL
                AND planned_in_stage_id IS NULL
                AND about_object_id IS NULL
                AND handles_object_id IS NOT NULL
            )
        )
);

CREATE UNIQUE INDEX idx_project_view_one_active_profile
    ON project_view_objects (community_id, object_type)
    WHERE object_type = 'project_profile' AND deleted_at IS NULL;
CREATE INDEX idx_project_view_objects_active_type
    ON project_view_objects (community_id, object_type, object_id)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_project_view_objects_under_goal
    ON project_view_objects (community_id, under_goal_id)
    WHERE deleted_at IS NULL AND under_goal_id IS NOT NULL;
CREATE INDEX idx_project_view_objects_under_plan
    ON project_view_objects (community_id, under_plan_id)
    WHERE deleted_at IS NULL AND under_plan_id IS NOT NULL;
CREATE INDEX idx_project_view_objects_planned_stage
    ON project_view_objects (community_id, planned_in_stage_id)
    WHERE deleted_at IS NULL AND planned_in_stage_id IS NOT NULL;
CREATE INDEX idx_project_view_objects_about
    ON project_view_objects (community_id, about_object_id)
    WHERE deleted_at IS NULL AND about_object_id IS NOT NULL;
CREATE INDEX idx_project_view_objects_handles
    ON project_view_objects (community_id, handles_object_id)
    WHERE deleted_at IS NULL AND handles_object_id IS NOT NULL;
CREATE INDEX idx_project_view_objects_source_event
    ON project_view_objects (community_id, source_event_id);
CREATE INDEX idx_project_view_objects_project_revision
    ON project_view_objects (community_id, project_revision);

CREATE TABLE project_view_mutations (
    community_id     UUID        NOT NULL,
    event_id         BYTEA       NOT NULL,
    project_revision BIGINT      NOT NULL,
    actor_pubkey     BYTEA       NOT NULL,
    operation        TEXT        NOT NULL,
    object_type      TEXT,
    object_id        UUID,
    result           JSONB       NOT NULL,
    accepted_at      TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, event_id),
    CONSTRAINT project_view_mutations_revision_unique
        UNIQUE (community_id, project_revision),
    CONSTRAINT project_view_mutations_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_view_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_mutations_event_id_check
        CHECK (octet_length(event_id) = 32),
    CONSTRAINT project_view_mutations_revision_check
        CHECK (project_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_view_mutations_actor_check
        CHECK (octet_length(actor_pubkey) = 32),
    CONSTRAINT project_view_mutations_operation_check
        CHECK (operation IN ('initialize', 'create', 'update', 'delete')),
    CONSTRAINT project_view_mutations_object_pair_check
        CHECK (
            (operation = 'initialize' AND object_type IS NULL AND object_id IS NULL)
            OR
            (
                operation IN ('create', 'update', 'delete')
                AND object_type IN (
                    'project_profile', 'goal', 'role', 'plan', 'stage',
                    'requirement', 'issue', 'work', 'resource'
                )
                AND object_id IS NOT NULL
            )
        ),
    CONSTRAINT project_view_mutations_result_check
        CHECK (jsonb_typeof(result) = 'object')
);

CREATE INDEX idx_project_view_mutations_accepted
    ON project_view_mutations (community_id, accepted_at);

CREATE FUNCTION project_view_adjust_active_count() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    delta INTEGER := 0;
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.deleted_at IS NULL THEN
            delta := 1;
        END IF;
    ELSIF OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL THEN
        delta := -1;
    ELSIF OLD.deleted_at IS NOT NULL AND NEW.deleted_at IS NULL THEN
        RAISE EXCEPTION 'Project View tombstone % cannot be reactivated', NEW.object_id
            USING ERRCODE = 'check_violation';
    END IF;

    IF delta <> 0 THEN
        UPDATE project_view_state
        SET active_object_count = active_object_count + delta
        WHERE community_id = NEW.community_id;

        IF NOT FOUND THEN
            RAISE EXCEPTION 'Project View state missing for community %', NEW.community_id
                USING ERRCODE = 'foreign_key_violation';
        END IF;
    END IF;

    RETURN NULL;
END
$$;

CREATE TRIGGER project_view_objects_adjust_active_count
    AFTER INSERT OR UPDATE OF deleted_at ON project_view_objects
    FOR EACH ROW
    EXECUTE FUNCTION project_view_adjust_active_count();

CREATE FUNCTION project_view_forbid_object_delete() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Project View objects cannot be hard-deleted; write a tombstone'
        USING ERRCODE = 'check_violation';
END
$$;

CREATE TRIGGER project_view_objects_forbid_delete
    BEFORE DELETE ON project_view_objects
    FOR EACH ROW
    EXECUTE FUNCTION project_view_forbid_object_delete();

CREATE FUNCTION project_view_validate_aggregate() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    target_community UUID := COALESCE(NEW.community_id, OLD.community_id);
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM project_view_state
        WHERE community_id = target_community
    ) THEN
        RAISE EXCEPTION 'Project View state missing for community %', target_community
            USING ERRCODE = 'foreign_key_violation';
    END IF;

    IF NOT EXISTS (
        SELECT 1
        FROM project_view_objects
        WHERE community_id = target_community
          AND object_id = target_community
          AND object_type = 'project_profile'
          AND deleted_at IS NULL
    ) THEN
        RAISE EXCEPTION 'Project View requires one active Profile for community %',
            target_community
            USING ERRCODE = 'check_violation';
    END IF;

    IF NOT EXISTS (
        SELECT 1
        FROM project_view_objects
        WHERE community_id = target_community
          AND object_type = 'goal'
          AND deleted_at IS NULL
    ) THEN
        RAISE EXCEPTION 'Project View requires at least one active Goal for community %',
            target_community
            USING ERRCODE = 'check_violation';
    END IF;

    RETURN NULL;
END
$$;

CREATE FUNCTION project_view_validate_object() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    current_object project_view_objects%ROWTYPE;
    state_revision BIGINT;
BEGIN
    SELECT *
    INTO current_object
    FROM project_view_objects
    WHERE community_id = NEW.community_id
      AND object_id = NEW.object_id;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'Project View object % is missing during validation', NEW.object_id
            USING ERRCODE = 'foreign_key_violation';
    END IF;

    SELECT project_revision
    INTO state_revision
    FROM project_view_state
    WHERE community_id = current_object.community_id;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'Project View state missing for community %',
            current_object.community_id
            USING ERRCODE = 'foreign_key_violation';
    END IF;

    IF current_object.project_revision > state_revision THEN
        RAISE EXCEPTION 'Project View object revision is ahead of state for community %',
            current_object.community_id
            USING ERRCODE = 'check_violation';
    END IF;

    IF current_object.deleted_at IS NULL THEN
        IF current_object.under_goal_id IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM project_view_objects target
            WHERE target.community_id = current_object.community_id
              AND target.object_id = current_object.under_goal_id
              AND target.object_type = 'goal'
              AND target.deleted_at IS NULL
        ) THEN
            RAISE EXCEPTION 'Project View under_goal relation has an invalid target'
                USING ERRCODE = 'foreign_key_violation';
        END IF;

        IF current_object.under_plan_id IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM project_view_objects target
            WHERE target.community_id = current_object.community_id
              AND target.object_id = current_object.under_plan_id
              AND target.object_type = 'plan'
              AND target.deleted_at IS NULL
        ) THEN
            RAISE EXCEPTION 'Project View under_plan relation has an invalid target'
                USING ERRCODE = 'foreign_key_violation';
        END IF;

        IF current_object.planned_in_stage_id IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM project_view_objects target
            WHERE target.community_id = current_object.community_id
              AND target.object_id = current_object.planned_in_stage_id
              AND target.object_type = 'stage'
              AND target.deleted_at IS NULL
        ) THEN
            RAISE EXCEPTION 'Project View planned_in_stage relation has an invalid target'
                USING ERRCODE = 'foreign_key_violation';
        END IF;

        IF current_object.about_object_id IS NOT NULL AND (
            current_object.about_object_id = current_object.object_id
            OR NOT EXISTS (
                SELECT 1 FROM project_view_objects target
                WHERE target.community_id = current_object.community_id
                  AND target.object_id = current_object.about_object_id
                  AND target.object_type = current_object.about_object_type
                  AND target.deleted_at IS NULL
            )
        ) THEN
            RAISE EXCEPTION 'Project View about relation has an invalid target'
                USING ERRCODE = 'foreign_key_violation';
        END IF;

        IF current_object.handles_object_id IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM project_view_objects target
            WHERE target.community_id = current_object.community_id
              AND target.object_id = current_object.handles_object_id
              AND target.object_type = current_object.handles_object_type
              AND target.object_type IN ('requirement', 'issue')
              AND target.deleted_at IS NULL
        ) THEN
            RAISE EXCEPTION 'Project View handles relation has an invalid target'
                USING ERRCODE = 'foreign_key_violation';
        END IF;
    ELSIF EXISTS (
        SELECT 1
        FROM project_view_objects source
        WHERE source.community_id = current_object.community_id
          AND source.deleted_at IS NULL
          AND (
              source.under_goal_id = current_object.object_id
              OR source.under_plan_id = current_object.object_id
              OR source.planned_in_stage_id = current_object.object_id
              OR source.about_object_id = current_object.object_id
              OR source.handles_object_id = current_object.object_id
          )
    ) THEN
        RAISE EXCEPTION 'Project View tombstone % still has an active inbound relation',
            current_object.object_id
            USING ERRCODE = 'foreign_key_violation';
    END IF;

    IF NOT EXISTS (
        SELECT 1
        FROM project_view_objects
        WHERE community_id = current_object.community_id
          AND object_id = current_object.community_id
          AND object_type = 'project_profile'
          AND deleted_at IS NULL
    ) THEN
        RAISE EXCEPTION 'Project View requires one active Profile for community %',
            current_object.community_id
            USING ERRCODE = 'check_violation';
    END IF;

    IF NOT EXISTS (
        SELECT 1
        FROM project_view_objects
        WHERE community_id = current_object.community_id
          AND object_type = 'goal'
          AND deleted_at IS NULL
    ) THEN
        RAISE EXCEPTION 'Project View requires at least one active Goal for community %',
            current_object.community_id
            USING ERRCODE = 'check_violation';
    END IF;

    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER project_view_state_validate
    AFTER INSERT OR UPDATE ON project_view_state
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_view_validate_aggregate();

CREATE CONSTRAINT TRIGGER project_view_objects_validate
    AFTER INSERT OR UPDATE ON project_view_objects
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_view_validate_object();

-- ── Project View v2 role continuity (keep in sync with migration 0026) ─────

-- Project View v2 role-continuity storage and membership consistency guards.
--
-- This migration is additive. Every existing and newly-created Community
-- remains on schema version 1, so applying it cannot advertise or enable v2.
-- A later explicit cutover must populate valid v2 state and atomically move a
-- single Community to project_view_schema_version = 2.

ALTER TABLE communities
    ADD COLUMN project_view_schema_version SMALLINT NOT NULL DEFAULT 1,
    ADD CONSTRAINT communities_project_view_schema_version_check
        CHECK (project_view_schema_version IN (1, 2));

ALTER TABLE project_view_state
    ADD COLUMN schema_version SMALLINT NOT NULL DEFAULT 1,
    ADD COLUMN last_change_id BYTEA,
    ADD COLUMN last_source_event_id BYTEA;

UPDATE project_view_state
SET last_change_id = last_event_id,
    last_source_event_id = last_event_id;

ALTER TABLE project_view_state
    ALTER COLUMN last_change_id SET NOT NULL,
    ADD CONSTRAINT project_view_state_schema_version_check
        CHECK (schema_version IN (1, 2)),
    ADD CONSTRAINT project_view_state_last_change_id_check
        CHECK (octet_length(last_change_id) = 32),
    ADD CONSTRAINT project_view_state_last_source_event_id_check
        CHECK (
            last_source_event_id IS NULL
            OR octet_length(last_source_event_id) = 32
        );

-- Preserve post-migration compatibility with v1 Relay binaries. Those
-- binaries only write last_event_id; this trigger mirrors it into the v2
-- source columns before NOT NULL/CHECK constraints run.
CREATE FUNCTION project_view_sync_v1_change_source() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.schema_version = 1 THEN
        NEW.last_change_id := NEW.last_event_id;
        NEW.last_source_event_id := NEW.last_event_id;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER project_view_state_sync_v1_change_source
    BEFORE INSERT OR UPDATE OF last_event_id, schema_version ON project_view_state
    FOR EACH ROW
    EXECUTE FUNCTION project_view_sync_v1_change_source();

ALTER TABLE project_view_objects
    DROP CONSTRAINT project_view_objects_schema_check;

ALTER TABLE project_view_objects
    ADD COLUMN role_level TEXT,
    ADD COLUMN responsible_role_id UUID;

ALTER TABLE project_view_objects
    ADD CONSTRAINT project_view_objects_schema_check
        CHECK (schema_version IN (1, 2));

ALTER TABLE project_view_objects
    ADD CONSTRAINT project_view_objects_v2_fields_check
        CHECK (
            (
                schema_version = 1
                AND role_level IS NULL
                AND responsible_role_id IS NULL
            )
            OR
            (
                schema_version = 2
                AND (
                    (
                        object_type = 'role'
                        AND role_level IS NOT NULL
                        AND role_level IN ('admin', 'member')
                        AND (
                            deleted_at IS NOT NULL
                            OR body->>'level' = role_level
                        )
                    )
                    OR
                    (
                        object_type <> 'role'
                        AND role_level IS NULL
                    )
                )
                AND (
                    object_type = 'work'
                    OR responsible_role_id IS NULL
                )
            )
        );

ALTER TABLE project_view_objects
    ADD CONSTRAINT project_view_objects_responsible_role_tombstone_check
        CHECK (deleted_at IS NULL OR responsible_role_id IS NULL);

ALTER TABLE project_view_objects
    ADD CONSTRAINT project_view_objects_responsible_role_fk
        FOREIGN KEY (community_id, responsible_role_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX idx_project_view_role_level
    ON project_view_objects (community_id, role_level, object_id)
    WHERE object_type = 'role' AND deleted_at IS NULL;

CREATE INDEX idx_project_view_work_responsible_role
    ON project_view_objects (community_id, responsible_role_id, object_id)
    WHERE object_type = 'work'
      AND deleted_at IS NULL
      AND responsible_role_id IS NOT NULL;

-- Unified v2 audit/idempotency source. v1 receipts remain in
-- project_view_mutations and are not rewritten.
CREATE TABLE project_view_changes (
    community_id             UUID        NOT NULL,
    change_id                BYTEA       NOT NULL,
    source_type              TEXT        NOT NULL,
    source_event_id          BYTEA,
    source_request_hash      BYTEA,
    source_audit_seq         BIGINT,
    idempotency_key_hash     BYTEA,
    actor_pubkey             BYTEA,
    acting_assignment_id     UUID,
    operation                TEXT        NOT NULL,
    subject                  JSONB       NOT NULL,
    project_revision         BIGINT      NOT NULL,
    result                   JSONB       NOT NULL,
    accepted_at              TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, change_id),
    CONSTRAINT project_view_changes_revision_unique
        UNIQUE (community_id, project_revision),
    CONSTRAINT project_view_changes_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_view_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_changes_audit_fk
        FOREIGN KEY (community_id, source_audit_seq)
        REFERENCES audit_log (community_id, seq)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_changes_change_id_check
        CHECK (octet_length(change_id) = 32),
    CONSTRAINT project_view_changes_source_type_check
        CHECK (source_type IN ('nostr_event', 'nip98_request', 'operator', 'system')),
    CONSTRAINT project_view_changes_source_event_id_check
        CHECK (source_event_id IS NULL OR octet_length(source_event_id) = 32),
    CONSTRAINT project_view_changes_source_request_hash_check
        CHECK (source_request_hash IS NULL OR octet_length(source_request_hash) = 32),
    CONSTRAINT project_view_changes_idempotency_hash_check
        CHECK (idempotency_key_hash IS NULL OR octet_length(idempotency_key_hash) = 32),
    CONSTRAINT project_view_changes_actor_check
        CHECK (actor_pubkey IS NULL OR octet_length(actor_pubkey) = 32),
    CONSTRAINT project_view_changes_source_shape_check
        CHECK (
            (
                source_type = 'nostr_event'
                AND source_event_id IS NOT NULL
                AND source_request_hash IS NULL
                AND source_audit_seq IS NULL
                AND idempotency_key_hash IS NULL
            )
            OR
            (
                source_type = 'nip98_request'
                AND source_event_id IS NOT NULL
                AND source_request_hash IS NOT NULL
                AND source_audit_seq IS NULL
                AND idempotency_key_hash IS NULL
            )
            OR
            (
                source_type IN ('operator', 'system')
                AND source_event_id IS NULL
                AND source_request_hash IS NULL
                AND source_audit_seq > 0
                AND idempotency_key_hash IS NOT NULL
            )
        ),
    CONSTRAINT project_view_changes_operation_check
        CHECK (operation <> '' AND octet_length(operation) <= 96),
    CONSTRAINT project_view_changes_subject_check
        CHECK (jsonb_typeof(subject) = 'object'),
    CONSTRAINT project_view_changes_result_check
        CHECK (jsonb_typeof(result) = 'object'),
    CONSTRAINT project_view_changes_revision_check
        CHECK (project_revision BETWEEN 1 AND 9007199254740991)
);

CREATE INDEX idx_project_view_changes_accepted
    ON project_view_changes (community_id, accepted_at, change_id);

CREATE UNIQUE INDEX idx_project_view_changes_source_event
    ON project_view_changes (community_id, source_event_id)
    WHERE source_event_id IS NOT NULL;

CREATE UNIQUE INDEX idx_project_view_changes_source_audit
    ON project_view_changes (community_id, source_audit_seq)
    WHERE source_audit_seq IS NOT NULL;

CREATE UNIQUE INDEX idx_project_view_changes_idempotency
    ON project_view_changes (community_id, idempotency_key_hash)
    WHERE idempotency_key_hash IS NOT NULL;

CREATE TABLE project_role_assignments (
    community_id             UUID        NOT NULL,
    assignment_id            UUID        NOT NULL,
    role_id                  UUID        NOT NULL,
    member_pubkey            TEXT        NOT NULL,
    proposal_id              UUID,
    started_at               TIMESTAMPTZ NOT NULL,
    started_by               BYTEA       NOT NULL,
    ended_at                 TIMESTAMPTZ,
    ended_by                 BYTEA,
    ended_reason             TEXT,
    source_change_id         BYTEA       NOT NULL,
    ended_source_change_id   BYTEA,
    project_revision         BIGINT      NOT NULL,
    projection_event_id      BYTEA,

    PRIMARY KEY (community_id, assignment_id),
    CONSTRAINT project_role_assignments_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_view_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_assignments_role_fk
        FOREIGN KEY (community_id, role_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_assignments_source_fk
        FOREIGN KEY (community_id, source_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_assignments_ended_source_fk
        FOREIGN KEY (community_id, ended_source_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_assignments_id_check
        CHECK (assignment_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_role_assignments_member_check
        CHECK (member_pubkey ~ '^[0-9a-f]{64}$'),
    CONSTRAINT project_role_assignments_started_by_check
        CHECK (octet_length(started_by) = 32),
    CONSTRAINT project_role_assignments_ended_by_check
        CHECK (ended_by IS NULL OR octet_length(ended_by) = 32),
    CONSTRAINT project_role_assignments_projection_check
        CHECK (projection_event_id IS NULL OR octet_length(projection_event_id) = 32),
    CONSTRAINT project_role_assignments_revision_check
        CHECK (project_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_role_assignments_end_shape_check
        CHECK (
            (
                ended_at IS NULL
                AND ended_by IS NULL
                AND ended_reason IS NULL
                AND ended_source_change_id IS NULL
            )
            OR
            (
                ended_at IS NOT NULL
                AND ended_at >= started_at
                AND ended_by IS NOT NULL
                AND ended_reason IN (
                    'revoked', 'replaced', 'membership_ended', 'unrecoverable'
                )
                AND ended_source_change_id IS NOT NULL
            )
        )
);

CREATE UNIQUE INDEX idx_project_role_assignments_active_role
    ON project_role_assignments (community_id, role_id)
    WHERE ended_at IS NULL;

CREATE UNIQUE INDEX idx_project_role_assignments_active_member
    ON project_role_assignments (community_id, member_pubkey)
    WHERE ended_at IS NULL;

CREATE INDEX idx_project_role_assignments_history
    ON project_role_assignments (community_id, role_id, started_at DESC, assignment_id);

CREATE TABLE project_role_assignment_proposals (
    community_id                     UUID        NOT NULL,
    proposal_id                      UUID        NOT NULL,
    role_id                          UUID        NOT NULL,
    candidate_pubkey                 TEXT        NOT NULL,
    proposal_type                    TEXT        NOT NULL,
    candidate_accepted_at            TIMESTAMPTZ,
    authorized_by                    BYTEA,
    authorized_at                    TIMESTAMPTZ,
    expected_target_assignment_id    UUID,
    expected_candidate_assignment_id UUID,
    expires_at                       TIMESTAMPTZ NOT NULL,
    status                           TEXT        NOT NULL,
    reason                           TEXT,
    created_by                       BYTEA       NOT NULL,
    created_at                       TIMESTAMPTZ NOT NULL,
    resolved_at                      TIMESTAMPTZ,
    source_change_id                 BYTEA       NOT NULL,
    project_revision                 BIGINT      NOT NULL,
    projection_event_id              BYTEA,

    PRIMARY KEY (community_id, proposal_id),
    CONSTRAINT project_role_proposals_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_view_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_proposals_role_fk
        FOREIGN KEY (community_id, role_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_proposals_target_assignment_fk
        FOREIGN KEY (community_id, expected_target_assignment_id)
        REFERENCES project_role_assignments (community_id, assignment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_proposals_candidate_assignment_fk
        FOREIGN KEY (community_id, expected_candidate_assignment_id)
        REFERENCES project_role_assignments (community_id, assignment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_proposals_source_fk
        FOREIGN KEY (community_id, source_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_proposals_id_check
        CHECK (proposal_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_role_proposals_candidate_check
        CHECK (candidate_pubkey ~ '^[0-9a-f]{64}$'),
    CONSTRAINT project_role_proposals_type_check
        CHECK (proposal_type IN ('request', 'offer')),
    CONSTRAINT project_role_proposals_status_check
        CHECK (status IN ('open', 'consumed', 'rejected', 'withdrawn', 'expired')),
    CONSTRAINT project_role_proposals_authorizer_check
        CHECK (
            (authorized_by IS NULL) = (authorized_at IS NULL)
            AND (authorized_by IS NULL OR octet_length(authorized_by) = 32)
        ),
    CONSTRAINT project_role_proposals_created_by_check
        CHECK (octet_length(created_by) = 32),
    CONSTRAINT project_role_proposals_projection_check
        CHECK (projection_event_id IS NULL OR octet_length(projection_event_id) = 32),
    CONSTRAINT project_role_proposals_revision_check
        CHECK (project_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_role_proposals_time_check
        CHECK (
            expires_at > created_at
            AND (candidate_accepted_at IS NULL OR candidate_accepted_at >= created_at)
            AND (authorized_at IS NULL OR authorized_at >= created_at)
            AND (
                (status = 'open' AND resolved_at IS NULL)
                OR (status <> 'open' AND resolved_at IS NOT NULL)
            )
        )
);

ALTER TABLE project_role_assignments
    ADD CONSTRAINT project_role_assignments_proposal_fk
        FOREIGN KEY (community_id, proposal_id)
        REFERENCES project_role_assignment_proposals (community_id, proposal_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

CREATE UNIQUE INDEX idx_project_role_proposals_open_candidate
    ON project_role_assignment_proposals (community_id, role_id, candidate_pubkey)
    WHERE status = 'open';

CREATE INDEX idx_project_role_proposals_candidate
    ON project_role_assignment_proposals
       (community_id, candidate_pubkey, status, expires_at);

CREATE TABLE project_work_commitments (
    community_id          UUID        NOT NULL,
    commitment_id         UUID        NOT NULL,
    work_id               UUID        NOT NULL,
    assignment_id         UUID        NOT NULL,
    accepted_at           TIMESTAMPTZ NOT NULL,
    accepted_by           BYTEA       NOT NULL,
    ended_at              TIMESTAMPTZ,
    ended_by              BYTEA,
    ended_reason          TEXT,
    source_change_id      BYTEA       NOT NULL,
    ended_source_change_id BYTEA,
    project_revision      BIGINT      NOT NULL,
    projection_event_id   BYTEA,

    PRIMARY KEY (community_id, commitment_id),
    CONSTRAINT project_work_commitments_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_view_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_work_commitments_work_fk
        FOREIGN KEY (community_id, work_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_work_commitments_assignment_fk
        FOREIGN KEY (community_id, assignment_id)
        REFERENCES project_role_assignments (community_id, assignment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_work_commitments_source_fk
        FOREIGN KEY (community_id, source_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_work_commitments_ended_source_fk
        FOREIGN KEY (community_id, ended_source_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_work_commitments_id_check
        CHECK (commitment_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_work_commitments_accepted_by_check
        CHECK (octet_length(accepted_by) = 32),
    CONSTRAINT project_work_commitments_ended_by_check
        CHECK (ended_by IS NULL OR octet_length(ended_by) = 32),
    CONSTRAINT project_work_commitments_projection_check
        CHECK (projection_event_id IS NULL OR octet_length(projection_event_id) = 32),
    CONSTRAINT project_work_commitments_revision_check
        CHECK (project_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_work_commitments_end_shape_check
        CHECK (
            (
                ended_at IS NULL
                AND ended_by IS NULL
                AND ended_reason IS NULL
                AND ended_source_change_id IS NULL
            )
            OR
            (
                ended_at IS NOT NULL
                AND ended_at >= accepted_at
                AND ended_by IS NOT NULL
                AND ended_reason IN ('released', 'replaced', 'assignment_ended', 'work_closed')
                AND ended_source_change_id IS NOT NULL
            )
        )
);

CREATE UNIQUE INDEX idx_project_work_commitments_active_work
    ON project_work_commitments (community_id, work_id)
    WHERE ended_at IS NULL;

CREATE INDEX idx_project_work_commitments_assignment
    ON project_work_commitments (community_id, assignment_id, accepted_at DESC);

CREATE TABLE project_role_checkpoints (
    community_id        UUID        NOT NULL,
    checkpoint_id       UUID        NOT NULL,
    role_id             UUID        NOT NULL,
    assignment_id       UUID        NOT NULL,
    body                JSONB       NOT NULL,
    created_by          BYTEA       NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL,
    source_change_id    BYTEA       NOT NULL,
    project_revision    BIGINT      NOT NULL,
    projection_event_id BYTEA,

    PRIMARY KEY (community_id, checkpoint_id),
    CONSTRAINT project_role_checkpoints_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_view_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_checkpoints_role_fk
        FOREIGN KEY (community_id, role_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_checkpoints_assignment_fk
        FOREIGN KEY (community_id, assignment_id)
        REFERENCES project_role_assignments (community_id, assignment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_checkpoints_source_fk
        FOREIGN KEY (community_id, source_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_checkpoints_body_check
        CHECK (jsonb_typeof(body) = 'object'),
    CONSTRAINT project_role_checkpoints_created_by_check
        CHECK (octet_length(created_by) = 32),
    CONSTRAINT project_role_checkpoints_projection_check
        CHECK (projection_event_id IS NULL OR octet_length(projection_event_id) = 32),
    CONSTRAINT project_role_checkpoints_revision_check
        CHECK (project_revision BETWEEN 1 AND 9007199254740991)
);

CREATE INDEX idx_project_role_checkpoints_history
    ON project_role_checkpoints (community_id, role_id, created_at DESC, checkpoint_id);

CREATE TABLE project_role_handoffs (
    community_id           UUID        NOT NULL,
    handoff_id             UUID        NOT NULL,
    role_id                UUID        NOT NULL,
    from_assignment_id     UUID,
    to_assignment_id       UUID,
    body                   JSONB       NOT NULL,
    system_generated       BOOLEAN     NOT NULL DEFAULT FALSE,
    created_by             BYTEA,
    created_at             TIMESTAMPTZ NOT NULL,
    source_change_id       BYTEA       NOT NULL,
    project_revision       BIGINT      NOT NULL,
    projection_event_id    BYTEA,

    PRIMARY KEY (community_id, handoff_id),
    CONSTRAINT project_role_handoffs_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_view_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_handoffs_role_fk
        FOREIGN KEY (community_id, role_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_handoffs_from_assignment_fk
        FOREIGN KEY (community_id, from_assignment_id)
        REFERENCES project_role_assignments (community_id, assignment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_handoffs_to_assignment_fk
        FOREIGN KEY (community_id, to_assignment_id)
        REFERENCES project_role_assignments (community_id, assignment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_handoffs_source_fk
        FOREIGN KEY (community_id, source_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_handoffs_body_check
        CHECK (jsonb_typeof(body) = 'object'),
    CONSTRAINT project_role_handoffs_actor_check
        CHECK (
            (system_generated AND created_by IS NULL)
            OR (
                NOT system_generated
                AND created_by IS NOT NULL
                AND octet_length(created_by) = 32
            )
        ),
    CONSTRAINT project_role_handoffs_assignment_check
        CHECK (from_assignment_id IS NOT NULL OR to_assignment_id IS NOT NULL),
    CONSTRAINT project_role_handoffs_projection_check
        CHECK (projection_event_id IS NULL OR octet_length(projection_event_id) = 32),
    CONSTRAINT project_role_handoffs_revision_check
        CHECK (project_revision BETWEEN 1 AND 9007199254740991)
);

CREATE INDEX idx_project_role_handoffs_history
    ON project_role_handoffs (community_id, role_id, created_at DESC, handoff_id);

CREATE TABLE project_role_continuity_references (
    community_id       UUID        NOT NULL,
    owner_type         TEXT        NOT NULL,
    owner_id           UUID        NOT NULL,
    position           INTEGER     NOT NULL,
    reference_type     TEXT        NOT NULL,
    object_id          UUID,
    assignment_id      UUID,
    commitment_id      UUID,
    nostr_event_id     BYTEA,
    label              TEXT,
    source_change_id   BYTEA       NOT NULL,

    PRIMARY KEY (community_id, owner_type, owner_id, position),
    CONSTRAINT project_role_references_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_view_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_references_object_fk
        FOREIGN KEY (community_id, object_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_references_assignment_fk
        FOREIGN KEY (community_id, assignment_id)
        REFERENCES project_role_assignments (community_id, assignment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_references_commitment_fk
        FOREIGN KEY (community_id, commitment_id)
        REFERENCES project_work_commitments (community_id, commitment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_references_source_fk
        FOREIGN KEY (community_id, source_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_role_references_owner_type_check
        CHECK (owner_type IN ('checkpoint', 'handoff')),
    CONSTRAINT project_role_references_position_check
        CHECK (position >= 0 AND position < 256),
    CONSTRAINT project_role_references_type_check
        CHECK (reference_type IN ('object', 'assignment', 'commitment', 'nostr_event')),
    CONSTRAINT project_role_references_target_shape_check
        CHECK (
            (
                reference_type = 'object'
                AND object_id IS NOT NULL
                AND assignment_id IS NULL
                AND commitment_id IS NULL
                AND nostr_event_id IS NULL
            )
            OR
            (
                reference_type = 'assignment'
                AND object_id IS NULL
                AND assignment_id IS NOT NULL
                AND commitment_id IS NULL
                AND nostr_event_id IS NULL
            )
            OR
            (
                reference_type = 'commitment'
                AND object_id IS NULL
                AND assignment_id IS NULL
                AND commitment_id IS NOT NULL
                AND nostr_event_id IS NULL
            )
            OR
            (
                reference_type = 'nostr_event'
                AND object_id IS NULL
                AND assignment_id IS NULL
                AND commitment_id IS NULL
                AND nostr_event_id IS NOT NULL
                AND octet_length(nostr_event_id) = 32
            )
        )
);

-- Validate the final v2 Role/Assignment/Membership shape at transaction
-- commit. Application checks produce friendly errors; this function is the
-- last line of defence against direct SQL, old admin paths, and future
-- regressions. v1 Communities return immediately and retain their exact
-- historical semantics.
CREATE FUNCTION project_role_continuity_validate_community(target_community UUID)
RETURNS VOID
LANGUAGE plpgsql AS $$
DECLARE
    target_schema SMALLINT;
    state_schema SMALLINT;
    owner_count INTEGER;
BEGIN
    SELECT project_view_schema_version
    INTO target_schema
    FROM communities
    WHERE id = target_community;

    IF NOT FOUND OR target_schema <> 2 THEN
        RETURN;
    END IF;

    SELECT schema_version
    INTO state_schema
    FROM project_view_state
    WHERE community_id = target_community;

    IF NOT FOUND OR state_schema <> 2 THEN
        RAISE EXCEPTION 'Project View v2 state missing for community %', target_community
            USING ERRCODE = 'check_violation';
    END IF;

    SELECT count(*)::integer
    INTO owner_count
    FROM relay_members
    WHERE community_id = target_community
      AND role = 'owner';

    IF owner_count <> 1 THEN
        RAISE EXCEPTION 'Project View v2 requires exactly one Community owner'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM relay_members member
        WHERE member.community_id = target_community
          AND member.pubkey !~ '^[0-9a-f]{64}$'
    ) THEN
        RAISE EXCEPTION 'Project View v2 membership pubkeys must be lowercase 32-byte hex'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM relay_members member
        JOIN users actor
          ON actor.community_id = member.community_id
         AND encode(actor.pubkey, 'hex') = member.pubkey
        WHERE member.community_id = target_community
          AND member.role = 'owner'
          AND actor.agent_owner_pubkey IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'A known managed Agent cannot be the Community owner'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM relay_members owner_member
        JOIN community_bans restriction
          ON restriction.community_id = owner_member.community_id
         AND restriction.pubkey = decode(owner_member.pubkey, 'hex')
        WHERE owner_member.community_id = target_community
          AND owner_member.role = 'owner'
          AND restriction.banned
          AND (
              restriction.ban_expires_at IS NULL
              OR restriction.ban_expires_at > clock_timestamp()
          )
    ) THEN
        RAISE EXCEPTION 'Community owner cannot be banned before ownership transfer'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_view_objects object
        WHERE object.community_id = target_community
          AND object.deleted_at IS NULL
          AND object.schema_version <> 2
    ) THEN
        RAISE EXCEPTION 'Active Project View objects must use schema version 2 after cutover'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_role_assignments assignment
        LEFT JOIN project_view_objects role_object
          ON role_object.community_id = assignment.community_id
         AND role_object.object_id = assignment.role_id
        LEFT JOIN relay_members member
          ON member.community_id = assignment.community_id
         AND member.pubkey = assignment.member_pubkey
        WHERE assignment.community_id = target_community
          AND assignment.ended_at IS NULL
          AND (
              role_object.object_id IS NULL
              OR role_object.object_type <> 'role'
              OR role_object.schema_version <> 2
              OR role_object.deleted_at IS NOT NULL
              OR role_object.body->'active' IS DISTINCT FROM 'true'::jsonb
              OR role_object.role_level NOT IN ('admin', 'member')
              OR member.pubkey IS NULL
              OR (
                  member.role <> 'owner'
                  AND member.role IS DISTINCT FROM role_object.role_level
              )
          )
    ) THEN
        RAISE EXCEPTION 'Active Role Assignment has an invalid Role or Community membership'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM relay_members member
        WHERE member.community_id = target_community
          AND member.role = 'admin'
          AND NOT EXISTS (
              SELECT 1
              FROM project_role_assignments assignment
              JOIN project_view_objects role_object
                ON role_object.community_id = assignment.community_id
               AND role_object.object_id = assignment.role_id
              WHERE assignment.community_id = member.community_id
                AND assignment.member_pubkey = member.pubkey
                AND assignment.ended_at IS NULL
                AND role_object.deleted_at IS NULL
                AND role_object.object_type = 'role'
                AND role_object.role_level = 'admin'
          )
    ) THEN
        RAISE EXCEPTION 'Non-owner Community admin requires one active Leader Assignment'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_role_assignments assignment
        JOIN users agent
          ON agent.community_id = assignment.community_id
         AND encode(agent.pubkey, 'hex') = assignment.member_pubkey
        WHERE assignment.community_id = target_community
          AND assignment.ended_at IS NULL
          AND agent.agent_owner_pubkey IS NOT NULL
          AND (
              NOT EXISTS (
                  SELECT 1
                  FROM relay_members owner_member
                  WHERE owner_member.community_id = assignment.community_id
                    AND owner_member.pubkey = encode(agent.agent_owner_pubkey, 'hex')
              )
              OR EXISTS (
                  SELECT 1
                  FROM users owner_actor
                  WHERE owner_actor.community_id = assignment.community_id
                    AND owner_actor.pubkey = agent.agent_owner_pubkey
                    AND owner_actor.agent_owner_pubkey IS NOT NULL
              )
              OR EXISTS (
                  SELECT 1
                  FROM community_bans restriction
                  WHERE restriction.community_id = assignment.community_id
                    AND restriction.pubkey IN (agent.pubkey, agent.agent_owner_pubkey)
                    AND restriction.banned
                    AND (
                        restriction.ban_expires_at IS NULL
                        OR restriction.ban_expires_at > clock_timestamp()
                    )
              )
          )
    ) THEN
        RAISE EXCEPTION 'Managed Agent Assignment requires an eligible Community-member owner'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_role_assignments assignment
        JOIN community_bans restriction
          ON restriction.community_id = assignment.community_id
         AND restriction.pubkey = decode(assignment.member_pubkey, 'hex')
        WHERE assignment.community_id = target_community
          AND assignment.ended_at IS NULL
          AND restriction.banned
          AND (
              restriction.ban_expires_at IS NULL
              OR restriction.ban_expires_at > clock_timestamp()
          )
    ) THEN
        RAISE EXCEPTION 'A persistently banned Member cannot retain an active Assignment'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_view_objects work_object
        LEFT JOIN project_view_objects role_object
          ON role_object.community_id = work_object.community_id
         AND role_object.object_id = work_object.responsible_role_id
        WHERE work_object.community_id = target_community
          AND work_object.deleted_at IS NULL
          AND work_object.object_type = 'work'
          AND work_object.responsible_role_id IS NOT NULL
          AND (
              role_object.object_id IS NULL
              OR role_object.deleted_at IS NOT NULL
              OR role_object.object_type <> 'role'
              OR role_object.body->'active' IS DISTINCT FROM 'true'::jsonb
          )
    ) THEN
        RAISE EXCEPTION 'Work responsible Role must be an active Role in the same Project'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_work_commitments commitment
        LEFT JOIN project_role_assignments assignment
          ON assignment.community_id = commitment.community_id
         AND assignment.assignment_id = commitment.assignment_id
        LEFT JOIN project_view_objects work_object
          ON work_object.community_id = commitment.community_id
         AND work_object.object_id = commitment.work_id
        WHERE commitment.community_id = target_community
          AND commitment.ended_at IS NULL
          AND (
              assignment.assignment_id IS NULL
              OR assignment.ended_at IS NOT NULL
              OR work_object.object_id IS NULL
              OR work_object.deleted_at IS NOT NULL
              OR work_object.object_type <> 'work'
              OR work_object.responsible_role_id IS DISTINCT FROM assignment.role_id
          )
    ) THEN
        RAISE EXCEPTION 'Active Work Commitment does not match its Assignment and responsible Role'
            USING ERRCODE = 'check_violation';
    END IF;
END
$$;

CREATE FUNCTION project_role_continuity_validate_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM project_role_continuity_validate_community(OLD.community_id);
    ELSE
        IF TG_OP = 'UPDATE' THEN
            IF OLD.community_id IS DISTINCT FROM NEW.community_id THEN
                PERFORM project_role_continuity_validate_community(OLD.community_id);
            END IF;
        END IF;
        PERFORM project_role_continuity_validate_community(NEW.community_id);
    END IF;
    RETURN NULL;
END
$$;

CREATE FUNCTION project_role_continuity_validate_community_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM project_role_continuity_validate_community(OLD.id);
    ELSE
        IF TG_OP = 'UPDATE' THEN
            IF OLD.id IS DISTINCT FROM NEW.id THEN
                PERFORM project_role_continuity_validate_community(OLD.id);
            END IF;
        END IF;
        PERFORM project_role_continuity_validate_community(NEW.id);
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER communities_role_continuity_validate
    AFTER INSERT OR UPDATE ON communities
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_community_row();

CREATE CONSTRAINT TRIGGER project_view_state_role_continuity_validate
    AFTER INSERT OR UPDATE ON project_view_state
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_row();

CREATE CONSTRAINT TRIGGER project_view_objects_role_continuity_validate
    AFTER INSERT OR UPDATE ON project_view_objects
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_row();

CREATE CONSTRAINT TRIGGER project_role_assignments_validate
    AFTER INSERT OR UPDATE ON project_role_assignments
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_row();

CREATE CONSTRAINT TRIGGER project_work_commitments_validate
    AFTER INSERT OR UPDATE ON project_work_commitments
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_row();

CREATE CONSTRAINT TRIGGER relay_members_role_continuity_validate
    AFTER INSERT OR UPDATE OR DELETE ON relay_members
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_row();

CREATE CONSTRAINT TRIGGER users_role_continuity_validate
    AFTER INSERT OR UPDATE OR DELETE ON users
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_row();

CREATE CONSTRAINT TRIGGER community_bans_role_continuity_validate
    AFTER INSERT OR UPDATE OR DELETE ON community_bans
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_row();

-- ── Project View v2 Proposal / Assignment state (migration 0027) ───────────

ALTER TABLE project_view_state
    ADD COLUMN open_proposal_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN active_assignment_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN active_commitment_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN checkpoint_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN handoff_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN membership_snapshot_event_id BYTEA,
    ADD CONSTRAINT project_view_state_v2_counts_check
        CHECK (
            open_proposal_count >= 0
            AND active_assignment_count >= 0
            AND active_commitment_count >= 0
            AND checkpoint_count >= 0
            AND handoff_count >= 0
        ),
    ADD CONSTRAINT project_view_state_membership_snapshot_check
        CHECK (
            membership_snapshot_event_id IS NULL
            OR octet_length(membership_snapshot_event_id) = 32
        );

ALTER TABLE project_role_assignment_proposals
    ADD COLUMN entity_revision BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN updated_at TIMESTAMPTZ,
    ADD COLUMN last_change_id BYTEA;

UPDATE project_role_assignment_proposals
SET updated_at = COALESCE(resolved_at, created_at),
    last_change_id = source_change_id;

ALTER TABLE project_role_assignment_proposals
    ALTER COLUMN updated_at SET NOT NULL,
    ALTER COLUMN last_change_id SET NOT NULL,
    ADD CONSTRAINT project_role_proposals_entity_revision_check
        CHECK (entity_revision BETWEEN 1 AND 9007199254740991),
    ADD CONSTRAINT project_role_proposals_updated_time_check
        CHECK (updated_at >= created_at),
    ADD CONSTRAINT project_role_proposals_last_change_check
        CHECK (octet_length(last_change_id) = 32),
    ADD CONSTRAINT project_role_proposals_last_change_fk
        FOREIGN KEY (community_id, last_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE project_role_assignments
    ADD COLUMN entity_revision BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN updated_at TIMESTAMPTZ,
    ADD COLUMN last_change_id BYTEA,
    ADD COLUMN replacement_requested_at TIMESTAMPTZ,
    ADD COLUMN replacement_request_reason TEXT,
    ADD COLUMN unable_reported_at TIMESTAMPTZ,
    ADD COLUMN unable_report_reason TEXT,
    ADD COLUMN replaced_by_assignment_id UUID;

UPDATE project_role_assignments
SET updated_at = COALESCE(ended_at, started_at),
    last_change_id = COALESCE(ended_source_change_id, source_change_id);

ALTER TABLE project_role_assignments
    ALTER COLUMN updated_at SET NOT NULL,
    ALTER COLUMN last_change_id SET NOT NULL,
    DROP CONSTRAINT project_role_assignments_end_shape_check,
    ADD CONSTRAINT project_role_assignments_entity_revision_check
        CHECK (entity_revision BETWEEN 1 AND 9007199254740991),
    ADD CONSTRAINT project_role_assignments_updated_time_check
        CHECK (updated_at >= started_at),
    ADD CONSTRAINT project_role_assignments_last_change_check
        CHECK (octet_length(last_change_id) = 32),
    ADD CONSTRAINT project_role_assignments_last_change_fk
        FOREIGN KEY (community_id, last_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT project_role_assignments_replaced_by_fk
        FOREIGN KEY (community_id, replaced_by_assignment_id)
        REFERENCES project_role_assignments (community_id, assignment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT project_role_assignments_replacement_report_check
        CHECK (
            (replacement_requested_at IS NULL)
                = (replacement_request_reason IS NULL)
            OR (
                replacement_requested_at IS NOT NULL
                AND replacement_request_reason IS NULL
            )
        ),
    ADD CONSTRAINT project_role_assignments_unable_report_check
        CHECK (
            (unable_reported_at IS NULL) = (unable_report_reason IS NULL)
            OR (
                unable_reported_at IS NOT NULL
                AND unable_report_reason IS NULL
            )
        ),
    ADD CONSTRAINT project_role_assignments_report_times_check
        CHECK (
            (replacement_requested_at IS NULL OR replacement_requested_at >= started_at)
            AND (unable_reported_at IS NULL OR unable_reported_at >= started_at)
        ),
    ADD CONSTRAINT project_role_assignments_end_shape_check
        CHECK (
            (
                ended_at IS NULL
                AND ended_by IS NULL
                AND ended_reason IS NULL
                AND ended_source_change_id IS NULL
                AND replaced_by_assignment_id IS NULL
            )
            OR
            (
                ended_at IS NOT NULL
                AND ended_at >= started_at
                AND ended_by IS NOT NULL
                AND ended_reason IN (
                    'revoked',
                    'replaced',
                    'membership_ended',
                    'unrecoverable',
                    'role_deactivated'
                )
                AND ended_source_change_id IS NOT NULL
                AND (
                    (ended_reason = 'replaced' AND replaced_by_assignment_id IS NOT NULL)
                    OR
                    (ended_reason <> 'replaced' AND replaced_by_assignment_id IS NULL)
                )
            )
        );

ALTER TABLE project_role_handoffs
    ADD COLUMN entity_revision BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN last_change_id BYTEA;

UPDATE project_role_handoffs
SET last_change_id = source_change_id;

ALTER TABLE project_role_handoffs
    ALTER COLUMN last_change_id SET NOT NULL,
    ADD CONSTRAINT project_role_handoffs_entity_revision_check
        CHECK (entity_revision BETWEEN 1 AND 9007199254740991),
    ADD CONSTRAINT project_role_handoffs_last_change_check
        CHECK (octet_length(last_change_id) = 32),
    ADD CONSTRAINT project_role_handoffs_last_change_fk
        FOREIGN KEY (community_id, last_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

CREATE FUNCTION project_role_continuity_validate_counts(target_community UUID)
RETURNS VOID
LANGUAGE plpgsql AS $$
DECLARE
    target_schema SMALLINT;
    stored_open_proposals INTEGER;
    stored_active_assignments INTEGER;
    stored_active_commitments INTEGER;
    stored_checkpoints INTEGER;
    stored_handoffs INTEGER;
BEGIN
    SELECT project_view_schema_version
    INTO target_schema
    FROM communities
    WHERE id = target_community;

    IF NOT FOUND OR target_schema <> 2 THEN
        RETURN;
    END IF;

    SELECT
        open_proposal_count,
        active_assignment_count,
        active_commitment_count,
        checkpoint_count,
        handoff_count
    INTO
        stored_open_proposals,
        stored_active_assignments,
        stored_active_commitments,
        stored_checkpoints,
        stored_handoffs
    FROM project_view_state
    WHERE community_id = target_community;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'Project View v2 state missing for community %', target_community
            USING ERRCODE = 'check_violation';
    END IF;

    IF stored_open_proposals <> (
        SELECT count(*)::integer
        FROM project_role_assignment_proposals
        WHERE community_id = target_community AND status = 'open'
    ) OR stored_active_assignments <> (
        SELECT count(*)::integer
        FROM project_role_assignments
        WHERE community_id = target_community AND ended_at IS NULL
    ) OR stored_active_commitments <> (
        SELECT count(*)::integer
        FROM project_work_commitments
        WHERE community_id = target_community AND ended_at IS NULL
    ) OR stored_checkpoints <> (
        SELECT count(*)::integer
        FROM project_role_checkpoints
        WHERE community_id = target_community
    ) OR stored_handoffs <> (
        SELECT count(*)::integer
        FROM project_role_handoffs
        WHERE community_id = target_community
    ) THEN
        RAISE EXCEPTION 'Project View v2 materialized entity counts are inconsistent'
            USING ERRCODE = 'check_violation';
    END IF;
END
$$;

CREATE FUNCTION project_role_continuity_validate_counts_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM project_role_continuity_validate_counts(OLD.community_id);
    ELSE
        IF TG_OP = 'UPDATE' AND OLD.community_id IS DISTINCT FROM NEW.community_id THEN
            PERFORM project_role_continuity_validate_counts(OLD.community_id);
        END IF;
        PERFORM project_role_continuity_validate_counts(NEW.community_id);
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER project_view_state_role_counts_validate
    AFTER INSERT OR UPDATE ON project_view_state
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();

CREATE CONSTRAINT TRIGGER project_role_proposals_counts_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_assignment_proposals
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();

CREATE CONSTRAINT TRIGGER project_role_assignments_counts_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_assignments
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();

CREATE CONSTRAINT TRIGGER project_work_commitments_counts_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_work_commitments
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();

CREATE CONSTRAINT TRIGGER project_role_checkpoints_counts_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_checkpoints
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();

CREATE CONSTRAINT TRIGGER project_role_handoffs_counts_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_handoffs
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();

-- Stage 5: make Work Commitment a first-class projected lifecycle entity.

ALTER TABLE project_work_commitments
    ADD COLUMN member_pubkey TEXT,
    ADD COLUMN entity_revision BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN updated_at TIMESTAMPTZ,
    ADD COLUMN last_change_id BYTEA;

UPDATE project_work_commitments commitment
SET member_pubkey = assignment.member_pubkey,
    updated_at = COALESCE(commitment.ended_at, commitment.accepted_at),
    last_change_id = COALESCE(
        commitment.ended_source_change_id,
        commitment.source_change_id
    )
FROM project_role_assignments assignment
WHERE assignment.community_id = commitment.community_id
  AND assignment.assignment_id = commitment.assignment_id;

ALTER TABLE project_work_commitments
    ALTER COLUMN member_pubkey SET NOT NULL,
    ALTER COLUMN updated_at SET NOT NULL,
    ALTER COLUMN last_change_id SET NOT NULL,
    ADD CONSTRAINT project_work_commitments_member_pubkey_check
        CHECK (member_pubkey ~ '^[0-9a-f]{64}$'),
    ADD CONSTRAINT project_work_commitments_entity_revision_check
        CHECK (entity_revision BETWEEN 1 AND 9007199254740991),
    ADD CONSTRAINT project_work_commitments_last_change_id_check
        CHECK (octet_length(last_change_id) = 32),
    ADD CONSTRAINT project_work_commitments_last_change_fk
        FOREIGN KEY (community_id, last_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

CREATE FUNCTION project_work_commitments_validate_stage5_community(
    target_community UUID
) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM communities
        WHERE id = target_community
          AND project_view_schema_version = 2
    ) THEN
        RETURN;
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_work_commitments commitment
        LEFT JOIN project_role_assignments assignment
          ON assignment.community_id = commitment.community_id
         AND assignment.assignment_id = commitment.assignment_id
        WHERE commitment.community_id = target_community
          AND (
              assignment.assignment_id IS NULL
              OR commitment.member_pubkey IS DISTINCT FROM assignment.member_pubkey
              OR commitment.accepted_by IS DISTINCT FROM
                 decode(commitment.member_pubkey, 'hex')
          )
    ) THEN
        RAISE EXCEPTION 'Work Commitment signer and Member must match its Assignment'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_work_commitments commitment
        LEFT JOIN project_view_objects work_object
          ON work_object.community_id = commitment.community_id
         AND work_object.object_id = commitment.work_id
        WHERE commitment.community_id = target_community
          AND commitment.ended_at IS NULL
          AND (
              work_object.object_id IS NULL
              OR work_object.deleted_at IS NOT NULL
              OR work_object.object_type <> 'work'
              OR work_object.body->>'status' NOT IN (
                  'pending',
                  'in_progress',
                  'paused',
                  'submitted'
              )
          )
    ) THEN
        RAISE EXCEPTION 'Active Work Commitment requires non-terminal Work'
            USING ERRCODE = 'check_violation';
    END IF;
END
$$;

CREATE FUNCTION project_work_commitments_validate_stage5_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM project_work_commitments_validate_stage5_community(OLD.community_id);
    ELSE
        IF TG_OP = 'UPDATE' AND OLD.community_id IS DISTINCT FROM NEW.community_id THEN
            PERFORM project_work_commitments_validate_stage5_community(OLD.community_id);
        END IF;
        PERFORM project_work_commitments_validate_stage5_community(NEW.community_id);
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER project_work_commitments_stage5_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_work_commitments
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_work_commitments_validate_stage5_row();

CREATE CONSTRAINT TRIGGER project_role_assignments_commitments_stage5_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_assignments
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_work_commitments_validate_stage5_row();

CREATE CONSTRAINT TRIGGER project_view_objects_commitments_stage5_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_view_objects
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_work_commitments_validate_stage5_row();
