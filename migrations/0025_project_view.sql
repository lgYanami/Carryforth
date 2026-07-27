-- Project View canonical state.
--
-- Project View remains disabled after this additive migration. Operators
-- enable one community at a time only after every relay pod is schema- and
-- signer-ready. The database flag is intentionally shared by all pods.
ALTER TABLE communities
    ADD COLUMN project_view_enabled BOOLEAN NOT NULL DEFAULT FALSE;

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

-- Maintain the scalar active-object count mechanically. Application code
-- supplies its expected count and verifies this trigger's result before
-- commit; it never assigns the count directly during normal mutation writes.
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

-- Current object rows are the permanent ID registry. Deletion is represented
-- only by an in-place tombstone so IDs cannot be reused.
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

-- Validate the final aggregate shape at transaction commit. These point
-- lookups permit Initialize to insert state, Profile, and Goals in multiple
-- statements without turning every mutation into a full object-table scan.
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

-- Validate only the changed canonical object and indexed inbound references.
-- A periodic integrity audit owns full-table drift detection.
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
