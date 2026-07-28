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
