-- Project Context Coordinate-search response-only storage fence.
--
-- Kind 40913 is produced only as a signed response to one authenticated
-- generic `/query` request. It is not canonical Nostr state and must never be
-- persisted, indexed, replayed, counted, searched, or fanned out.

ALTER TABLE events
    ADD CONSTRAINT events_kind_not_project_context_coordinate_search_result
        CHECK (kind <> 40913) NOT VALID;
ALTER TABLE events
    VALIDATE CONSTRAINT events_kind_not_project_context_coordinate_search_result;
