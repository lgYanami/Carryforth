-- Migration 0051: Project Context Edge v2 and terminal Meeting coordinates.
--
-- This is an additive, fail-closed migration. Existing canonical Edge,
-- binding, change, receipt, Context Revision, and historical projection rows
-- are retained. Any enabled v1 catalog is disabled before the schema opens;
-- an explicit Relay-signed reprojection is required before v2 can be enabled.

UPDATE communities community
SET project_context_edge_enabled = FALSE
WHERE community.project_context_edge_enabled
  AND EXISTS (
      SELECT 1
      FROM project_context_edge_state state
      WHERE state.community_id = community.id
        AND state.schema_version = 1
  );

ALTER TABLE project_context_edge_state
    ALTER COLUMN schema_version SET DEFAULT 2,
    DROP CONSTRAINT project_context_edge_state_schema_check,
    ADD CONSTRAINT project_context_edge_state_schema_check
        CHECK (schema_version IN (1, 2));

ALTER TABLE project_context_edge_coordinates
    DROP CONSTRAINT project_context_edge_coordinates_shape_check,
    ADD CONSTRAINT project_context_edge_coordinates_shape_check
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
            OR (
                coordinate_type = 'meeting'
                AND coordinate_subtype IS NULL
                AND canonical_key =
                    'meeting:' || community_id::text || ':' || coordinate_id::text
            )
        );

-- Keep the edge-key-v1 domain separator and existing family bytes. Meeting is
-- appended as the previously unallocated 0x02 family, preserving every v1 key.
CREATE OR REPLACE FUNCTION project_context_compute_edge_key(
    target_community UUID,
    target_edge BYTEA
)
RETURNS BYTEA
LANGUAGE plpgsql STABLE AS $$
DECLARE
    coordinate_count INTEGER;
    coordinate_row RECORD;
    payload BYTEA;
    subtype_byte TEXT;
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
        CASE coordinate_row.coordinate_type
            WHEN 'project_view_object' THEN
                subtype_byte := CASE coordinate_row.coordinate_subtype
                    WHEN 'project_profile' THEN '00'
                    WHEN 'goal' THEN '01'
                    WHEN 'role' THEN '02'
                    WHEN 'plan' THEN '03'
                    WHEN 'stage' THEN '04'
                    WHEN 'requirement' THEN '05'
                    WHEN 'issue' THEN '06'
                    WHEN 'work' THEN '07'
                    WHEN 'resource' THEN '08'
                    ELSE NULL
                END;
                IF subtype_byte IS NULL THEN
                    RAISE EXCEPTION 'unsupported Project Context object subtype %',
                        coordinate_row.coordinate_subtype
                        USING ERRCODE = 'check_violation';
                END IF;
                payload := payload || decode('00', 'hex')
                    || decode(subtype_byte, 'hex')
                    || uuid_send(coordinate_row.coordinate_id);
            WHEN 'document' THEN
                payload := payload || decode('01', 'hex')
                    || uuid_send(coordinate_row.coordinate_id);
            WHEN 'meeting' THEN
                payload := payload || decode('02', 'hex')
                    || uuid_send(coordinate_row.coordinate_id);
            ELSE
                RAISE EXCEPTION 'unsupported Project Context coordinate type %',
                    coordinate_row.coordinate_type
                    USING ERRCODE = 'check_violation';
        END CASE;
    END LOOP;
    RETURN sha256(payload);
END
$$;

