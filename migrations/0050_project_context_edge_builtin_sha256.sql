-- Migration 0050: remove the runtime pgcrypto lookup from the Project Context hash guard.
--
-- Migration installs start at 0001 and therefore have pgcrypto, but the
-- checked-in fresh-schema path is also supported and its schema planner does
-- not materialize CREATE EXTENSION statements. PostgreSQL's built-in
-- sha256(bytea) returns the same 32-byte digest without that dependency.

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
        IF coordinate_row.coordinate_type = 'project_view_object' THEN
            payload := payload || decode('00', 'hex') || decode(
                CASE coordinate_row.coordinate_subtype
                    WHEN 'project_profile' THEN '00'
                    WHEN 'goal' THEN '01'
                    WHEN 'role' THEN '02'
                    WHEN 'plan' THEN '03'
                    WHEN 'stage' THEN '04'
                    WHEN 'requirement' THEN '05'
                    WHEN 'issue' THEN '06'
                    WHEN 'work' THEN '07'
                    WHEN 'resource' THEN '08'
                END,
                'hex'
            ) || uuid_send(coordinate_row.coordinate_id);
        ELSE
            payload := payload || decode('01', 'hex') || uuid_send(coordinate_row.coordinate_id);
        END IF;
    END LOOP;
    RETURN sha256(payload);
END
$$;
