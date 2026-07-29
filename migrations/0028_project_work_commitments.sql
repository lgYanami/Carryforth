-- Stage 5: make Work Commitment a first-class projected lifecycle entity.
--
-- The original v2 storage reservation captured the foreign keys and terminal
-- shape. Live commands additionally need immutable Member attribution,
-- entity-local revision fencing, and the accepted change that produced the
-- latest head.

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
