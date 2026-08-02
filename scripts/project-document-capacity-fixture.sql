\set ON_ERROR_STOP on

CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- Synthetic-only Stage 7 capacity fixture. This deliberately bypasses the
-- business transition triggers in its disposable database so 100k rows can be
-- generated in set-based SQL. Signer rotation/parity correctness is exercised
-- separately with real signed events; this fixture measures storage growth,
-- keyset query plans, latency, and bounded client memory.

BEGIN;
SET LOCAL session_replication_role = 'replica';

INSERT INTO communities (
    id, host, project_view_schema_version, project_document_enabled
) VALUES (
    '00000000-0000-4000-8000-00000000c000', :'fixture_host', 2, TRUE
);

INSERT INTO relay_members (community_id, pubkey, role)
VALUES (
    '00000000-0000-4000-8000-00000000c000',
    :'reader_pubkey',
    'member'
);

-- The active signer is secp256k1 private key 1. Event signatures are synthetic
-- fixed-width values because this fixture does not run cryptographic parity;
-- row reconstruction and the exact production history query still run.
INSERT INTO events (
    community_id, id, pubkey, created_at, kind, tags, content, sig,
    received_at, channel_id, d_tag
) VALUES (
    '00000000-0000-4000-8000-00000000c000',
    digest('stage7-capacity-meta', 'sha256'),
    decode(:'relay_pubkey', 'hex'),
    '2026-01-01 00:00:00+00',
    40907,
    '[]'::jsonb,
    '{}',
    decode(repeat('00', 64), 'hex'),
    clock_timestamp(),
    NULL,
    'buzz:project-document:00000000-0000-4000-8000-00000000c000:meta'
);

INSERT INTO project_document_state (
    community_id, schema_version, catalog_revision, active_document_count,
    last_change_id, last_actor_pubkey, projection_pubkey,
    projection_generation, meta_projection_event_id, initialized_at, updated_at
) VALUES (
    '00000000-0000-4000-8000-00000000c000',
    1,
    (:'hot_revisions')::bigint + (:'wide_documents')::bigint,
    (:'wide_documents')::bigint + 1,
    digest('stage7-capacity-source:wide:' || :'wide_documents', 'sha256'),
    decode(:'reader_pubkey', 'hex'),
    decode(:'relay_pubkey', 'hex'),
    1,
    digest('stage7-capacity-meta', 'sha256'),
    '2026-01-01 00:00:00+00',
    '2026-01-01 00:00:00+00'::timestamptz
        + ((:'hot_revisions')::bigint + (:'wide_documents')::bigint) * interval '1 second'
);

WITH hot AS (
    SELECT
        n,
        '00000000-0000-4000-8000-00000000c001'::uuid AS document_id,
        n::bigint AS document_revision,
        n::bigint AS catalog_revision,
        digest('stage7-capacity-source:hot:' || n, 'sha256') AS change_id,
        '2026-01-01 00:00:00+00'::timestamptz + n * interval '1 second' AS accepted_at
    FROM generate_series(1, (:'hot_revisions')::integer) n
), wide AS (
    SELECT
        n,
        ('10000000-0000-4000-8000-' || lpad(n::text, 12, '0'))::uuid AS document_id,
        1::bigint AS document_revision,
        (:'hot_revisions')::bigint + n AS catalog_revision,
        digest('stage7-capacity-source:wide:' || n, 'sha256') AS change_id,
        '2026-01-01 00:00:00+00'::timestamptz
            + ((:'hot_revisions')::bigint + n) * interval '1 second' AS accepted_at
    FROM generate_series(1, (:'wide_documents')::integer) n
), revisions AS (
    SELECT * FROM hot UNION ALL SELECT * FROM wide
)
INSERT INTO project_document_changes (
    community_id, change_id, source_type, source_event_id, actor_pubkey,
    operation, document_id, expected_document_revision, document_revision,
    catalog_revision, result, accepted_at
)
SELECT
    '00000000-0000-4000-8000-00000000c000',
    change_id,
    'nostr_event',
    change_id,
    decode(:'reader_pubkey', 'hex'),
    CASE WHEN document_revision = 1 THEN 'create' ELSE 'update' END,
    document_id,
    document_revision - 1,
    document_revision,
    catalog_revision,
    jsonb_build_object(
        'schema_version', 1,
        'change_id', encode(change_id, 'hex'),
        'actor', :'reader_pubkey',
        'operation', CASE WHEN document_revision = 1 THEN 'create' ELSE 'update' END,
        'document_id', document_id::text,
        'expected_document_revision', document_revision - 1,
        'document_revision', document_revision,
        'catalog_revision', catalog_revision,
        'state', 'active',
        'accepted_at', accepted_at
    ),
    accepted_at
