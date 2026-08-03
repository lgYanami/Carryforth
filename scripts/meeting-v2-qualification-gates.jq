def gate($name; $pass; $observed; $expected):
  {
    gate: $name,
    pass: ($pass == true),
    observed: $observed,
    expected: $expected
  };

def required_scenarios:
  ["mixed", "all_agent", "moderator_abort", "admin_abort"];

def required_zero_invariants:
  [
    "boardFloorOverlap",
    "floorBeforeBoardTerminal",
    "boardAcceptedDuringOfferOrGrant",
    "turnWithoutBoardRead",
    "lateBoardLanded",
    "boardChangedSpeechRevision",
    "postEndRevisionChange",
    "pendingRuntimeReservations",
    "unauthorizedBoardAccess",
    "externalWrites"
  ];

def board_changed_between_intent_and_grant($events; $scenario):
  [$events[]
    | select(
        .qualificationScenario == $scenario
        and .kind == "meeting_v2_board_load_completed"
        and .payload.turn_type == "participant_intent"
      )] as $intents
  | [$events[]
      | select(
          .qualificationScenario == $scenario
          and .kind == "meeting_v2_board_load_completed"
          and .payload.turn_type == "granted_speech"
        )] as $grants
  | any(
      $intents[];
      . as $intent
      | any(
          $grants[];
          .acceptanceRole == $intent.acceptanceRole
          and .timestamp > $intent.timestamp
          and .payload.board_event_id != $intent.payload.board_event_id
        )
    );

def has_intent_and_later_grant_board_reads($events; $scenario):
  [$events[]
    | select(
        .qualificationScenario == $scenario
        and .kind == "meeting_v2_board_load_completed"
        and .payload.turn_type == "participant_intent"
      )] as $intents
  | [$events[]
      | select(
          .qualificationScenario == $scenario
          and .kind == "meeting_v2_board_load_completed"
          and .payload.turn_type == "granted_speech"
        )] as $grants
  | any(
      $intents[];
      . as $intent
      | any(
          $grants[];
          .acceptanceRole == $intent.acceptanceRole
          and .timestamp > $intent.timestamp
        )
    );

def scenario_session($meetings; $scenario):
  first($meetings[] | select(.scenario == $scenario) | .sessionId);

