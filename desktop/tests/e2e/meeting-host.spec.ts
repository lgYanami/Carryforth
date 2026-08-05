import { expect, test } from "@playwright/test";

import type {
  MeetingHostState,
  MeetingLoadResult,
  MeetingOpenHandoff,
  MeetingPendingIntent,
  MeetingSnapshot,
} from "../../src/shared/api/tauriMeetings";
import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

const CURRENT = "deadbeef".repeat(8);
const HUMAN = TEST_IDENTITIES.bob.pubkey;
const AGENT = TEST_IDENTITIES.charlie.pubkey;
const IDS = {
  lifecycle: "40000000-0000-4000-8000-000000000001",
  retry: "40000000-0000-4000-8000-000000000002",
  decisions: "40000000-0000-4000-8000-000000000003",
  timeout: "40000000-0000-4000-8000-000000000004",
  priority: "40000000-0000-4000-8000-000000000005",
  agentHost: "40000000-0000-4000-8000-000000000006",
  preemptedDraft: "40000000-0000-4000-8000-000000000007",
  handoff: "40000000-0000-4000-8000-000000000008",
} as const;

function pendingIntent(input: {
  id: string;
  authorPubkey: string;
  summary: string;
}): MeetingPendingIntent {
  return {
    intentId: input.id,
    currentEventId: input.id,
    authorPubkey: input.authorPubkey,
    basisSpeechRevision: 0,
    summary: input.summary,
    addressedTo: null,
    createdAtMs: Date.now() - 5_000,
    deferred: false,
    selectionAttemptCount: 0,
    lastOfferId: null,
    lastAttemptOutcome: null,
    eligibleDecisionEpoch: 1,
    selectable: true,
  };
}

function openHandoff(input: {
  id: string;
  fromPubkey: string;
  toPubkey: string;
  reason: string;
}): MeetingOpenHandoff {
  return {
    handoffId: input.id,
    sourceSpeechEventId: "9".repeat(64),
    fromPubkey: input.fromPubkey,
    toPubkey: input.toPubkey,
    reasonType: "question",
    reasonText: input.reason,
    createdAtMs: Date.now() - 4_000,
    attemptCount: 0,
    lastOfferId: null,
    lastGrantId: null,
    lastAttemptOutcome: null,
    blockedBy: null,
    moderatorRetryBlocked: false,
    eligibleDecisionEpoch: 1,
    attemptActive: false,
    selectable: true,
  };
}

