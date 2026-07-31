-- Project Document v1 canonical state and immutable history.
--
-- This migration is intentionally capability-off. It creates the complete
-- trusted storage kernel, but every existing and future Community keeps the
-- feature disabled until a later, explicit bootstrap/enable operation.

ALTER TABLE communities
    ADD COLUMN project_document_enabled BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE project_document_state (
    community_id                UUID        NOT NULL,
    schema_version              SMALLINT    NOT NULL DEFAULT 1,
    catalog_revision            BIGINT      NOT NULL,
    active_document_count       BIGINT      NOT NULL,
    last_change_id              BYTEA,
    last_actor_pubkey           BYTEA,
    projection_pubkey           BYTEA       NOT NULL,
    projection_generation       BIGINT      NOT NULL,
    meta_projection_event_id    BYTEA       NOT NULL,
    initialized_at              TIMESTAMPTZ NOT NULL,
    updated_at                  TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id),
    CONSTRAINT project_document_state_community_fk
        FOREIGN KEY (community_id)
        REFERENCES communities (id)
        ON DELETE NO ACTION,
    CONSTRAINT project_document_state_schema_check
        CHECK (schema_version = 1),
    CONSTRAINT project_document_state_catalog_revision_check
        CHECK (catalog_revision BETWEEN 0 AND 9007199254740991),
    CONSTRAINT project_document_state_active_count_check
        CHECK (active_document_count BETWEEN 0 AND 9007199254740991),
    CONSTRAINT project_document_state_last_change_check
        CHECK (last_change_id IS NULL OR octet_length(last_change_id) = 32),
    CONSTRAINT project_document_state_last_actor_check
        CHECK (last_actor_pubkey IS NULL OR octet_length(last_actor_pubkey) = 32),
    CONSTRAINT project_document_state_projection_pubkey_check
        CHECK (octet_length(projection_pubkey) = 32),
    CONSTRAINT project_document_state_projection_generation_check
        CHECK (projection_generation BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_document_state_meta_event_check
        CHECK (octet_length(meta_projection_event_id) = 32),
    CONSTRAINT project_document_state_time_check
        CHECK (updated_at >= initialized_at),
    CONSTRAINT project_document_state_zero_shape_check
        CHECK (
            (
                catalog_revision = 0
                AND active_document_count = 0
                AND last_change_id IS NULL
                AND last_actor_pubkey IS NULL
            )
            OR
            (
                catalog_revision > 0
                AND last_change_id IS NOT NULL
                AND last_actor_pubkey IS NOT NULL
            )
        )
);

