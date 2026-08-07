-- Migration 0053.
-- Project Context v2 can expose Meeting coordinates, so newly enabling it
-- requires the durable Community-wide Meeting read contract to be published.
-- Existing enabled Communities are preserved for an explicit, audited cutover.

CREATE FUNCTION project_context_requires_meeting_community_read() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.project_context_edge_enabled
       AND NOT OLD.project_context_edge_enabled
       AND NOT NEW.meeting_community_read_enabled
    THEN
        RAISE EXCEPTION
            'Project Context v2 requires published Meeting Community reads'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_project_context_requires_meeting_community_read
    BEFORE UPDATE OF project_context_edge_enabled
    ON communities
    FOR EACH ROW
    EXECUTE FUNCTION project_context_requires_meeting_community_read();