function meetingSeed(input: {
  id: string;
  title: string;
  boardPhase?: "board_pending" | "floor_ready";
  boardOutcome?: "updated" | "unchanged" | "timed_out" | "preempted" | null;
  pendingIntents?: MeetingPendingIntent[];
  openHandoffs?: MeetingOpenHandoff[];
  priorityRequest?: boolean;
  agentHost?: boolean;
  consecutiveModeratorSpeeches?: number;
}) {
  const now = Date.now();
  const moderatorPubkey = input.agentHost ? AGENT : CURRENT;
  const boardPhase = input.boardPhase ?? "board_pending";
  const boardOutcome =
    input.boardOutcome === undefined
      ? boardPhase === "floor_ready"
        ? "updated"
        : null
      : input.boardOutcome;
  const floor = {
    stateEventId: "1".repeat(64),
    humanQueue: input.priorityRequest
      ? [
          {
            requestId: "2".repeat(64),
            requesterPubkey: HUMAN,
            queuePosition: 1,
            state: "offered" as const,
          },
        ]
      : [],
    offer: input.priorityRequest
      ? {
          offerId: "3".repeat(64),
          targetPubkey: HUMAN,
          targetParticipantType: "human" as const,
          allocationSource: "human_request" as const,
          turnRole: "participant" as const,
          selectionReason: null,
          sourceIntentId: null,
          sourceRequestId: "2".repeat(64),
          sourceHandoffId: null,
          sourceSpeechEventId: null,
          handoffContext: null,
          createdAtMs: now - 1_000,
          ackDeadlineMs: now + 30_000,
        }
      : null,
    grant: null,
  };
  const canSelect =
    !input.agentHost && boardPhase === "floor_ready" && !input.priorityRequest;
  const host: MeetingHostState = {
    controlToken: "4".repeat(64),
    stateEventId: floor.stateEventId,
    controlEpoch: 1,
    decisionEpoch: 1,
    decisionDeadlineMs: canSelect ? now + 90_000 : null,
    nextActionAtMs: null,
    consecutiveModeratorSpeeches: input.consecutiveModeratorSpeeches ?? 0,
    forcedReturnToModerator: false,
    pendingIntents: input.pendingIntents ?? [],
    openHandoffs: input.openHandoffs ?? [],
    boardControl: {
      phase: boardPhase,
      controlEpoch: 1,
      boardWindow: 1,
      boardStartedAtMs: now - 10_000,
      boardDeadlineAtMs:
        boardPhase === "board_pending" ? now + 120_000 : now - 5_000,
      boardCompletedAtMs: boardPhase === "floor_ready" ? now - 6_000 : null,
      boardOutcome,
    },
    canSelect,
    canClose:
      canSelect && (boardOutcome === "updated" || boardOutcome === "unchanged"),
    canRecall: false,
  };
  const snapshot: MeetingSnapshot = {
    meetingId: input.id,
    title: input.title,
    description: "Exercise the complete Human host discussion lifecycle.",
    sourceChannelId: null,
    schemaVersion: 3,
    policy: "moderated-board-actions-v2",
    hostPubkey: moderatorPubkey,
    moderatorPubkey,
    createEventId: "5".repeat(64),
    createdAt: Math.floor(now / 1_000) - 60,
    lifecycle: "active",
    phase: input.priorityRequest ? "offered" : "moderator_control",
    stateRevision: 1,
    floorRevision: 1,
    intentRevision: input.pendingIntents?.length ?? 0,
    speechRevision: 0,
    currentSpeakerPubkey: null,
    currentOfferPubkey: input.priorityRequest ? HUMAN : null,
    floor,
    host,
    participants: [
      {
        pubkey: moderatorPubkey,
        participantType: input.agentHost ? "agent" : "human",
        channelRole: "owner",
      },
      ...(moderatorPubkey === CURRENT
        ? []
        : [
            {
              pubkey: CURRENT,
              participantType: "human" as const,
              channelRole: "member",
            },
          ]),
      { pubkey: HUMAN, participantType: "human", channelRole: "member" },
      ...(moderatorPubkey === AGENT
        ? []
        : [
            {
              pubkey: AGENT,
              participantType: "agent" as const,
              channelRole: "bot",
            },
          ]),
    ],
    board: {
      eventId: "6".repeat(64),
      format: "markdown",
      body: "# Goal\nComplete the host lifecycle.\n\n## Agenda\n- Review Board\n- Arrange Speech",
      moderatorPubkey,
      updatedAt: Math.floor(now / 1_000) - 20,
      source: "projection",
    },
    action: null,
    end: null,
    latestSpeechAt: null,
  };
  const result: MeetingLoadResult = { status: "ready", snapshot };
  return { id: input.id, title: input.title, result, speeches: [] };
}

async function openMeeting(
  page: import("@playwright/test").Page,
  meetingId: string,
) {
  await page.goto("/");
  await page.getByTestId(`meeting-row-${meetingId}`).click();
  await expect(page.getByTestId("meeting-screen")).toBeVisible();
}

