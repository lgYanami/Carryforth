-- Project Context Edge v1 canonical storage.
--
-- The migration is deliberately capability-off.  A signed revision-zero
-- reset projection must be committed by the explicit bootstrap transaction
-- before an operator may enable any Community.

ALTER TABLE communities
    ADD COLUMN project_context_edge_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD CONSTRAINT communities_project_context_edge_gate_check
        CHECK (
            NOT project_context_edge_enabled
            OR (
                project_view_schema_version = 3
                AND project_view_enabled
                AND project_document_enabled
            )
        );

CREATE TABLE project_context_edge_state (
    community_id             UUID        NOT NULL,
    schema_version           SMALLINT    NOT NULL DEFAULT 1,
    context_revision         BIGINT      NOT NULL,
    active_edge_count        BIGINT      NOT NULL,
    bound_document_count     BIGINT      NOT NULL,
    last_change_id           BYTEA,
    last_actor_pubkey        BYTEA,
    projection_pubkey        BYTEA       NOT NULL,
    projection_generation    BIGINT      NOT NULL,
    meta_projection_event_id BYTEA       NOT NULL,
    initialized_at           TIMESTAMPTZ NOT NULL,
    updated_at               TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id),
    CONSTRAINT project_context_edge_state_community_fk
        FOREIGN KEY (community_id) REFERENCES communities (id) ON DELETE NO ACTION,
    CONSTRAINT project_context_edge_state_schema_check CHECK (schema_version = 1),
    CONSTRAINT project_context_edge_state_revision_check
        CHECK (context_revision BETWEEN 0 AND 9007199254740991),
    CONSTRAINT project_context_edge_state_count_check
        CHECK (
            active_edge_count BETWEEN 0 AND 9007199254740991
            AND bound_document_count BETWEEN 0 AND 9007199254740991
            AND active_edge_count <= bound_document_count
            AND ((active_edge_count = 0) = (bound_document_count = 0))
        ),
    CONSTRAINT project_context_edge_state_last_change_check
        CHECK (last_change_id IS NULL OR octet_length(last_change_id) = 32),
    CONSTRAINT project_context_edge_state_last_actor_check
        CHECK (last_actor_pubkey IS NULL OR octet_length(last_actor_pubkey) = 32),
    CONSTRAINT project_context_edge_state_projection_pubkey_check
        CHECK (octet_length(projection_pubkey) = 32),
    CONSTRAINT project_context_edge_state_projection_generation_check
        CHECK (projection_generation BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_context_edge_state_meta_event_check
        CHECK (octet_length(meta_projection_event_id) = 32),
    CONSTRAINT project_context_edge_state_time_check CHECK (updated_at >= initialized_at),
    CONSTRAINT project_context_edge_state_zero_shape_check
        CHECK (
            (
                context_revision = 0
                AND active_edge_count = 0
                AND bound_document_count = 0
                AND last_change_id IS NULL
                AND last_actor_pubkey IS NULL
                AND updated_at = initialized_at
            )
            OR (
                context_revision > 0
                AND last_change_id IS NOT NULL
                AND last_actor_pubkey IS NOT NULL
            )
        )
);

