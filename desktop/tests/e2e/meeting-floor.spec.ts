import { expect, test } from "@playwright/test";

import type {
  MeetingFloorState,
  MeetingLoadResult,
  MeetingSnapshot,
} from "../../src/shared/api/tauriMeetings";
import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

const CURRENT = "deadbeef".repeat(8);
const HOST = TEST_IDENTITIES.alice.pubkey;
const HUMAN = TEST_IDENTITIES.bob.pubkey;
const AGENT = TEST_IDENTITIES.charlie.pubkey;
const MEETING_ID = "30000000-0000-4000-8000-000000000001";

type FloorMode =
  | "moderator_control"
  | "offer_self"
  | "grant_self"
  | "grant_other";

function floorFor(mode: FloorMode): MeetingFloorState {
  const now = Date.now();
  return {
    stateEventId: "1".repeat(64),
    humanQueue: [],
    offer:
      mode === "offer_self"
        ? {
            offerId: "a".repeat(64),
            targetPubkey: CURRENT,
            targetParticipantType: "human",
            allocationSource: "directed_handoff",
            turnRole: "participant",
            selectionReason: null,
            sourceIntentId: null,
            sourceRequestId: null,
            sourceHandoffId: "c".repeat(64),
            sourceSpeechEventId: "d".repeat(64),
            handoffContext: {
              fromPubkey: HUMAN,
              reasonType: "question",
              reasonText: "Can you confirm the Desktop retry boundary?",
            },
            createdAtMs: now,
            ackDeadlineMs: now + 60_000,
          }
        : null,
    grant:
      mode === "grant_self" || mode === "grant_other"
        ? {
            grantId: "b".repeat(64),
            holderPubkey: mode === "grant_self" ? CURRENT : AGENT,
            allocationSource: "moderator_select",
            turnRole: "participant",
            selectionReason: "Contribute to the review",
            sourceIntentId: "e".repeat(64),
            sourceRequestId: null,
            sourceHandoffId: null,
            sourceSpeechEventId: null,
            handoffContext: null,
            createdAtMs: now,
            softLeaseExpiresAtMs: now + 60_000,
            hardDeadlineMs: now + 120_000,
            progressSeq: 0,
          }
        : null,
  };
}

function meetingSeed(
  mode: FloorMode,
  options?: { currentType?: "human" | "agent"; host?: string },
) {
  const host = options?.host ?? HOST;
  const floor = floorFor(mode);
  const phase =
    mode === "offer_self"
      ? "offered"
      : mode === "grant_self" || mode === "grant_other"
        ? "granted"
        : "moderator_control";
  const snapshot: MeetingSnapshot = {
    meetingId: MEETING_ID,
    title: "Human Floor lifecycle",
    description: "Exercise the verified Desktop Floor boundary.",
    sourceChannelId: null,
    schemaVersion: 3,
    policy: "moderated-board-actions-v3",
    hostPubkey: host,
    moderatorPubkey: host,
    createEventId: "c".repeat(64),
    createdAt: Math.floor(Date.now() / 1_000) - 60,
    lifecycle: "active",
    phase,
    stateRevision: 1,
    floorRevision: 1,
    intentRevision: 0,
    speechRevision: 0,
    currentSpeakerPubkey:
      mode === "grant_self" ? CURRENT : mode === "grant_other" ? AGENT : null,
    currentOfferPubkey: mode === "offer_self" ? CURRENT : null,
    floor,
    host: {
      controlToken: "2".repeat(64),
      stateEventId: floor.stateEventId,
      controlEpoch: 1,
      decisionEpoch: 1,
      decisionDeadlineMs:
        mode === "moderator_control" ? Date.now() + 90_000 : null,
      nextActionAtMs: null,
      consecutiveModeratorSpeeches: 0,
      forcedReturnToModerator: false,
      pendingIntents: [],
      openHandoffs: [],
      boardControl: {
        phase: "floor_ready",
        controlEpoch: 1,
        boardWindow: 1,
        boardStartedAtMs: Date.now() - 10_000,
        boardDeadlineAtMs: Date.now() - 5_000,
        boardCompletedAtMs: Date.now() - 6_000,
        boardOutcome: "updated",
      },
      canSelect: mode === "moderator_control",
      canClose: mode === "moderator_control",
      canRecall: mode === "offer_self" || mode === "grant_other",
    },
    participants: [
      {
        pubkey: host,
        participantType: "human",
        channelRole: "owner",
      },
      ...(host === CURRENT
        ? []
        : [
            {
              pubkey: CURRENT,
              participantType: options?.currentType ?? ("human" as const),
              channelRole: "member",
            },
          ]),
      { pubkey: HUMAN, participantType: "human", channelRole: "member" },
      { pubkey: AGENT, participantType: "agent", channelRole: "bot" },
    ],
    board: {
      eventId: "d".repeat(64),
      format: "markdown",
      body: "# Goal\nVerify Human Floor participation.\n\n## Agenda\n- Request\n- Speech",
      moderatorPubkey: host,
      updatedAt: Math.floor(Date.now() / 1_000),
      source: "projection",
    },
    action: null,
    end: null,
    latestSpeechAt: null,
  };
  const result: MeetingLoadResult = { status: "ready", snapshot };
  return {
    id: MEETING_ID,
    title: snapshot.title,
    result,
    speeches: [],
  };
}

