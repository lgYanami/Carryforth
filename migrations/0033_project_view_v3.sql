-- Project View v3 canonical storage, reviewed Resource cutover, and durable
-- maintenance/provisioning ledgers.
--
-- This migration is capability-off. Existing Communities remain on their
-- current major, project_context_enabled defaults false, and no Community is
-- prepared or cut over implicitly.

ALTER TABLE communities
    DROP CONSTRAINT communities_project_view_schema_version_check,
    ADD COLUMN project_context_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN project_view_preparation_operation_id UUID,
    ADD CONSTRAINT communities_project_view_schema_version_check
        CHECK (project_view_schema_version IN (1, 2, 3)),
    ADD CONSTRAINT communities_project_context_gate_check
        CHECK (NOT project_context_enabled OR project_view_schema_version = 3);

ALTER TABLE project_view_state
    DROP CONSTRAINT project_view_state_schema_version_check,
    ADD CONSTRAINT project_view_state_schema_version_check
        CHECK (schema_version IN (1, 2, 3));

ALTER TABLE project_view_objects
    DROP CONSTRAINT project_view_objects_schema_check,
    DROP CONSTRAINT project_view_objects_v2_fields_check,
    ALTER COLUMN source_event_id DROP NOT NULL,
    ADD COLUMN guide_document_id UUID,
    ADD COLUMN source_type TEXT,
    ADD COLUMN source_change_id BYTEA,
    ADD COLUMN source_provenance_id UUID;

UPDATE project_view_objects
SET source_type = 'nostr_event',
    source_change_id = source_event_id;

ALTER TABLE project_view_objects
    ALTER COLUMN source_type SET NOT NULL,
    ALTER COLUMN source_change_id SET NOT NULL,
    ADD CONSTRAINT project_view_objects_schema_check
        CHECK (schema_version IN (1, 2, 3)),
    ADD CONSTRAINT project_view_objects_source_type_check
        CHECK (source_type IN ('nostr_event', 'operator', 'system')),
    ADD CONSTRAINT project_view_objects_source_change_check
        CHECK (octet_length(source_change_id) = 32),
    ADD CONSTRAINT project_view_objects_source_shape_check
        CHECK (
            (
                source_type = 'nostr_event'
                AND source_event_id IS NOT NULL
                AND source_change_id = source_event_id
            )
            OR (
                source_type IN ('operator', 'system')
                AND source_event_id IS NULL
            )
        ),
    ADD CONSTRAINT project_view_objects_v2_fields_check
        CHECK (
            (
                schema_version = 1
                AND role_level IS NULL
                AND responsible_role_id IS NULL
                AND guide_document_id IS NULL
                AND source_provenance_id IS NULL
                AND source_type = 'nostr_event'
            )
            OR (
                schema_version = 2
                AND (
                    (
                        object_type = 'role'
                        AND role_level IN ('admin', 'member')
                        AND (
                            deleted_at IS NOT NULL
                            OR body->>'level' = role_level
                        )
                    )
                    OR (object_type <> 'role' AND role_level IS NULL)
                )
                AND (object_type = 'work' OR responsible_role_id IS NULL)
                AND guide_document_id IS NULL
                AND source_provenance_id IS NULL
                AND source_type = 'nostr_event'
            )
            OR (
                schema_version = 3
                AND (
                    (
                        object_type = 'role'
                        AND role_level IN ('admin', 'member')
                        AND (
                            deleted_at IS NOT NULL
                            OR body->>'level' = role_level
                        )
                    )
                    OR (object_type <> 'role' AND role_level IS NULL)
                )
                AND (object_type = 'work' OR responsible_role_id IS NULL)
                AND source_provenance_id IS NOT NULL
                AND (
                    (
                        deleted_at IS NOT NULL
                        AND guide_document_id IS NULL
                    )
                    OR (
                        deleted_at IS NULL
                        AND jsonb_typeof(body->'context_references') = 'array'
                        AND (
                            (
                                object_type = 'resource'
                                AND guide_document_id IS NOT NULL
                                AND body->>'guide_document_id' = guide_document_id::text
                                AND NOT body ? 'resource_type'
                                AND NOT body ? 'locator'
                                AND NOT body ? 'description'
                            )
                            OR (
                                object_type <> 'resource'
                                AND guide_document_id IS NULL
                            )
                        )
                    )
                )
            )
        );

-- Durable fleet-wide maintenance fence. The mutable current pointer is kept
-- separate from immutable historical epochs and idempotent operation receipts.
CREATE TABLE project_view_maintenance (
    community_id UUID        NOT NULL,
    state        TEXT        NOT NULL DEFAULT 'normal',
    current_epoch BIGINT,
    updated_at   TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id),
    CONSTRAINT project_view_maintenance_community_fk
        FOREIGN KEY (community_id) REFERENCES communities (id) ON DELETE NO ACTION,
    CONSTRAINT project_view_maintenance_state_check
        CHECK (state IN ('normal', 'draining', 'frozen')),
    CONSTRAINT project_view_maintenance_epoch_shape_check
        CHECK (
            (state = 'normal' AND current_epoch IS NULL)
            OR (
                state IN ('draining', 'frozen')
                AND current_epoch BETWEEN 1 AND 9007199254740991
            )
        )
);

CREATE TABLE project_view_maintenance_epochs (
    community_id                     UUID        NOT NULL,
    maintenance_epoch                BIGINT      NOT NULL,
    base_meta_event_id               BYTEA       NOT NULL,
    base_project_revision            BIGINT      NOT NULL,
    base_projection_generation       BIGINT      NOT NULL,
    required_client_protocol_version BIGINT      NOT NULL,
    requested_by                     BYTEA       NOT NULL,
    requested_at                     TIMESTAMPTZ NOT NULL,
    begin_audit_seq                  BIGINT      NOT NULL,
    begin_idempotency_key_hash       BYTEA       NOT NULL,
    begin_request_hash               BYTEA       NOT NULL,
    begin_receipt                    JSONB       NOT NULL,
    outcome                          TEXT        NOT NULL DEFAULT 'active',
    completed_at                     TIMESTAMPTZ,
    updated_at                       TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, maintenance_epoch),
    CONSTRAINT project_view_maintenance_epochs_idempotency_unique
        UNIQUE (community_id, begin_idempotency_key_hash),
    CONSTRAINT project_view_maintenance_epochs_current_fk
        FOREIGN KEY (community_id)
        REFERENCES project_view_maintenance (community_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_epochs_audit_fk
        FOREIGN KEY (community_id, begin_audit_seq)
        REFERENCES audit_log (community_id, seq)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_epochs_range_check
        CHECK (
            maintenance_epoch BETWEEN 1 AND 9007199254740991
            AND base_project_revision BETWEEN 1 AND 9007199254740991
            AND base_projection_generation BETWEEN 1 AND 9007199254740991
            AND required_client_protocol_version BETWEEN 1 AND 9007199254740991
            AND begin_audit_seq > 0
        ),
    CONSTRAINT project_view_maintenance_epochs_bytes_check
        CHECK (
            octet_length(base_meta_event_id) = 32
            AND octet_length(requested_by) = 32
            AND octet_length(begin_idempotency_key_hash) = 32
            AND octet_length(begin_request_hash) = 32
        ),
    CONSTRAINT project_view_maintenance_epochs_outcome_check
        CHECK (outcome IN ('active', 'aborted', 'cutover_committed', 'resumed')),
    CONSTRAINT project_view_maintenance_epochs_terminal_check
        CHECK (
            (outcome = 'active' AND completed_at IS NULL)
            OR (
                outcome <> 'active'
                AND completed_at IS NOT NULL
                AND completed_at >= requested_at
            )
        ),
    CONSTRAINT project_view_maintenance_epochs_time_check
        CHECK (updated_at >= requested_at)
);

ALTER TABLE project_view_maintenance
    ADD CONSTRAINT project_view_maintenance_current_epoch_fk
        FOREIGN KEY (community_id, current_epoch)
        REFERENCES project_view_maintenance_epochs (community_id, maintenance_epoch)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE project_view_maintenance_operations (
    community_id          UUID        NOT NULL,
    maintenance_epoch     BIGINT      NOT NULL,
    operation_id          UUID        NOT NULL,
    operation             TEXT        NOT NULL,
    idempotency_key_hash  BYTEA       NOT NULL,
    canonical_request_hash BYTEA      NOT NULL,
    requested_by          BYTEA       NOT NULL,
    audit_seq             BIGINT      NOT NULL,
    result_receipt        JSONB       NOT NULL,
    accepted_at           TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, maintenance_epoch, operation_id),
    CONSTRAINT project_view_maintenance_operations_idempotency_unique
        UNIQUE (community_id, idempotency_key_hash),
    CONSTRAINT project_view_maintenance_operations_epoch_fk
        FOREIGN KEY (community_id, maintenance_epoch)
        REFERENCES project_view_maintenance_epochs (community_id, maintenance_epoch)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_operations_audit_fk
        FOREIGN KEY (community_id, audit_seq)
        REFERENCES audit_log (community_id, seq)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_operations_name_check
        CHECK (operation IN (
            'freeze', 'abort', 'cutover', 'verify', 'repair', 'reproject', 'resume'
        )),
    CONSTRAINT project_view_maintenance_operations_shape_check
        CHECK (
            operation_id <> '00000000-0000-0000-0000-000000000000'::uuid
            AND maintenance_epoch BETWEEN 1 AND 9007199254740991
            AND audit_seq > 0
            AND octet_length(idempotency_key_hash) = 32
            AND octet_length(canonical_request_hash) = 32
            AND octet_length(requested_by) = 32
        )
);