FROM revisions;

WITH hot AS (
    SELECT
        n,
        '00000000-0000-4000-8000-00000000c001'::uuid AS document_id,
        n::bigint AS document_revision,
        n::bigint AS catalog_revision,
        digest('stage7-capacity-source:hot:' || n, 'sha256') AS change_id,
        digest('stage7-capacity-revision:hot:' || n, 'sha256') AS event_id,
        '2026-01-01 00:00:00+00'::timestamptz + n * interval '1 second' AS canonical_at
    FROM generate_series(1, (:'hot_revisions')::integer) n
), wide AS (
    SELECT
        n,
        ('10000000-0000-4000-8000-' || lpad(n::text, 12, '0'))::uuid AS document_id,
        1::bigint AS document_revision,
        (:'hot_revisions')::bigint + n AS catalog_revision,
        digest('stage7-capacity-source:wide:' || n, 'sha256') AS change_id,
        digest('stage7-capacity-revision:wide:' || n, 'sha256') AS event_id,
        '2026-01-01 00:00:00+00'::timestamptz
            + ((:'hot_revisions')::bigint + n) * interval '1 second' AS canonical_at
    FROM generate_series(1, (:'wide_documents')::integer) n
), revisions AS (
    SELECT * FROM hot UNION ALL SELECT * FROM wide
)
INSERT INTO project_document_revisions (
    community_id, document_id, document_revision, catalog_revision, state,
    title, summary, content_markdown, actor_pubkey, canonical_at,
    source_change_id, source_event_id, projection_generation, projection_event_id
)
SELECT
    '00000000-0000-4000-8000-00000000c000',
    document_id,
    document_revision,
    catalog_revision,
    'active',
    'Synthetic capacity revision ' || catalog_revision,
    'Stage 7 synthetic-only fixture',
    repeat('x', 256 + (catalog_revision % 769)::integer),
    decode(:'reader_pubkey', 'hex'),
    canonical_at,
    change_id,
    change_id,
    1,
    event_id
FROM revisions;

WITH hot AS (
    SELECT
        n,
        digest('stage7-capacity-revision:hot:' || n, 'sha256') AS event_id,
        '2026-01-01 00:00:00+00'::timestamptz + n * interval '1 second' AS created_at,
        repeat('x', 256 + (n % 769)::integer) AS content,
        'buzz:project-document:00000000-0000-4000-8000-00000000c000:'
            || '00000000-0000-4000-8000-00000000c001:' || n AS d_tag
    FROM generate_series(1, (:'hot_revisions')::integer) n
), wide AS (
    SELECT
        n,
        digest('stage7-capacity-revision:wide:' || n, 'sha256') AS event_id,
        '2026-01-01 00:00:00+00'::timestamptz
            + ((:'hot_revisions')::bigint + n) * interval '1 second' AS created_at,
        repeat('x', 256 + (((:'hot_revisions')::bigint + n) % 769)::integer) AS content,
        'buzz:project-document:00000000-0000-4000-8000-00000000c000:'
            || ('10000000-0000-4000-8000-' || lpad(n::text, 12, '0')) || ':1' AS d_tag
    FROM generate_series(1, (:'wide_documents')::integer) n
), events_to_insert AS (
    SELECT * FROM hot UNION ALL SELECT * FROM wide
)
INSERT INTO events (
    community_id, id, pubkey, created_at, kind, tags, content, sig,
    received_at, channel_id, d_tag
)
SELECT
    '00000000-0000-4000-8000-00000000c000',
    event_id,
    decode(:'relay_pubkey', 'hex'),
    created_at,
    40906,
    '[]'::jsonb,
    content,
    decode(repeat('00', 64), 'hex'),
    clock_timestamp(),
    NULL,
    d_tag