async function openMeeting(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.getByTestId(`meeting-row-${MEETING_ID}`).click();
  await expect(page.getByTestId("meeting-screen")).toBeVisible();
}

test("Human completes Request, Offer, Grant, Speech and atomic Directed Handoff", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetingFloorActionDelayMs: 200,
    meetings: [meetingSeed("moderator_control")],
  });
  await openMeeting(page);

  await page.getByTestId("meeting-floor-request").click();
  await expect(page.getByTestId("meeting-offer-controls")).toBeVisible();
  await expect(
    page.getByTestId(`meeting-attention-${MEETING_ID}`),
  ).toBeVisible();

  await page.getByTestId("meeting-offer-accept").click();
  await expect(page.getByTestId("meeting-speech-composer")).toHaveCount(0);
  await expect(page.getByTestId("meeting-speech-composer")).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
            .filter(
              (entry) => entry.command === "ensure_meeting_human_grant_renewal",
            )
            .at(-1)?.payload.input ?? null,
      ),
    )
    .not.toBeNull();
  const renewalInput = await page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
        .filter(
          (entry) => entry.command === "ensure_meeting_human_grant_renewal",
        )
        .at(-1)?.payload.input,
  );
  expect(renewalInput).toBeDefined();
  expect(renewalInput).toMatchObject({ meetingId: MEETING_ID });
  expect(renewalInput?.grantId).toMatch(/^[0-9a-f]{64}$/);

  const speechInput = page.getByTestId("meeting-speech-input");
  await speechInput.fill("The exact-retry boundary is ready for review.");
  await page.getByTestId("meeting-board-trigger").click();
  await expect(page.getByTestId("meeting-board-wide")).toBeHidden();
  await expect(speechInput).toHaveValue(
    "The exact-retry boundary is ready for review.",
  );
  await page.getByTestId("meeting-board-trigger").click();
  await expect(page.getByTestId("meeting-board-wide")).toBeVisible();
  await expect(speechInput).toHaveValue(
    "The exact-retry boundary is ready for review.",
  );
  await page.getByTestId("meeting-handoff-toggle").click();
  await page.getByTestId("meeting-handoff-target").selectOption(HUMAN);
  await page
    .getByTestId("meeting-handoff-reason")
    .fill("Please verify the participant-facing behavior.");
  await page.getByTestId("meeting-speech-submit").click();

  await expect(page.getByTestId("meeting-speech-timeline")).toContainText(
    "The exact-retry boundary is ready for review.",
  );
  await expect(page.getByTestId("meeting-speech-identity-1")).toHaveText(
    "human",
  );
  const renderedHandoff = page.getByTestId("meeting-speech-handoff-1");
  await expect(renderedHandoff).toContainText("Question");
  await expect(renderedHandoff).toContainText("bob");
  await expect(renderedHandoff).toContainText(
    "Please verify the participant-facing behavior.",
  );
  await expect(page.getByTestId("meeting-speech-composer")).toHaveCount(0);
  await expect(page.getByTestId(`meeting-attention-${MEETING_ID}`)).toHaveCount(
    0,
  );
  const submittedSpeechInput = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter((entry) => entry.command === "submit_meeting_floor_action")
      .map((entry) => entry.payload.input)
      .find((input) => input.action.type === "speech"),
  );
  expect(submittedSpeechInput.action.handoff).toEqual({
    targetPubkey: HUMAN,
    handoffType: "question",
    reason: "Please verify the participant-facing behavior.",
  });
});

