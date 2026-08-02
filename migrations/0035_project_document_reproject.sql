-- Project Document inactive-generation staging and full-history reprojection.
--
-- Signed replacement events remain outside `events` until one atomic
-- activation transaction rebinds every canonical pointer. Ordinary Nostr
-- queries therefore cannot observe a partially built signer generation.

CREATE TABLE project_document_reprojects (
    operation_id               UUID        NOT NULL,
    community_id               UUID        NOT NULL REFERENCES project_document_state(community_id),
    state                      TEXT        NOT NULL,
    source_projection_pubkey   BYTEA       NOT NULL,
    source_projection_generation BIGINT    NOT NULL,
    target_projection_pubkey   BYTEA       NOT NULL,
    target_projection_generation BIGINT    NOT NULL,
    catalog_revision           BIGINT      NOT NULL,
    active_document_count      BIGINT      NOT NULL,
    document_count             BIGINT      NOT NULL,
    revision_count             BIGINT      NOT NULL,
    started_at                 TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    ready_at                   TIMESTAMPTZ,
    activated_at               TIMESTAMPTZ,

    PRIMARY KEY (community_id, operation_id),
    CONSTRAINT project_document_reprojects_state_check
        CHECK (state IN ('staging', 'ready', 'activated', 'aborted')),
    CONSTRAINT project_document_reprojects_signer_check
        CHECK (octet_length(source_projection_pubkey) = 32
               AND octet_length(target_projection_pubkey) = 32),
    CONSTRAINT project_document_reprojects_generation_check
        CHECK (source_projection_generation BETWEEN 1 AND 9007199254740990
               AND target_projection_generation = source_projection_generation + 1),
    CONSTRAINT project_document_reprojects_count_check
        CHECK (catalog_revision BETWEEN 0 AND 9007199254740991
               AND active_document_count BETWEEN 0 AND 9007199254740991
               AND document_count BETWEEN 0 AND 9007199254740991
               AND revision_count BETWEEN 0 AND 9007199254740991
               AND active_document_count <= document_count
               AND revision_count = catalog_revision),
    CONSTRAINT project_document_reprojects_time_shape_check
        CHECK ((state = 'staging' AND ready_at IS NULL AND activated_at IS NULL)
            OR (state = 'ready' AND ready_at IS NOT NULL AND activated_at IS NULL)
            OR (state = 'activated' AND ready_at IS NOT NULL AND activated_at IS NOT NULL)
            OR (state = 'aborted' AND activated_at IS NULL))
);

CREATE UNIQUE INDEX idx_project_document_reprojects_open
    ON project_document_reprojects (community_id)
    WHERE state IN ('staging', 'ready');

CREATE INDEX idx_project_document_reprojects_latest
    ON project_document_reprojects (community_id, started_at DESC, operation_id DESC);

CREATE TABLE project_document_reproject_events (
    community_id       UUID        NOT NULL,
    operation_id       UUID        NOT NULL,
    event_key           TEXT        NOT NULL,
    projection_type    TEXT        NOT NULL,
    document_id        UUID,
    document_revision  BIGINT,
    event_id            BYTEA       NOT NULL,
    pubkey              BYTEA       NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL,
    kind                INT         NOT NULL,
    tags                JSONB       NOT NULL,
    content             TEXT        NOT NULL,
    sig                 BYTEA       NOT NULL,
    d_tag               TEXT        NOT NULL,
    staged_at           TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),

    PRIMARY KEY (community_id, operation_id, event_key),
    CONSTRAINT project_document_reproject_events_operation_fk
        FOREIGN KEY (community_id, operation_id)
        REFERENCES project_document_reprojects (community_id, operation_id)
        ON DELETE CASCADE,
    CONSTRAINT project_document_reproject_events_type_check
        CHECK (projection_type IN ('revision', 'head', 'meta')),
    CONSTRAINT project_document_reproject_events_key_check
        CHECK (event_key <> ''),
    CONSTRAINT project_document_reproject_events_identity_check
        CHECK (
            (projection_type = 'revision' AND document_id IS NOT NULL
                AND document_revision BETWEEN 1 AND 9007199254740991 AND kind = 40906)
            OR (projection_type = 'head' AND document_id IS NOT NULL
                AND document_revision BETWEEN 1 AND 9007199254740991 AND kind = 40905)
            OR (projection_type = 'meta' AND document_id IS NULL
                AND document_revision IS NULL AND kind = 40907)
        ),
    CONSTRAINT project_document_reproject_events_event_check
        CHECK (octet_length(event_id) = 32 AND octet_length(pubkey) = 32
               AND octet_length(sig) = 64 AND jsonb_typeof(tags) = 'array'),
    CONSTRAINT project_document_reproject_events_event_unique
        UNIQUE (community_id, operation_id, event_id)
);

