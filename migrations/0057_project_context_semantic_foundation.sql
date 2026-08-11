-- Migration 0057.
-- Project Context semantic-index derived state. Capability-off, no backfill.
--
-- pgvector is an operator-installed PostgreSQL prerequisite. This migration
-- deliberately does not install or remove the extension.

DO $$
DECLARE
    installed_version TEXT;
BEGIN
    SELECT extversion INTO installed_version
    FROM pg_extension
    WHERE extname = 'vector';

    IF installed_version IS NULL THEN
        RAISE EXCEPTION 'Project Context semantic schema requires installed pgvector'
            USING ERRCODE = 'feature_not_supported',
                  HINT = 'Install pgvector 0.8.5 in this database and run buzz-admin semantic preflight before migrating.';
    END IF;

    IF split_part(installed_version, '.', 1) <> '0'
       OR split_part(installed_version, '.', 2) <> '8'
       OR split_part(split_part(installed_version, '.', 3), '-', 1)::INT < 5
    THEN
        RAISE EXCEPTION 'unsupported pgvector version %', installed_version
            USING ERRCODE = 'feature_not_supported',
                  HINT = 'Buzz requires pgvector 0.8.5.x for the first semantic generation.';
    END IF;

    IF to_regtype('vector') IS NULL OR to_regtype('halfvec') IS NULL THEN
        RAISE EXCEPTION 'pgvector vector/halfvec types are unavailable'
            USING ERRCODE = 'feature_not_supported';
    END IF;
END;
$$;

ALTER TABLE communities
    ADD COLUMN semantic_index_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN semantic_active_generation_id UUID;