test("queued Human request can be withdrawn without interrupting a Grant", async ({
  page,
}) => {
  await installMockBridge(page, { meetings: [meetingSeed("grant_other")] });
  await openMeeting(page);

  await page.getByTestId("meeting-floor-request").click();
  await expect(page.getByTestId("meeting-floor-withdraw")).toBeVisible();
  await expect(page.getByTestId("meeting-status-strip")).toContainText(
    "has the floor",
  );
  await page.getByTestId("meeting-participants-trigger").click();
  await expect(
    page.getByTestId(`meeting-participant-status-${CURRENT}`),
  ).toHaveText("Floor requested");
  await expect(
    page.getByTestId(`meeting-participant-${CURRENT}`),
  ).toContainText("Queue 1");
  await expect(
    page.getByTestId(`meeting-participant-status-${AGENT}`),
  ).toHaveText("Speaking");
  await page.keyboard.press("Escape");
  await page.getByTestId("meeting-floor-withdraw").click();
  await expect(page.getByTestId("meeting-floor-request")).toBeVisible();
  await expect(page.getByTestId("meeting-speech-composer")).toHaveCount(0);
});

test("Human can decline an addressed Offer with a bounded reason", async ({
  page,
}) => {
  await installMockBridge(page, { meetings: [meetingSeed("offer_self")] });
  await openMeeting(page);

  await page.getByTestId("meeting-participants-trigger").click();
  await expect(
    page.getByTestId(`meeting-participant-status-${CURRENT}`),
  ).toHaveText("Waiting for ACK");
  await page.keyboard.press("Escape");
  await page.getByTestId("meeting-offer-decline").click();
  await page
    .getByTestId("meeting-offer-decline-reason")
    .fill("I do not have the evidence yet.");
  await page.getByTestId("meeting-offer-decline-confirm").click();
  await expect(page.getByTestId("meeting-floor-request")).toBeVisible();
  const decline = await page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
        .filter((entry) => entry.command === "submit_meeting_floor_action")
        .at(-1)?.payload.input.action,
  );
  expect(decline).toEqual({
    type: "offer_decline",
    reason: "I do not have the evidence yet.",
  });
});

test("expired Offer disables local action and only refreshes authoritative state", async ({
  page,
}) => {
  const seed = meetingSeed("offer_self");
  if (seed.result.status !== "ready" || !seed.result.snapshot.floor?.offer) {
    throw new Error("test requires an active Offer");
  }
  seed.result.snapshot.floor.offer.ackDeadlineMs = Date.now() - 1_000;
  await installMockBridge(page, { meetings: [seed] });
  await openMeeting(page);

  await expect(page.getByTestId("meeting-offer-accept")).toBeDisabled();
  await expect(page.getByTestId("meeting-offer-controls")).toContainText(
    "Checking authoritative state",
  );
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window.__BUZZ_E2E_COMMAND_LOG__ ?? []).filter(
            (entry) => entry.command === "get_meeting_snapshot",
          ).length,
      ),
    )
    .toBeGreaterThan(1);
  const floorWrites = await page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMAND_LOG__ ?? []).filter(
        (entry) => entry.command === "submit_meeting_floor_action",
      ).length,
  );
  expect(floorWrites).toBe(0);
});

test("Agent Grant never starts a Human Desktop renewal", async ({ page }) => {
  await installMockBridge(page, { meetings: [meetingSeed("grant_other")] });
  await openMeeting(page);

  await expect(page.getByTestId("meeting-status-strip")).toContainText(
    "has the floor",
  );
  const renewals = await page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMAND_LOG__ ?? []).filter(
        (entry) => entry.command === "ensure_meeting_human_grant_renewal",
      ).length,
  );
  expect(renewals).toBe(0);
});

