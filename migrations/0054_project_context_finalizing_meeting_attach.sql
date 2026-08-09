-- Migration 0054.
-- Project Context may attach a schema-v3 Meeting while its frozen Action
-- Finalization window is active. Keep private Action commands private: the
-- SQL liveness guard verifies their accepted receipt instead of requiring an
-- ordinary Event row. Canonical Board and State projections remain Relay
-- signed through the current Project Context projection identity.

CREATE FUNCTION project_context_meeting_is_attachable(
    target_community UUID,
    target_meeting UUID
)
RETURNS BOOLEAN
LANGUAGE SQL STABLE AS $$
    SELECT project_context_meeting_is_terminal(target_community, target_meeting)
        OR EXISTS (
            SELECT 1
            FROM meeting_sessions session
            JOIN channels channel
              ON channel.community_id = session.community_id
             AND channel.id = session.session_id
            JOIN meeting_v2_bootstrap_state runtime
              ON runtime.community_id = session.community_id
             AND runtime.session_id = session.session_id
            JOIN meeting_v2_action_runs action_run
              ON action_run.community_id = session.community_id
             AND action_run.session_id = session.session_id
             AND action_run.terminal_status IS NULL
            JOIN meeting_v2_action_command_receipts begin_receipt
              ON begin_receipt.community_id = action_run.community_id
             AND begin_receipt.session_id = action_run.session_id
             AND begin_receipt.command_event_id = action_run.begin_event_id
             AND begin_receipt.action_run_id = action_run.action_run_id
            JOIN meeting_current_boards current_board
              ON current_board.community_id = action_run.community_id
             AND current_board.session_id = action_run.session_id
             AND current_board.board_event_id = action_run.board_event_id
            JOIN project_context_edge_state context_state
              ON context_state.community_id = session.community_id
             AND context_state.schema_version = 2
            JOIN events create_event
              ON create_event.community_id = session.community_id
             AND create_event.channel_id = session.session_id
             AND create_event.id = session.create_event_id
             AND create_event.kind = 42100
             AND create_event.pubkey = session.host_pubkey
             AND create_event.deleted_at IS NULL
            JOIN events board_projection
              ON board_projection.community_id = session.community_id
             AND board_projection.channel_id = session.session_id
             AND board_projection.id = action_run.board_event_id
             AND board_projection.kind = 42110
             AND board_projection.pubkey = context_state.projection_pubkey
             AND board_projection.deleted_at IS NULL
            JOIN meeting_baton_state state
              ON state.community_id = session.community_id
             AND state.session_id = session.session_id
            JOIN meeting_baton_state_history history
              ON history.community_id = state.community_id
             AND history.session_id = state.session_id
             AND history.state_revision = state.state_revision
             AND history.state_event_id = state.state_event_id
            JOIN events state_projection
              ON state_projection.community_id = state.community_id
             AND state_projection.channel_id = state.session_id
             AND state_projection.id = state.state_event_id
             AND state_projection.kind = 42103
             AND state_projection.pubkey = context_state.projection_pubkey
             AND state_projection.deleted_at IS NULL
            WHERE session.community_id = target_community
              AND session.session_id = target_meeting
              AND session.schema_version = 3
              AND session.status = 'active'
              AND session.floor_policy_version = 'moderated-board-actions-v3'
              AND channel.room_kind = 'meeting'
              AND channel.deleted_at IS NULL
              AND runtime.runtime_phase = 'finalizing_actions'
              AND runtime.control_epoch = action_run.control_epoch
              AND runtime.board_window = action_run.board_window
              AND action_run.action_condition IN ('runnable', 'blocked')
              AND octet_length(action_run.begin_event_id) = 32
              AND octet_length(action_run.board_event_id) = 32
              AND begin_receipt.author_pubkey = session.host_pubkey
              AND begin_receipt.action = 'begin'
              AND begin_receipt.action_window_epoch = 1
              AND begin_receipt.accepted
              AND begin_receipt.outcome_code = 'action_finalization_began'
              AND state.phase = 'moderator_idle'
              AND state.state_revision > 0
              AND state.control_epoch = action_run.control_epoch
              AND state.active_offer_id IS NULL
              AND state.active_grant_id IS NULL
              AND state.active_decision_attempt_id IS NULL
              AND state.next_action_at IS NULL
              AND history.transition_primary_type IN (
                  'action_finalization_began',
                  'action_lease_renewed',
                  'action_blocked',
                  'action_retried',
                  'action_deadline_exceeded',
                  'action_lease_expired',
                  'action_operator_deadline_exceeded'
              )
        );
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
                      AND NOT project_context_meeting_is_attachable(
                          coordinate.community_id,
                          coordinate.coordinate_id
                      )
                  )
                  OR coordinate.coordinate_type NOT IN (
                      'project_view_object', 'document', 'meeting'
                  )
              )
        ) THEN
            RAISE EXCEPTION 'Project Context attach requires active coordinates, an attachable Meeting, and an active Context Document'
                USING ERRCODE = 'check_violation';
        END IF;
    END IF;
    RETURN NULL;
END
$$;