CREATE TABLE semantic_index_generations (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE NO ACTION,
    generation_id UUID NOT NULL,
    lifecycle TEXT NOT NULL DEFAULT 'building'
        CHECK (lifecycle IN (
            'building', 'ready', 'active', 'rollback_ready', 'retired', 'failed'
        )),
    extractor_version TEXT NOT NULL,
    input_contract_version TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    dimensions INT NOT NULL CHECK (dimensions BETWEEN 1 AND 16000),
    distance_metric TEXT NOT NULL CHECK (distance_metric = 'cosine'),
    normalization TEXT NOT NULL CHECK (normalization = 'none'),
    provider_boundary TEXT NOT NULL
        CHECK (provider_boundary IN ('external', 'deterministic_fake')),
    model_contract_digest BYTEA NOT NULL
        CHECK (octet_length(model_contract_digest) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    created_by TEXT NOT NULL CHECK (octet_length(btrim(created_by)) BETWEEN 1 AND 255),
    rebuild_completed_at TIMESTAMPTZ,
    ready_at TIMESTAMPTZ,
    activated_at TIMESTAMPTZ,
    retired_at TIMESTAMPTZ,
    failed_at TIMESTAMPTZ,
    failure_code TEXT CHECK (
        failure_code IS NULL OR octet_length(btrim(failure_code)) BETWEEN 1 AND 128
    ),
    PRIMARY KEY (community_id, generation_id),
    UNIQUE (community_id, generation_id, dimensions, model_contract_digest),
    UNIQUE (community_id, model_contract_digest),
    CHECK (generation_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CHECK (octet_length(btrim(extractor_version)) BETWEEN 1 AND 255),
    CHECK (octet_length(btrim(input_contract_version)) BETWEEN 1 AND 255),
    CHECK (octet_length(btrim(provider)) BETWEEN 1 AND 255),
    CHECK (octet_length(btrim(model)) BETWEEN 1 AND 255),
    CHECK (
        (lifecycle = 'building'
         AND ready_at IS NULL AND activated_at IS NULL
         AND retired_at IS NULL AND failed_at IS NULL AND failure_code IS NULL)
        OR (lifecycle = 'ready'
            AND ready_at IS NOT NULL AND activated_at IS NULL
            AND retired_at IS NULL AND failed_at IS NULL AND failure_code IS NULL)
        OR (lifecycle IN ('active', 'rollback_ready')
            AND ready_at IS NOT NULL AND activated_at IS NOT NULL
            AND retired_at IS NULL AND failed_at IS NULL AND failure_code IS NULL)
        OR (lifecycle = 'retired'
            AND ready_at IS NOT NULL
            AND retired_at IS NOT NULL AND failed_at IS NULL AND failure_code IS NULL)
        OR (lifecycle = 'failed'
            AND failed_at IS NOT NULL AND failure_code IS NOT NULL)
    )
);

CREATE UNIQUE INDEX semantic_index_generations_one_active
    ON semantic_index_generations (community_id)
    WHERE lifecycle = 'active';
CREATE INDEX semantic_index_generations_lifecycle
    ON semantic_index_generations (community_id, lifecycle, generation_id);

ALTER TABLE communities
    ADD CONSTRAINT communities_semantic_active_generation_fk
        FOREIGN KEY (id, semantic_active_generation_id)
        REFERENCES semantic_index_generations (community_id, generation_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE semantic_sources (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE NO ACTION,
    source_family TEXT NOT NULL
        CHECK (source_family IN ('project_view', 'project_document', 'meeting')),
    source_subtype TEXT NOT NULL,
    source_id UUID NOT NULL,
    eligibility TEXT NOT NULL DEFAULT 'unknown'
        CHECK (eligibility IN ('unknown', 'eligible', 'ineligible')),
    ineligibility_reason TEXT CHECK (
        ineligibility_reason IS NULL OR ineligibility_reason IN (
            'tombstone', 'deleted', 'invalid_canonical_state',
            'source_capability_unavailable'
        )
    ),
    lifecycle_class TEXT NOT NULL DEFAULT 'active'
        CHECK (lifecycle_class IN (
            'active', 'finalizing', 'terminal', 'tombstone', 'deleted'
        )),
    source_status TEXT,
    source_basis JSONB,
    snapshot_digest BYTEA CHECK (
        snapshot_digest IS NULL OR octet_length(snapshot_digest) = 32
    ),
    invalidation_epoch BIGINT NOT NULL DEFAULT 1
        CHECK (invalidation_epoch BETWEEN 1 AND 9007199254740991),
    coverage_state TEXT NOT NULL DEFAULT 'dirty'
        CHECK (coverage_state IN (
            'dirty', 'missing', 'building', 'current', 'failed',
            'unsupported', 'ineligible'
        )),
    observed_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, source_family, source_subtype, source_id),
    UNIQUE (
        community_id, source_family, source_subtype, source_id,
        invalidation_epoch, snapshot_digest
    ),
    CHECK (source_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CHECK (
        (source_family = 'project_view' AND source_subtype IN (
            'project_profile', 'goal', 'role', 'plan', 'stage',
            'requirement', 'issue', 'work', 'resource'
        ))
        OR (source_family = 'project_document' AND source_subtype = 'document')
        OR (source_family = 'meeting' AND source_subtype = 'meeting')
    ),
    CHECK ((source_basis IS NULL) = (snapshot_digest IS NULL)),
    CHECK (
        (eligibility = 'ineligible' AND ineligibility_reason IS NOT NULL
         AND coverage_state = 'ineligible')
        OR (eligibility <> 'ineligible' AND ineligibility_reason IS NULL
            AND coverage_state <> 'ineligible')
    ),
    CHECK (source_status IS NULL OR octet_length(btrim(source_status)) BETWEEN 1 AND 128)
);

CREATE INDEX semantic_sources_coverage
    ON semantic_sources (community_id, coverage_state, source_family, source_id);
CREATE INDEX semantic_sources_scan
    ON semantic_sources (community_id, source_family, source_subtype, source_id);

CREATE TABLE semantic_unit_sets (
    community_id UUID NOT NULL,
    unit_set_id UUID NOT NULL,
    source_family TEXT NOT NULL,
    source_subtype TEXT NOT NULL,
    source_id UUID NOT NULL,
    source_invalidation_epoch BIGINT NOT NULL
        CHECK (source_invalidation_epoch BETWEEN 1 AND 9007199254740991),
    source_basis JSONB NOT NULL,
    source_snapshot_digest BYTEA NOT NULL
        CHECK (octet_length(source_snapshot_digest) = 32),
    extractor_version TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'staging'
        CHECK (state IN ('staging', 'active', 'retired')),
    complete_unit_count INT NOT NULL CHECK (complete_unit_count > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    activated_at TIMESTAMPTZ,
    retired_at TIMESTAMPTZ,
    PRIMARY KEY (community_id, unit_set_id),
    UNIQUE (
        community_id, source_family, source_subtype, source_id,
        source_snapshot_digest, extractor_version
    ),
    UNIQUE (
        community_id, unit_set_id, source_family, source_subtype,
        source_id, source_snapshot_digest
    ),
    FOREIGN KEY (community_id, source_family, source_subtype, source_id)
        REFERENCES semantic_sources (
            community_id, source_family, source_subtype, source_id
        ) ON DELETE NO ACTION,
    CHECK (unit_set_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CHECK (octet_length(btrim(extractor_version)) BETWEEN 1 AND 255),
    CHECK (
        (state = 'staging' AND activated_at IS NULL AND retired_at IS NULL)
        OR (state = 'active' AND activated_at IS NOT NULL AND retired_at IS NULL)
        OR (state = 'retired' AND retired_at IS NOT NULL)
    )
);

CREATE INDEX semantic_unit_sets_source
    ON semantic_unit_sets (
        community_id, source_family, source_subtype, source_id, created_at
    );
CREATE INDEX semantic_unit_sets_gc
    ON semantic_unit_sets (community_id, state, retired_at, created_at);

CREATE TABLE semantic_units (
    community_id UUID NOT NULL,
    unit_set_id UUID NOT NULL,
    unit_key TEXT NOT NULL,
    ordinal INT NOT NULL CHECK (ordinal >= 0),
    unit_kind TEXT NOT NULL CHECK (unit_kind IN ('overview', 'content_chunk')),
    source_path TEXT,
    semantic_text TEXT NOT NULL,
    semantic_text_digest BYTEA NOT NULL
        CHECK (octet_length(semantic_text_digest) = 32),
    summary_coverage TEXT NOT NULL
        CHECK (summary_coverage IN ('title_only', 'title_and_summary')),
    extraction_provenance JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, unit_set_id, unit_key),
    UNIQUE (community_id, unit_set_id, ordinal),
    FOREIGN KEY (community_id, unit_set_id)
        REFERENCES semantic_unit_sets (community_id, unit_set_id)
        ON DELETE CASCADE,
    CHECK (octet_length(btrim(unit_key)) BETWEEN 1 AND 512),
    -- PostgreSQL text rejects NUL at the protocol/type boundary; Rust also
    -- validates it before write. Calling chr(0) here would itself fail on every
    -- otherwise valid INSERT.
    CHECK (btrim(semantic_text) <> ''),
    CHECK (source_path IS NULL OR octet_length(source_path) BETWEEN 1 AND 2048),
    CHECK (
        (unit_kind = 'overview' AND unit_key = 'overview'
         AND ordinal = 0 AND source_path IS NULL)
        OR unit_kind = 'content_chunk'
    )
);

CREATE INDEX semantic_units_text_digest
    ON semantic_units (community_id, semantic_text_digest, unit_set_id);

CREATE TABLE semantic_embeddings (
    community_id UUID NOT NULL,
    unit_set_id UUID NOT NULL,
    unit_key TEXT NOT NULL,
    generation_id UUID NOT NULL,
    dimensions INT NOT NULL CHECK (dimensions BETWEEN 1 AND 16000),
    model_contract_digest BYTEA NOT NULL
        CHECK (octet_length(model_contract_digest) = 32),
    response_model TEXT NOT NULL,
    embedding public.vector NOT NULL,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, unit_set_id, unit_key, generation_id),
    FOREIGN KEY (community_id, unit_set_id, unit_key)
        REFERENCES semantic_units (community_id, unit_set_id, unit_key)
        ON DELETE CASCADE,
    FOREIGN KEY (
        community_id, generation_id, dimensions, model_contract_digest
    ) REFERENCES semantic_index_generations (
        community_id, generation_id, dimensions, model_contract_digest
    ) ON DELETE NO ACTION,
    CHECK (vector_dims(embedding) = dimensions),
    CHECK (octet_length(btrim(response_model)) BETWEEN 1 AND 255)
);

CREATE INDEX semantic_embeddings_generation
    ON semantic_embeddings (community_id, generation_id, unit_set_id, unit_key);

CREATE TABLE semantic_source_generation_heads (
    community_id UUID NOT NULL,
    generation_id UUID NOT NULL,
    source_family TEXT NOT NULL,
    source_subtype TEXT NOT NULL,
    source_id UUID NOT NULL,
    unit_set_id UUID NOT NULL,
    source_invalidation_epoch BIGINT NOT NULL,
    source_snapshot_digest BYTEA NOT NULL
        CHECK (octet_length(source_snapshot_digest) = 32),
    complete_unit_count INT NOT NULL CHECK (complete_unit_count > 0),
    complete_embedding_count INT NOT NULL CHECK (complete_embedding_count > 0),
    activated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (
        community_id, generation_id, source_family, source_subtype, source_id
    ),
    FOREIGN KEY (community_id, generation_id)
        REFERENCES semantic_index_generations (community_id, generation_id)
        ON DELETE CASCADE,
    FOREIGN KEY (
        community_id, source_family, source_subtype, source_id,
        source_invalidation_epoch, source_snapshot_digest
    ) REFERENCES semantic_sources (
        community_id, source_family, source_subtype, source_id,
        invalidation_epoch, snapshot_digest
    ) ON DELETE NO ACTION,
    FOREIGN KEY (
        community_id, unit_set_id, source_family, source_subtype,
        source_id, source_snapshot_digest
    ) REFERENCES semantic_unit_sets (
        community_id, unit_set_id, source_family, source_subtype,
        source_id, source_snapshot_digest
    ) ON DELETE NO ACTION,
    CHECK (complete_unit_count = complete_embedding_count)
);

CREATE INDEX semantic_source_generation_heads_source
    ON semantic_source_generation_heads (
        community_id, source_family, source_subtype, source_id, generation_id
    );

CREATE TABLE semantic_index_jobs (
    community_id UUID NOT NULL,
    generation_id UUID NOT NULL,
    source_family TEXT NOT NULL,
    source_subtype TEXT NOT NULL,
    source_id UUID NOT NULL,
    desired_invalidation_epoch BIGINT NOT NULL
        CHECK (desired_invalidation_epoch BETWEEN 1 AND 9007199254740991),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'claimed', 'retry', 'succeeded', 'poison')),
    attempts INT NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    claim_id UUID,
    lease_until TIMESTAMPTZ,
    claimed_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    error_code TEXT CHECK (
        error_code IS NULL OR octet_length(btrim(error_code)) BETWEEN 1 AND 128
    ),
    error_detail TEXT CHECK (
        error_detail IS NULL OR octet_length(error_detail) BETWEEN 1 AND 512
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (
        community_id, generation_id, source_family, source_subtype, source_id
    ),
    FOREIGN KEY (community_id, generation_id)
        REFERENCES semantic_index_generations (community_id, generation_id)
        ON DELETE CASCADE,
    FOREIGN KEY (community_id, source_family, source_subtype, source_id)
        REFERENCES semantic_sources (
            community_id, source_family, source_subtype, source_id
        ) ON DELETE CASCADE,
    CHECK (
        (state = 'claimed' AND claim_id IS NOT NULL
         AND lease_until IS NOT NULL AND claimed_at IS NOT NULL
         AND completed_at IS NULL)
        OR (state IN ('pending', 'retry') AND claim_id IS NULL
            AND lease_until IS NULL AND claimed_at IS NULL
            AND completed_at IS NULL)
        OR (state IN ('succeeded', 'poison') AND claim_id IS NULL
            AND lease_until IS NULL AND completed_at IS NOT NULL)
    )
);

CREATE INDEX semantic_index_jobs_due
    ON semantic_index_jobs (next_attempt_at, community_id, generation_id)
    WHERE state IN ('pending', 'retry');
CREATE INDEX semantic_index_jobs_lease
    ON semantic_index_jobs (lease_until, community_id, generation_id)
    WHERE state = 'claimed';
CREATE INDEX semantic_index_jobs_coverage
    ON semantic_index_jobs (community_id, generation_id, state, updated_at);

CREATE TABLE semantic_rebuild_operations (
    community_id UUID NOT NULL,
    operation_id UUID NOT NULL,
    generation_id UUID NOT NULL,
    scope_family TEXT NOT NULL
        CHECK (scope_family IN ('all', 'project_view', 'project_document', 'meeting')),
    current_family TEXT NOT NULL
        CHECK (current_family IN ('project_view', 'project_document', 'meeting')),
    after_source_subtype TEXT,
    after_source_id UUID,
    state TEXT NOT NULL DEFAULT 'running'
        CHECK (state IN ('running', 'completed', 'cancelled')),
    started_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    completed_at TIMESTAMPTZ,
    PRIMARY KEY (community_id, operation_id),
    FOREIGN KEY (community_id, generation_id)
        REFERENCES semantic_index_generations (community_id, generation_id)
        ON DELETE CASCADE,
    CHECK (operation_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CHECK ((after_source_subtype IS NULL) = (after_source_id IS NULL)),
    CHECK (
        (state = 'running' AND completed_at IS NULL)
        OR (state IN ('completed', 'cancelled') AND completed_at IS NOT NULL)
    )
);

CREATE INDEX semantic_rebuild_operations_state
    ON semantic_rebuild_operations (community_id, state, updated_at, operation_id);

CREATE TABLE semantic_provider_rate_gates (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    next_request_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, provider),
    CHECK (octet_length(btrim(provider)) BETWEEN 1 AND 255)
);

CREATE FUNCTION semantic_mark_source_changed(
    p_community_id UUID,
    p_source_family TEXT,
    p_source_subtype TEXT,
    p_source_id UUID,
    p_eligible BOOLEAN,
    p_lifecycle_class TEXT,
    p_source_status TEXT,
    p_ineligibility_reason TEXT DEFAULT NULL
) RETURNS VOID AS $$
DECLARE
    next_epoch BIGINT;
    now_at TIMESTAMPTZ := clock_timestamp();
BEGIN
    DELETE FROM semantic_source_generation_heads
    WHERE community_id = p_community_id
      AND source_family = p_source_family
      AND source_subtype = p_source_subtype
      AND source_id = p_source_id;

    INSERT INTO semantic_sources (
        community_id, source_family, source_subtype, source_id,
        eligibility, ineligibility_reason, lifecycle_class, source_status,
        source_basis, snapshot_digest, invalidation_epoch, coverage_state,
        observed_at, updated_at
    ) VALUES (
        p_community_id, p_source_family, p_source_subtype, p_source_id,
        CASE WHEN p_eligible THEN 'unknown' ELSE 'ineligible' END,
        CASE WHEN p_eligible THEN NULL ELSE p_ineligibility_reason END,
        p_lifecycle_class, p_source_status,
        NULL, NULL, 1,
        CASE WHEN p_eligible THEN 'dirty' ELSE 'ineligible' END,
        NULL, now_at
    )
    ON CONFLICT (community_id, source_family, source_subtype, source_id)
    DO UPDATE SET
        eligibility = CASE WHEN p_eligible THEN 'unknown' ELSE 'ineligible' END,
        ineligibility_reason = CASE
            WHEN p_eligible THEN NULL ELSE p_ineligibility_reason
        END,
        lifecycle_class = p_lifecycle_class,
        source_status = p_source_status,
        source_basis = NULL,
        snapshot_digest = NULL,
        invalidation_epoch = semantic_sources.invalidation_epoch + 1,
        coverage_state = CASE WHEN p_eligible THEN 'dirty' ELSE 'ineligible' END,
        observed_at = NULL,
        updated_at = now_at
    RETURNING invalidation_epoch INTO next_epoch;

    UPDATE semantic_unit_sets unit_set
    SET state = 'retired', retired_at = COALESCE(unit_set.retired_at, now_at)
    WHERE unit_set.community_id = p_community_id
      AND unit_set.source_family = p_source_family
      AND unit_set.source_subtype = p_source_subtype
      AND unit_set.source_id = p_source_id
      AND unit_set.state = 'active'
      AND NOT EXISTS (
          SELECT 1
          FROM semantic_source_generation_heads head
          WHERE head.community_id = unit_set.community_id
            AND head.unit_set_id = unit_set.unit_set_id
      );

    IF p_eligible THEN
        INSERT INTO semantic_index_jobs (
            community_id, generation_id, source_family, source_subtype,
            source_id, desired_invalidation_epoch, state, attempts,
            next_attempt_at, claim_id, lease_until, claimed_at, completed_at,
            error_code, error_detail, created_at, updated_at
        )
        SELECT p_community_id, generation.generation_id,
               p_source_family, p_source_subtype, p_source_id,
               next_epoch, 'pending', 0, now_at,
               NULL, NULL, NULL, NULL, NULL, NULL, now_at, now_at
        FROM semantic_index_generations generation
        WHERE generation.community_id = p_community_id
          AND generation.lifecycle IN (
              'building', 'ready', 'active', 'rollback_ready'
          )
        ON CONFLICT (
            community_id, generation_id, source_family, source_subtype, source_id
        ) DO UPDATE SET
            desired_invalidation_epoch = EXCLUDED.desired_invalidation_epoch,
            state = 'pending',
            attempts = 0,
            next_attempt_at = EXCLUDED.next_attempt_at,
            claim_id = NULL,
            lease_until = NULL,
            claimed_at = NULL,
            completed_at = NULL,
            error_code = NULL,
            error_detail = NULL,
            updated_at = EXCLUDED.updated_at;
    ELSE
        DELETE FROM semantic_index_jobs
        WHERE community_id = p_community_id
          AND source_family = p_source_family
          AND source_subtype = p_source_subtype
          AND source_id = p_source_id;
    END IF;
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION semantic_capture_project_view_source() RETURNS TRIGGER AS $$
DECLARE
    status_value TEXT;
    lifecycle_value TEXT;
BEGIN
    IF TG_OP = 'UPDATE'
       AND NEW.schema_version IS NOT DISTINCT FROM OLD.schema_version
       AND NEW.object_type IS NOT DISTINCT FROM OLD.object_type
       AND NEW.object_revision IS NOT DISTINCT FROM OLD.object_revision
       AND NEW.body IS NOT DISTINCT FROM OLD.body
       AND NEW.deleted_at IS NOT DISTINCT FROM OLD.deleted_at
       AND NEW.source_type IS NOT DISTINCT FROM OLD.source_type
       AND NEW.source_change_id IS NOT DISTINCT FROM OLD.source_change_id
    THEN
        RETURN NEW;
    END IF;

    status_value := CASE
        WHEN NEW.object_type = 'role' AND NEW.deleted_at IS NULL
            THEN CASE WHEN (NEW.body->>'active')::BOOLEAN THEN 'active' ELSE 'inactive' END
        WHEN NEW.deleted_at IS NULL THEN NEW.body->>'status'
        ELSE NULL
    END;
    lifecycle_value := CASE
        WHEN NEW.deleted_at IS NOT NULL THEN 'tombstone'
        WHEN status_value IN (
            'completed', 'cancelled', 'satisfied', 'withdrawn',
            'resolved', 'closed', 'inactive'
        ) THEN 'terminal'
        ELSE 'active'
    END;

    PERFORM semantic_mark_source_changed(
        NEW.community_id, 'project_view', NEW.object_type, NEW.object_id,
        NEW.schema_version = 3 AND NEW.deleted_at IS NULL,
        lifecycle_value, status_value,
        CASE
            WHEN NEW.deleted_at IS NOT NULL THEN 'tombstone'
            ELSE 'source_capability_unavailable'
        END
    );
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER project_view_objects_semantic_capture
    AFTER INSERT OR UPDATE OF
        schema_version, object_type, object_revision, body, deleted_at,
        source_type, source_change_id
    ON project_view_objects
    FOR EACH ROW EXECUTE FUNCTION semantic_capture_project_view_source();

CREATE FUNCTION semantic_capture_project_document_source() RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'UPDATE'
       AND NEW.current_revision IS NOT DISTINCT FROM OLD.current_revision
       AND NEW.state IS NOT DISTINCT FROM OLD.state
       AND NEW.current_source_change_id IS NOT DISTINCT FROM OLD.current_source_change_id
    THEN
        RETURN NEW;
    END IF;

    PERFORM semantic_mark_source_changed(
        NEW.community_id, 'project_document', 'document', NEW.document_id,
        NEW.state = 'active',
        CASE WHEN NEW.state = 'active' THEN 'active' ELSE 'tombstone' END,
        NEW.state,
        CASE WHEN NEW.state = 'active' THEN NULL ELSE 'tombstone' END
    );
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER project_documents_semantic_capture
    AFTER INSERT OR UPDATE OF current_revision, state, current_source_change_id
    ON project_documents
    FOR EACH ROW EXECUTE FUNCTION semantic_capture_project_document_source();

CREATE FUNCTION semantic_capture_meeting_session_source() RETURNS TRIGGER AS $$
DECLARE
    channel_deleted BOOLEAN;
BEGIN
    IF TG_OP = 'UPDATE'
       AND NEW.summary IS NOT DISTINCT FROM OLD.summary
       AND NEW.status IS NOT DISTINCT FROM OLD.status
       AND NEW.end_event_id IS NOT DISTINCT FROM OLD.end_event_id
       AND NEW.schema_version IS NOT DISTINCT FROM OLD.schema_version
       AND NEW.floor_policy_version IS NOT DISTINCT FROM OLD.floor_policy_version
    THEN
        RETURN NEW;
    END IF;

    SELECT channel.deleted_at IS NOT NULL INTO channel_deleted
    FROM channels channel
    WHERE channel.community_id = NEW.community_id
      AND channel.id = NEW.session_id
      AND channel.room_kind = 'meeting';

    PERFORM semantic_mark_source_changed(
        NEW.community_id, 'meeting', 'meeting', NEW.session_id,
        COALESCE(NOT channel_deleted, FALSE),
        CASE WHEN NEW.status = 'ended' THEN 'terminal' ELSE 'active' END,
        NEW.status,
        CASE WHEN COALESCE(channel_deleted, TRUE) THEN 'deleted' ELSE NULL END
    );
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER meeting_sessions_semantic_capture
    AFTER INSERT OR UPDATE OF
        summary, status, end_event_id, schema_version, floor_policy_version
    ON meeting_sessions
    FOR EACH ROW EXECUTE FUNCTION semantic_capture_meeting_session_source();

CREATE FUNCTION semantic_capture_meeting_runtime_source() RETURNS TRIGGER AS $$
DECLARE
    session_status TEXT;
    channel_deleted BOOLEAN;
BEGIN
    IF TG_OP = 'UPDATE' AND NEW.runtime_phase IS NOT DISTINCT FROM OLD.runtime_phase THEN
        RETURN NEW;
    END IF;

    SELECT session.status, channel.deleted_at IS NOT NULL
      INTO session_status, channel_deleted
    FROM meeting_sessions session
    JOIN channels channel
      ON channel.community_id = session.community_id
     AND channel.id = session.session_id
     AND channel.room_kind = 'meeting'
    WHERE session.community_id = NEW.community_id
      AND session.session_id = NEW.session_id;

    IF FOUND THEN
        PERFORM semantic_mark_source_changed(
            NEW.community_id, 'meeting', 'meeting', NEW.session_id,
            NOT channel_deleted,
            CASE
                WHEN session_status = 'ended' OR NEW.runtime_phase = 'ended' THEN 'terminal'
                WHEN NEW.runtime_phase = 'finalizing_actions' THEN 'finalizing'
                ELSE 'active'
            END,
            session_status,
            CASE WHEN channel_deleted THEN 'deleted' ELSE NULL END
        );
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER meeting_v2_runtime_semantic_capture
    AFTER INSERT OR UPDATE OF runtime_phase
    ON meeting_v2_bootstrap_state
    FOR EACH ROW EXECUTE FUNCTION semantic_capture_meeting_runtime_source();

CREATE FUNCTION semantic_capture_meeting_channel_source() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.room_kind <> 'meeting'
       OR (
           NEW.name IS NOT DISTINCT FROM OLD.name
           AND NEW.visibility IS NOT DISTINCT FROM OLD.visibility
           AND NEW.deleted_at IS NOT DISTINCT FROM OLD.deleted_at
       )
    THEN
        RETURN NEW;
    END IF;

    IF EXISTS (
        SELECT 1 FROM meeting_sessions session
        WHERE session.community_id = NEW.community_id
          AND session.session_id = NEW.id
    ) THEN
        PERFORM semantic_mark_source_changed(
            NEW.community_id, 'meeting', 'meeting', NEW.id,
            NEW.deleted_at IS NULL,
            CASE
                WHEN NEW.deleted_at IS NOT NULL THEN 'deleted'
                WHEN EXISTS (
                    SELECT 1 FROM meeting_sessions session
                    WHERE session.community_id = NEW.community_id
                      AND session.session_id = NEW.id
                      AND session.status = 'ended'
                ) THEN 'terminal'
                ELSE 'active'
            END,
            (
                SELECT session.status FROM meeting_sessions session
                WHERE session.community_id = NEW.community_id
                  AND session.session_id = NEW.id
            ),
            CASE WHEN NEW.deleted_at IS NULL THEN NULL ELSE 'deleted' END
        );
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER channels_meeting_semantic_capture
    AFTER UPDATE OF name, visibility, deleted_at
    ON channels
    FOR EACH ROW EXECUTE FUNCTION semantic_capture_meeting_channel_source();