CREATE TABLE project_context_edges (
    community_id             UUID        NOT NULL,
    edge_key                 BYTEA       NOT NULL,
    state                    TEXT        NOT NULL,
    canonical_coordinates    JSONB       NOT NULL,
    last_context_revision    BIGINT      NOT NULL,
    current_source_change_id BYTEA       NOT NULL,
    updated_at               TIMESTAMPTZ NOT NULL,
    updated_by               BYTEA       NOT NULL,

    PRIMARY KEY (community_id, edge_key),
    CONSTRAINT project_context_edges_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_context_edge_state (community_id)
        ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_context_edges_exact_set_unique
        UNIQUE (community_id, canonical_coordinates),
    CONSTRAINT project_context_edges_key_check CHECK (octet_length(edge_key) = 32),
    CONSTRAINT project_context_edges_state_check CHECK (state IN ('active', 'deleted')),
    CONSTRAINT project_context_edges_coordinates_check
        CHECK (
            jsonb_typeof(canonical_coordinates) = 'array'
            AND jsonb_array_length(canonical_coordinates) >= 2
        ),
    CONSTRAINT project_context_edges_revision_check
        CHECK (last_context_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_context_edges_source_check
        CHECK (octet_length(current_source_change_id) = 32),
    CONSTRAINT project_context_edges_actor_check CHECK (octet_length(updated_by) = 32)
);

CREATE TABLE project_context_edge_coordinates (
    community_id       UUID    NOT NULL,
    edge_key           BYTEA   NOT NULL,
    ordinal            INTEGER NOT NULL,
    coordinate_type    TEXT    NOT NULL,
    coordinate_subtype TEXT,
    coordinate_id      UUID    NOT NULL,
    canonical_key      TEXT    NOT NULL,

    PRIMARY KEY (community_id, edge_key, ordinal),
    CONSTRAINT project_context_edge_coordinates_edge_fk
        FOREIGN KEY (community_id, edge_key)
        REFERENCES project_context_edges (community_id, edge_key)
        ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_context_edge_coordinates_identity_unique
        UNIQUE (community_id, edge_key, canonical_key),
    CONSTRAINT project_context_edge_coordinates_ordinal_check CHECK (ordinal >= 0),
    CONSTRAINT project_context_edge_coordinates_id_check
        CHECK (coordinate_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_context_edge_coordinates_shape_check
        CHECK (
            (
                coordinate_type = 'project_view_object'
                AND coordinate_subtype IN (
                    'project_profile', 'goal', 'role', 'plan', 'stage',
                    'requirement', 'issue', 'work', 'resource'
                )
                AND canonical_key =
                    'pv:' || community_id::text || ':' || coordinate_subtype || ':' || coordinate_id::text
                AND (
                    (coordinate_subtype = 'project_profile' AND coordinate_id = community_id)
                    OR (coordinate_subtype <> 'project_profile' AND coordinate_id <> community_id)
                )
            )
            OR (
                coordinate_type = 'document'
                AND coordinate_subtype IS NULL
                AND canonical_key =
                    'document:' || community_id::text || ':' || coordinate_id::text
            )
        )
);

CREATE TABLE project_context_document_bindings (
    community_id               UUID        NOT NULL,
    context_document_id        UUID        NOT NULL,
    edge_key                   BYTEA       NOT NULL,
    state                      TEXT        NOT NULL,
    binding_context_revision   BIGINT      NOT NULL,
    current_source_change_id   BYTEA       NOT NULL,
    current_projection_event_id BYTEA      NOT NULL,
    updated_at                 TIMESTAMPTZ NOT NULL,
    updated_by                 BYTEA       NOT NULL,

    PRIMARY KEY (community_id, context_document_id),
    CONSTRAINT project_context_document_bindings_edge_fk
        FOREIGN KEY (community_id, edge_key)
        REFERENCES project_context_edges (community_id, edge_key)
        ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_context_document_bindings_document_fk
        FOREIGN KEY (community_id, context_document_id)
        REFERENCES project_documents (community_id, document_id)
        ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_context_document_bindings_id_check
        CHECK (context_document_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_context_document_bindings_key_check CHECK (octet_length(edge_key) = 32),
    CONSTRAINT project_context_document_bindings_state_check CHECK (state IN ('active', 'deleted')),
    CONSTRAINT project_context_document_bindings_revision_check
        CHECK (binding_context_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_context_document_bindings_source_check
        CHECK (octet_length(current_source_change_id) = 32),
    CONSTRAINT project_context_document_bindings_projection_check
        CHECK (octet_length(current_projection_event_id) = 32),
    CONSTRAINT project_context_document_bindings_actor_check CHECK (octet_length(updated_by) = 32)
);

CREATE TABLE project_context_edge_changes (
    community_id             UUID        NOT NULL,
    change_id                BYTEA       NOT NULL,
    source_type              TEXT        NOT NULL,
    source_event_id          BYTEA       NOT NULL,
    actor_pubkey             BYTEA       NOT NULL,
    acting_assignment_id     UUID,
    operation                TEXT        NOT NULL,
    expected_context_revision BIGINT     NOT NULL,
    context_revision         BIGINT      NOT NULL,
    edge_key                 BYTEA       NOT NULL,
    edge_state               TEXT        NOT NULL,
    edge_document_count      BIGINT      NOT NULL,
    context_document_id      UUID        NOT NULL,
    canonical_coordinates    JSONB       NOT NULL,
    result                   JSONB       NOT NULL,
    accepted_at              TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, change_id),
    CONSTRAINT project_context_edge_changes_state_fk
        FOREIGN KEY (community_id)
        REFERENCES project_context_edge_state (community_id)
        ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_context_edge_changes_revision_unique
        UNIQUE (community_id, context_revision),
    CONSTRAINT project_context_edge_changes_source_unique
        UNIQUE (community_id, source_event_id),
    CONSTRAINT project_context_edge_changes_source_shape_check
        CHECK (
            source_type = 'nostr_event'
            AND change_id = source_event_id
            AND octet_length(change_id) = 32
            AND octet_length(actor_pubkey) = 32
        ),
    CONSTRAINT project_context_edge_changes_assignment_check
        CHECK (
            acting_assignment_id IS NULL
            OR acting_assignment_id <> '00000000-0000-0000-0000-000000000000'::uuid
        ),
    CONSTRAINT project_context_edge_changes_operation_check CHECK (operation IN ('attach', 'detach')),
    CONSTRAINT project_context_edge_changes_revision_check
        CHECK (
            expected_context_revision BETWEEN 0 AND 9007199254740990
            AND context_revision = expected_context_revision + 1
        ),
    CONSTRAINT project_context_edge_changes_edge_check
        CHECK (octet_length(edge_key) = 32 AND edge_state IN ('active', 'deleted')),
    CONSTRAINT project_context_edge_changes_count_check
        CHECK (
            edge_document_count BETWEEN 0 AND 9007199254740991
            AND (
                (edge_state = 'active' AND edge_document_count > 0)
                OR (edge_state = 'deleted' AND edge_document_count = 0)
            )
        ),
    CONSTRAINT project_context_edge_changes_document_check
        CHECK (context_document_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_context_edge_changes_coordinates_check
        CHECK (
            jsonb_typeof(canonical_coordinates) = 'array'
            AND jsonb_array_length(canonical_coordinates) >= 2
        ),
    CONSTRAINT project_context_edge_changes_result_check
        CHECK (
            jsonb_typeof(result) = 'object'
            AND result ?& ARRAY[
                'schema_version', 'change_id', 'actor', 'operation',
                'expected_context_revision', 'context_revision', 'edge_key',
                'edge_state', 'edge_document_count', 'context_document_id', 'accepted_at'
            ]
            AND (result - ARRAY[
                'schema_version', 'change_id', 'actor', 'acting_assignment_id', 'operation',
                'expected_context_revision', 'context_revision', 'edge_key',
                'edge_state', 'edge_document_count', 'context_document_id', 'accepted_at'
            ]) = '{}'::jsonb
        )
);

ALTER TABLE project_context_edge_state
    ADD CONSTRAINT project_context_edge_state_last_change_fk
        FOREIGN KEY (community_id, last_change_id)
        REFERENCES project_context_edge_changes (community_id, change_id)
        ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE project_context_edges
    ADD CONSTRAINT project_context_edges_source_fk
        FOREIGN KEY (community_id, current_source_change_id)
        REFERENCES project_context_edge_changes (community_id, change_id)
        ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE project_context_document_bindings
    ADD CONSTRAINT project_context_document_bindings_source_fk
        FOREIGN KEY (community_id, current_source_change_id)
        REFERENCES project_context_edge_changes (community_id, change_id)
        ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX idx_project_context_edges_active
    ON project_context_edges (community_id, state, edge_key);
CREATE INDEX idx_project_context_edges_updated
    ON project_context_edges (community_id, updated_at, edge_key);
CREATE INDEX idx_project_context_edge_coordinates_lookup
    ON project_context_edge_coordinates (
        community_id, coordinate_type, coordinate_subtype, coordinate_id, edge_key
    );
CREATE INDEX idx_project_context_edge_coordinates_edge
    ON project_context_edge_coordinates (community_id, edge_key, ordinal);
CREATE INDEX idx_project_context_bindings_edge
    ON project_context_document_bindings (community_id, edge_key, state, context_document_id);
CREATE INDEX idx_project_context_bindings_active_document
    ON project_context_document_bindings (community_id, context_document_id)
    WHERE state = 'active';
CREATE INDEX idx_project_context_bindings_projection
    ON project_context_document_bindings (community_id, current_projection_event_id);
CREATE INDEX idx_project_context_changes_accepted
    ON project_context_edge_changes (community_id, accepted_at, change_id);

-- Reconstruct the exact v1 SHA-256 edge identity from normalized rows.  UUID
-- bytes and the u32 coordinate count use the same big-endian encoding as the
-- pure Rust domain crate.
CREATE FUNCTION project_context_compute_edge_key(target_community UUID, target_edge BYTEA)
RETURNS BYTEA
LANGUAGE plpgsql STABLE AS $$
DECLARE
    coordinate_count INTEGER;
    coordinate_row RECORD;
    payload BYTEA;
BEGIN
    SELECT count(*)::integer INTO coordinate_count
    FROM project_context_edge_coordinates
    WHERE community_id = target_community AND edge_key = target_edge;

    payload := convert_to('buzz-project-context-edge-v1', 'UTF8')
        || decode('00', 'hex')
        || uuid_send(target_community)
        || int4send(coordinate_count);

    FOR coordinate_row IN
        SELECT coordinate_type, coordinate_subtype, coordinate_id
        FROM project_context_edge_coordinates
        WHERE community_id = target_community AND edge_key = target_edge
        ORDER BY ordinal
    LOOP
        IF coordinate_row.coordinate_type = 'project_view_object' THEN
            payload := payload || decode('00', 'hex') || decode(
                CASE coordinate_row.coordinate_subtype
                    WHEN 'project_profile' THEN '00'
                    WHEN 'goal' THEN '01'
                    WHEN 'role' THEN '02'
                    WHEN 'plan' THEN '03'
                    WHEN 'stage' THEN '04'
                    WHEN 'requirement' THEN '05'
                    WHEN 'issue' THEN '06'
                    WHEN 'work' THEN '07'
                    WHEN 'resource' THEN '08'
                END,
                'hex'
            ) || uuid_send(coordinate_row.coordinate_id);
        ELSE
            payload := payload || decode('01', 'hex') || uuid_send(coordinate_row.coordinate_id);
        END IF;
    END LOOP;
    RETURN digest(payload, 'sha256');
END
$$;

CREATE FUNCTION project_context_reject_hard_delete() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Project Context canonical state cannot be hard-deleted'
        USING ERRCODE = 'check_violation';
END
$$;

CREATE TRIGGER project_context_edge_state_no_delete
    BEFORE DELETE ON project_context_edge_state
    FOR EACH ROW EXECUTE FUNCTION project_context_reject_hard_delete();
CREATE TRIGGER project_context_edges_no_delete
    BEFORE DELETE ON project_context_edges
    FOR EACH ROW EXECUTE FUNCTION project_context_reject_hard_delete();
CREATE TRIGGER project_context_edge_coordinates_no_delete
    BEFORE DELETE ON project_context_edge_coordinates
    FOR EACH ROW EXECUTE FUNCTION project_context_reject_hard_delete();
CREATE TRIGGER project_context_bindings_no_delete
    BEFORE DELETE ON project_context_document_bindings
    FOR EACH ROW EXECUTE FUNCTION project_context_reject_hard_delete();
CREATE TRIGGER project_context_changes_no_delete
    BEFORE DELETE ON project_context_edge_changes
    FOR EACH ROW EXECUTE FUNCTION project_context_reject_hard_delete();

CREATE FUNCTION project_context_edge_coordinates_immutable() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Project Context edge coordinates are immutable'
        USING ERRCODE = 'check_violation';
END
$$;
CREATE TRIGGER project_context_edge_coordinates_no_update
    BEFORE UPDATE ON project_context_edge_coordinates
    FOR EACH ROW EXECUTE FUNCTION project_context_edge_coordinates_immutable();

CREATE FUNCTION project_context_changes_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Project Context changes are append-only'
        USING ERRCODE = 'check_violation';
END
$$;
CREATE TRIGGER project_context_changes_immutable
    BEFORE UPDATE ON project_context_edge_changes
    FOR EACH ROW EXECUTE FUNCTION project_context_changes_append_only();

CREATE FUNCTION project_context_edge_state_guard_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('buzz.project_context_reproject', true) = 'on' THEN
        IF ROW(OLD.community_id, OLD.schema_version, OLD.context_revision,
               OLD.active_edge_count, OLD.bound_document_count, OLD.last_change_id,
               OLD.last_actor_pubkey, OLD.initialized_at, OLD.updated_at)
           IS DISTINCT FROM
           ROW(NEW.community_id, NEW.schema_version, NEW.context_revision,
               NEW.active_edge_count, NEW.bound_document_count, NEW.last_change_id,
               NEW.last_actor_pubkey, NEW.initialized_at, NEW.updated_at)
           OR NEW.projection_generation <> OLD.projection_generation + 1 THEN
            RAISE EXCEPTION 'Project Context reproject may only replace signer generation and metadata pointer'
                USING ERRCODE = 'check_violation';
        END IF;
        RETURN NEW;
    END IF;
    IF OLD.community_id IS DISTINCT FROM NEW.community_id
       OR OLD.schema_version IS DISTINCT FROM NEW.schema_version
       OR OLD.projection_pubkey IS DISTINCT FROM NEW.projection_pubkey
       OR OLD.projection_generation IS DISTINCT FROM NEW.projection_generation
       OR OLD.initialized_at IS DISTINCT FROM NEW.initialized_at
       OR NEW.context_revision <> OLD.context_revision + 1
       OR NEW.updated_at <= OLD.updated_at
       OR abs(NEW.active_edge_count - OLD.active_edge_count) > 1
       OR abs(NEW.bound_document_count - OLD.bound_document_count) <> 1 THEN
        RAISE EXCEPTION 'Project Context catalog may only advance by one canonical change'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER project_context_edge_state_monotonic_update
    BEFORE UPDATE ON project_context_edge_state
    FOR EACH ROW EXECUTE FUNCTION project_context_edge_state_guard_update();

CREATE FUNCTION project_context_edges_guard_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.community_id IS DISTINCT FROM NEW.community_id
       OR OLD.edge_key IS DISTINCT FROM NEW.edge_key
       OR OLD.canonical_coordinates IS DISTINCT FROM NEW.canonical_coordinates
       OR NEW.last_context_revision <= OLD.last_context_revision
       OR NEW.updated_at <= OLD.updated_at THEN
        RAISE EXCEPTION 'Project Context edge identity is immutable and observations are monotonic'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER project_context_edges_monotonic_update
    BEFORE UPDATE ON project_context_edges
    FOR EACH ROW EXECUTE FUNCTION project_context_edges_guard_update();

CREATE FUNCTION project_context_bindings_guard_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('buzz.project_context_reproject', true) = 'on' THEN
        IF ROW(OLD.community_id, OLD.context_document_id, OLD.edge_key, OLD.state,
               OLD.binding_context_revision, OLD.current_source_change_id,
               OLD.updated_at, OLD.updated_by)
           IS DISTINCT FROM
           ROW(NEW.community_id, NEW.context_document_id, NEW.edge_key, NEW.state,
               NEW.binding_context_revision, NEW.current_source_change_id,
               NEW.updated_at, NEW.updated_by) THEN
            RAISE EXCEPTION 'Project Context reproject may only replace binding projection pointers'
                USING ERRCODE = 'check_violation';
        END IF;
        RETURN NEW;
    END IF;
    IF OLD.community_id IS DISTINCT FROM NEW.community_id
       OR OLD.context_document_id IS DISTINCT FROM NEW.context_document_id
       OR NEW.binding_context_revision <= OLD.binding_context_revision
       OR NEW.updated_at <= OLD.updated_at
       OR OLD.state = NEW.state
       OR (OLD.state = 'active' AND OLD.edge_key IS DISTINCT FROM NEW.edge_key) THEN
        RAISE EXCEPTION 'Project Context binding may only detach or reuse a deleted transport row'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER project_context_bindings_monotonic_update
    BEFORE UPDATE ON project_context_document_bindings
    FOR EACH ROW EXECUTE FUNCTION project_context_bindings_guard_update();

-- Preserve the Project Document reprojection bypass while adding the Context
-- Document deletion guard to the ordinary business path.
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
    IF OLD.state = 'active' AND NEW.state = 'deleted' AND EXISTS (
        SELECT 1 FROM project_context_document_bindings binding
        WHERE binding.community_id = OLD.community_id
          AND binding.context_document_id = OLD.document_id
          AND binding.state = 'active'
    ) THEN
        RAISE EXCEPTION 'active Context Document must be detached before deletion'
            USING ERRCODE = 'check_violation';
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

CREATE FUNCTION project_context_validate_community(target_community UUID) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    state_row project_context_edge_state%ROWTYPE;
    actual_active_edges BIGINT;
    actual_active_bindings BIGINT;
    normalized_coordinates JSONB;
    edge_row RECORD;
    meta_content JSONB;
    expected_meta JSONB;
BEGIN
    SELECT * INTO state_row
    FROM project_context_edge_state
    WHERE community_id = target_community;
    IF NOT FOUND THEN RETURN; END IF;

    SELECT count(*) INTO actual_active_edges
    FROM project_context_edges
    WHERE community_id = target_community AND state = 'active';
    SELECT count(*) INTO actual_active_bindings
    FROM project_context_document_bindings
    WHERE community_id = target_community AND state = 'active';
    IF actual_active_edges <> state_row.active_edge_count
       OR actual_active_bindings <> state_row.bound_document_count THEN
        RAISE EXCEPTION 'Project Context counts do not match canonical rows'
            USING ERRCODE = 'check_violation';
    END IF;

    FOR edge_row IN
        SELECT * FROM project_context_edges WHERE community_id = target_community
    LOOP
        IF (SELECT count(*) FROM project_context_edge_coordinates coordinate
            WHERE coordinate.community_id = target_community
              AND coordinate.edge_key = edge_row.edge_key) < 2 THEN
            RAISE EXCEPTION 'Project Context edge has fewer than two coordinates'
                USING ERRCODE = 'check_violation';
        END IF;
        IF EXISTS (
            SELECT 1 FROM (
                SELECT ordinal,
                       row_number() OVER (
                           ORDER BY
                               CASE coordinate_type WHEN 'project_view_object' THEN 0 ELSE 1 END,
                               CASE coordinate_subtype
                                   WHEN 'project_profile' THEN 0 WHEN 'goal' THEN 1
                                   WHEN 'role' THEN 2 WHEN 'plan' THEN 3
                                   WHEN 'stage' THEN 4 WHEN 'requirement' THEN 5
                                   WHEN 'issue' THEN 6 WHEN 'work' THEN 7
                                   WHEN 'resource' THEN 8 ELSE 0
                               END,
                               coordinate_id
                       ) - 1 AS expected_ordinal
                FROM project_context_edge_coordinates
                WHERE community_id = target_community AND edge_key = edge_row.edge_key
            ) ordered_coordinates
            WHERE ordinal <> expected_ordinal
        ) THEN
            RAISE EXCEPTION 'Project Context coordinates are not contiguous canonical order'
                USING ERRCODE = 'check_violation';
        END IF;
        SELECT jsonb_agg(
            CASE coordinate_type
                WHEN 'project_view_object' THEN jsonb_build_object(
                    'coordinate_type', 'project_view_object',
                    'object_type', coordinate_subtype,
                    'object_id', coordinate_id
                )
                ELSE jsonb_build_object(
                    'coordinate_type', 'document',
                    'document_id', coordinate_id
                )
            END ORDER BY ordinal
        ) INTO normalized_coordinates
        FROM project_context_edge_coordinates
        WHERE community_id = target_community AND edge_key = edge_row.edge_key;
        IF normalized_coordinates IS DISTINCT FROM edge_row.canonical_coordinates
           OR project_context_compute_edge_key(target_community, edge_row.edge_key)
                IS DISTINCT FROM edge_row.edge_key THEN
            RAISE EXCEPTION 'Project Context JSON, normalized coordinates, and edge key disagree'
                USING ERRCODE = 'check_violation';
        END IF;
        IF EXISTS (
            SELECT 1 FROM project_context_edge_coordinates coordinate
            WHERE coordinate.community_id = target_community
              AND coordinate.edge_key = edge_row.edge_key
              AND (
                  (
                      coordinate.coordinate_type = 'project_view_object'
                      AND NOT EXISTS (
                          SELECT 1 FROM project_view_objects object
                          WHERE object.community_id = coordinate.community_id
                            AND object.object_id = coordinate.coordinate_id
                            AND object.object_type = coordinate.coordinate_subtype
                      )
                  )
                  OR (
                      coordinate.coordinate_type = 'document'
                      AND NOT EXISTS (
                          SELECT 1 FROM project_documents document
                          WHERE document.community_id = coordinate.community_id
                            AND document.document_id = coordinate.coordinate_id
                      )
                  )
              )
        ) THEN
            RAISE EXCEPTION 'Project Context coordinate identity or type is invalid'
                USING ERRCODE = 'foreign_key_violation';
        END IF;
        IF (edge_row.state = 'active' AND NOT EXISTS (
                SELECT 1 FROM project_context_document_bindings binding
                WHERE binding.community_id = target_community
                  AND binding.edge_key = edge_row.edge_key AND binding.state = 'active'
            )) OR (edge_row.state = 'deleted' AND EXISTS (
                SELECT 1 FROM project_context_document_bindings binding
                WHERE binding.community_id = target_community
                  AND binding.edge_key = edge_row.edge_key AND binding.state = 'active'
            )) THEN
            RAISE EXCEPTION 'Project Context edge lifecycle disagrees with active bindings'
                USING ERRCODE = 'check_violation';
        END IF;
        IF NOT EXISTS (
            SELECT 1 FROM project_context_edge_changes change
            WHERE change.community_id = target_community
              AND change.change_id = edge_row.current_source_change_id
              AND change.context_revision = edge_row.last_context_revision
              AND change.edge_key = edge_row.edge_key
              AND change.edge_state = edge_row.state
              AND change.edge_document_count = (
                  SELECT count(*) FROM project_context_document_bindings binding
                  WHERE binding.community_id = target_community
                    AND binding.edge_key = edge_row.edge_key AND binding.state = 'active'
              )
              AND change.canonical_coordinates = edge_row.canonical_coordinates
              AND change.actor_pubkey = edge_row.updated_by
              AND change.accepted_at = edge_row.updated_at
        ) THEN
            RAISE EXCEPTION 'Project Context edge does not match its latest accepted change'
                USING ERRCODE = 'check_violation';
        END IF;
    END LOOP;

    IF EXISTS (
        SELECT 1
        FROM project_context_document_bindings binding
        JOIN project_context_edges edge
          ON edge.community_id = binding.community_id AND edge.edge_key = binding.edge_key
        LEFT JOIN project_documents document
          ON document.community_id = binding.community_id
         AND document.document_id = binding.context_document_id
        LEFT JOIN project_context_edge_changes change
          ON change.community_id = binding.community_id
         AND change.change_id = binding.current_source_change_id
        LEFT JOIN events projection
          ON projection.community_id = binding.community_id
         AND projection.id = binding.current_projection_event_id
         AND projection.kind = 40908
         AND projection.pubkey = state_row.projection_pubkey
         AND projection.deleted_at IS NULL
        WHERE binding.community_id = target_community AND (
            (binding.state = 'active' AND (edge.state <> 'active' OR document.state <> 'active'))
            OR change.change_id IS NULL
            OR change.context_revision <> binding.binding_context_revision
            OR change.edge_key <> binding.edge_key
            OR change.context_document_id <> binding.context_document_id
            OR change.actor_pubkey <> binding.updated_by
            OR change.accepted_at <> binding.updated_at
            OR (change.operation = 'attach') <> (binding.state = 'active')
            OR projection.id IS NULL
            OR (projection.content::jsonb - 'updated_at') IS DISTINCT FROM jsonb_build_object(
                'schema_version', 1,
                'projection_type', 'context_edge_binding',
                'project_id', binding.community_id,
                'projection_generation', state_row.projection_generation,
                'context_revision', binding.binding_context_revision,
                'edge_key', encode(binding.edge_key, 'hex'),
                'coordinates', edge.canonical_coordinates,
                'context_document_id', binding.context_document_id,
                'state', binding.state,
                'source_event_id', encode(binding.current_source_change_id, 'hex')
            )
            OR (projection.content::jsonb->>'updated_at')::timestamptz <> binding.updated_at
        )
    ) THEN
        RAISE EXCEPTION 'Project Context binding/change/Document/projection parity failed'
            USING ERRCODE = 'check_violation';
    END IF;

    SELECT event.content::jsonb INTO meta_content
    FROM events event
    WHERE event.community_id = target_community
      AND event.id = state_row.meta_projection_event_id
      AND event.kind = 40909
      AND event.pubkey = state_row.projection_pubkey
      AND event.deleted_at IS NULL;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'Project Context metadata pointer is missing or invalid'
            USING ERRCODE = 'check_violation';
    END IF;
    IF (meta_content->>'reset')::boolean THEN
        expected_meta := jsonb_build_object(
            'schema_version', 1,
            'projection_type', 'context_meta',
            'project_id', target_community,
            'projection_generation', state_row.projection_generation,
            'context_revision', state_row.context_revision,
            'active_edge_count', state_row.active_edge_count,
            'bound_document_count', state_row.bound_document_count,
            'reset', true,
            'changed_bindings', '[]'::jsonb
        );
    ELSE
        IF state_row.context_revision = 0 THEN
            RAISE EXCEPTION 'Project Context revision-zero metadata must be a reset'
                USING ERRCODE = 'check_violation';
        END IF;
        SELECT jsonb_build_object(
            'schema_version', 1,
            'projection_type', 'context_meta',
            'project_id', target_community,
            'projection_generation', state_row.projection_generation,
            'context_revision', state_row.context_revision,
            'active_edge_count', state_row.active_edge_count,
            'bound_document_count', state_row.bound_document_count,
            'reset', false,
            'changed_bindings', jsonb_build_array(jsonb_build_object(
                'context_document_id', binding.context_document_id,
                'edge_key', encode(binding.edge_key, 'hex'),
                'binding_coordinate',
                    'project-context-edge:' || target_community::text || ':binding:' || binding.context_document_id::text,
                'binding_event_id', encode(binding.current_projection_event_id, 'hex'),
                'state', binding.state
            )),
            'source_event_id', encode(change.source_event_id, 'hex')
        ) INTO expected_meta
        FROM project_context_edge_changes change
        JOIN project_context_document_bindings binding
          ON binding.community_id = change.community_id
         AND binding.context_document_id = change.context_document_id
         AND binding.current_source_change_id = change.change_id
        WHERE change.community_id = target_community
          AND change.change_id = state_row.last_change_id;
        IF expected_meta IS NULL THEN
            RAISE EXCEPTION 'Project Context latest change has no current binding'
                USING ERRCODE = 'check_violation';
        END IF;
    END IF;
    IF (meta_content - 'updated_at') IS DISTINCT FROM expected_meta
       OR (meta_content->>'updated_at')::timestamptz <> state_row.updated_at THEN
        RAISE EXCEPTION 'Project Context metadata projection does not match canonical state'
            USING ERRCODE = 'check_violation';
    END IF;

    IF state_row.context_revision > 0 AND NOT EXISTS (
        SELECT 1 FROM project_context_edge_changes change
        WHERE change.community_id = target_community
          AND change.change_id = state_row.last_change_id
          AND change.context_revision = state_row.context_revision
          AND change.actor_pubkey = state_row.last_actor_pubkey
          AND change.accepted_at = state_row.updated_at
          AND EXISTS (
              SELECT 1 FROM events command
              WHERE command.community_id = target_community
                AND command.id = change.source_event_id
                AND command.kind = 44302
                AND command.pubkey = change.actor_pubkey
                AND command.deleted_at IS NULL
          )
    ) THEN
        RAISE EXCEPTION 'Project Context catalog source does not match its latest change'
            USING ERRCODE = 'check_violation';
    END IF;
    IF EXISTS (
        SELECT 1 FROM project_context_edge_changes change
        WHERE change.community_id = target_community
          AND change.context_revision > 1
          AND NOT EXISTS (
              SELECT 1 FROM project_context_edge_changes previous
              WHERE previous.community_id = change.community_id
                AND previous.context_revision = change.context_revision - 1
                AND previous.accepted_at < change.accepted_at
          )
    ) THEN
        RAISE EXCEPTION 'Project Context change history is not contiguous and monotonic'
            USING ERRCODE = 'check_violation';
    END IF;
END
$$;

-- Attach liveness is transition evidence, not a permanent coordinate
-- constraint.  This deferred row trigger runs only for the newly accepted
-- attach while the Community lock still serializes source-domain changes.
CREATE FUNCTION project_context_validate_new_change() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.operation = 'attach' THEN
        IF NOT EXISTS (
            SELECT 1 FROM project_documents document
            WHERE document.community_id = NEW.community_id
              AND document.document_id = NEW.context_document_id
              AND document.state = 'active'
        ) OR EXISTS (
            SELECT 1 FROM project_context_edge_coordinates coordinate
            WHERE coordinate.community_id = NEW.community_id
              AND coordinate.edge_key = NEW.edge_key
              AND (
                  (
                      coordinate.coordinate_type = 'project_view_object'
                      AND NOT EXISTS (
                          SELECT 1 FROM project_view_objects object
                          WHERE object.community_id = coordinate.community_id
                            AND object.object_id = coordinate.coordinate_id
                            AND object.object_type = coordinate.coordinate_subtype
                            AND object.deleted_at IS NULL
                      )
                  )
                  OR (
                      coordinate.coordinate_type = 'document'
                      AND NOT EXISTS (
                          SELECT 1 FROM project_documents document
                          WHERE document.community_id = coordinate.community_id
                            AND document.document_id = coordinate.coordinate_id
                            AND document.state = 'active'
                      )
                  )
              )
        ) THEN
            RAISE EXCEPTION 'Project Context attach requires active coordinates and Context Document'
                USING ERRCODE = 'check_violation';
        END IF;
    END IF;
    RETURN NULL;
END
$$;

CREATE FUNCTION project_context_validate_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM project_context_validate_community(OLD.community_id);
    ELSE
        IF TG_OP = 'UPDATE' AND OLD.community_id IS DISTINCT FROM NEW.community_id THEN
            PERFORM project_context_validate_community(OLD.community_id);
        END IF;
        PERFORM project_context_validate_community(NEW.community_id);
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER project_context_edge_state_validate
    AFTER INSERT OR UPDATE ON project_context_edge_state
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_row();
CREATE CONSTRAINT TRIGGER project_context_edges_validate
    AFTER INSERT OR UPDATE ON project_context_edges
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_row();
CREATE CONSTRAINT TRIGGER project_context_edge_coordinates_validate
    AFTER INSERT ON project_context_edge_coordinates
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_row();
CREATE CONSTRAINT TRIGGER project_context_bindings_validate
    AFTER INSERT OR UPDATE ON project_context_document_bindings
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_row();
CREATE CONSTRAINT TRIGGER project_context_changes_validate
    AFTER INSERT ON project_context_edge_changes
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_row();
CREATE CONSTRAINT TRIGGER project_context_changes_liveness_validate
    AFTER INSERT ON project_context_edge_changes
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_new_change();
CREATE CONSTRAINT TRIGGER project_documents_project_context_validate
    AFTER INSERT OR UPDATE ON project_documents
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_row();

CREATE FUNCTION project_context_validate_capability(target_community UUID) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    enabled BOOLEAN;
BEGIN
    SELECT project_context_edge_enabled INTO enabled
    FROM communities WHERE id = target_community;
    IF NOT FOUND OR NOT enabled THEN RETURN; END IF;

    IF NOT EXISTS (
        SELECT 1
        FROM communities community
        JOIN project_view_maintenance maintenance
          ON maintenance.community_id = community.id AND maintenance.state = 'normal'
        JOIN project_view_state view_state
          ON view_state.community_id = community.id AND view_state.schema_version = 3
        JOIN project_document_state document_state
          ON document_state.community_id = community.id AND document_state.schema_version = 1
        JOIN project_context_edge_state context_state
          ON context_state.community_id = community.id AND context_state.schema_version = 1
        WHERE community.id = target_community
          AND community.archived_at IS NULL
          AND community.project_view_schema_version = 3
          AND community.project_view_enabled
          AND community.project_document_enabled
          AND view_state.projection_pubkey = document_state.projection_pubkey
          AND document_state.projection_pubkey = context_state.projection_pubkey
    ) THEN
        RAISE EXCEPTION 'Project Context capability prerequisites are not ready'
            USING ERRCODE = 'check_violation';
    END IF;
    PERFORM project_view_v3_validate_community(target_community);
    PERFORM project_document_validate_community(target_community);
    PERFORM project_context_validate_community(target_community);
END
$$;

-- Separate wrappers avoid referencing columns that do not exist on every
-- trigger relation through a polymorphic NEW/OLD record.
CREATE FUNCTION project_context_validate_capability_community_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM project_context_validate_capability(COALESCE(NEW.id, OLD.id));
    RETURN NULL;
END
$$;

CREATE FUNCTION project_context_validate_capability_scoped_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM project_context_validate_capability(COALESCE(NEW.community_id, OLD.community_id));
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER communities_project_context_edge_validate
    AFTER INSERT OR UPDATE ON communities
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_capability_community_row();
CREATE CONSTRAINT TRIGGER project_view_maintenance_context_edge_validate
    AFTER INSERT OR UPDATE ON project_view_maintenance
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_capability_scoped_row();
CREATE CONSTRAINT TRIGGER project_view_state_context_edge_validate
    AFTER INSERT OR UPDATE ON project_view_state
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_capability_scoped_row();
CREATE CONSTRAINT TRIGGER project_document_state_context_edge_validate
    AFTER INSERT OR UPDATE ON project_document_state
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_capability_scoped_row();
CREATE CONSTRAINT TRIGGER project_context_edge_state_capability_validate
    AFTER INSERT OR UPDATE ON project_context_edge_state
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_context_validate_capability_scoped_row();
