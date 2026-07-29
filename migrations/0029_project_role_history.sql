-- Stage 6: append-only Role Checkpoints, structured Handoffs, and typed
-- continuity references.
--
-- The tables were reserved in 0026 so their tenant-leading foreign keys were
-- present before v2 cutover. This migration completes their live write shape
-- and adds deferred cross-entry validation.

ALTER TABLE project_role_checkpoints
    ADD COLUMN based_on_project_revision BIGINT,
    ADD COLUMN supersedes_checkpoint_id UUID,
    ADD COLUMN entity_revision BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN last_change_id BYTEA;

UPDATE project_role_checkpoints
SET based_on_project_revision = project_revision - 1,
    last_change_id = source_change_id;

ALTER TABLE project_role_checkpoints
    ALTER COLUMN based_on_project_revision SET NOT NULL,
    ALTER COLUMN last_change_id SET NOT NULL,
    ADD CONSTRAINT project_role_checkpoints_basis_check
        CHECK (
            based_on_project_revision BETWEEN 1 AND 9007199254740991
            AND based_on_project_revision < project_revision
        ),
    ADD CONSTRAINT project_role_checkpoints_entity_revision_check
        CHECK (entity_revision = 1),
    ADD CONSTRAINT project_role_checkpoints_content_check
        CHECK (
            COALESCE(
                jsonb_typeof(body->'summary') = 'string'
                AND body->>'summary' <> ''
                AND jsonb_typeof(body->'current_focus') = 'array'
                AND jsonb_typeof(body->'progress') = 'array'
                AND jsonb_typeof(body->'blockers') = 'array'
                AND jsonb_typeof(body->'risks') = 'array'
                AND jsonb_typeof(body->'open_questions') = 'array'
                AND jsonb_typeof(body->'next_steps') = 'array'
                AND body->'references' = '[]'::jsonb,
                FALSE
            )
        ),
    ADD CONSTRAINT project_role_checkpoints_last_change_check
        CHECK (octet_length(last_change_id) = 32),
    ADD CONSTRAINT project_role_checkpoints_last_change_fk
        FOREIGN KEY (community_id, last_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT project_role_checkpoints_supersedes_fk
        FOREIGN KEY (community_id, supersedes_checkpoint_id)
        REFERENCES project_role_checkpoints (community_id, checkpoint_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE project_role_handoffs
    ADD COLUMN checkpoint_id UUID,
    ALTER COLUMN from_assignment_id SET NOT NULL,
    ADD CONSTRAINT project_role_handoffs_append_revision_check
        CHECK (entity_revision = 1),
    ADD CONSTRAINT project_role_handoffs_content_check
        CHECK (
            COALESCE(
                jsonb_typeof(body->'cause') = 'string'
                AND jsonb_typeof(body->'affected_commitment_ids') = 'array'
                AND jsonb_typeof(body->'content') = 'object'
                AND (
                    NOT (body->'content' ? 'summary')
                    OR jsonb_typeof(body->'content'->'summary') = 'string'
                )
                AND jsonb_typeof(body->'content'->'unresolved_items') = 'array'
                AND body->'content'->'references' = '[]'::jsonb,
                FALSE
            )
        );

ALTER TABLE project_role_handoffs
    ADD CONSTRAINT project_role_handoffs_checkpoint_fk
        FOREIGN KEY (community_id, checkpoint_id)
        REFERENCES project_role_checkpoints (community_id, checkpoint_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX idx_project_role_checkpoints_role_revision
    ON project_role_checkpoints (
        community_id,
        role_id,
        project_revision DESC,
        checkpoint_id DESC
    );

CREATE INDEX idx_project_role_handoffs_role_revision
    ON project_role_handoffs (
        community_id,
        role_id,
        project_revision DESC,
        handoff_id DESC
    );

-- History rows are immutable facts. Projection pointers are materialization
-- metadata and may be rebound during a trusted reprojection, but no semantic
-- column may change and rows may never be deleted.
CREATE FUNCTION project_role_history_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Role continuity history is append-only'
            USING ERRCODE = 'check_violation';
    END IF;
    IF (to_jsonb(OLD) - 'projection_event_id')
        IS DISTINCT FROM
       (to_jsonb(NEW) - 'projection_event_id') THEN
        RAISE EXCEPTION 'Role continuity history facts cannot be updated'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER project_role_checkpoints_append_only
    BEFORE UPDATE OR DELETE ON project_role_checkpoints
    FOR EACH ROW
    EXECUTE FUNCTION project_role_history_append_only();

CREATE TRIGGER project_role_handoffs_append_only
    BEFORE UPDATE OR DELETE ON project_role_handoffs
    FOR EACH ROW
    EXECUTE FUNCTION project_role_history_append_only();

CREATE FUNCTION project_role_references_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Role continuity references are append-only'
        USING ERRCODE = 'check_violation';
END
$$;

CREATE TRIGGER project_role_references_append_only
    BEFORE UPDATE OR DELETE ON project_role_continuity_references
    FOR EACH ROW
    EXECUTE FUNCTION project_role_references_append_only();

CREATE FUNCTION project_role_history_validate_stage6_community(
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

CREATE FUNCTION project_role_history_validate_stage6_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM project_role_history_validate_stage6_community(OLD.community_id);
    ELSE
        IF TG_OP = 'UPDATE' AND OLD.community_id IS DISTINCT FROM NEW.community_id THEN
            PERFORM project_role_history_validate_stage6_community(OLD.community_id);
        END IF;
        PERFORM project_role_history_validate_stage6_community(NEW.community_id);
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER project_role_checkpoints_stage6_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_checkpoints
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_history_validate_stage6_row();

CREATE CONSTRAINT TRIGGER project_role_handoffs_stage6_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_handoffs
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_history_validate_stage6_row();

CREATE CONSTRAINT TRIGGER project_role_references_stage6_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_continuity_references
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_history_validate_stage6_row();