CREATE TABLE project_view_maintenance_invalidations (
    community_id                    UUID        NOT NULL,
    maintenance_epoch               BIGINT      NOT NULL,
    invalidation_id                 UUID        NOT NULL,
    phase                           TEXT        NOT NULL,
    source_type                     TEXT        NOT NULL,
    source_change_id                BYTEA,
    source_audit_seq                BIGINT,
    invalidated_at                  TIMESTAMPTZ NOT NULL,
    resolved_by_operation_id        UUID,
    resolved_meta_event_id          BYTEA,
    resolved_project_revision       BIGINT,
    resolved_projection_generation BIGINT,

    PRIMARY KEY (community_id, maintenance_epoch, invalidation_id),
    CONSTRAINT project_view_maintenance_invalidations_epoch_fk
        FOREIGN KEY (community_id, maintenance_epoch)
        REFERENCES project_view_maintenance_epochs (community_id, maintenance_epoch)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_invalidations_change_fk
        FOREIGN KEY (community_id, source_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_invalidations_audit_fk
        FOREIGN KEY (community_id, source_audit_seq)
        REFERENCES audit_log (community_id, seq)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_invalidations_operation_fk
        FOREIGN KEY (
            community_id, maintenance_epoch, resolved_by_operation_id
        )
        REFERENCES project_view_maintenance_operations (
            community_id, maintenance_epoch, operation_id
        )
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_invalidations_phase_check
        CHECK (phase IN ('pre_cutover', 'post_cutover')),
    CONSTRAINT project_view_maintenance_invalidations_source_check
        CHECK (
            (
                source_type = 'project_view_change'
                AND source_change_id IS NOT NULL
                AND octet_length(source_change_id) = 32
                AND source_audit_seq IS NULL
            )
            OR (
                source_type = 'community_audit'
                AND source_change_id IS NULL
                AND source_audit_seq > 0
            )
        ),
    CONSTRAINT project_view_maintenance_invalidations_resolution_check
        CHECK (
            (
                resolved_by_operation_id IS NULL
                AND resolved_meta_event_id IS NULL
                AND resolved_project_revision IS NULL
                AND resolved_projection_generation IS NULL
            )
            OR (
                resolved_by_operation_id IS NOT NULL
                AND octet_length(resolved_meta_event_id) = 32
                AND resolved_project_revision BETWEEN 1 AND 9007199254740991
                AND resolved_projection_generation BETWEEN 1 AND 9007199254740991
            )
        )
);

ALTER TABLE project_runtime_supervisor_bindings
    ADD CONSTRAINT project_runtime_bindings_maintenance_identity_unique
        UNIQUE (community_id, binding_id, assignment_id, supervisor_pubkey);

ALTER TABLE project_runtime_leases
    ADD CONSTRAINT project_runtime_leases_maintenance_identity_unique
        UNIQUE (
            community_id, binding_id, assignment_id, runtime_id, runtime_epoch
        );

ALTER TABLE project_role_assignments
    ADD CONSTRAINT project_role_assignments_maintenance_identity_unique
        UNIQUE (community_id, assignment_id, member_pubkey);

CREATE TABLE project_view_maintenance_assignment_baselines (
    community_id            UUID        NOT NULL,
    maintenance_epoch       BIGINT      NOT NULL,
    assignment_id           UUID        NOT NULL,
    member_pubkey            TEXT        NOT NULL,
    binding_id              UUID        NOT NULL,
    supervisor_pubkey       BYTEA       NOT NULL,
    state_at_begin          TEXT        NOT NULL,
    last_polled_at          TIMESTAMPTZ,
    client_protocol_version BIGINT,
    client_build            TEXT,

    PRIMARY KEY (community_id, maintenance_epoch, assignment_id),
    CONSTRAINT project_view_maintenance_assignment_binding_unique
        UNIQUE (community_id, maintenance_epoch, binding_id),
    CONSTRAINT project_view_maintenance_assignment_runtime_unique
        UNIQUE (
            community_id, maintenance_epoch, binding_id, assignment_id,
            supervisor_pubkey
        ),
    CONSTRAINT project_view_maintenance_assignment_identity_unique
        UNIQUE (
            community_id, maintenance_epoch, binding_id, assignment_id,
            member_pubkey, supervisor_pubkey
        ),
    CONSTRAINT project_view_maintenance_assignment_epoch_fk
        FOREIGN KEY (community_id, maintenance_epoch)
        REFERENCES project_view_maintenance_epochs (community_id, maintenance_epoch)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_assignment_binding_fk
        FOREIGN KEY (community_id, binding_id, assignment_id, supervisor_pubkey)
        REFERENCES project_runtime_supervisor_bindings (
            community_id, binding_id, assignment_id, supervisor_pubkey
        )
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_assignment_assignment_fk
        FOREIGN KEY (community_id, assignment_id, member_pubkey)
        REFERENCES project_role_assignments (
            community_id, assignment_id, member_pubkey
        )
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_assignment_shape_check
        CHECK (
            state_at_begin IN ('idle', 'has_runtime')
            AND octet_length(supervisor_pubkey) = 32
            AND member_pubkey ~ '^[0-9a-f]{64}$'
            AND (
                client_protocol_version IS NULL
                OR client_protocol_version BETWEEN 1 AND 9007199254740991
            )
            AND (client_build IS NULL OR octet_length(client_build) BETWEEN 1 AND 256)
        )
);

CREATE TABLE project_view_maintenance_runtime_baselines (
    community_id         UUID   NOT NULL,
    maintenance_epoch    BIGINT NOT NULL,
    binding_id           UUID   NOT NULL,
    assignment_id        UUID   NOT NULL,
    runtime_id           UUID   NOT NULL,
    runtime_epoch        BIGINT NOT NULL,
    supervisor_pubkey    BYTEA  NOT NULL,
    availability_at_begin TEXT  NOT NULL,

    PRIMARY KEY (
        community_id, maintenance_epoch, binding_id, assignment_id,
        runtime_id, runtime_epoch
    ),
    CONSTRAINT project_view_maintenance_runtime_identity_unique
        UNIQUE (
            community_id, maintenance_epoch, binding_id, assignment_id,
            runtime_id, runtime_epoch, supervisor_pubkey
        ),
    CONSTRAINT project_view_maintenance_runtime_epoch_fk
        FOREIGN KEY (community_id, maintenance_epoch)
        REFERENCES project_view_maintenance_epochs (community_id, maintenance_epoch)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_runtime_assignment_fk
        FOREIGN KEY (
            community_id, maintenance_epoch, binding_id, assignment_id,
            supervisor_pubkey
        )
        REFERENCES project_view_maintenance_assignment_baselines (
            community_id, maintenance_epoch, binding_id, assignment_id,
            supervisor_pubkey
        )
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_runtime_lease_fk
        FOREIGN KEY (
            community_id, binding_id, assignment_id, runtime_id, runtime_epoch
        )
        REFERENCES project_runtime_leases (
            community_id, binding_id, assignment_id, runtime_id, runtime_epoch
        )
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_runtime_shape_check
        CHECK (
            runtime_epoch BETWEEN 1 AND 9007199254740991
            AND octet_length(supervisor_pubkey) = 32
            AND availability_at_begin IN ('available', 'recovering', 'unavailable')
        )
);

CREATE TABLE project_view_maintenance_ack_requests (
    community_id          UUID        NOT NULL,
    maintenance_epoch     BIGINT      NOT NULL,
    ack_request_id        UUID        NOT NULL,
    agent_pubkey          BYTEA       NOT NULL,
    ack_type              TEXT        NOT NULL,
    idempotency_key_hash  BYTEA       NOT NULL,
    canonical_request_hash BYTEA      NOT NULL,
    auth_event_id         BYTEA       NOT NULL,
    result_receipt        JSONB       NOT NULL,
    accepted_at           TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, maintenance_epoch, ack_request_id),
    CONSTRAINT project_view_maintenance_ack_idempotency_unique
        UNIQUE (community_id, idempotency_key_hash),
    CONSTRAINT project_view_maintenance_ack_epoch_fk
        FOREIGN KEY (community_id, maintenance_epoch)
        REFERENCES project_view_maintenance_epochs (community_id, maintenance_epoch)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_ack_shape_check
        CHECK (
            ack_type IN ('assignment', 'runtime')
            AND octet_length(agent_pubkey) = 32
            AND octet_length(idempotency_key_hash) = 32
            AND octet_length(canonical_request_hash) = 32
            AND octet_length(auth_event_id) = 32
        )
);