($manifest[0] // {}) as $m
| ($invariants[0] // {}) as $i
| ($events // []) as $e
| ($roster[0] // []) as $r
| ($meetings[0] // []) as $mtg
| ($security[0] // {}) as $security_probes
| ($i.scenarios.mixed // {}) as $mixed
| ($i.scenarios.all_agent // {}) as $all_agent
| ($i.scenarios.moderator_abort // {}) as $moderator_abort
| ($i.scenarios.admin_abort // {}) as $admin_abort
| [
    gate(
      "manifest_identity";
      ($m.evidenceSchema == "buzz-meeting-v2-qualification-v1"
        and $m.protocol.schemaVersion == "3"
        and $m.protocol.policy == "moderated-board-v1"
        and ($m.buzzCommit | type == "string" and test("^[0-9a-f]{40}$"))
        and ($m.sourceTree.statusSha256 | type == "string" and test("^[0-9a-f]{64}$"))
        and ($m.sourceTree.diffSha256 | type == "string" and test("^[0-9a-f]{64}$"))
        and ($m.sourceTree.afterStatusSha256 | type == "string" and test("^[0-9a-f]{64}$"))
        and ($m.sourceTree.afterDiffSha256 | type == "string" and test("^[0-9a-f]{64}$")));
      {
        evidenceSchema: $m.evidenceSchema,
        protocol: $m.protocol,
        buzzCommit: $m.buzzCommit,
        sourceTree: $m.sourceTree
      };
      "v1 evidence schema, v=3 + moderated-board-v1, full commit and source-tree hashes"
    ),
    gate(
      "real_provider";
      ($m.provider.real == true
        and $m.provider.authenticated == true
        and $m.provider.catalogSupported == true
        and $m.provider.adapter == "@agentclientprotocol/codex-acp"
        and $m.provider.adapterVersion == "1.1.7"
        and ($m.provider.model | type == "string" and length > 0)
        and $m.provider.moderatorReasoning == "max"
        and $m.provider.participantReasoning == "high"
        and ($m.provider.agentSessionsExercised // -1)
          == ([$r[] | select(.participantType == "agent")] | length)
        and $m.providerEvidenceVerified == true);
      ($m.provider + {evidenceVerified: $m.providerEvidenceVerified});
      "authenticated real Codex, exact ACP 1.1.7 adapter, catalog-backed model and every Agent exercised"
    ),
    gate(
      "capability_preflight";
      ($m.capabilities.relayRuntime == true
        and $m.capabilities.createEnabledObserved == true
        and $m.capabilities.createDisabledDrainObserved == true
        and $m.capabilities.acpV2Participant == true
        and $m.capabilities.acpV2Moderator == true
        and $m.capabilityEvidenceVerified == true);
      ($m.capabilities + {evidenceVerified: $m.capabilityEvidenceVerified});
      "Relay runtime/create/drain and acceptance-enabled ACP participant/moderator capabilities"
    ),
    gate(
      "artifact_integrity";
      ($m.sha256Verified == true
        and $m.metricsEvidenceVerified == true
        and $m.processEvidenceVerified == true);
      {
        sha256Verified: $m.sha256Verified,
        metricsEvidenceVerified: $m.metricsEvidenceVerified,
        processEvidenceVerified: $m.processEvidenceVerified
      };
      "all immutable artifacts hashed, required metrics present, and every declared Relay/Agent process log verified"
    ),
    gate(
      "workspace_and_external_effects";
      ($m.workspaceVerified == true
        and $m.workspaceChanged == false
        and $m.projectViewDependencies == 0
        and $m.externalWrites == 0);
      {
        workspaceVerified: $m.workspaceVerified,
        workspaceChanged: $m.workspaceChanged,
        projectViewDependencies: $m.projectViewDependencies,
        externalWrites: $m.externalWrites
      };
      "matching workspace snapshots and no declared workspace, Project View, or external writes"
    ),
    gate(
      "scenario_and_roster_topology";
      (($mtg | length) == 4
        and ([$mtg[].scenario] | sort) == (required_scenarios | sort)
        and ([$mtg[].sessionId] | unique | length) == 4
        and all(required_scenarios[];
          . as $scenario
          | ([ $r[] | select(.scenario == $scenario and .meetingRole == "moderator") ] | length) == 1
          and ([ $r[]
                 | select(
                     .scenario == $scenario
                     and .meetingRole == "moderator"
                     and .participantType == "agent"
                   ) ] | length) == 1
          and $i.scenarios[$scenario].sessionId == scenario_session($mtg; $scenario)
          and $m.scenarios[$scenario].sessionId == scenario_session($mtg; $scenario))
        and ([$r[] | [.scenario, .pubkey] | join(":")] | unique | length) == ($r | length));
      {
        meetings: $mtg,
        rosterCounts: (reduce required_scenarios[] as $scenario ({};
          .[$scenario] = {
            humans: ([$r[] | select(.scenario == $scenario and .participantType == "human")] | length),
            agents: ([$r[] | select(.scenario == $scenario and .participantType == "agent")] | length),
            moderators: ([$r[] | select(.scenario == $scenario and .meetingRole == "moderator")] | length)
          }))
      };
      "exact four-scenario matrix, one Agent moderator per Meeting, unique roster entries and matching Session IDs"
    ),
    gate(
      "observer_topology_and_privacy";
      (($e | length) > 0
        and ([$e[].qualificationScenario] | unique | sort) == (required_scenarios | sort)
        and ([$e[] | [.qualificationScenario, .acceptanceRole] | join(":")] | unique | sort)
          == ([$r[]
                | select(.participantType == "agent")
                | [.scenario, .role]
                | join(":")] | unique | sort)
        and all($e[];
          . as $event
          | ($event.qualificationScenario | IN("mixed", "all_agent", "moderator_abort", "admin_abort"))
          and any($r[];
            .scenario == $event.qualificationScenario
            and .role == $event.acceptanceRole
            and .participantType == "agent")
          and ($event.channelId == null
            or $event.channelId == scenario_session($mtg; $event.qualificationScenario)))
        and ([
          $e[]
          | ..
          | objects
          | keys[]
          | select(IN("content", "prompt", "raw", "rawOutput", "raw_output", "error"))
        ] | length) == 0);
      {
        eventCount: ($e | length),
        scenarios: ([$e[].qualificationScenario] | unique | sort),
        roles: ([$e[].acceptanceRole] | unique | length)
      };
      "every Agent and scenario emits evidence mapped to its declared Meeting, with no content, prompt, raw output, or raw error field"
    ),
    gate(
      "participant_current_board_refresh";
      (all(["mixed", "all_agent"][];
          . as $scenario
          | has_intent_and_later_grant_board_reads($e; $scenario)
          and ($i.scenarios[$scenario].boardChangedBetweenIntentAndGrant ==
            board_changed_between_intent_and_grant($e; $scenario)))
        and any(["mixed", "all_agent"][];
          . as $scenario
          | board_changed_between_intent_and_grant($e; $scenario)));
      {
        mixed: {
          hasIntentAndGrantReads: has_intent_and_later_grant_board_reads($e; "mixed"),
          boardChanged: board_changed_between_intent_and_grant($e; "mixed"),
          databaseBoardChanged: $mixed.boardChangedBetweenIntentAndGrant
        },
        allAgent: {
          hasIntentAndGrantReads: has_intent_and_later_grant_board_reads($e; "all_agent"),
          boardChanged: board_changed_between_intent_and_grant($e; "all_agent"),
          databaseBoardChanged: $all_agent.boardChangedBetweenIntentAndGrant
        }
      };
      "both successful scenarios independently read current Board for Intent and Grant; at least one observes a changed Board, with DB/observer agreement"
    ),
    gate(
      "mixed_roster";
      (($mixed.humans // -1) >= 2
        and ($mixed.agents // -1) >= 2
        and $mixed.humans == ([$r[] | select(.scenario == "mixed" and .participantType == "human")] | length)
        and $mixed.agents == ([$r[] | select(.scenario == "mixed" and .participantType == "agent")] | length));
      {humans: $mixed.humans, agents: $mixed.agents};
      "at least two Humans and two Agents, cross-checked against roster.tsv"
    ),
    gate(
      "mixed_lifecycle";
      (($mixed.boardUpdates // 0) >= 2
        and ($mixed.floorDecisions // 0) >= 2
        and ($mixed.distinctSpeakers // 0) >= 3
        and ($mixed.humanBoardPreemptions // 0) >= 1
        and ([$e[] | select(
          .qualificationScenario == "mixed"
          and .kind == "meeting_v2_host_turn_discarded"
        )] | length) >= 1
        and ($mixed.resolvedHandoffs // 0) >= 1
        and ($mixed.moderatorSelfSpeeches // 0) >= 1
        and any($e[];
          . as $event
          | $event.qualificationScenario == "mixed"
          and $event.acceptanceRole == first($r[] | select(.scenario == "mixed" and .meetingRole == "moderator") | .role)
          and $event.kind == "meeting_v1_speech_submitted")
        and $mixed.terminalOutcome == "closed");
      $mixed;
      "two Board updates, multiple speakers, fresh Grant Board, Human preemption, Handoff, moderator self Speech, closed"
    ),
    gate(
      "all_agent_roster";
      (($all_agent.humans // -1) == 0
        and ($all_agent.agents // -1) >= 3
        and $all_agent.humans == ([$r[] | select(.scenario == "all_agent" and .participantType == "human")] | length)
        and $all_agent.agents == ([$r[] | select(.scenario == "all_agent" and .participantType == "agent")] | length));
      {humans: $all_agent.humans, agents: $all_agent.agents};
      "one moderator Agent and at least two participant Agents, cross-checked against roster.tsv"
    ),
    gate(
      "all_agent_lifecycle";
      (($all_agent.boardUpdates // 0) >= 2
        and ($all_agent.floorDecisions // 0) >= 2
        and ($all_agent.distinctSpeakers // 0) >= 3
        and ($all_agent.resolvedHandoffs // 0) >= 1
        and ($all_agent.moderatorSelfSpeeches // 0) >= 1
        and any($e[];
          . as $event
          | $event.qualificationScenario == "all_agent"
          and $event.acceptanceRole == first($r[] | select(.scenario == "all_agent" and .meetingRole == "moderator") | .role)
          and $event.kind == "meeting_v1_speech_submitted")
        and $all_agent.terminalOutcome == "closed");
      $all_agent;
      "all-Agent multi-round Board/Floor/Handoff/moderator-self-Speech lifecycle closes normally"
    ),
    gate(
      "moderator_abort";
      ($moderator_abort.terminalOutcome == "aborted"
        and $moderator_abort.initiator == "moderator_agent"
        and ($moderator_abort.reasonCode | type == "string" and length > 0)
        and any($e[];
          .qualificationScenario == "moderator_abort"
          and .kind == "meeting_v2_floor_turn_completed"
          and .payload.action == "ABORT"
          and .payload.reason_code == $moderator_abort.reasonCode));
      $moderator_abort;
      "moderator Agent actively aborts with a structured reason and observer evidence"
    ),
    gate(
      "admin_abort";
      ($admin_abort.terminalOutcome == "aborted"
        and ($admin_abort.initiator == "admin" or $admin_abort.initiator == "security")
        and $admin_abort.reasonCode == "participant_revoked");
      $admin_abort;
      "admin or security revocation independently aborts the Meeting"
    ),
    gate(
      "security_probes";
      ($m.securityProbes.outsiderReadDenied == true
        and $m.securityProbes.outsiderBoardWriteDenied == true
        and $m.securityProbes.createDisabledDenied == true
        and $m.securityProbes.postEndWriteDenied == true
        and $m.securityProbes == $security_probes);
      {manifest: $m.securityProbes, artifact: $security_probes};
      "non-participant read/write and post-End mutation are all denied"
    ),
    gate(
      "runtime_health";
      (($m.runtimeAnomalies // -1) == 0);
      {runtimeAnomalies: $m.runtimeAnomalies};
      "no ACP respawn, panic, unsupported model, auth failure, or unexpected runtime anomaly"
    ),
    gate(
      "zero_invariants";
      (($i.zero | type) == "object"
        and all(required_zero_invariants[];
          . as $key | (($i.zero | has($key)) and $i.zero[$key] == 0))
        and all(($i.zero // {})[]; . == 0));
      ($i.zero // {});
      "every required hard-invariant counter exists and equals zero"
    ),
    gate(
      "v2_observer_evidence";
      (all(["mixed", "all_agent"][];
        . as $scenario
        | any($e[];
          .qualificationScenario == $scenario
          and .kind == "meeting_v2_board_load_completed"
          and .payload.turn_type == "moderator_board")
        and any($e[];
          .qualificationScenario == $scenario
          and .kind == "meeting_v2_board_load_completed"
          and .payload.turn_type == "moderator_floor")
        and any($e[];
          .qualificationScenario == $scenario
          and .kind == "meeting_v2_board_load_completed"
          and .payload.turn_type == "participant_intent")
        and any($e[];
          .qualificationScenario == $scenario
          and .kind == "meeting_v2_board_load_completed"
          and .payload.turn_type == "granted_speech")
        and any($e[];
          .qualificationScenario == $scenario
          and .kind == "meeting_v2_board_turn_completed")
        and any($e[];
          .qualificationScenario == $scenario
          and .kind == "meeting_v1_speech_submitted")
        and any($e[];
          .qualificationScenario == $scenario
          and .kind == "meeting_v1_state_applied")));
      {
        boardLoads: ([$e[] | select(.kind == "meeting_v2_board_load_completed")] | length),
        boardTurns: ([$e[] | select(.kind == "meeting_v2_board_turn_completed")] | length),
        floorTurns: ([$e[] | select(
          .kind == "meeting_v2_floor_turn_completed"
          or .kind == "meeting_v1_moderator_decision_completed"
        )] | length),
        speeches: ([$e[] | select(.kind == "meeting_v1_speech_submitted")] | length),
        states: ([$e[] | select(.kind == "meeting_v1_state_applied")] | length)
      };
      "both successful discussion scenarios have per-Turn Board reads plus Board/Floor/Speech/State evidence"
    )
  ] as $gates
| {
    evidenceSchema: "buzz-meeting-v2-qualification-gates-v1",
    passed: all($gates[]; .pass),
    failedGates: [$gates[] | select(.pass != true) | .gate],
    gates: $gates
  }