CREATE TABLE project_documents (
    community_id                UUID        NOT NULL,
    document_id                 UUID        NOT NULL,
    current_revision            BIGINT      NOT NULL,
    state                       TEXT        NOT NULL,
    created_at                  TIMESTAMPTZ NOT NULL,
    created_by                  BYTEA       NOT NULL,
    updated_at                  TIMESTAMPTZ NOT NULL,
    updated_by                  BYTEA       NOT NULL,
    deleted_at                  TIMESTAMPTZ,
    current_source_change_id    BYTEA       NOT NULL,
    current_head_event_id       BYTEA       NOT NULL,
    current_revision_event_id   BYTEA       NOT NULL,

    PRIMARY KEY (community_id, document_id),
    CONSTRAINT project_documents_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_document_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_documents_id_check
        CHECK (document_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_documents_revision_check
        CHECK (current_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_documents_state_check
        CHECK (state IN ('active', 'deleted')),
    CONSTRAINT project_documents_actor_check
        CHECK (octet_length(created_by) = 32 AND octet_length(updated_by) = 32),
    CONSTRAINT project_documents_time_check
        CHECK (updated_at >= created_at),
    CONSTRAINT project_documents_deleted_shape_check
        CHECK (
            (state = 'active' AND deleted_at IS NULL)
            OR
            (state = 'deleted' AND deleted_at = updated_at)
        ),
    CONSTRAINT project_documents_source_check
        CHECK (octet_length(current_source_change_id) = 32),
    CONSTRAINT project_documents_head_event_check
        CHECK (octet_length(current_head_event_id) = 32),
    CONSTRAINT project_documents_revision_event_check
        CHECK (octet_length(current_revision_event_id) = 32),
    CONSTRAINT project_documents_current_revision_unique
        UNIQUE (community_id, document_id, current_revision)
);

CREATE TABLE project_document_revisions (
    community_id              UUID        NOT NULL,
    document_id               UUID        NOT NULL,
    document_revision         BIGINT      NOT NULL,
    catalog_revision          BIGINT      NOT NULL,
    state                     TEXT        NOT NULL,
    title                     TEXT,
    summary                   TEXT,
    content_markdown          TEXT,
    actor_pubkey              BYTEA       NOT NULL,
    canonical_at              TIMESTAMPTZ NOT NULL,
    source_change_id          BYTEA       NOT NULL,
    source_event_id           BYTEA,
    projection_generation     BIGINT      NOT NULL,
    projection_event_id       BYTEA       NOT NULL,

    PRIMARY KEY (community_id, document_id, document_revision),
    CONSTRAINT project_document_revisions_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_document_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_document_revisions_id_check
        CHECK (document_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_document_revisions_revision_check
        CHECK (document_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_document_revisions_catalog_revision_check
        CHECK (catalog_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_document_revisions_catalog_unique
        UNIQUE (community_id, catalog_revision),
    CONSTRAINT project_document_revisions_state_check
        CHECK (state IN ('active', 'deleted')),
    CONSTRAINT project_document_revisions_body_shape_check
        CHECK (
            (
                state = 'active'
                AND title IS NOT NULL
                AND title <> ''
                AND content_markdown IS NOT NULL
                AND (summary IS NULL OR summary <> '')
            )
            OR
            (
                state = 'deleted'
                AND title IS NULL
                AND summary IS NULL
                AND content_markdown IS NULL
            )
        ),
    CONSTRAINT project_document_revisions_actor_check
        CHECK (octet_length(actor_pubkey) = 32),
    CONSTRAINT project_document_revisions_source_change_check
        CHECK (octet_length(source_change_id) = 32),
    CONSTRAINT project_document_revisions_source_event_check
        CHECK (source_event_id IS NULL OR octet_length(source_event_id) = 32),
    CONSTRAINT project_document_revisions_projection_generation_check
        CHECK (projection_generation BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_document_revisions_projection_event_check
        CHECK (octet_length(projection_event_id) = 32)
);

CREATE TABLE project_document_changes (
    community_id                 UUID        NOT NULL,
    change_id                   BYTEA       NOT NULL,
    source_type                 TEXT        NOT NULL,
    source_event_id             BYTEA,
    source_request_hash         BYTEA,
    source_audit_seq            BIGINT,
    idempotency_key_hash        BYTEA,
    actor_pubkey                BYTEA,
    acting_assignment_id        UUID,
    operation                   TEXT        NOT NULL,
    document_id                 UUID        NOT NULL,
    expected_document_revision  BIGINT      NOT NULL,
    document_revision           BIGINT      NOT NULL,
    catalog_revision            BIGINT      NOT NULL,
    result                      JSONB       NOT NULL,
    accepted_at                 TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, change_id),
    CONSTRAINT project_document_changes_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_document_state (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_document_changes_audit_fk
        FOREIGN KEY (community_id, source_audit_seq)
        REFERENCES audit_log (community_id, seq)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_document_changes_change_id_check
        CHECK (octet_length(change_id) = 32),
    CONSTRAINT project_document_changes_source_type_check
        CHECK (source_type IN ('nostr_event', 'nip98_request', 'operator', 'system')),
    CONSTRAINT project_document_changes_source_event_check
        CHECK (source_event_id IS NULL OR octet_length(source_event_id) = 32),
    CONSTRAINT project_document_changes_request_hash_check
        CHECK (source_request_hash IS NULL OR octet_length(source_request_hash) = 32),
    CONSTRAINT project_document_changes_idempotency_hash_check
        CHECK (idempotency_key_hash IS NULL OR octet_length(idempotency_key_hash) = 32),
    CONSTRAINT project_document_changes_actor_check
        CHECK (actor_pubkey IS NULL OR octet_length(actor_pubkey) = 32),
    CONSTRAINT project_document_changes_source_shape_check
        CHECK (
            (
                source_type = 'nostr_event'
                AND source_event_id IS NOT NULL
                AND change_id = source_event_id
                AND source_request_hash IS NULL
                AND source_audit_seq IS NULL
                AND idempotency_key_hash IS NULL
                AND actor_pubkey IS NOT NULL
            )
            OR
            (
                source_type = 'nip98_request'
                AND source_event_id IS NOT NULL
                AND source_request_hash IS NOT NULL
                AND source_audit_seq IS NULL
                AND idempotency_key_hash IS NULL
                AND actor_pubkey IS NOT NULL
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
    CONSTRAINT project_document_changes_assignment_check
        CHECK (
            acting_assignment_id IS NULL
            OR acting_assignment_id <> '00000000-0000-0000-0000-000000000000'::uuid
        ),
    CONSTRAINT project_document_changes_operation_check
        CHECK (operation IN ('create', 'update', 'delete')),
    CONSTRAINT project_document_changes_document_id_check
        CHECK (document_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_document_changes_expected_revision_check
        CHECK (expected_document_revision BETWEEN 0 AND 9007199254740991),
    CONSTRAINT project_document_changes_revision_check
        CHECK (
            document_revision BETWEEN 1 AND 9007199254740991
            AND document_revision = expected_document_revision + 1
            AND (
                (operation = 'create' AND expected_document_revision = 0)
                OR
                (operation IN ('update', 'delete') AND expected_document_revision > 0)
            )
        ),
    CONSTRAINT project_document_changes_catalog_revision_check
        CHECK (catalog_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_document_changes_catalog_unique
        UNIQUE (community_id, catalog_revision),
    CONSTRAINT project_document_changes_result_check
        CHECK (
            jsonb_typeof(result) = 'object'
            AND result ?& ARRAY[
                'schema_version', 'change_id', 'actor', 'operation', 'document_id',
                'expected_document_revision', 'document_revision', 'catalog_revision',
                'state', 'accepted_at'
            ]
            AND (result - ARRAY[
                'schema_version', 'change_id', 'actor', 'acting_assignment_id', 'operation',
                'document_id', 'expected_document_revision', 'document_revision',
                'catalog_revision', 'state', 'accepted_at'
            ]) = '{}'::jsonb
        )
);

CREATE UNIQUE INDEX idx_project_document_changes_source_event
    ON project_document_changes (community_id, source_event_id)
    WHERE source_event_id IS NOT NULL;

CREATE UNIQUE INDEX idx_project_document_changes_source_audit
    ON project_document_changes (community_id, source_audit_seq)
    WHERE source_audit_seq IS NOT NULL;

CREATE UNIQUE INDEX idx_project_document_changes_idempotency
    ON project_document_changes (community_id, idempotency_key_hash)
    WHERE idempotency_key_hash IS NOT NULL;

CREATE INDEX idx_project_document_changes_accepted
    ON project_document_changes (community_id, accepted_at, change_id);

ALTER TABLE project_documents
    ADD CONSTRAINT project_documents_current_revision_fk
        FOREIGN KEY (community_id, document_id, current_revision)
        REFERENCES project_document_revisions (community_id, document_id, document_revision)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT project_documents_current_source_fk
        FOREIGN KEY (community_id, current_source_change_id)
        REFERENCES project_document_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE project_document_revisions
    ADD CONSTRAINT project_document_revisions_source_change_fk
        FOREIGN KEY (community_id, source_change_id)
        REFERENCES project_document_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE project_document_changes
    ADD CONSTRAINT project_document_changes_revision_fk
        FOREIGN KEY (community_id, document_id, document_revision)
        REFERENCES project_document_revisions (community_id, document_id, document_revision)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX idx_project_documents_active
    ON project_documents (community_id, state, document_id);

CREATE INDEX idx_project_documents_current_revision
    ON project_documents (community_id, document_id, current_revision);

CREATE INDEX idx_project_document_revisions_history
    ON project_document_revisions (community_id, document_id, document_revision DESC);

CREATE INDEX idx_project_document_revisions_catalog
    ON project_document_revisions (community_id, catalog_revision, document_id);

CREATE INDEX idx_project_document_revisions_source_event
    ON project_document_revisions (community_id, source_event_id)
    WHERE source_event_id IS NOT NULL;

CREATE INDEX idx_project_document_revisions_projection_event
    ON project_document_revisions (community_id, projection_event_id);

CREATE INDEX idx_project_documents_head_event
    ON project_documents (community_id, current_head_event_id);

CREATE FUNCTION project_document_reject_hard_delete() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Project Document canonical history cannot be hard-deleted'
        USING ERRCODE = 'check_violation';
END
$$;

CREATE TRIGGER project_documents_no_delete
    BEFORE DELETE ON project_documents
    FOR EACH ROW EXECUTE FUNCTION project_document_reject_hard_delete();

CREATE TRIGGER project_document_revisions_no_delete
    BEFORE DELETE ON project_document_revisions
    FOR EACH ROW EXECUTE FUNCTION project_document_reject_hard_delete();

CREATE TRIGGER project_document_changes_no_delete
    BEFORE DELETE ON project_document_changes
    FOR EACH ROW EXECUTE FUNCTION project_document_reject_hard_delete();

CREATE FUNCTION project_document_revisions_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(OLD.community_id, OLD.document_id, OLD.document_revision,
           OLD.catalog_revision, OLD.state, OLD.title, OLD.summary,
           OLD.content_markdown, OLD.actor_pubkey, OLD.canonical_at,
           OLD.source_change_id, OLD.source_event_id)
       IS DISTINCT FROM
       ROW(NEW.community_id, NEW.document_id, NEW.document_revision,
           NEW.catalog_revision, NEW.state, NEW.title, NEW.summary,
           NEW.content_markdown, NEW.actor_pubkey, NEW.canonical_at,
           NEW.source_change_id, NEW.source_event_id) THEN
        RAISE EXCEPTION 'Project Document revision business fields are immutable'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER project_document_revisions_immutable
    BEFORE UPDATE ON project_document_revisions
    FOR EACH ROW EXECUTE FUNCTION project_document_revisions_append_only();

CREATE FUNCTION project_document_changes_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Project Document changes are append-only'
        USING ERRCODE = 'check_violation';
END
$$;

CREATE TRIGGER project_document_changes_immutable
    BEFORE UPDATE ON project_document_changes
    FOR EACH ROW EXECUTE FUNCTION project_document_changes_append_only();

CREATE FUNCTION project_documents_guard_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
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

CREATE TRIGGER project_documents_monotonic_update
    BEFORE UPDATE ON project_documents
    FOR EACH ROW EXECUTE FUNCTION project_documents_guard_update();

CREATE FUNCTION project_document_state_guard_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
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

CREATE TRIGGER project_document_state_monotonic_update
    BEFORE UPDATE ON project_document_state
    FOR EACH ROW EXECUTE FUNCTION project_document_state_guard_update();

CREATE FUNCTION project_document_validate_community(target_community UUID) RETURNS void
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
            OR NOT EXISTS (SELECT 1 FROM events revision_event
                           WHERE revision_event.community_id = document.community_id
                             AND revision_event.id = document.current_revision_event_id
                             AND revision_event.kind = 40906
                             AND revision_event.pubkey = state_row.projection_pubkey
                             AND revision_event.deleted_at IS NULL)
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

CREATE FUNCTION project_document_validate_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
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

CREATE CONSTRAINT TRIGGER project_document_state_validate
    AFTER INSERT OR UPDATE ON project_document_state
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_document_validate_row();

CREATE CONSTRAINT TRIGGER project_documents_validate
    AFTER INSERT OR UPDATE ON project_documents
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_document_validate_row();

CREATE CONSTRAINT TRIGGER project_document_revisions_validate
    AFTER INSERT OR UPDATE ON project_document_revisions
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_document_validate_row();

CREATE CONSTRAINT TRIGGER project_document_changes_validate
    AFTER INSERT ON project_document_changes
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_document_validate_row();
