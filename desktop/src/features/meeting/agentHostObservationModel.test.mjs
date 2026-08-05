import assert from "node:assert/strict";
import test from "node:test";

import {
  agentHostActionStatus,
  agentHostBoardOutcomeLabel,
  agentHostHandoffStatus,
  agentHostIntentStatus,
  agentHostPhasePresentation,
} from "./agentHostObservationModel.ts";

function snapshot() {
  return {
    lifecycle: "active",
    currentSpeakerPubkey: null,
    currentOfferPubkey: null,
    floor: {
      offer: null,
      grant: null,
    },
    host: {
      decisionEpoch: 3,
      decisionDeadlineMs: 15_000,
      boardControl: {
        phase: "floor_ready",
        boardDeadlineAtMs: 10_000,
        boardOutcome: "updated",
      },
    },
    action: null,
  };
}

test("maps the verified host state to one stable product phase", () => {
  const value = snapshot();
  assert.equal(agentHostPhasePresentation(value).kind, "floor_decision");

  value.host.boardControl.phase = "board_pending";
  assert.deepEqual(agentHostPhasePresentation(value), {
    kind: "board_maintenance",
    title: "Board maintenance",
    description:
      "The Agent host is reviewing or updating the Board before the next Floor decision.",
    deadlineMs: 10_000,
  });

  value.host.boardControl.phase = "floor_ready";
  value.floor.offer = { ackDeadlineMs: 20_000 };
  assert.equal(agentHostPhasePresentation(value).kind, "offer");

  value.floor.grant = { hardDeadlineMs: 30_000 };
  assert.equal(agentHostPhasePresentation(value).kind, "grant");

  value.lifecycle = "finalizing_actions";
  value.action = {
    condition: "runnable",
    actionDeadlineAtMs: 40_000,
    terminalStatus: null,
  };
  assert.deepEqual(agentHostPhasePresentation(value), {
    kind: "action_finalization",
    title: "Action finalization",
    description:
      "The Agent host is recording the frozen final Board in the relevant systems.",
    deadlineMs: 40_000,
  });

  value.lifecycle = "closed";
  assert.equal(agentHostPhasePresentation(value).kind, "complete");
});

test("uses product language for Board and action outcomes", () => {
  const value = snapshot();
  assert.equal(agentHostBoardOutcomeLabel(value), "Board updated");

  value.host.boardControl.boardOutcome = "preempted";
  assert.equal(
    agentHostBoardOutcomeLabel(value),
    "Board maintenance preempted by Floor priority",
  );

  value.action = {
    condition: "blocked",
    terminalStatus: null,
  };
  assert.equal(agentHostActionStatus(value), "Action recording is blocked");

  value.action.terminalStatus = "completed_closed";
  assert.equal(
    agentHostActionStatus(value),
    "Actions confirmed and Meeting closed",
  );
});

test("summarizes Intent and Handoff readiness without exposing protocol fences", () => {
  const value = snapshot();
  assert.equal(
    agentHostIntentStatus(
      {
        deferred: false,
        lastAttemptOutcome: null,
        eligibleDecisionEpoch: 3,
        selectable: true,
      },
      value,
    ),
    "Ready for host decision",
  );
  assert.equal(
    agentHostIntentStatus(
      {
        deferred: true,
        lastAttemptOutcome: null,
        eligibleDecisionEpoch: 3,
        selectable: false,
      },
      value,
    ),
    "Deferred",
  );

  assert.equal(
    agentHostHandoffStatus({
      attemptActive: false,
      moderatorRetryBlocked: true,
      blockedBy: null,
      lastAttemptOutcome: null,
      selectable: false,
    }),
    "Retry blocked",
  );
  assert.equal(
    agentHostHandoffStatus({
      attemptActive: true,
      moderatorRetryBlocked: false,
      blockedBy: null,
      lastAttemptOutcome: null,
      selectable: false,
    }),
    "Active Offer or Grant",
  );
});
