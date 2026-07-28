-- Complete the stage-2 Role Proposal / Assignment persistence shape.
--
-- 0026 intentionally installed only the cross-entry consistency kernel while
-- every Community remained on schema v1. This additive migration supplies the
-- per-entity revisions, lifecycle reports, replacement links, membership
-- snapshot pointer, and materialized v2 meta counts used by the first v2
-- coordinator.

ALTER TABLE project_view_state
    ADD COLUMN open_proposal_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN active_assignment_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN active_commitment_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN checkpoint_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN handoff_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN membership_snapshot_event_id BYTEA,
    ADD CONSTRAINT project_view_state_v2_counts_check
        CHECK (
            open_proposal_count >= 0
            AND active_assignment_count >= 0
            AND active_commitment_count >= 0
            AND checkpoint_count >= 0
            AND handoff_count >= 0
        ),
    ADD CONSTRAINT project_view_state_membership_snapshot_check
        CHECK (
            membership_snapshot_event_id IS NULL
            OR octet_length(membership_snapshot_event_id) = 32
        );

ALTER TABLE project_role_assignment_proposals
    ADD COLUMN entity_revision BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN updated_at TIMESTAMPTZ,
    ADD COLUMN last_change_id BYTEA;

UPDATE project_role_assignment_proposals
SET updated_at = COALESCE(resolved_at, created_at),
    last_change_id = source_change_id;

ALTER TABLE project_role_assignment_proposals
    ALTER COLUMN updated_at SET NOT NULL,
    ALTER COLUMN last_change_id SET NOT NULL,
    ADD CONSTRAINT project_role_proposals_entity_revision_check
        CHECK (entity_revision BETWEEN 1 AND 9007199254740991),
    ADD CONSTRAINT project_role_proposals_updated_time_check
        CHECK (updated_at >= created_at),
    ADD CONSTRAINT project_role_proposals_last_change_check
        CHECK (octet_length(last_change_id) = 32),
    ADD CONSTRAINT project_role_proposals_last_change_fk
        FOREIGN KEY (community_id, last_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE project_role_assignments
    ADD COLUMN entity_revision BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN updated_at TIMESTAMPTZ,
    ADD COLUMN last_change_id BYTEA,
    ADD COLUMN replacement_requested_at TIMESTAMPTZ,
    ADD COLUMN replacement_request_reason TEXT,
    ADD COLUMN unable_reported_at TIMESTAMPTZ,
    ADD COLUMN unable_report_reason TEXT,
    ADD COLUMN replaced_by_assignment_id UUID;

UPDATE project_role_assignments
SET updated_at = COALESCE(ended_at, started_at),
    last_change_id = COALESCE(ended_source_change_id, source_change_id);

ALTER TABLE project_role_assignments
    ALTER COLUMN updated_at SET NOT NULL,
    ALTER COLUMN last_change_id SET NOT NULL,
    DROP CONSTRAINT project_role_assignments_end_shape_check,
    ADD CONSTRAINT project_role_assignments_entity_revision_check
        CHECK (entity_revision BETWEEN 1 AND 9007199254740991),
    ADD CONSTRAINT project_role_assignments_updated_time_check
        CHECK (updated_at >= started_at),
    ADD CONSTRAINT project_role_assignments_last_change_check
        CHECK (octet_length(last_change_id) = 32),
    ADD CONSTRAINT project_role_assignments_last_change_fk
        FOREIGN KEY (community_id, last_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT project_role_assignments_replaced_by_fk
        FOREIGN KEY (community_id, replaced_by_assignment_id)
        REFERENCES project_role_assignments (community_id, assignment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT project_role_assignments_replacement_report_check
        CHECK (
            (replacement_requested_at IS NULL)
                = (replacement_request_reason IS NULL)
            OR (
                replacement_requested_at IS NOT NULL
                AND replacement_request_reason IS NULL
            )
        ),
    ADD CONSTRAINT project_role_assignments_unable_report_check
        CHECK (
            (unable_reported_at IS NULL) = (unable_report_reason IS NULL)
            OR (
                unable_reported_at IS NOT NULL
                AND unable_report_reason IS NULL
            )
        ),
    ADD CONSTRAINT project_role_assignments_report_times_check
        CHECK (
            (replacement_requested_at IS NULL OR replacement_requested_at >= started_at)
            AND (unable_reported_at IS NULL OR unable_reported_at >= started_at)
        ),
    ADD CONSTRAINT project_role_assignments_end_shape_check
        CHECK (
            (
                ended_at IS NULL
                AND ended_by IS NULL
                AND ended_reason IS NULL
                AND ended_source_change_id IS NULL
                AND replaced_by_assignment_id IS NULL
            )
            OR
            (
                ended_at IS NOT NULL
                AND ended_at >= started_at
                AND ended_by IS NOT NULL
                AND ended_reason IN (
                    'revoked',
                    'replaced',
                    'membership_ended',
                    'unrecoverable',
                    'role_deactivated'
                )
                AND ended_source_change_id IS NOT NULL
                AND (
                    (ended_reason = 'replaced' AND replaced_by_assignment_id IS NOT NULL)
                    OR
                    (ended_reason <> 'replaced' AND replaced_by_assignment_id IS NULL)
                )
            )
        );

ALTER TABLE project_role_handoffs
    ADD COLUMN entity_revision BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN last_change_id BYTEA;

UPDATE project_role_handoffs
SET last_change_id = source_change_id;

ALTER TABLE project_role_handoffs
    ALTER COLUMN last_change_id SET NOT NULL,
    ADD CONSTRAINT project_role_handoffs_entity_revision_check
        CHECK (entity_revision BETWEEN 1 AND 9007199254740991),
    ADD CONSTRAINT project_role_handoffs_last_change_check
        CHECK (octet_length(last_change_id) = 32),
    ADD CONSTRAINT project_role_handoffs_last_change_fk
        FOREIGN KEY (community_id, last_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED;

-- Materialized meta counts are guarded at deferred-constraint time, after the
-- coordinator has written every entity and the new state row.
CREATE FUNCTION project_role_continuity_validate_counts(target_community UUID)
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

    IF NOT FOUND OR target_schema <> 2 THEN
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

CREATE FUNCTION project_role_continuity_validate_counts_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM project_role_continuity_validate_counts(OLD.community_id);
    ELSE
        IF TG_OP = 'UPDATE' AND OLD.community_id IS DISTINCT FROM NEW.community_id THEN
            PERFORM project_role_continuity_validate_counts(OLD.community_id);
        END IF;
        PERFORM project_role_continuity_validate_counts(NEW.community_id);
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER project_view_state_role_counts_validate
    AFTER INSERT OR UPDATE ON project_view_state
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();

CREATE CONSTRAINT TRIGGER project_role_proposals_counts_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_assignment_proposals
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();

CREATE CONSTRAINT TRIGGER project_role_assignments_counts_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_assignments
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();

CREATE CONSTRAINT TRIGGER project_work_commitments_counts_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_work_commitments
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();

CREATE CONSTRAINT TRIGGER project_role_checkpoints_counts_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_checkpoints
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();

CREATE CONSTRAINT TRIGGER project_role_handoffs_counts_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_handoffs
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_continuity_validate_counts_row();
