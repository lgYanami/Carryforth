-- New Communities enter the current Project View schema major without
-- silently recreating legacy v1 runtime state. Existing rows keep their exact
-- schema version and must continue to use the explicit migration/recovery
-- workflows.
--
-- Project View remains disabled by default. A schema-3 Community still needs
-- owner-authorized prepare-v3/initialize-v3 before ordinary runtime use.
ALTER TABLE communities
    ALTER COLUMN project_view_schema_version SET DEFAULT 3;

-- One database-owned predicate defines the only valid schema-v3 lifecycle
-- before canonical Project View state exists. Relay discovery and the Rust
-- prepare/initialize paths call this same function so a future code rollout
-- cannot silently drift from the deferred database invariant.
CREATE FUNCTION project_view_v3_bootstrap_lifecycle_valid(target_community UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
STRICT
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM communities community
        JOIN project_view_maintenance maintenance
          ON maintenance.community_id = community.id
        WHERE community.id = target_community
          AND community.project_view_schema_version = 3
          AND NOT community.project_view_enabled
          AND NOT community.project_context_enabled
          AND maintenance.state = 'normal'
          AND maintenance.current_epoch IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM project_view_state state
              WHERE state.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_view_maintenance_epochs epoch
              WHERE epoch.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_view_v3_resource_mappings mapping
              WHERE mapping.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_view_context_operations context_operation
              WHERE context_operation.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_view_objects object
              WHERE object.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_view_mutations mutation
              WHERE mutation.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_view_changes change
              WHERE change.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_role_assignments assignment
              WHERE assignment.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_role_assignment_proposals proposal
              WHERE proposal.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_work_commitments commitment
              WHERE commitment.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_role_checkpoints checkpoint
              WHERE checkpoint.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_role_handoffs handoff
              WHERE handoff.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM project_role_continuity_references reference
              WHERE reference.community_id = community.id
          )
          AND NOT EXISTS (
              SELECT 1
              FROM relay_members member
              WHERE member.community_id = community.id
                AND member.pubkey !~ '^[0-9a-f]{64}$'
          )
          AND (
              SELECT count(*)
              FROM relay_members owner
              WHERE owner.community_id = community.id
                AND owner.role = 'owner'
          ) <= 1
          AND (
              NOT EXISTS (
                  SELECT 1
                  FROM relay_members member
                  WHERE member.community_id = community.id
              )
              OR EXISTS (
                  SELECT 1
                  FROM relay_members owner
                  WHERE owner.community_id = community.id
                    AND owner.role = 'owner'
              )
          )
          AND NOT EXISTS (
              SELECT 1
              FROM relay_members owner
              JOIN users actor
                ON actor.community_id = owner.community_id
               AND encode(actor.pubkey, 'hex') = owner.pubkey
              WHERE owner.community_id = community.id
                AND owner.role = 'owner'
                AND actor.agent_owner_pubkey IS NOT NULL
          )
          AND NOT EXISTS (
              SELECT 1
              FROM relay_members owner
              JOIN community_bans restriction
                ON restriction.community_id = owner.community_id
               AND restriction.pubkey = decode(owner.pubkey, 'hex')
              WHERE owner.community_id = community.id
                AND owner.role = 'owner'
                AND restriction.banned
                AND (
                    restriction.ban_expires_at IS NULL
                    OR restriction.ban_expires_at > CURRENT_TIMESTAMP
                )
          )
          AND (
              (
                  community.project_view_preparation_operation_id IS NULL
                  AND NOT EXISTS (
                      SELECT 1
                      FROM project_view_provisioning_operations preparation
                      WHERE preparation.community_id = community.id
                  )
              )
              OR (
                  community.project_view_preparation_operation_id IS NOT NULL
                  AND EXISTS (
                      SELECT 1
                      FROM relay_members owner
                      WHERE owner.community_id = community.id
                        AND owner.role = 'owner'
                  )
                  AND (
                      SELECT count(*)
                      FROM project_view_provisioning_operations preparation
                      WHERE preparation.community_id = community.id
                  ) = 1
                  AND EXISTS (
                      SELECT 1
                      FROM project_view_provisioning_operations preparation
                      WHERE preparation.community_id = community.id
                        AND preparation.operation_id =
                            community.project_view_preparation_operation_id
                        AND preparation.operation = 'prepare_v3'
                        AND preparation.target_schema_version = 3
                        AND preparation.consumed_by_change_id IS NULL
                        AND preparation.consumed_at IS NULL
                  )
              )
          )
    )
$$;

-- Project Documents are independent versioned assets. Their row-level
-- validation must not manufacture a dependency on canonical Project View
-- state while the Community is still in the valid v3 bootstrap lifecycle.
-- Once Project View state exists (or the bootstrap shape is anomalous), the
-- original full cross-asset validator remains mandatory.
CREATE OR REPLACE FUNCTION project_view_v3_validate_row() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    target_community UUID;
BEGIN
    IF TG_OP = 'DELETE' THEN
        target_community := OLD.community_id;
    ELSE
        target_community := NEW.community_id;
    END IF;
    IF TG_TABLE_NAME IN ('project_documents', 'project_document_revisions')
       AND project_view_v3_bootstrap_lifecycle_valid(target_community) THEN
        RETURN NULL;
    END IF;
    PERFORM project_view_v3_validate_community(target_community);
    RETURN NULL;
END
$$;

-- Migration 0033 tightened the deferred Role-continuity validator for the v3
-- cutover, but its missing-state exception only admitted a Community after a
-- prepare-v3 receipt already existed. That creates a bootstrap cycle for a
-- genuinely new schema-v3 Community: the Community and its first Human owner
-- must commit before that owner can authorize prepare-v3.
--
-- Keep the initialized v2/v3 invariants unchanged while admitting exactly two
-- uninitialized v3 coordinates:
--   * a disabled Community with a pristine Project View footprint; or
--   * the existing exact, unconsumed prepare-v3 state.
-- The expected normal maintenance seed and empty canonical footprint are
-- mandatory in both cases; only the provisioning-receipt shape differs.
-- Community membership, users, channels, messages, and Project Documents are
-- deliberately independent of whether Project View has been initialized.
-- No existing Community row is rewritten by this migration.
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

    IF NOT FOUND THEN
        IF target_schema = 3
           AND project_view_v3_bootstrap_lifecycle_valid(target_community) THEN
            RETURN;
        END IF;
        RAISE EXCEPTION 'Project View state missing outside the valid schema-v3 bootstrap lifecycle for community %', target_community
            USING ERRCODE = 'check_violation';
    END IF;

    IF state_schema IS DISTINCT FROM target_schema THEN
        RAISE EXCEPTION 'Project View state schema mismatches community %', target_community
            USING ERRCODE = 'check_violation';
    END IF;

    SELECT count(*)::integer
    INTO owner_count
    FROM relay_members
    WHERE community_id = target_community
      AND role = 'owner';

    IF owner_count <> 1 THEN
        RAISE EXCEPTION 'Project View requires exactly one Community owner'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM relay_members member
        WHERE member.community_id = target_community
          AND member.pubkey !~ '^[0-9a-f]{64}$'
    ) THEN
        RAISE EXCEPTION 'Project View membership pubkeys must be lowercase 32-byte hex'
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
        RAISE EXCEPTION 'Active Project View objects must match the Community schema version'
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
