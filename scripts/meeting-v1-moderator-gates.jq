def gate($name; $pass; $observed; $expected):
  {
    gate: $name,
    pass: $pass,
    observed: $observed,
    expected: $expected
  };

def events($source; $kind):
  [$source[] | select(.kind == $kind)];

def all_true:
  if length == 0 then true else all(.[]; . == true) end;

. as $all
| events($all; "meeting_v1_moderator_decision_started") as $starts
| [$starts[].turnId | select(type == "string")] as $moderator_turn_ids
| events($all; "meeting_v1_moderator_decision_completed") as $completed
| events($all; "meeting_v1_moderator_attempt_registered") as $registered
| [events($all; "harness_started")[].acceptanceRole] | unique as $harness_roles
| events($all; "turn_started") as $turns
| events($all; "prompt_request_started") as $prompt_requests
| [$prompt_requests[].acceptanceRole] | unique as $turn_roles
| events($all; "model_applied") as $models
| [
    gate(
      "acceptance_feature_enabled";
      ((events($all; "harness_started") | length) == $expected_agents
        and (events($all; "harness_started")
          | [.[].payload.meetingV1Acceptance == true]
          | all_true));
      (events($all; "harness_started")
        | map({acceptance: .payload.meetingV1Acceptance})
        | length);
      $expected_agents
    ),
    gate(
      "exercised_session_model_applied";
      (
        ($models
          | [
              .[]
              | if (.acceptanceRole | test("-agent1$"))
                then .payload.model_id == "gpt-5.6-sol[max]"
                else .payload.model_id == "gpt-5.6-sol[high]"
                end
            ]
          | all_true)
        and
        ($prompt_requests
          | [
              .[] as $prompt
              | any(
                  $models[];
                  .acceptanceRole == $prompt.acceptanceRole
                  and .seq < $prompt.seq
                  and (
                    if ($prompt.acceptanceRole | test("-agent1$"))
                    then .payload.model_id == "gpt-5.6-sol[max]"
                    else .payload.model_id == "gpt-5.6-sol[high]"
                    end
                  )
                )
            ]
          | all_true)
      );
      {
        exercised_roles: $turn_roles,
        applied: ($models | map({
          role: .acceptanceRole,
          model_id: .payload.model_id
        }))
      };
      "every exercised ACP Session applies gpt-5.6-sol[max|high] before its Prompt"
    ),
    gate(
      "one_prompt_request_per_started_turn";
      ($turns
        | [
            .[] as $turn
            | ([
                $prompt_requests[]
                | select(
                    .acceptanceRole == $turn.acceptanceRole
                    and .turnId == $turn.turnId
                  )
              ] | length) == 1
          ]
        | all_true);
      {
        turns: ($turns | length),
        prompt_requests: ($prompt_requests | length)
      };
      "exactly one session/prompt request for every started harness Turn"
    ),
    gate(
      "adapter_process_identity_complete";
      (
        (events($all; "agent_process_started") | length) == $expected_agents
        and
        ($harness_roles
          | [
              .[] as $role
              | ([events($all; "agent_process_started")[]
                  | select(.acceptanceRole == $role)] | length) == 1
            ]
          | all_true)
      );
      (events($all; "agent_process_started")
        | map({role: .acceptanceRole, pid: .payload.pid}));
      "exactly one adapter process per harness"
    ),
    gate(
      "adapter_process_did_not_respawn";
      ((events($all; "respawn_scheduled") + events($all; "respawn_completed") + events($all; "respawn_failed"))
        | length) == 0;
      ((events($all; "respawn_scheduled") + events($all; "respawn_completed") + events($all; "respawn_failed"))
        | length);
      0
    ),
    gate(
      "speculative_moderator_agenda_absent";
      (events($all; "meeting_v1_moderator_agenda_started") | length) == 0;
      (events($all; "meeting_v1_moderator_agenda_started") | length);
      0
    ),
    gate(
      "moderator_decision_exercised";
      ($starts | length) > 0;
      ($starts | length);
      ">0"
    ),
    gate(
      "moderator_dispatch_only_with_control";
      ($starts
        | [.[].payload.phase | . == "moderator_control" or . == "moderator_idle"]
        | all_true);
      ($starts | map(.payload.phase));
      ["moderator_control", "moderator_idle"]
    ),
    gate(
      "decision_start_matches_registered_attempt";
      ($starts
        | [
            .[] as $start
            | ([
                $registered[]
                | select(
                    .payload.attempt_id == $start.payload.attempt_id
                    and .payload.candidate_snapshot_hash == $start.payload.candidate_snapshot_hash
                    and .payload.control_epoch == $start.payload.control_epoch
                    and .payload.decision_epoch == $start.payload.decision_epoch
                  )
              ] | length) == 1
          ]
        | all_true);
      ($starts | length);
      "every decision start has one matching Relay-registered attempt"
    ),
    gate(
      "candidate_cohort_evidence_complete";
      ($starts
        | [
            .[]
            | (.payload.attempt_id | type == "string")
              and (.payload.candidate_snapshot_hash | type == "string")
              and (.payload.candidate_count == (.payload.candidate_sources | length))
          ]
        | all_true);
      ($starts
        | map({
            attempt_id: .payload.attempt_id,
            candidate_count: .payload.candidate_count,
            source_count: (.payload.candidate_sources | length)
          }));
      "attempt/hash present and candidate_count == candidate_sources length"
    ),
    gate(
      "one_natural_terminal_per_moderator_turn";
      ($starts
        | [
            .[] as $start
            | ([
                $completed[]
                | select(.turnId == $start.turnId and .payload.outcome == "natural_terminal")
              ] | length) == 1
          ]
        | all_true);
      ($completed | map({turn_id: .turnId, outcome: .payload.outcome}));
      "exactly one natural terminal for every Moderator turn"
    ),
    gate(
      "one_prompt_terminal_per_moderator_turn";
      ($starts
        | [
            .[] as $start
            | ([
                $all[]
                | select(.kind == "prompt_terminal" and .turnId == $start.turnId)
              ] | length) == 1
          ]
        | all_true);
      ([
        $all[]
        | select(
            .kind == "prompt_terminal"
            and (.turnId as $id | $moderator_turn_ids | index($id)) != null
          )
        | {turn_id: .turnId, outcome: .payload.outcome}
      ]);
      "exactly one prompt terminal for every Moderator turn"
    ),
    gate(
      "moderator_state_driven_cancel_absent";
      ([
        $all[]
        | select(
            (.kind == "acp_cancel_requested" or .kind == "acp_session_cancel_sent")
            and (.turnId as $id | $moderator_turn_ids | index($id)) != null
          )
      ] | length) == 0;
      ([
        $all[]
        | select(
            (.kind == "acp_cancel_requested" or .kind == "acp_session_cancel_sent")
            and (.turnId as $id | $moderator_turn_ids | index($id)) != null
          )
      ] | length);
      0
    ),
    gate(
      "moderator_cancelled_terminal_absent";
      ([
        $all[]
        | select(
            .kind == "prompt_terminal"
            and .payload.outcome == "cancelled"
            and (.turnId as $id | $moderator_turn_ids | index($id)) != null
          )
      ] | length) == 0;
      ([
        $all[]
        | select(
            .kind == "prompt_terminal"
            and .payload.outcome == "cancelled"
            and (.turnId as $id | $moderator_turn_ids | index($id)) != null
          )
      ] | length);
      0
    ),
    gate(
      "cancel_drain_timeout_absent";
      ([
        $all[]
        | select(
            .kind == "prompt_terminal"
            and .payload.outcome == "cancel_drain_timeout"
          )
      ] | length) == 0;
      ([
        $all[]
        | select(
            .kind == "prompt_terminal"
            and .payload.outcome == "cancel_drain_timeout"
          )
      ] | length);
      0
    ),
    gate(
      "moderator_action_uncertain_absent";
      ([
        $all[]
        | select(
            (.kind | startswith("meeting_v1_"))
            and (.kind | endswith("_submitted"))
            and .payload.outcome == "uncertain"
          )
      ] | length) == 0;
      ([
        $all[]
        | select(
            (.kind | startswith("meeting_v1_"))
            and (.kind | endswith("_submitted"))
            and .payload.outcome == "uncertain"
          )
      ] | length);
      0
    ),
    gate(
      "agent_moderator_primary_action_attempt_bound";
      ([
        $all[]
        | select(
            .kind == "meeting_v1_moderator_action_submitted"
            and (
              .payload.action == "select_intent"
              or .payload.action == "select_handoff"
              or .payload.action == "moderator_speak"
              or .payload.action == "withdraw_self"
            )
            and (
              ((.payload.attempt_id // "") == "")
              or (
                .payload.attempt_id as $attempt
                | any(
                    $registered[];
                    .payload.attempt_id == $attempt
                  )
                | not
              )
            )
          )
      ] | length) == 0;
      ([
        $all[]
        | select(
            .kind == "meeting_v1_moderator_action_submitted"
            and (
              .payload.action == "select_intent"
              or .payload.action == "select_handoff"
              or .payload.action == "moderator_speak"
              or .payload.action == "withdraw_self"
            )
            and (
              ((.payload.attempt_id // "") == "")
              or (
                .payload.attempt_id as $attempt
                | any(
                    $registered[];
                    .payload.attempt_id == $attempt
                  )
                | not
              )
            )
          )
      ] | length);
      0
    ),
    gate(
      "moderator_turn_has_exactly_one_disposition";
      ($starts
        | [
            .[] as $start
            | ([
                $all[]
                | select(
                    (
                      .kind == "meeting_v1_moderator_decision_committed"
                      or .kind == "meeting_v1_moderator_decision_discarded"
                      or .kind == "meeting_v1_moderator_decision_retry_requested"
                    )
                    and .payload.attempt_id == $start.payload.attempt_id
                  )
              ] | length) == 1
          ]
        | all_true);
      ($starts | map({attempt_id: .payload.attempt_id, turn_id: .turnId}));
      "every Moderator Turn is committed, discarded, or retry-required exactly once"
    )
  ]
| {
    passed: all(.[]; .pass),
    gate_count: length,
    failed_gates: [.[] | select(.pass == false) | .gate],
    gates: .
  }