test("Human host completes Board, self Intent, Offer, Grant, Speech and direct Close", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [
      meetingSeed({
        id: IDS.lifecycle,
        title: "Host lifecycle",
        consecutiveModeratorSpeeches: 1,
        pendingIntents: [
          pendingIntent({
            id: "d".repeat(64),
            authorPubkey: AGENT,
            summary: "Present the participant verification",
          }),
        ],
      }),
    ],
  });
  await openMeeting(page, IDS.lifecycle);

  await expect(page.getByTestId("meeting-board-editor")).toBeVisible();
  await expect(page.getByTestId("meeting-host-console")).toContainText(
    "Board Maintenance",
  );
  await expect(page.getByText(/Board ·/)).toBeVisible();
  await expect(page.getByTestId("meeting-host-close")).toHaveCount(0);
  await page.getByTestId("meeting-participants-trigger").click();
  await expect(
    page.getByTestId(`meeting-participant-status-${AGENT}`),
  ).toHaveText("Intent pending");
  await expect(
    page.getByTestId(`meeting-participant-status-${CURRENT}`),
  ).toHaveText("Idle");
  await page.keyboard.press("Escape");

  await page
    .getByTestId("meeting-board-editor")
    .fill(
      "# Goal\nComplete the host lifecycle.\n\n## Decision\nBoard reviewed.",
    );
  await page.getByTestId("meeting-board-save").click();
  await expect(page.getByTestId("meeting-board-editor")).toHaveCount(0);
  await expect(page.getByText(/Floor ·/)).toBeVisible();
  await expect(page.getByTestId("meeting-host-close")).toBeVisible();

  await page
    .getByTestId("meeting-self-intent-new-summary")
    .fill("Summarize the final decision");
  await page.getByTestId("meeting-self-intent-submit").click();
  await expect(page.getByTestId("meeting-host-self-intent")).toBeVisible();
  await page
    .getByTestId("meeting-self-intent-summary")
    .fill("State the final decision and next check");
  await page.getByTestId("meeting-self-intent-refresh").click();
  await expect(page.getByTestId("meeting-self-intent-summary")).toHaveValue(
    "State the final decision and next check",
  );
  await page.getByTestId("meeting-self-intent-withdraw").click();
  await expect(page.getByTestId("meeting-host-self-intent")).toHaveCount(0);
  await page
    .getByTestId("meeting-self-intent-new-summary")
    .fill("State the final decision after reconsidering");
  await page.getByTestId("meeting-self-intent-submit").click();

  await expect(page.getByTestId("meeting-self-intent-deferral")).toBeVisible();
  await page
    .getByTestId("meeting-self-intent-deferral")
    .fill("Resolve the blocking conclusion before the queued verification.");
  await page.getByTestId("meeting-self-intent-select").click();
  await expect(page.getByTestId("meeting-offer-controls")).toBeVisible();
  await expect(page.getByTestId("meeting-speech-composer")).toHaveCount(0);
  await page.getByTestId("meeting-offer-accept").click();
  await expect(page.getByTestId("meeting-speech-composer")).toBeVisible();
  await page
    .getByTestId("meeting-speech-input")
    .fill("The Board now records the final decision.");
  await page.getByTestId("meeting-speech-submit").click();

  await expect(page.getByTestId("meeting-speech-timeline")).toContainText(
    "The Board now records the final decision.",
  );
  await expect(page.getByTestId("meeting-board-editor")).toBeVisible();
  await page.getByTestId("meeting-board-unchanged").click();
  await page.getByTestId("meeting-host-close").click();
  await expect(page.getByTestId("meeting-host-close-dialog")).toContainText(
    "discussion goal has been reached",
  );
  await page.getByTestId("meeting-host-close-confirm").click();
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-lifecycle",
    "closed",
  );

  const actionTypes = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter(
        (entry) =>
          entry.command === "submit_meeting_host_action" ||
          entry.command === "submit_meeting_floor_action",
      )
      .map((entry) => entry.payload.input.action.type),
  );
  expect(actionTypes).toEqual([
    "board_update",
    "intent_submit",
    "intent_refresh",
    "intent_withdraw",
    "intent_submit",
    "select_intent",
    "offer_ack",
    "speech",
    "board_unchanged",
    "close",
  ]);
});

test("indeterminate host action retries the exact event and Abort remains explicit", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetingHostIndeterminateResponses: 1,
    meetings: [meetingSeed({ id: IDS.retry, title: "Host exact retry" })],
  });
  await openMeeting(page, IDS.retry);

  await page.getByTestId("meeting-board-unchanged").click();
  await expect(page.getByTestId("meeting-host-indeterminate")).toBeVisible();
  await page.getByTestId("meeting-host-retry").click();
  await expect(page.getByTestId("meeting-host-indeterminate")).toHaveCount(0);

  const inputs = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter((entry) => entry.command === "submit_meeting_host_action")
      .map((entry) => entry.payload.input),
  );
  expect(inputs).toHaveLength(2);
  expect(inputs[1]).toEqual(inputs[0]);

  await page.getByTestId("meeting-host-abort").click();
  await expect(page.getByTestId("meeting-host-abort-dialog")).toContainText(
    "does not roll back external effects",
  );
  await page
    .getByLabel("Meeting abort category")
    .selectOption("discussion_blocked");
  await page
    .getByLabel("Meeting abort explanation")
    .fill("The required reviewer is unavailable.");
  await page.getByTestId("meeting-host-abort-confirm").click();
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-lifecycle",
    "aborted",
  );
});