test("Human Grant renewal failure is visible without discarding the draft", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetingGrantRenewalError: "mock renewal unavailable",
    meetings: [meetingSeed("grant_self")],
  });
  await openMeeting(page);

  const speechInput = page.getByTestId("meeting-speech-input");
  await speechInput.fill("Preserve this contribution while renewal recovers.");
  await expect(page.getByTestId("meeting-grant-renewal-error")).toBeVisible();
  await expect(speechInput).toHaveValue(
    "Preserve this contribution while renewal recovers.",
  );
});

test("Yield consumes the Grant and preserves a non-replayable stale draft", async ({
  page,
}) => {
  await installMockBridge(page, { meetings: [meetingSeed("grant_self")] });
  await openMeeting(page);

  await page
    .getByTestId("meeting-speech-input")
    .fill("Keep this draft available for manual copy.");
  await page.getByTestId("meeting-yield").click();
  await page.getByTestId("meeting-yield-confirm").click();

  await expect(page.getByTestId("meeting-speech-composer")).toHaveCount(0);
  await expect(page.getByTestId("meeting-stale-speech-draft")).toContainText(
    "preserved",
  );
  await expect(page.getByTestId("meeting-floor-request")).toBeVisible();
  const yieldAction = await page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
        .filter((entry) => entry.command === "submit_meeting_floor_action")
        .at(-1)?.payload.input.action,
  );
  expect(yieldAction).toEqual({
    type: "grant_yield",
    reasonCode: "cancelled",
  });
});

test("indeterminate Speech retries the exact submission and materializes once", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetingFloorIndeterminateResponses: 1,
    meetings: [meetingSeed("grant_self")],
  });
  await openMeeting(page);

  await page
    .getByTestId("meeting-speech-input")
    .fill("This Speech must exist exactly once.");
  await page.getByTestId("meeting-speech-submit").click();
  await expect(page.getByTestId("meeting-floor-indeterminate")).toBeVisible();
  await expect(page.getByTestId("meeting-floor-retry")).toBeVisible();
  await page.getByTestId("meeting-floor-retry").click();

  await expect(page.getByTestId("meeting-speech-timeline")).toContainText(
    "This Speech must exist exactly once.",
  );
  await expect(
    page
      .getByTestId("meeting-speech-timeline")
      .getByText("This Speech must exist exactly once."),
  ).toHaveCount(1);
  const inputs = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter((entry) => entry.command === "submit_meeting_floor_action")
      .map((entry) => entry.payload.input),
  );
  expect(inputs).toHaveLength(2);
  expect(inputs[1]).toEqual(inputs[0]);
});

test("definitive refusal unlocks a fresh Floor submission", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetingFloorErrors: ["relay rejected: stale Meeting State"],
    meetings: [meetingSeed("moderator_control")],
  });
  await openMeeting(page);

  await page.getByTestId("meeting-floor-request").click();
  await expect(page.getByTestId("meeting-floor-error")).toContainText(
    "stale Meeting State",
  );
  await expect(page.getByTestId("meeting-floor-request")).toBeEnabled();
  await page.getByTestId("meeting-floor-request").click();
  await expect(page.getByTestId("meeting-offer-controls")).toBeVisible();

  const inputs = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter((entry) => entry.command === "submit_meeting_floor_action")
      .map((entry) => entry.payload.input),
  );
  expect(inputs).toHaveLength(2);
  expect(inputs[1].submissionId).not.toBe(inputs[0].submissionId);
});

test("shared controls allow an externally offered Human host", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [meetingSeed("offer_self", { host: CURRENT })],
  });
  await openMeeting(page);

  await expect(page.getByTestId("meeting-offer-controls")).toBeVisible();
  await expect(page.getByTestId("meeting-floor-request")).toHaveCount(0);
});

test("frozen Agent identity receives no Human Floor controls", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [meetingSeed("grant_self", { currentType: "agent" })],
  });
  await openMeeting(page);

  await expect(page.getByTestId("meeting-read-only-floor")).toContainText(
    "Human Floor controls are not available",
  );
  await expect(page.getByTestId("meeting-floor-request")).toHaveCount(0);
  await expect(page.getByTestId("meeting-offer-accept")).toHaveCount(0);
  await expect(page.getByTestId("meeting-speech-composer")).toHaveCount(0);
  await expect(page.getByTestId(`meeting-attention-${MEETING_ID}`)).toHaveCount(
    0,
  );
});