-- Reprojection may preserve a v2 schema or perform the one-way v1 -> v2
-- transition. All business state remains byte-for-byte stable.
CREATE OR REPLACE FUNCTION project_context_edge_state_guard_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('buzz.project_context_reproject', true) = 'on' THEN
        IF ROW(OLD.community_id, OLD.context_revision,
               OLD.active_edge_count, OLD.bound_document_count, OLD.last_change_id,
               OLD.last_actor_pubkey, OLD.initialized_at, OLD.updated_at)
           IS DISTINCT FROM
           ROW(NEW.community_id, NEW.context_revision,
               NEW.active_edge_count, NEW.bound_document_count, NEW.last_change_id,
               NEW.last_actor_pubkey, NEW.initialized_at, NEW.updated_at)
           OR NEW.projection_generation <> OLD.projection_generation + 1
           OR NOT (
               NEW.schema_version = OLD.schema_version
               OR (OLD.schema_version = 1 AND NEW.schema_version = 2)
           ) THEN
            RAISE EXCEPTION 'Project Context reproject may only upgrade schema and replace signer generation/pointers'
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

-- SQL-side attach guard for the same verified terminal projection consumed by
-- the Rust resolver. This is transition evidence, not a permanent FK: detach
-- and existing historical Edges never depend on current Meeting hydration.
CREATE OR REPLACE FUNCTION project_context_meeting_is_terminal(
    target_community UUID,
    target_meeting UUID
)
RETURNS BOOLEAN
LANGUAGE plpgsql STABLE AS $$
DECLARE
    session_row RECORD;
    state_row RECORD;
    normalized_outcome TEXT;
BEGIN
    SELECT session.*, channel.room_kind, channel.deleted_at AS channel_deleted_at
    INTO session_row
    FROM meeting_sessions session
    JOIN channels channel
      ON channel.community_id = session.community_id
     AND channel.id = session.session_id
    WHERE session.community_id = target_community
      AND session.session_id = target_meeting;
    IF NOT FOUND
       OR session_row.room_kind <> 'meeting'
       OR session_row.channel_deleted_at IS NOT NULL
       OR session_row.status <> 'ended'
       OR session_row.end_event_id IS NULL
       OR session_row.ended_by IS NULL THEN
        RETURN FALSE;
    END IF;

    IF session_row.schema_version = 1 THEN
        SELECT floor_revision AS state_revision, state_event_id, phase, outcome
        INTO state_row
        FROM meeting_rounds
        WHERE community_id = target_community
          AND session_id = target_meeting
          AND round_number = session_row.current_round;
        IF NOT FOUND
           OR state_row.phase IS DISTINCT FROM 'closed'
           OR state_row.outcome IS DISTINCT FROM 'ended' THEN
            RETURN FALSE;
        END IF;
        normalized_outcome := CASE
            WHEN session_row.ended_by = session_row.host_pubkey THEN 'closed'
            ELSE 'aborted'
        END;
    ELSIF session_row.schema_version IN (2, 3) THEN
        SELECT state_revision, state_event_id, phase
        INTO state_row
        FROM meeting_baton_state
        WHERE community_id = target_community AND session_id = target_meeting;
        IF NOT FOUND OR state_row.phase IS DISTINCT FROM 'ended' THEN
            RETURN FALSE;
        END IF;
        normalized_outcome := CASE
            WHEN session_row.schema_version = 3 THEN session_row.terminal_outcome
            WHEN session_row.ended_by = session_row.host_pubkey THEN 'closed'
            ELSE 'aborted'
        END;
    ELSE
        RETURN FALSE;
    END IF;

    IF state_row.state_revision <= 0
       OR octet_length(state_row.state_event_id) <> 32
       OR normalized_outcome IS NULL
       OR normalized_outcome NOT IN ('closed', 'aborted') THEN
        RETURN FALSE;
    END IF;
    RETURN EXISTS (
        SELECT 1 FROM events event
        WHERE event.community_id = target_community
          AND event.channel_id = target_meeting
          AND event.id = session_row.create_event_id
          AND event.kind = 42100
          AND event.pubkey = session_row.host_pubkey
          AND event.deleted_at IS NULL
    ) AND EXISTS (
        SELECT 1 FROM events event
        WHERE event.community_id = target_community
          AND event.channel_id = target_meeting
          AND event.id = state_row.state_event_id
          AND event.kind = 42103
          AND event.deleted_at IS NULL
    ) AND EXISTS (
        SELECT 1 FROM events event
        WHERE event.community_id = target_community
          AND event.channel_id = target_meeting
          AND event.id = session_row.end_event_id
          AND event.kind = 42101
          AND event.pubkey = session_row.ended_by
          AND event.deleted_at IS NULL
    );