test("host can reject Intent, dismiss Handoff, then select an Intent into an Offer", async ({
  page,
}) => {
  const rejectedId = "a".repeat(64);
  const selectedId = "b".repeat(64);
  const handoffId = "c".repeat(64);
  await installMockBridge(page, {
    meetings: [
      meetingSeed({
        id: IDS.decisions,
        title: "Host decision pool",
        boardPhase: "floor_ready",
        pendingIntents: [
          pendingIntent({
            id: rejectedId,
            authorPubkey: HUMAN,
            summary: "Repeat the already-settled context",
          }),
          pendingIntent({
            id: selectedId,
            authorPubkey: AGENT,
            summary: "Present the verification result",
          }),
        ],
        openHandoffs: [
          openHandoff({
            id: handoffId,
            fromPubkey: HUMAN,
            toPubkey: AGENT,
            reason: "Can the Agent verify this edge case?",
          }),
        ],
      }),
    ],
  });
  await openMeeting(page, IDS.decisions);

  await page
    .getByTestId(`meeting-host-intent-${rejectedId}`)
    .getByTestId("meeting-host-intent-reject")
    .click();
  await page
    .getByTestId(`meeting-host-intent-${rejectedId}`)
    .getByLabel("Intent rejection explanation")
    .fill("This was resolved by the current Board.");
  await page
    .getByTestId(`meeting-host-intent-${rejectedId}`)
    .getByTestId("meeting-host-intent-reject-confirm")
    .click();
  await expect(
    page.getByTestId(`meeting-host-intent-${rejectedId}`),
  ).toHaveCount(0);

  await page
    .getByTestId(`meeting-host-handoff-${handoffId}`)
    .getByTestId("meeting-host-handoff-dismiss")
    .click();
  await page
    .getByTestId(`meeting-host-handoff-${handoffId}`)
    .getByLabel("Handoff dismissal explanation")
    .fill("The Board already contains the answer.");
  await page
    .getByTestId(`meeting-host-handoff-${handoffId}`)
    .getByTestId("meeting-host-handoff-dismiss-confirm")
    .click();
  await expect(
    page.getByTestId(`meeting-host-handoff-${handoffId}`),
  ).toHaveCount(0);

  await page
    .getByTestId(`meeting-host-intent-${selectedId}`)
    .getByTestId("meeting-host-intent-select")
    .click();
  await expect(page.getByTestId("meeting-host-console")).toContainText(
    "An Offer was created",
  );
  await expect(page.getByTestId("meeting-speech-composer")).toHaveCount(0);
  await expect(page.getByTestId("meeting-host-recall")).toBeVisible();
  await page.getByTestId("meeting-host-recall").click();

  const lastHostAction = await page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
        .filter((entry) => entry.command === "submit_meeting_host_action")
        .at(-1)?.payload.input.action.type,
  );
  expect(lastHostAction).toBe("recall");
});

test("Board timeout and Human Request priority never expose a normal Close", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [
      meetingSeed({
        id: IDS.timeout,
        title: "Timed out Board",
        boardPhase: "floor_ready",
        boardOutcome: "timed_out",
      }),
      meetingSeed({
        id: IDS.priority,
        title: "Human priority",
        boardPhase: "floor_ready",
        boardOutcome: "preempted",
        priorityRequest: true,
      }),
    ],
  });
  await openMeeting(page, IDS.timeout);

  await expect(page.getByTestId("meeting-board-timeout-notice")).toContainText(
    "own full deadline",
  );
  await expect(page.getByTestId("meeting-host-close")).toHaveCount(0);
  await expect(page.getByText(/Floor ·/)).toBeVisible();
  await expect(page.getByTestId("meeting-host-idle")).toBeVisible();

  await page.getByTestId(`meeting-row-${IDS.priority}`).click();
  await expect(page.getByTestId("meeting-host-human-priority")).toContainText(
    "cannot reject or reorder",
  );
  await expect(page.getByTestId("meeting-host-recall")).toHaveCount(0);
  await expect(page.getByTestId("meeting-host-intent-select")).toHaveCount(0);
  await expect(page.getByTestId("meeting-host-close")).toHaveCount(0);
});