CREATE TABLE project_view_maintenance_assignment_acks (
    community_id            UUID        NOT NULL,
    maintenance_epoch       BIGINT      NOT NULL,
    ack_request_id          UUID        NOT NULL,
    binding_id              UUID        NOT NULL,
    assignment_id           UUID        NOT NULL,
    member_pubkey            TEXT        NOT NULL,
    supervisor_pubkey       BYTEA       NOT NULL,
    status                  TEXT        NOT NULL,
    client_protocol_version BIGINT      NOT NULL,
    client_build            TEXT        NOT NULL,
    acked_at                TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, maintenance_epoch, assignment_id),
    CONSTRAINT project_view_maintenance_assignment_ack_request_unique
        UNIQUE (community_id, maintenance_epoch, ack_request_id),
    CONSTRAINT project_view_maintenance_assignment_ack_request_fk
        FOREIGN KEY (community_id, maintenance_epoch, ack_request_id)
        REFERENCES project_view_maintenance_ack_requests (
            community_id, maintenance_epoch, ack_request_id
        )
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_assignment_ack_baseline_fk
        FOREIGN KEY (
            community_id, maintenance_epoch, binding_id, assignment_id,
            member_pubkey, supervisor_pubkey
        )
        REFERENCES project_view_maintenance_assignment_baselines (
            community_id, maintenance_epoch, binding_id, assignment_id,
            member_pubkey, supervisor_pubkey
        )
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_assignment_ack_shape_check
        CHECK (
            status = 'quiesced'
            AND client_protocol_version BETWEEN 1 AND 9007199254740991
            AND octet_length(client_build) BETWEEN 1 AND 256
            AND octet_length(supervisor_pubkey) = 32
        )
);

CREATE TABLE project_view_maintenance_acks (
    community_id       UUID        NOT NULL,
    maintenance_epoch  BIGINT      NOT NULL,
    ack_request_id     UUID        NOT NULL,
    binding_id         UUID        NOT NULL,
    assignment_id      UUID        NOT NULL,
    runtime_id         UUID        NOT NULL,
    runtime_epoch      BIGINT      NOT NULL,
    supervisor_pubkey  BYTEA       NOT NULL,
    status             TEXT        NOT NULL,
    acked_at           TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (
        community_id, maintenance_epoch, binding_id, assignment_id,
        runtime_id, runtime_epoch
    ),
    CONSTRAINT project_view_maintenance_runtime_ack_request_unique
        UNIQUE (community_id, maintenance_epoch, ack_request_id),
    CONSTRAINT project_view_maintenance_runtime_ack_request_fk
        FOREIGN KEY (community_id, maintenance_epoch, ack_request_id)
        REFERENCES project_view_maintenance_ack_requests (
            community_id, maintenance_epoch, ack_request_id
        )
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_runtime_ack_baseline_fk
        FOREIGN KEY (
            community_id, maintenance_epoch, binding_id, assignment_id,
            runtime_id, runtime_epoch, supervisor_pubkey
        )
        REFERENCES project_view_maintenance_runtime_baselines (
            community_id, maintenance_epoch, binding_id, assignment_id,
            runtime_id, runtime_epoch, supervisor_pubkey
        )
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_maintenance_runtime_ack_shape_check
        CHECK (
            status IN ('suspended', 'terminal')
            AND runtime_epoch BETWEEN 1 AND 9007199254740991
            AND octet_length(supervisor_pubkey) = 32
        )
);