END
$$;

-- Validate either a frozen v1 migration source or the current v2 catalog. A
-- schema-1 catalog remains a closed two-family union and cannot gain Meeting
-- rows while waiting for operator reprojection.
CREATE OR REPLACE FUNCTION project_context_validate_community(target_community UUID) RETURNS void
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
    IF state_row.schema_version NOT IN (1, 2) THEN
        RAISE EXCEPTION 'unsupported Project Context schema %', state_row.schema_version
            USING ERRCODE = 'check_violation';
    END IF;
    IF state_row.schema_version = 1 AND EXISTS (
        SELECT 1 FROM project_context_edge_coordinates
        WHERE community_id = target_community AND coordinate_type = 'meeting'
    ) THEN
        RAISE EXCEPTION 'Project Context v1 cannot contain Meeting coordinates'
            USING ERRCODE = 'check_violation';
    END IF;

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
                               CASE coordinate_type
                                   WHEN 'project_view_object' THEN 0
                                   WHEN 'document' THEN 1
                                   WHEN 'meeting' THEN 2
                                   ELSE 3
                               END,
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
                WHEN 'document' THEN jsonb_build_object(
                    'coordinate_type', 'document',
                    'document_id', coordinate_id
                )
                WHEN 'meeting' THEN jsonb_build_object(
                    'coordinate_type', 'meeting',
                    'meeting_id', coordinate_id
                )
                ELSE NULL
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
                  OR (
                      coordinate.coordinate_type = 'meeting'
                      AND NOT EXISTS (
                          SELECT 1
                          FROM meeting_sessions session
                          JOIN channels channel
                            ON channel.community_id = session.community_id
                           AND channel.id = session.session_id
                          WHERE session.community_id = coordinate.community_id
                            AND session.session_id = coordinate.coordinate_id
                            AND channel.room_kind = 'meeting'
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
                'schema_version', state_row.schema_version,
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
            'schema_version', state_row.schema_version,
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
            'schema_version', state_row.schema_version,
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

CREATE OR REPLACE FUNCTION project_context_validate_new_change() RETURNS trigger
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
                  OR (
                      coordinate.coordinate_type = 'meeting'
                      AND NOT project_context_meeting_is_terminal(
                          coordinate.community_id,
                          coordinate.coordinate_id
                      )
                  )
                  OR coordinate.coordinate_type NOT IN (
                      'project_view_object', 'document', 'meeting'
                  )
              )
        ) THEN
            RAISE EXCEPTION 'Project Context attach requires active coordinates, a terminal Meeting, and an active Context Document'
                USING ERRCODE = 'check_violation';
        END IF;
    END IF;
    RETURN NULL;
END
$$;

-- Recompile the wrapper after introducing the schema-dispatching validator.
CREATE OR REPLACE FUNCTION project_context_validate_row() RETURNS trigger
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

CREATE OR REPLACE FUNCTION project_context_validate_capability(target_community UUID) RETURNS void
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
          ON context_state.community_id = community.id AND context_state.schema_version = 2
        WHERE community.id = target_community
          AND community.archived_at IS NULL
          AND community.project_view_schema_version = 3
          AND community.project_view_enabled
          AND community.project_document_enabled
          AND view_state.projection_pubkey = document_state.projection_pubkey
          AND document_state.projection_pubkey = context_state.projection_pubkey
    ) THEN
        RAISE EXCEPTION 'Project Context v2 capability prerequisites are not ready'
            USING ERRCODE = 'check_violation';
    END IF;
    PERFORM project_view_v3_validate_community(target_community);
    PERFORM project_document_validate_community(target_community);
    PERFORM project_context_validate_community(target_community);
END
$$;