FROM events_to_insert;

INSERT INTO project_documents (
    community_id, document_id, current_revision, state, created_at, created_by,
    updated_at, updated_by, current_source_change_id,
    current_head_event_id, current_revision_event_id
) VALUES (
    '00000000-0000-4000-8000-00000000c000',
    '00000000-0000-4000-8000-00000000c001',
    (:'hot_revisions')::bigint,
    'active',
    '2026-01-01 00:00:01+00',
    decode(:'reader_pubkey', 'hex'),
    '2026-01-01 00:00:00+00'::timestamptz + (:'hot_revisions')::bigint * interval '1 second',
    decode(:'reader_pubkey', 'hex'),
    digest('stage7-capacity-source:hot:' || :'hot_revisions', 'sha256'),
    digest('stage7-capacity-head:hot', 'sha256'),
    digest('stage7-capacity-revision:hot:' || :'hot_revisions', 'sha256')
);

INSERT INTO project_documents (
    community_id, document_id, current_revision, state, created_at, created_by,
    updated_at, updated_by, current_source_change_id,
    current_head_event_id, current_revision_event_id
)
SELECT
    '00000000-0000-4000-8000-00000000c000',
    ('10000000-0000-4000-8000-' || lpad(n::text, 12, '0'))::uuid,
    1,
    'active',
    '2026-01-01 00:00:00+00'::timestamptz
        + ((:'hot_revisions')::bigint + n) * interval '1 second',
    decode(:'reader_pubkey', 'hex'),
    '2026-01-01 00:00:00+00'::timestamptz
        + ((:'hot_revisions')::bigint + n) * interval '1 second',
    decode(:'reader_pubkey', 'hex'),
    digest('stage7-capacity-source:wide:' || n, 'sha256'),
    digest('stage7-capacity-head:wide:' || n, 'sha256'),
    digest('stage7-capacity-revision:wide:' || n, 'sha256')
FROM generate_series(1, (:'wide_documents')::integer) n;

INSERT INTO events (
    community_id, id, pubkey, created_at, kind, tags, content, sig,
    received_at, channel_id, d_tag
) VALUES (
    '00000000-0000-4000-8000-00000000c000',
    digest('stage7-capacity-head:hot', 'sha256'),
    decode(:'relay_pubkey', 'hex'),
    '2026-01-01 00:00:00+00'::timestamptz + (:'hot_revisions')::bigint * interval '1 second',
    40905,
    '[]'::jsonb,
    '{}',
    decode(repeat('00', 64), 'hex'),
    clock_timestamp(),
    NULL,
    'buzz:project-document:00000000-0000-4000-8000-00000000c000:'
        || '00000000-0000-4000-8000-00000000c001:head'
);

INSERT INTO events (
    community_id, id, pubkey, created_at, kind, tags, content, sig,
    received_at, channel_id, d_tag
)
SELECT
    '00000000-0000-4000-8000-00000000c000',
    digest('stage7-capacity-head:wide:' || n, 'sha256'),
    decode(:'relay_pubkey', 'hex'),
    '2026-01-01 00:00:00+00'::timestamptz
        + ((:'hot_revisions')::bigint + n) * interval '1 second',
    40905,
    '[]'::jsonb,
    '{}',
    decode(repeat('00', 64), 'hex'),
    clock_timestamp(),
    NULL,
    'buzz:project-document:00000000-0000-4000-8000-00000000c000:'
        || ('10000000-0000-4000-8000-' || lpad(n::text, 12, '0')) || ':head'
FROM generate_series(1, (:'wide_documents')::integer) n;

COMMIT;

ANALYZE project_document_revisions;
ANALYZE project_documents;
ANALYZE events;