ALTER TABLE project_view_objects
    ADD CONSTRAINT project_view_objects_guide_document_fk
        FOREIGN KEY (community_id, guide_document_id)
        REFERENCES project_documents (community_id, document_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX idx_project_view_objects_source_change
    ON project_view_objects (community_id, source_change_id, object_id);

CREATE INDEX idx_project_view_objects_guide_document
    ON project_view_objects (community_id, guide_document_id, object_id)
    WHERE guide_document_id IS NOT NULL;

-- One immutable source record per v3 object business revision. Legacy v1
-- mutation origins and typed v2/v3 changes remain distinct closed branches.
CREATE TABLE project_view_object_provenance (
    community_id               UUID     NOT NULL,
    provenance_id              UUID     NOT NULL,
    object_id                  UUID     NOT NULL,
    object_type                TEXT     NOT NULL,
    source_type                TEXT     NOT NULL,
    source_change_id           BYTEA    NOT NULL,
    source_event_id            BYTEA,
    source_project_revision    BIGINT   NOT NULL,
    source_actor_pubkey        BYTEA,
    legacy_mutation_event_id   BYTEA,
    project_view_change_id     BYTEA,

    PRIMARY KEY (community_id, provenance_id),
    CONSTRAINT project_view_object_provenance_object_change_unique
        UNIQUE (community_id, object_id, source_change_id),
    CONSTRAINT project_view_object_provenance_object_fk
        FOREIGN KEY (community_id, object_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_object_provenance_legacy_fk
        FOREIGN KEY (community_id, legacy_mutation_event_id)
        REFERENCES project_view_mutations (community_id, event_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_object_provenance_change_fk
        FOREIGN KEY (community_id, project_view_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_object_provenance_id_check
        CHECK (provenance_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_view_object_provenance_type_check
        CHECK (object_type IN (
            'project_profile', 'goal', 'role', 'plan', 'stage',
            'requirement', 'issue', 'work', 'resource'
        )),
    CONSTRAINT project_view_object_provenance_source_type_check
        CHECK (source_type IN ('nostr_event', 'operator', 'system')),
    CONSTRAINT project_view_object_provenance_bytes_check
        CHECK (
            octet_length(source_change_id) = 32
            AND (source_event_id IS NULL OR octet_length(source_event_id) = 32)
            AND (source_actor_pubkey IS NULL OR octet_length(source_actor_pubkey) = 32)
            AND (
                legacy_mutation_event_id IS NULL
                OR octet_length(legacy_mutation_event_id) = 32
            )
            AND (
                project_view_change_id IS NULL
                OR octet_length(project_view_change_id) = 32
            )
        ),
    CONSTRAINT project_view_object_provenance_revision_check
        CHECK (source_project_revision BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_view_object_provenance_origin_check
        CHECK ((legacy_mutation_event_id IS NULL) <> (project_view_change_id IS NULL)),
    CONSTRAINT project_view_object_provenance_source_shape_check
        CHECK (
            (
                source_type = 'nostr_event'
                AND source_event_id = source_change_id
                AND source_actor_pubkey IS NOT NULL
            )
            OR (
                source_type IN ('operator', 'system')
                AND source_event_id IS NULL
            )
        )
);

ALTER TABLE project_view_objects
    ADD CONSTRAINT project_view_objects_source_provenance_fk
        FOREIGN KEY (community_id, source_provenance_id)
        REFERENCES project_view_object_provenance (community_id, provenance_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

-- Two normalized Context tables provide real same-Community foreign keys and
-- bounded reverse indexes. JSON remains a signed projection copy, never the
-- deletion-authority index.
CREATE TABLE project_view_resource_context_references (
    community_id       UUID NOT NULL,
    source_object_id   UUID NOT NULL,
    target_resource_id UUID NOT NULL,

    PRIMARY KEY (community_id, source_object_id, target_resource_id),
    CONSTRAINT project_view_resource_context_source_fk
        FOREIGN KEY (community_id, source_object_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_resource_context_target_fk
        FOREIGN KEY (community_id, target_resource_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_resource_context_distinct_check
        CHECK (source_object_id <> target_resource_id)
);

CREATE INDEX idx_project_view_resource_context_reverse
    ON project_view_resource_context_references (
        community_id, target_resource_id, source_object_id
    );

CREATE TABLE project_view_document_context_references (
    community_id             UUID     NOT NULL,
    source_object_id         UUID     NOT NULL,
    target_document_id       UUID     NOT NULL,
    reference_mode           TEXT     NOT NULL,
    target_document_revision BIGINT,
    revision_key             BIGINT GENERATED ALWAYS AS (
        COALESCE(target_document_revision, 0)
    ) STORED,

    PRIMARY KEY (
        community_id, source_object_id, target_document_id,
        reference_mode, revision_key
    ),
    CONSTRAINT project_view_document_context_source_fk
        FOREIGN KEY (community_id, source_object_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_document_context_target_fk
        FOREIGN KEY (community_id, target_document_id)
        REFERENCES project_documents (community_id, document_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_document_context_revision_fk
        FOREIGN KEY (
            community_id, target_document_id, target_document_revision
        )
        REFERENCES project_document_revisions (
            community_id, document_id, document_revision
        )
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_document_context_shape_check
        CHECK (
            (reference_mode = 'live' AND target_document_revision IS NULL)
            OR (
                reference_mode = 'pinned'
                AND target_document_revision BETWEEN 1 AND 9007199254740991
            )
        )
);

CREATE INDEX idx_project_view_document_context_reverse
    ON project_view_document_context_references (
        community_id, target_document_id, reference_mode, source_object_id
    );

CREATE INDEX idx_project_view_document_context_revision_reverse
    ON project_view_document_context_references (
        community_id, target_document_id, target_document_revision, source_object_id
    ) WHERE target_document_revision IS NOT NULL;

-- Stable review staging. This table may advance through draft/reviewed/
-- consumed; cutover authority is copied into the immutable child ledger below.
CREATE TABLE project_view_v3_resource_mappings (
    community_id                  UUID        NOT NULL,
    resource_id                   UUID        NOT NULL,
    guide_document_id             UUID        NOT NULL,
    legacy_object_revision        BIGINT      NOT NULL,
    legacy_projection_event_id    BYTEA       NOT NULL,
    legacy_body_digest            BYTEA       NOT NULL,
    guide_document_revision       BIGINT,
    guide_head_event_id           BYTEA,
    guide_revision_event_id       BYTEA,
    guide_content_digest          BYTEA,
    reviewed_v3_payload           JSONB,
    v3_payload_digest             BYTEA,
    mapping_entry_digest          BYTEA,
    reviewed_by_pubkey            BYTEA,
    reviewed_at_unix_micros       BIGINT,
    review_digest                 BYTEA,
    review_signature              BYTEA,
    manifest_digest               BYTEA,
    status                        TEXT        NOT NULL DEFAULT 'draft',
    created_at                    TIMESTAMPTZ NOT NULL,
    updated_at                    TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, resource_id),
    CONSTRAINT project_view_v3_resource_mappings_community_fk
        FOREIGN KEY (community_id) REFERENCES communities (id) ON DELETE NO ACTION,
    -- A draft deliberately pre-allocates its Guide UUID before the Human
    -- publishes that Document.  Guide existence is therefore checked when a
    -- mapping advances to `reviewed` (and again during cutover), not by an
    -- unconditional FK on the draft row.
    CONSTRAINT project_view_v3_resource_mappings_status_check
        CHECK (status IN ('draft', 'reviewed', 'consumed')),
    CONSTRAINT project_view_v3_resource_mappings_revision_check
        CHECK (
            legacy_object_revision BETWEEN 1 AND 9007199254740991
            AND (
                guide_document_revision IS NULL
                OR guide_document_revision BETWEEN 1 AND 9007199254740991
            )
        ),
    CONSTRAINT project_view_v3_resource_mappings_bytes_check
        CHECK (
            octet_length(legacy_projection_event_id) = 32
            AND octet_length(legacy_body_digest) = 32
            AND (guide_head_event_id IS NULL OR octet_length(guide_head_event_id) = 32)
            AND (
                guide_revision_event_id IS NULL
                OR octet_length(guide_revision_event_id) = 32
            )
            AND (
                guide_content_digest IS NULL
                OR octet_length(guide_content_digest) = 32
            )
            AND (v3_payload_digest IS NULL OR octet_length(v3_payload_digest) = 32)
            AND (
                mapping_entry_digest IS NULL
                OR octet_length(mapping_entry_digest) = 32
            )
            AND (reviewed_by_pubkey IS NULL OR octet_length(reviewed_by_pubkey) = 32)
            AND (review_digest IS NULL OR octet_length(review_digest) = 32)
            AND (review_signature IS NULL OR octet_length(review_signature) = 64)
            AND (manifest_digest IS NULL OR octet_length(manifest_digest) = 32)
        ),
    CONSTRAINT project_view_v3_resource_mappings_review_shape_check
        CHECK (
            status = 'draft'
            OR (
                guide_document_revision IS NOT NULL
                AND guide_head_event_id IS NOT NULL
                AND guide_revision_event_id IS NOT NULL
                AND guide_content_digest IS NOT NULL
                AND reviewed_v3_payload IS NOT NULL
                AND v3_payload_digest IS NOT NULL
                AND mapping_entry_digest IS NOT NULL
                AND reviewed_by_pubkey IS NOT NULL
                AND reviewed_at_unix_micros IS NOT NULL
                AND review_digest IS NOT NULL
                AND review_signature IS NOT NULL
                AND manifest_digest IS NOT NULL
            )
        ),
    CONSTRAINT project_view_v3_resource_mappings_time_check
        CHECK (updated_at >= created_at)
);

CREATE TABLE project_view_v3_cutovers (
    community_id                UUID        NOT NULL,
    cutover_change_id           BYTEA       NOT NULL,
    maintenance_epoch           BIGINT      NOT NULL,
    idempotency_key_hash        BYTEA       NOT NULL,
    canonical_request_hash      BYTEA       NOT NULL,
    manifest_digest             BYTEA       NOT NULL,
    manifest_entry_count        INTEGER     NOT NULL,
    base_meta_event_id          BYTEA       NOT NULL,
    base_project_revision       BIGINT      NOT NULL,
    base_projection_generation BIGINT      NOT NULL,
    target_schema_version       SMALLINT    NOT NULL,
    result_receipt              JSONB       NOT NULL,
    accepted_at                 TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, cutover_change_id),
    CONSTRAINT project_view_v3_cutovers_idempotency_unique
        UNIQUE (community_id, idempotency_key_hash),
    CONSTRAINT project_view_v3_cutovers_change_fk
        FOREIGN KEY (community_id, cutover_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_v3_cutovers_bytes_check
        CHECK (
            octet_length(cutover_change_id) = 32
            AND octet_length(idempotency_key_hash) = 32
            AND octet_length(canonical_request_hash) = 32
            AND octet_length(manifest_digest) = 32
            AND octet_length(base_meta_event_id) = 32
        ),
    CONSTRAINT project_view_v3_cutovers_shape_check
        CHECK (
            maintenance_epoch BETWEEN 1 AND 9007199254740991
            AND base_project_revision BETWEEN 1 AND 9007199254740991
            AND base_projection_generation BETWEEN 1 AND 9007199254740991
            AND manifest_entry_count BETWEEN 0 AND 4096
            AND target_schema_version = 3
        )
);

ALTER TABLE project_view_v3_cutovers
    ADD CONSTRAINT project_view_v3_cutovers_maintenance_fk
        FOREIGN KEY (community_id, maintenance_epoch)
        REFERENCES project_view_maintenance_epochs (community_id, maintenance_epoch)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE project_view_v3_committed_resource_entries (
    community_id                 UUID        NOT NULL,
    cutover_change_id            BYTEA       NOT NULL,
    resource_id                  UUID        NOT NULL,
    guide_document_id            UUID        NOT NULL,
    legacy_object_revision       BIGINT      NOT NULL,
    legacy_projection_event_id   BYTEA       NOT NULL,
    legacy_body_digest           BYTEA       NOT NULL,
    mapping_entry_digest         BYTEA       NOT NULL,
    reviewed_v3_payload          JSONB       NOT NULL,
    v3_payload_digest            BYTEA       NOT NULL,
    guide_document_revision      BIGINT      NOT NULL,
    guide_head_event_id          BYTEA       NOT NULL,
    guide_revision_event_id      BYTEA       NOT NULL,
    guide_content_digest         BYTEA       NOT NULL,
    reviewed_by_pubkey           BYTEA       NOT NULL,
    reviewed_at_unix_micros      BIGINT      NOT NULL,
    review_digest                BYTEA       NOT NULL,
    review_signature             BYTEA       NOT NULL,
    committed_at                 TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, cutover_change_id, resource_id),
    CONSTRAINT project_view_v3_committed_resource_cutover_fk
        FOREIGN KEY (community_id, cutover_change_id)
        REFERENCES project_view_v3_cutovers (community_id, cutover_change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_v3_committed_resource_object_fk
        FOREIGN KEY (community_id, resource_id)
        REFERENCES project_view_objects (community_id, object_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_v3_committed_resource_guide_fk
        FOREIGN KEY (community_id, guide_document_id, guide_document_revision)
        REFERENCES project_document_revisions (
            community_id, document_id, document_revision
        )
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_v3_committed_resource_revision_check
        CHECK (
            legacy_object_revision BETWEEN 1 AND 9007199254740991
            AND guide_document_revision BETWEEN 1 AND 9007199254740991
        ),
    CONSTRAINT project_view_v3_committed_resource_bytes_check
        CHECK (
            octet_length(cutover_change_id) = 32
            AND octet_length(legacy_projection_event_id) = 32
            AND octet_length(legacy_body_digest) = 32
            AND octet_length(mapping_entry_digest) = 32
            AND octet_length(v3_payload_digest) = 32
            AND octet_length(guide_head_event_id) = 32
            AND octet_length(guide_revision_event_id) = 32
            AND octet_length(guide_content_digest) = 32
            AND octet_length(reviewed_by_pubkey) = 32
            AND octet_length(review_digest) = 32
            AND octet_length(review_signature) = 64
        )
);

CREATE INDEX idx_project_view_v3_committed_resource_mapping
    ON project_view_v3_committed_resource_entries (
        community_id, mapping_entry_digest, resource_id
    );

-- Empty-state explicit schema-v3 preparation exists outside Project View
-- state so it can safely describe an uninitialized Community.
CREATE TABLE project_view_provisioning_operations (
    community_id          UUID        NOT NULL,
    operation_id          UUID        NOT NULL,
    operation             TEXT        NOT NULL,
    target_schema_version SMALLINT    NOT NULL,
    idempotency_key_hash  BYTEA       NOT NULL,
    canonical_request_hash BYTEA      NOT NULL,
    requested_by          BYTEA       NOT NULL,
    audit_seq             BIGINT      NOT NULL,
    result_receipt        JSONB       NOT NULL,
    accepted_at           TIMESTAMPTZ NOT NULL,
    consumed_by_change_id BYTEA,
    consumed_at           TIMESTAMPTZ,

    PRIMARY KEY (community_id, operation_id),
    CONSTRAINT project_view_provisioning_idempotency_unique
        UNIQUE (community_id, idempotency_key_hash),
    CONSTRAINT project_view_provisioning_community_fk
        FOREIGN KEY (community_id) REFERENCES communities (id) ON DELETE NO ACTION,
    CONSTRAINT project_view_provisioning_audit_fk
        FOREIGN KEY (community_id, audit_seq)
        REFERENCES audit_log (community_id, seq)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_provisioning_consumed_change_fk
        FOREIGN KEY (community_id, consumed_by_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_provisioning_shape_check
        CHECK (
            operation = 'prepare_v3'
            AND target_schema_version = 3
            AND audit_seq > 0
            AND octet_length(idempotency_key_hash) = 32
            AND octet_length(canonical_request_hash) = 32
            AND octet_length(requested_by) = 32
            AND (consumed_by_change_id IS NULL OR octet_length(consumed_by_change_id) = 32)
            AND ((consumed_by_change_id IS NULL) = (consumed_at IS NULL))
        )
);

ALTER TABLE communities
    ADD CONSTRAINT communities_project_view_preparation_fk
        FOREIGN KEY (id, project_view_preparation_operation_id)
        REFERENCES project_view_provisioning_operations (community_id, operation_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT communities_project_view_preparation_shape_check
        CHECK (
            project_view_preparation_operation_id IS NULL
            OR (
                project_view_schema_version = 3
                AND NOT project_view_enabled
                AND NOT project_context_enabled
            )
        );

-- Evidence and receipt ledgers are append-only.  Their foreign keys are not a
-- sufficient integrity boundary: an UPDATE could otherwise rewrite the
-- provenance of an unchanged current object or maintenance decision.
CREATE FUNCTION project_view_v3_reject_ledger_mutation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% is append-only', TG_TABLE_NAME
        USING ERRCODE = 'check_violation';
END
$$;

CREATE TRIGGER project_view_mutations_v3_immutable
    BEFORE UPDATE OR DELETE ON project_view_mutations
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();

CREATE TRIGGER project_view_changes_v3_immutable
    BEFORE UPDATE OR DELETE ON project_view_changes
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();

CREATE TRIGGER project_view_object_provenance_immutable
    BEFORE UPDATE OR DELETE ON project_view_object_provenance
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();

CREATE TRIGGER project_view_v3_cutovers_immutable
    BEFORE UPDATE OR DELETE ON project_view_v3_cutovers
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();

CREATE TRIGGER project_view_v3_committed_resources_immutable
    BEFORE UPDATE OR DELETE ON project_view_v3_committed_resource_entries
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();

CREATE TRIGGER project_view_maintenance_operations_immutable
    BEFORE UPDATE OR DELETE ON project_view_maintenance_operations
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();

CREATE TRIGGER project_view_maintenance_ack_requests_immutable
    BEFORE UPDATE OR DELETE ON project_view_maintenance_ack_requests
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();

CREATE TRIGGER project_view_maintenance_assignment_acks_immutable
    BEFORE UPDATE OR DELETE ON project_view_maintenance_assignment_acks
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();

CREATE TRIGGER project_view_maintenance_runtime_baselines_immutable
    BEFORE UPDATE OR DELETE ON project_view_maintenance_runtime_baselines
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();

CREATE TRIGGER project_view_maintenance_runtime_acks_immutable
    BEFORE UPDATE OR DELETE ON project_view_maintenance_acks
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();

CREATE FUNCTION project_view_maintenance_epoch_monotonic() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Project View maintenance epochs are append-only'
            USING ERRCODE = 'check_violation';
    END IF;
    IF ROW(OLD.community_id, OLD.maintenance_epoch, OLD.base_meta_event_id,
           OLD.base_project_revision, OLD.base_projection_generation,
           OLD.required_client_protocol_version, OLD.requested_by,
           OLD.requested_at, OLD.begin_audit_seq,
           OLD.begin_idempotency_key_hash, OLD.begin_request_hash,
           OLD.begin_receipt)
       IS DISTINCT FROM
       ROW(NEW.community_id, NEW.maintenance_epoch, NEW.base_meta_event_id,
           NEW.base_project_revision, NEW.base_projection_generation,
           NEW.required_client_protocol_version, NEW.requested_by,
           NEW.requested_at, NEW.begin_audit_seq,
           NEW.begin_idempotency_key_hash, NEW.begin_request_hash,
           NEW.begin_receipt)
       OR NOT (
           (OLD.outcome = 'active' AND NEW.outcome IN ('aborted', 'cutover_committed'))
           OR (OLD.outcome = 'cutover_committed' AND NEW.outcome = 'resumed')
       )
       OR NEW.completed_at IS NULL
       OR NEW.updated_at < OLD.updated_at THEN
        RAISE EXCEPTION 'Project View maintenance epoch may only advance to a valid terminal outcome'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER project_view_maintenance_epochs_monotonic
    BEFORE UPDATE OR DELETE ON project_view_maintenance_epochs
    FOR EACH ROW EXECUTE FUNCTION project_view_maintenance_epoch_monotonic();

CREATE FUNCTION project_view_maintenance_assignment_diagnostics_monotonic()
RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Project View maintenance Assignment baselines are append-only'
            USING ERRCODE = 'check_violation';
    END IF;
    IF ROW(OLD.community_id, OLD.maintenance_epoch, OLD.assignment_id,
           OLD.member_pubkey, OLD.binding_id, OLD.supervisor_pubkey,
           OLD.state_at_begin)
       IS DISTINCT FROM
       ROW(NEW.community_id, NEW.maintenance_epoch, NEW.assignment_id,
           NEW.member_pubkey, NEW.binding_id, NEW.supervisor_pubkey,
           NEW.state_at_begin)
       OR (OLD.last_polled_at IS NOT NULL AND
           (NEW.last_polled_at IS NULL OR NEW.last_polled_at < OLD.last_polled_at))
       OR (OLD.client_protocol_version IS NOT NULL AND
           (NEW.client_protocol_version IS NULL OR
            NEW.client_protocol_version < OLD.client_protocol_version)) THEN
        RAISE EXCEPTION 'Project View maintenance Assignment diagnostics must advance monotonically'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER project_view_maintenance_assignment_baselines_monotonic
    BEFORE UPDATE OR DELETE ON project_view_maintenance_assignment_baselines
    FOR EACH ROW
    EXECUTE FUNCTION project_view_maintenance_assignment_diagnostics_monotonic();

CREATE FUNCTION project_view_maintenance_invalidation_monotonic() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Project View maintenance invalidations are append-only'
            USING ERRCODE = 'check_violation';
    END IF;
    IF ROW(OLD.community_id, OLD.maintenance_epoch, OLD.invalidation_id,
           OLD.phase, OLD.source_type, OLD.source_change_id,
           OLD.source_audit_seq, OLD.invalidated_at)
       IS DISTINCT FROM
       ROW(NEW.community_id, NEW.maintenance_epoch, NEW.invalidation_id,
           NEW.phase, NEW.source_type, NEW.source_change_id,
           NEW.source_audit_seq, NEW.invalidated_at)
       OR OLD.resolved_by_operation_id IS NOT NULL
       OR NEW.resolved_by_operation_id IS NULL
       OR NEW.resolved_meta_event_id IS NULL
       OR NEW.resolved_project_revision IS NULL
       OR NEW.resolved_projection_generation IS NULL THEN
        RAISE EXCEPTION 'Project View maintenance invalidation may be resolved exactly once'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER project_view_maintenance_invalidations_monotonic
    BEFORE UPDATE OR DELETE ON project_view_maintenance_invalidations
    FOR EACH ROW EXECUTE FUNCTION project_view_maintenance_invalidation_monotonic();

CREATE FUNCTION project_view_provisioning_monotonic() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Project View provisioning operations are append-only'
            USING ERRCODE = 'check_violation';
    END IF;
    IF ROW(OLD.community_id, OLD.operation_id, OLD.operation,
           OLD.target_schema_version, OLD.idempotency_key_hash,
           OLD.canonical_request_hash, OLD.requested_by, OLD.audit_seq,
           OLD.result_receipt, OLD.accepted_at)
       IS DISTINCT FROM
       ROW(NEW.community_id, NEW.operation_id, NEW.operation,
           NEW.target_schema_version, NEW.idempotency_key_hash,
           NEW.canonical_request_hash, NEW.requested_by, NEW.audit_seq,
           NEW.result_receipt, NEW.accepted_at)
       OR OLD.consumed_by_change_id IS NOT NULL
       OR NEW.consumed_by_change_id IS NULL
       OR NEW.consumed_at IS NULL THEN
        RAISE EXCEPTION 'Project View provisioning operation may be consumed exactly once'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER project_view_provisioning_operations_monotonic
    BEFORE UPDATE OR DELETE ON project_view_provisioning_operations
    FOR EACH ROW EXECUTE FUNCTION project_view_provisioning_monotonic();

CREATE FUNCTION project_view_v3_resource_mapping_monotonic() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Project View v3 Resource mappings cannot be deleted'
            USING ERRCODE = 'check_violation';
    END IF;
    IF ROW(OLD.community_id, OLD.resource_id, OLD.guide_document_id,
           OLD.created_at)
       IS DISTINCT FROM
       ROW(NEW.community_id, NEW.resource_id, NEW.guide_document_id,
           NEW.created_at)
       OR NEW.updated_at < OLD.updated_at
       OR (
           NOT (OLD.status = 'draft' AND NEW.status = 'draft')
           AND ROW(OLD.legacy_object_revision,
                   OLD.legacy_projection_event_id,
                   OLD.legacy_body_digest)
               IS DISTINCT FROM
               ROW(NEW.legacy_object_revision,
                   NEW.legacy_projection_event_id,
                   NEW.legacy_body_digest)
       )
       OR NOT (
           (OLD.status = 'draft' AND NEW.status = 'draft' AND
               NEW.guide_document_revision IS NULL AND
               NEW.guide_head_event_id IS NULL AND
               NEW.guide_revision_event_id IS NULL AND
               NEW.guide_content_digest IS NULL AND
               NEW.reviewed_v3_payload IS NULL AND
               NEW.v3_payload_digest IS NULL AND
               NEW.mapping_entry_digest IS NULL AND
               NEW.reviewed_by_pubkey IS NULL AND
               NEW.reviewed_at_unix_micros IS NULL AND
               NEW.review_digest IS NULL AND
               NEW.review_signature IS NULL AND
               NEW.manifest_digest IS NULL)
           OR (OLD.status = 'draft' AND NEW.status = 'reviewed')
           OR (OLD.status = 'reviewed' AND NEW.status = 'reviewed' AND
               ROW(OLD.guide_document_revision, OLD.guide_head_event_id,
                   OLD.guide_revision_event_id, OLD.guide_content_digest,
                   OLD.reviewed_v3_payload, OLD.v3_payload_digest,
                   OLD.mapping_entry_digest, OLD.reviewed_by_pubkey,
                   OLD.reviewed_at_unix_micros, OLD.review_digest,
                   OLD.review_signature, OLD.manifest_digest)
               IS NOT DISTINCT FROM
               ROW(NEW.guide_document_revision, NEW.guide_head_event_id,
                   NEW.guide_revision_event_id, NEW.guide_content_digest,
                   NEW.reviewed_v3_payload, NEW.v3_payload_digest,
                   NEW.mapping_entry_digest, NEW.reviewed_by_pubkey,
                   NEW.reviewed_at_unix_micros, NEW.review_digest,
                   NEW.review_signature, NEW.manifest_digest))
           OR (OLD.status = 'reviewed' AND NEW.status = 'consumed' AND
               ROW(OLD.guide_document_revision, OLD.guide_head_event_id,
                   OLD.guide_revision_event_id, OLD.guide_content_digest,
                   OLD.reviewed_v3_payload, OLD.v3_payload_digest,
                   OLD.mapping_entry_digest, OLD.reviewed_by_pubkey,
                   OLD.reviewed_at_unix_micros, OLD.review_digest,
                   OLD.review_signature, OLD.manifest_digest)
               IS NOT DISTINCT FROM
               ROW(NEW.guide_document_revision, NEW.guide_head_event_id,
                   NEW.guide_revision_event_id, NEW.guide_content_digest,
                   NEW.reviewed_v3_payload, NEW.v3_payload_digest,
                   NEW.mapping_entry_digest, NEW.reviewed_by_pubkey,
                   NEW.reviewed_at_unix_micros, NEW.review_digest,
                   NEW.review_signature, NEW.manifest_digest))
       ) THEN
        RAISE EXCEPTION 'Project View v3 Resource mapping transition is not monotonic'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER project_view_v3_resource_mappings_monotonic
    BEFORE UPDATE OR DELETE ON project_view_v3_resource_mappings
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_resource_mapping_monotonic();

INSERT INTO project_view_maintenance (community_id, state, current_epoch, updated_at)
SELECT id, 'normal', NULL, clock_timestamp()
FROM communities;

CREATE FUNCTION project_view_maintenance_seed() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO project_view_maintenance (
        community_id, state, current_epoch, updated_at
    ) VALUES (NEW.id, 'normal', NULL, clock_timestamp());
    RETURN NEW;
END
$$;

CREATE TRIGGER communities_project_view_maintenance_seed
    AFTER INSERT ON communities
    FOR EACH ROW EXECUTE FUNCTION project_view_maintenance_seed();

-- Full v3 canonical parity. This is independent of Rust validation and runs
-- at transaction end so object rows, provenance, Guides, and normalized
-- references can be replaced atomically under the Community lock.
CREATE FUNCTION project_view_v3_validate_community(target_community UUID)
RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    target_schema SMALLINT;
    state_schema SMALLINT;
BEGIN
    SELECT project_view_schema_version
    INTO target_schema
    FROM communities
    WHERE id = target_community;

    IF NOT FOUND OR target_schema <> 3 THEN
        RETURN;
    END IF;

    SELECT schema_version
    INTO state_schema
    FROM project_view_state
    WHERE community_id = target_community;

    IF NOT FOUND OR state_schema <> 3 THEN
        RAISE EXCEPTION 'Project View v3 state missing for community %', target_community
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_view_objects object
        LEFT JOIN project_view_object_provenance provenance
          ON provenance.community_id = object.community_id
         AND provenance.provenance_id = object.source_provenance_id
        WHERE object.community_id = target_community
          AND (
              object.schema_version <> 3
              OR provenance.provenance_id IS NULL
              OR provenance.object_id <> object.object_id
              OR provenance.object_type <> object.object_type
              OR provenance.source_type <> object.source_type
              OR provenance.source_change_id <> object.source_change_id
              OR provenance.source_event_id IS DISTINCT FROM object.source_event_id
              OR provenance.source_project_revision <> object.project_revision
              OR (
                  object.source_type = 'nostr_event'
                  AND provenance.source_actor_pubkey IS DISTINCT FROM object.updated_by
              )
          )
    ) THEN
        RAISE EXCEPTION 'Project View v3 object/provenance parity failed'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_view_object_provenance provenance
        LEFT JOIN project_view_mutations legacy
          ON legacy.community_id = provenance.community_id
         AND legacy.event_id = provenance.legacy_mutation_event_id
        LEFT JOIN project_view_changes change
          ON change.community_id = provenance.community_id
         AND change.change_id = provenance.project_view_change_id
        WHERE provenance.community_id = target_community
          AND (
              (
                  provenance.legacy_mutation_event_id IS NOT NULL
                  AND (
                      legacy.event_id IS NULL
                      OR provenance.source_type <> 'nostr_event'
                      OR provenance.source_change_id <> legacy.event_id
                      OR provenance.source_actor_pubkey <> legacy.actor_pubkey
                  )
              )
              OR (
                  provenance.project_view_change_id IS NOT NULL
                  AND (
                      change.change_id IS NULL
                      OR provenance.source_type <> change.source_type
                      OR provenance.source_change_id <> change.change_id
                      OR provenance.source_event_id IS DISTINCT FROM change.source_event_id
                      OR provenance.source_actor_pubkey IS DISTINCT FROM change.actor_pubkey
                      OR provenance.source_project_revision <> change.project_revision
                  )
              )
          )
    ) THEN
        RAISE EXCEPTION 'Project View v3 provenance origin parity failed'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_view_objects resource
        LEFT JOIN project_documents guide
          ON guide.community_id = resource.community_id
         AND guide.document_id = resource.guide_document_id
        WHERE resource.community_id = target_community
          AND resource.schema_version = 3
          AND resource.object_type = 'resource'
          AND resource.deleted_at IS NULL
          AND (
              guide.document_id IS NULL
              OR guide.state <> 'active'
              OR resource.body->>'guide_document_id' <> resource.guide_document_id::text
          )
    ) THEN
        RAISE EXCEPTION 'Project View v3 Resource Guide target is not active'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_view_resource_context_references reference
        LEFT JOIN project_view_objects source
          ON source.community_id = reference.community_id
         AND source.object_id = reference.source_object_id
        LEFT JOIN project_view_objects target
          ON target.community_id = reference.community_id
         AND target.object_id = reference.target_resource_id
        WHERE reference.community_id = target_community
          AND (
              source.object_id IS NULL
              OR source.schema_version <> 3
              OR source.deleted_at IS NOT NULL
              OR source.object_type = 'resource'
              OR target.object_id IS NULL
              OR target.schema_version <> 3
              OR target.deleted_at IS NOT NULL
              OR target.object_type <> 'resource'
          )
    ) THEN
        RAISE EXCEPTION 'Project View v3 normalized Resource Context target is invalid'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_view_document_context_references reference
        LEFT JOIN project_view_objects source
          ON source.community_id = reference.community_id
         AND source.object_id = reference.source_object_id
        LEFT JOIN project_documents document
          ON document.community_id = reference.community_id
         AND document.document_id = reference.target_document_id
        LEFT JOIN project_document_revisions revision
          ON revision.community_id = reference.community_id
         AND revision.document_id = reference.target_document_id
         AND revision.document_revision = reference.target_document_revision
        WHERE reference.community_id = target_community
          AND (
              source.object_id IS NULL
              OR source.schema_version <> 3
              OR source.deleted_at IS NOT NULL
              OR document.document_id IS NULL
              OR (
                  reference.reference_mode = 'live'
                  AND document.state <> 'active'
              )
              OR (
                  reference.reference_mode = 'pinned'
                  AND (revision.document_revision IS NULL OR revision.state <> 'active')
              )
          )
    ) THEN
        RAISE EXCEPTION 'Project View v3 normalized Document Context target is invalid'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_view_objects source
        WHERE source.community_id = target_community
          AND source.schema_version = 3
          AND (
              (
                  source.deleted_at IS NOT NULL
                  AND (
                      EXISTS (
                          SELECT 1
                          FROM project_view_resource_context_references reference
                          WHERE reference.community_id = source.community_id
                            AND reference.source_object_id = source.object_id
                      )
                      OR EXISTS (
                          SELECT 1
                          FROM project_view_document_context_references reference
                          WHERE reference.community_id = source.community_id
                            AND reference.source_object_id = source.object_id
                      )
                  )
              )
              OR (
                  source.deleted_at IS NULL
                  AND (
                      SELECT count(*)
                      FROM (
                          SELECT 1
                          FROM project_view_resource_context_references reference
                          WHERE reference.community_id = source.community_id
                            AND reference.source_object_id = source.object_id
                          UNION ALL
                          SELECT 1
                          FROM project_view_document_context_references reference
                          WHERE reference.community_id = source.community_id
                            AND reference.source_object_id = source.object_id
                      ) coordinates
                  ) > 64
              )
              OR (
                  source.deleted_at IS NULL
                  AND source.body->'context_references' IS DISTINCT FROM (
                      SELECT COALESCE(
                          jsonb_agg(coordinate.payload ORDER BY
                              coordinate.kind_order,
                              coordinate.target_id,
                              coordinate.mode_order,
                              coordinate.revision_key
                          ),
                          '[]'::jsonb
                      )
                      FROM (
                          SELECT
                              0 AS kind_order,
                              reference.target_resource_id AS target_id,
                              0 AS mode_order,
                              0::bigint AS revision_key,
                              jsonb_build_object(
                                  'type', 'resource',
                                  'resource_id', reference.target_resource_id
                              ) AS payload
                          FROM project_view_resource_context_references reference
                          WHERE reference.community_id = source.community_id
                            AND reference.source_object_id = source.object_id
                          UNION ALL
                          SELECT
                              1 AS kind_order,
                              reference.target_document_id AS target_id,
                              CASE WHEN reference.reference_mode = 'live' THEN 0 ELSE 1 END,
                              reference.revision_key,
                              CASE
                                  WHEN reference.reference_mode = 'live' THEN
                                      jsonb_build_object(
                                          'type', 'document',
                                          'document_id', reference.target_document_id,
                                          'mode', 'live'
                                      )
                                  ELSE
                                      jsonb_build_object(
                                          'type', 'document',
                                          'document_id', reference.target_document_id,
                                          'mode', 'pinned',
                                          'document_revision', reference.target_document_revision
                                      )
                              END AS payload
                          FROM project_view_document_context_references reference
                          WHERE reference.community_id = source.community_id
                            AND reference.source_object_id = source.object_id
                      ) coordinate
                  )
              )
          )
    ) THEN
        RAISE EXCEPTION 'Project View v3 Context body/normalized parity failed'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_view_v3_committed_resource_entries committed
        LEFT JOIN project_view_objects resource
          ON resource.community_id = committed.community_id
         AND resource.object_id = committed.resource_id
        LEFT JOIN project_view_object_provenance provenance
          ON provenance.community_id = committed.community_id
         AND provenance.object_id = committed.resource_id
         AND provenance.source_change_id = committed.cutover_change_id
        WHERE committed.community_id = target_community
          AND (
              resource.object_id IS NULL
              OR resource.object_type <> 'resource'
              OR resource.schema_version <> 3
              OR provenance.provenance_id IS NULL
              OR provenance.object_type <> 'resource'
              OR provenance.source_type <> 'operator'
              OR provenance.project_view_change_id <> committed.cutover_change_id
          )
    ) THEN
        RAISE EXCEPTION 'Project View v3 committed Resource attribution failed'
            USING ERRCODE = 'check_violation';
    END IF;
END
$$;

CREATE FUNCTION project_view_v3_validate_row() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    target_community UUID;
BEGIN
    IF TG_OP = 'DELETE' THEN
        target_community := OLD.community_id;
    ELSE
        target_community := NEW.community_id;
    END IF;
    PERFORM project_view_v3_validate_community(target_community);
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER project_view_objects_v3_validate
    AFTER INSERT OR UPDATE ON project_view_objects
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_validate_row();

CREATE CONSTRAINT TRIGGER project_view_provenance_v3_validate
    AFTER INSERT OR UPDATE ON project_view_object_provenance
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_validate_row();

CREATE CONSTRAINT TRIGGER project_view_resource_context_v3_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_view_resource_context_references
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_validate_row();

CREATE CONSTRAINT TRIGGER project_view_document_context_v3_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_view_document_context_references
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_validate_row();

CREATE CONSTRAINT TRIGGER project_view_committed_resource_v3_validate
    AFTER INSERT OR UPDATE ON project_view_v3_committed_resource_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_validate_row();

CREATE CONSTRAINT TRIGGER project_documents_project_view_v3_validate
    AFTER INSERT OR UPDATE ON project_documents
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_validate_row();

CREATE CONSTRAINT TRIGGER project_document_revisions_project_view_v3_validate
    AFTER INSERT OR UPDATE ON project_document_revisions
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_validate_row();

-- Every unified maintenance acknowledgement has exactly one child of the
-- declared variant; neither child may exist without its immutable request.
CREATE FUNCTION project_view_maintenance_validate_ack_request(
    target_community UUID,
    target_epoch BIGINT,
    target_request UUID
) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    requested_type TEXT;
    assignment_count INTEGER;
    runtime_count INTEGER;
BEGIN
    SELECT ack_type INTO requested_type
    FROM project_view_maintenance_ack_requests
    WHERE community_id = target_community
      AND maintenance_epoch = target_epoch
      AND ack_request_id = target_request;

    IF NOT FOUND THEN
        RETURN;
    END IF;

    SELECT count(*)::integer INTO assignment_count
    FROM project_view_maintenance_assignment_acks
    WHERE community_id = target_community
      AND maintenance_epoch = target_epoch
      AND ack_request_id = target_request;

    SELECT count(*)::integer INTO runtime_count
    FROM project_view_maintenance_acks
    WHERE community_id = target_community
      AND maintenance_epoch = target_epoch
      AND ack_request_id = target_request;

    IF (requested_type = 'assignment' AND assignment_count <> 1)
       OR (requested_type = 'assignment' AND runtime_count <> 0)
       OR (requested_type = 'runtime' AND runtime_count <> 1)
       OR (requested_type = 'runtime' AND assignment_count <> 0) THEN
        RAISE EXCEPTION 'maintenance acknowledgement request/child parity failed'
            USING ERRCODE = 'check_violation';
    END IF;
END
$$;

CREATE FUNCTION project_view_maintenance_validate_ack_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM project_view_maintenance_validate_ack_request(
        COALESCE(NEW.community_id, OLD.community_id),
        COALESCE(NEW.maintenance_epoch, OLD.maintenance_epoch),
        COALESCE(NEW.ack_request_id, OLD.ack_request_id)
    );
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER project_view_maintenance_ack_request_validate
    AFTER INSERT OR UPDATE ON project_view_maintenance_ack_requests
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_view_maintenance_validate_ack_row();

CREATE CONSTRAINT TRIGGER project_view_maintenance_assignment_ack_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_view_maintenance_assignment_acks
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_view_maintenance_validate_ack_row();

CREATE CONSTRAINT TRIGGER project_view_maintenance_runtime_ack_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_view_maintenance_acks
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION project_view_maintenance_validate_ack_row();
-- Role-continuity invariants apply unchanged to schema v2 and v3.

CREATE OR REPLACE FUNCTION project_role_continuity_validate_community(target_community UUID)
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

    IF NOT FOUND OR target_schema NOT IN (2, 3) THEN
        RETURN;
    END IF;

    SELECT schema_version
    INTO state_schema
    FROM project_view_state
    WHERE community_id = target_community;

    IF NOT FOUND OR state_schema IS DISTINCT FROM target_schema THEN
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
          AND object.schema_version IS DISTINCT FROM target_schema
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
              OR role_object.schema_version IS DISTINCT FROM target_schema
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

CREATE OR REPLACE FUNCTION project_role_continuity_validate_counts(target_community UUID)
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

    IF NOT FOUND OR target_schema NOT IN (2, 3) THEN
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

CREATE OR REPLACE FUNCTION project_work_commitments_validate_stage5_community(
    target_community UUID
) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM communities
        WHERE id = target_community
          AND project_view_schema_version IN (2, 3)
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

CREATE OR REPLACE FUNCTION project_role_history_validate_stage6_community(
    target_community UUID
) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM communities
        WHERE id = target_community
          AND project_view_schema_version IN (2, 3)
    ) THEN
        RETURN;
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_role_checkpoints checkpoint
        LEFT JOIN project_role_assignments assignment
          ON assignment.community_id = checkpoint.community_id
         AND assignment.assignment_id = checkpoint.assignment_id
        LEFT JOIN project_role_checkpoints superseded
          ON superseded.community_id = checkpoint.community_id
         AND superseded.checkpoint_id = checkpoint.supersedes_checkpoint_id
        LEFT JOIN project_view_changes source_change
          ON source_change.community_id = checkpoint.community_id
         AND source_change.change_id = checkpoint.source_change_id
        WHERE checkpoint.community_id = target_community
          AND (
              assignment.assignment_id IS NULL
              OR assignment.role_id <> checkpoint.role_id
              OR checkpoint.created_by IS DISTINCT FROM
                 decode(assignment.member_pubkey, 'hex')
              OR source_change.project_revision IS DISTINCT FROM
                 checkpoint.project_revision
              OR source_change.actor_pubkey IS DISTINCT FROM checkpoint.created_by
              OR source_change.operation IS DISTINCT FROM 'append_checkpoint'
              OR (
                  assignment.ended_at IS NOT NULL
                  AND checkpoint.project_revision >= assignment.project_revision
              )
              OR checkpoint.based_on_project_revision >= checkpoint.project_revision
              OR (
                  checkpoint.supersedes_checkpoint_id IS NOT NULL
                  AND (
                      superseded.checkpoint_id IS NULL
                      OR superseded.role_id <> checkpoint.role_id
                      OR superseded.assignment_id <> checkpoint.assignment_id
                      OR superseded.project_revision >= checkpoint.project_revision
                  )
              )
          )
    ) THEN
        RAISE EXCEPTION 'Role Checkpoint attribution or revision basis is inconsistent'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_role_handoffs handoff
        LEFT JOIN project_role_assignments source_assignment
          ON source_assignment.community_id = handoff.community_id
         AND source_assignment.assignment_id = handoff.from_assignment_id
        LEFT JOIN project_role_assignments target_assignment
          ON target_assignment.community_id = handoff.community_id
         AND target_assignment.assignment_id = handoff.to_assignment_id
        LEFT JOIN project_role_checkpoints checkpoint
          ON checkpoint.community_id = handoff.community_id
         AND checkpoint.checkpoint_id = handoff.checkpoint_id
        LEFT JOIN project_view_changes source_change
          ON source_change.community_id = handoff.community_id
         AND source_change.change_id = handoff.source_change_id
        WHERE handoff.community_id = target_community
          AND (
              source_assignment.assignment_id IS NULL
              OR source_assignment.role_id <> handoff.role_id
              OR source_change.project_revision IS DISTINCT FROM
                 handoff.project_revision
              OR (
                  handoff.to_assignment_id IS NOT NULL
                  AND (
                      target_assignment.assignment_id IS NULL
                      OR target_assignment.role_id <> handoff.role_id
                  )
              )
              OR (
                  handoff.checkpoint_id IS NOT NULL
                  AND (
                      checkpoint.checkpoint_id IS NULL
                      OR checkpoint.role_id <> handoff.role_id
                      OR checkpoint.assignment_id <> handoff.from_assignment_id
                  )
              )
              OR handoff.body->>'cause' IS NULL
              OR handoff.body->>'cause' NOT IN (
                  'planned',
                  'revoked',
                  'replaced',
                  'membership_ended',
                  'unrecoverable',
                  'role_deactivated',
                  'other'
              )
              OR (
                  NOT handoff.system_generated
                  AND (
                      handoff.created_by IS DISTINCT FROM
                          decode(source_assignment.member_pubkey, 'hex')
                      OR source_change.actor_pubkey IS DISTINCT FROM
                         handoff.created_by
                      OR source_change.operation IS DISTINCT FROM 'append_handoff'
                      OR handoff.body->>'cause' NOT IN ('planned', 'other')
                      OR (
                          source_assignment.ended_at IS NOT NULL
                          AND handoff.project_revision >=
                              source_assignment.project_revision
                      )
                  )
              )
          )
    ) THEN
        RAISE EXCEPTION 'Role Handoff attribution, cause, or target is inconsistent'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_role_handoffs handoff
        CROSS JOIN LATERAL jsonb_array_elements_text(
            COALESCE(handoff.body->'affected_commitment_ids', '[]'::jsonb)
        ) affected(commitment_id)
        LEFT JOIN project_work_commitments commitment
          ON commitment.community_id = handoff.community_id
         AND commitment.commitment_id = affected.commitment_id::uuid
        WHERE handoff.community_id = target_community
          AND (
              commitment.commitment_id IS NULL
              OR commitment.assignment_id <> handoff.from_assignment_id
          )
    ) THEN
        RAISE EXCEPTION 'Role Handoff references a Commitment from another Assignment'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_role_continuity_references reference
        LEFT JOIN project_role_checkpoints checkpoint
          ON reference.owner_type = 'checkpoint'
         AND checkpoint.community_id = reference.community_id
         AND checkpoint.checkpoint_id = reference.owner_id
        LEFT JOIN project_role_handoffs handoff
          ON reference.owner_type = 'handoff'
         AND handoff.community_id = reference.community_id
         AND handoff.handoff_id = reference.owner_id
        WHERE reference.community_id = target_community
          AND (
              (
                  reference.owner_type = 'checkpoint'
                  AND (
                      checkpoint.checkpoint_id IS NULL
                      OR checkpoint.source_change_id <> reference.source_change_id
                  )
              )
              OR (
                  reference.owner_type = 'handoff'
                  AND (
                      handoff.handoff_id IS NULL
                      OR handoff.source_change_id <> reference.source_change_id
                  )
              )
              OR (
                  reference.reference_type = 'nostr_event'
                  AND NOT EXISTS (
                      SELECT 1
                      FROM events event
                      WHERE event.community_id = reference.community_id
                        AND event.id = reference.nostr_event_id
                  )
              )
          )
    ) THEN
        RAISE EXCEPTION 'Role continuity reference owner or target is inconsistent'
            USING ERRCODE = 'check_violation';
    END IF;
END
$$;
