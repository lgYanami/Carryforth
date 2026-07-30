#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

fixture() {
  jq -nc '
    {
      seq: 1,
      kind: "harness_started",
      acceptanceRole: "m1-agent1",
      payload: {meetingV1Acceptance: true}
    },
    {
      seq: 2,
      kind: "agent_process_started",
      acceptanceRole: "m1-agent1",
      payload: {pid: 42}
    },
    {
      seq: 3,
      kind: "meeting_v1_moderator_attempt_registered",
      acceptanceRole: "m1-agent1",
      payload: {
        attempt_id: "attempt-1",
        candidate_snapshot_hash: "hash-1",
        control_epoch: 1,
        decision_epoch: 1
      }
    },
    {
      seq: 4,
      kind: "meeting_v1_moderator_decision_started",
      acceptanceRole: "m1-agent1",
      turnId: "turn-1",
      payload: {
        attempt_id: "attempt-1",
        candidate_snapshot_hash: "hash-1",
        control_epoch: 1,
        decision_epoch: 1,
        phase: "moderator_control",
        candidate_count: 1,
        candidate_sources: [{source_id: "intent-1"}]
      }
    },
    {
      seq: 5,
      kind: "turn_started",
      acceptanceRole: "m1-agent1",
      turnId: "turn-1",
      payload: {}
    },
    {
      seq: 6,
      kind: "model_applied",
      acceptanceRole: "m1-agent1",
      turnId: "turn-1",
      payload: {model_id: "gpt-5.6-sol[max]"}
    },
    {
      seq: 7,
      kind: "prompt_request_started",
      acceptanceRole: "m1-agent1",
      turnId: "turn-1",
      payload: {}
    },
    {
      seq: 8,
      kind: "meeting_v1_moderator_decision_completed",
      acceptanceRole: "m1-agent1",
      turnId: "turn-1",
      payload: {attempt_id: "attempt-1", outcome: "natural_terminal"}
    },
    {
      seq: 9,
      kind: "prompt_terminal",
      acceptanceRole: "m1-agent1",
      turnId: "turn-1",
      payload: {outcome: "success"}
    },
    {
      seq: 10,
      kind: "meeting_v1_moderator_decision_committed",
      acceptanceRole: "m1-agent1",
      turnId: "turn-1",
      payload: {attempt_id: "attempt-1"}
    },
    {
      seq: 11,
      kind: "meeting_v1_moderator_action_submitted",
      acceptanceRole: "m1-agent1",
      turnId: "turn-1",
      payload: {
        attempt_id: "attempt-1",
        action: "select_intent",
        outcome: "accepted"
      }
    }
  '
}

evaluate() {
  jq -s \
    --argjson expected_agents 1 \
    -f scripts/meeting-v1-moderator-gates.jq
}

fixture | evaluate |
  jq -e '.passed == true and .failed_gates == []' >/dev/null

{
  fixture
  jq -nc '{
    seq: 12,
    kind: "acp_session_cancel_sent",
    acceptanceRole: "m1-agent1",
    turnId: "turn-1",
    payload: {}
  }'
} | evaluate |
  jq -e '
    .passed == false
    and (.failed_gates | index("moderator_state_driven_cancel_absent") != null)
  ' >/dev/null

fixture |
  jq -c '
    if .kind == "meeting_v1_moderator_action_submitted"
    then .payload.attempt_id = null
    else .
    end
  ' |
  evaluate |
  jq -e '
    .passed == false
    and (
      .failed_gates
      | index("agent_moderator_primary_action_attempt_bound") != null
    )
  ' >/dev/null

printf 'Meeting V1 Moderator hard-gate fixtures passed\n'