CREATE INDEX idx_project_document_reproject_events_revision
    ON project_document_reproject_events
       (community_id, operation_id, document_id, document_revision)
    WHERE projection_type = 'revision';

CREATE UNIQUE INDEX idx_project_document_reproject_events_revision_identity
    ON project_document_reproject_events
       (community_id, operation_id, document_id, document_revision)
    WHERE projection_type = 'revision';

CREATE UNIQUE INDEX idx_project_document_reproject_events_head_identity
    ON project_document_reproject_events (community_id, operation_id, document_id)
    WHERE projection_type = 'head';

CREATE UNIQUE INDEX idx_project_document_reproject_events_meta_identity
    ON project_document_reproject_events (community_id, operation_id)
    WHERE projection_type = 'meta';

-- The original guards allow only business transitions. Generation activation
-- uses a transaction-local marker and still permits pointer-only/current-state
-- changes; all canonical business fields remain protected.
CREATE OR REPLACE FUNCTION project_documents_guard_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('buzz.project_document_reproject', true) = 'on' THEN
        IF ROW(OLD.community_id, OLD.document_id, OLD.current_revision, OLD.state,
               OLD.created_at, OLD.created_by, OLD.updated_at, OLD.updated_by,
               OLD.deleted_at, OLD.current_source_change_id)
           IS DISTINCT FROM
           ROW(NEW.community_id, NEW.document_id, NEW.current_revision, NEW.state,
               NEW.created_at, NEW.created_by, NEW.updated_at, NEW.updated_by,
               NEW.deleted_at, NEW.current_source_change_id) THEN
            RAISE EXCEPTION 'Project Document reproject may only replace projection pointers'
                USING ERRCODE = 'check_violation';
        END IF;
        RETURN NEW;
    END IF;
    IF OLD.community_id IS DISTINCT FROM NEW.community_id
       OR OLD.document_id IS DISTINCT FROM NEW.document_id
       OR OLD.created_at IS DISTINCT FROM NEW.created_at
       OR OLD.created_by IS DISTINCT FROM NEW.created_by
       OR NEW.current_revision <> OLD.current_revision + 1
       OR NEW.updated_at <= OLD.updated_at THEN
        RAISE EXCEPTION 'Project Document current row may only advance by one immutable revision'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

CREATE OR REPLACE FUNCTION project_document_state_guard_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('buzz.project_document_reproject', true) = 'on' THEN
        IF ROW(OLD.community_id, OLD.schema_version, OLD.catalog_revision,
               OLD.active_document_count, OLD.last_change_id, OLD.last_actor_pubkey,
               OLD.initialized_at, OLD.updated_at)
           IS DISTINCT FROM
           ROW(NEW.community_id, NEW.schema_version, NEW.catalog_revision,
               NEW.active_document_count, NEW.last_change_id, NEW.last_actor_pubkey,
               NEW.initialized_at, NEW.updated_at)
           OR NEW.projection_generation <> OLD.projection_generation + 1 THEN
            RAISE EXCEPTION 'Project Document reproject may only advance signer generation and pointers'
                USING ERRCODE = 'check_violation';
        END IF;
        RETURN NEW;
    END IF;
    IF OLD.community_id IS DISTINCT FROM NEW.community_id
       OR OLD.schema_version IS DISTINCT FROM NEW.schema_version
       OR OLD.initialized_at IS DISTINCT FROM NEW.initialized_at
       OR OLD.projection_pubkey IS DISTINCT FROM NEW.projection_pubkey
       OR OLD.projection_generation IS DISTINCT FROM NEW.projection_generation
       OR NEW.catalog_revision <> OLD.catalog_revision + 1
       OR NEW.updated_at <= OLD.updated_at
       OR abs(NEW.active_document_count - OLD.active_document_count) > 1 THEN
        RAISE EXCEPTION 'Project Document catalog may only advance by one canonical change'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

CREATE OR REPLACE FUNCTION project_document_validate_community(target_community UUID) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    state_row project_document_state%ROWTYPE;
    actual_active_count BIGINT;
