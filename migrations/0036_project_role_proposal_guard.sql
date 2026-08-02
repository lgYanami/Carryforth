-- Keep open Role Proposals anchored to an active Role.
--
-- Application-level guards provide actionable errors before projection work,
-- while these deferred constraints protect direct SQL, older writers, and
-- future write paths from committing an orphaned continuity graph.

CREATE FUNCTION project_role_open_proposal_validate_community(target_community UUID)
RETURNS VOID
LANGUAGE plpgsql AS $$
DECLARE
    target_schema SMALLINT;
BEGIN
    SELECT project_view_schema_version
    INTO target_schema
    FROM communities
    WHERE id = target_community;

    IF NOT FOUND OR target_schema NOT IN (2, 3) THEN
        RETURN;
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_role_assignment_proposals proposal
        LEFT JOIN project_view_objects role_object
          ON role_object.community_id = proposal.community_id
         AND role_object.object_id = proposal.role_id
        WHERE proposal.community_id = target_community
          AND proposal.status = 'open'
          AND (
              role_object.object_id IS NULL
              OR role_object.object_type <> 'role'
              OR role_object.schema_version IS DISTINCT FROM target_schema
              OR role_object.deleted_at IS NOT NULL
              OR role_object.body->'active' IS DISTINCT FROM 'true'::jsonb
          )
    ) THEN
        RAISE EXCEPTION 'Open Role Proposal references a missing or inactive Role'
            USING ERRCODE = 'check_violation';
    END IF;
END
$$;

CREATE FUNCTION project_role_open_proposal_validate_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM project_role_open_proposal_validate_community(OLD.community_id);
    ELSE
        IF TG_OP = 'UPDATE' AND OLD.community_id IS DISTINCT FROM NEW.community_id THEN
            PERFORM project_role_open_proposal_validate_community(OLD.community_id);
        END IF;
        PERFORM project_role_open_proposal_validate_community(NEW.community_id);
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER project_view_objects_open_proposal_role_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_view_objects
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_open_proposal_validate_row();

CREATE CONSTRAINT TRIGGER project_role_proposals_role_validate
    AFTER INSERT OR UPDATE OR DELETE ON project_role_assignment_proposals
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_role_open_proposal_validate_row();

-- Fail migration rather than allowing an already-corrupt v2/v3 Community to
-- continue advertising a snapshot the SDK cannot verify.
DO $$
DECLARE
    target_community UUID;
BEGIN
    FOR target_community IN
        SELECT id
        FROM communities
        WHERE project_view_schema_version IN (2, 3)
    LOOP
        PERFORM project_role_open_proposal_validate_community(target_community);
    END LOOP;
END
$$;