test("Human Request preempts a dirty Board draft and makes the old window non-submittable", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetingHostActionDelayMs: 250,
    meetings: [
      meetingSeed({ id: IDS.preemptedDraft, title: "Board preemption" }),
    ],
  });
  await openMeeting(page, IDS.preemptedDraft);

  await page
    .getByTestId("meeting-board-editor")
    .fill("# Goal\nPreserve this unsubmitted edit after preemption.");
  await page.getByTestId("meeting-board-save").click();
  const preempted = await page.evaluate(
    async ({ meetingId, requesterPubkey }) => {
      const changed = window.__BUZZ_E2E_PREEMPT_MEETING_BOARD__?.({
        meetingId,
        requesterPubkey,
      });
      await window.__BUZZ_E2E_QUERY_CLIENT__?.invalidateQueries({
        queryKey: ["meetings"],
      });
      return changed;
    },
    { meetingId: IDS.preemptedDraft, requesterPubkey: HUMAN },
  );
  expect(preempted).toBe(true);

  await expect(page.getByTestId("meeting-board-editor")).toHaveCount(0);
  await expect(page.getByTestId("meeting-stale-board-draft")).toContainText(
    "cannot be submitted against a later window",
  );
  await expect(page.getByTestId("meeting-host-human-priority")).toBeVisible();
  await expect(page.getByTestId("meeting-host-close")).toHaveCount(0);
  await expect(page.getByTestId("meeting-host-error")).toContainText(
    "control changed",
  );
});

test("host selects an open Directed Handoff into an Offer without creating a Grant", async ({
  page,
}) => {
  const handoffId = "e".repeat(64);
  await installMockBridge(page, {
    meetings: [
      meetingSeed({
        id: IDS.handoff,
        title: "Directed handoff",
        boardPhase: "floor_ready",
        openHandoffs: [
          openHandoff({
            id: handoffId,
            fromPubkey: HUMAN,
            toPubkey: AGENT,
            reason: "Please verify the proposed constraint.",
          }),
        ],
      }),
    ],
  });
  await openMeeting(page, IDS.handoff);

  await page
    .getByTestId(`meeting-host-handoff-${handoffId}`)
    .getByTestId("meeting-host-handoff-select")
    .click();
  await expect(page.getByTestId("meeting-host-console")).toContainText(
    "An Offer was created from directed handoff",
  );
  await expect(page.getByTestId("meeting-speech-composer")).toHaveCount(0);
});

test("Agent-hosted Meeting exposes no Human host mutations", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [
      meetingSeed({
        id: IDS.agentHost,
        title: "Agent hosted",
        agentHost: true,
        boardPhase: "floor_ready",
        boardOutcome: "preempted",
        priorityRequest: true,
        pendingIntents: [
          {
            ...pendingIntent({
              id: "f".repeat(64),
              authorPubkey: HUMAN,
              summary: "Verify the participant-facing evidence.",
            }),
            selectable: false,
          },
        ],
        openHandoffs: [
          {
            ...openHandoff({
              id: "e".repeat(64),
              fromPubkey: HUMAN,
              toPubkey: CURRENT,
              reason: "Confirm the read-only observation boundary.",
            }),
            selectable: false,
          },
        ],
      }),
    ],
  });
  await openMeeting(page, IDS.agentHost);

  const observation = page.getByTestId("meeting-host-observation");
  await expect(observation).toBeVisible();
  await expect(observation).toContainText("Agent host progress");
  await expect(
    page.getByTestId("meeting-host-observation-phase"),
  ).toContainText("Waiting for Floor acknowledgement");
  await expect(
    page.getByTestId("meeting-host-observation-phase"),
  ).toContainText("Board maintenance preempted by Floor priority");
  await expect(
    page.getByTestId("meeting-host-observation-floor"),
  ).toContainText("bob");
  await expect(
    page.getByTestId("meeting-host-observation-floor"),
  ).toContainText("Waiting for ACK");
  await expect(
    page.getByTestId("meeting-host-observation-intents"),
  ).toContainText("Verify the participant-facing evidence.");
  await expect(
    page.getByTestId("meeting-host-observation-handoffs"),
  ).toContainText("Confirm the read-only observation boundary.");
  await expect(
    observation.locator("button, input, select, textarea"),
  ).toHaveCount(0);
  await expect(page.getByTestId("meeting-host-console")).toHaveCount(0);
  await expect(page.getByTestId("meeting-board-editor")).toHaveCount(0);
  await expect(page.getByTestId("meeting-host-close")).toHaveCount(0);
  await expect(page.getByTestId("meeting-floor-request")).toBeVisible();
  await page.getByTestId("meeting-floor-request").click();

  const hostWrites = await page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMAND_LOG__ ?? []).filter(
        (entry) => entry.command === "submit_meeting_host_action",
      ).length,
  );
  expect(hostWrites).toBe(0);
});