BEGIN
    SELECT * INTO state_row FROM project_document_state WHERE community_id = target_community;
    IF NOT FOUND THEN RETURN; END IF;
    SELECT count(*) INTO actual_active_count FROM project_documents
        WHERE community_id = target_community AND state = 'active';
    IF actual_active_count <> state_row.active_document_count THEN
        RAISE EXCEPTION 'Project Document active count does not match canonical Documents'
            USING ERRCODE = 'check_violation';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM events event
        WHERE event.community_id = target_community
          AND event.id = state_row.meta_projection_event_id
          AND event.kind = 40907 AND event.pubkey = state_row.projection_pubkey
          AND event.deleted_at IS NULL
    ) THEN
        RAISE EXCEPTION 'Project Document metadata pointer is missing or invalid'
            USING ERRCODE = 'check_violation';
    END IF;
    IF EXISTS (
        SELECT 1 FROM project_documents document
        LEFT JOIN project_document_revisions revision
          ON revision.community_id = document.community_id
         AND revision.document_id = document.document_id
         AND revision.document_revision = document.current_revision
        LEFT JOIN project_document_changes change
          ON change.community_id = document.community_id
         AND change.change_id = document.current_source_change_id
        WHERE document.community_id = target_community AND (
            revision.document_id IS NULL OR change.change_id IS NULL
            OR revision.catalog_revision <> change.catalog_revision
            OR revision.state <> document.state
            OR revision.actor_pubkey <> document.updated_by
            OR revision.canonical_at <> document.updated_at
            OR revision.source_change_id <> document.current_source_change_id
            OR revision.projection_event_id <> document.current_revision_event_id
            OR revision.projection_generation <> state_row.projection_generation
            OR change.document_id <> document.document_id
            OR change.document_revision <> document.current_revision
            OR change.actor_pubkey <> document.updated_by
            OR change.accepted_at <> document.updated_at
            OR (document.current_revision = 1 AND
                (document.created_at <> document.updated_at OR document.created_by <> document.updated_by))
            OR NOT EXISTS (SELECT 1 FROM events head_event
                           WHERE head_event.community_id = document.community_id
                             AND head_event.id = document.current_head_event_id
                             AND head_event.kind = 40905
                             AND head_event.pubkey = state_row.projection_pubkey
                             AND head_event.deleted_at IS NULL)
        )
    ) THEN
        RAISE EXCEPTION 'Project Document current/revision/change/projection parity failed'
            USING ERRCODE = 'check_violation';
    END IF;
    IF state_row.catalog_revision > 0 AND NOT EXISTS (
        SELECT 1 FROM project_document_changes change
        WHERE change.community_id = target_community
          AND change.change_id = state_row.last_change_id
          AND change.catalog_revision = state_row.catalog_revision
          AND change.actor_pubkey = state_row.last_actor_pubkey
          AND change.accepted_at = state_row.updated_at
    ) THEN
        RAISE EXCEPTION 'Project Document catalog source does not match its latest change'
            USING ERRCODE = 'check_violation';
    END IF;
    IF EXISTS (
        SELECT 1 FROM project_document_revisions revision
        WHERE revision.community_id = target_community AND revision.document_revision > 1
          AND NOT EXISTS (
              SELECT 1 FROM project_document_revisions previous
              WHERE previous.community_id = revision.community_id
                AND previous.document_id = revision.document_id
                AND previous.document_revision = revision.document_revision - 1
                AND previous.canonical_at < revision.canonical_at)
    ) THEN
        RAISE EXCEPTION 'Project Document revision history is not contiguous and monotonic'
            USING ERRCODE = 'check_violation';
    END IF;
END
$$;

-- Full-history materialization validation is intentionally an operator/
-- activation gate, not a per-read or per-business-write scan.
CREATE FUNCTION project_document_validate_history_projection(target_community UUID) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    state_row project_document_state%ROWTYPE;
BEGIN
    SELECT * INTO state_row FROM project_document_state WHERE community_id = target_community;
    IF NOT FOUND THEN RETURN; END IF;
    IF EXISTS (
        SELECT 1 FROM project_document_revisions revision
        WHERE revision.community_id = target_community
          AND (revision.projection_generation <> state_row.projection_generation
               OR NOT EXISTS (
                   SELECT 1 FROM events revision_event
                   WHERE revision_event.community_id = revision.community_id
                     AND revision_event.id = revision.projection_event_id
                     AND revision_event.kind = 40906
                     AND revision_event.pubkey = state_row.projection_pubkey
                     AND revision_event.deleted_at IS NULL))
    ) THEN
        RAISE EXCEPTION 'Project Document history generation/projection parity failed'
            USING ERRCODE = 'check_violation';
    END IF;
END
$$;

CREATE OR REPLACE FUNCTION project_document_validate_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    -- Activation updates every immutable materialization pointer in one
    -- transaction and performs one explicit whole-community validation after
    -- rebinding. Avoid an O(revisions^2) deferred-trigger storm.
    IF current_setting('buzz.project_document_reproject', true) = 'on' THEN
        RETURN NULL;
    END IF;
    IF TG_OP = 'DELETE' THEN
        PERFORM project_document_validate_community(OLD.community_id);
    ELSE
        IF TG_OP = 'UPDATE' AND OLD.community_id IS DISTINCT FROM NEW.community_id THEN
            PERFORM project_document_validate_community(OLD.community_id);
        END IF;
        PERFORM project_document_validate_community(NEW.community_id);
    END IF;
    RETURN NULL;
END
$$;
