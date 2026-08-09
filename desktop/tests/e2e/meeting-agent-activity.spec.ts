import { expect, test } from "@playwright/test";

import type {
  MeetingLoadResult,
  MeetingSnapshot,
} from "../../src/shared/api/tauriMeetings";
import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

const CURRENT = "deadbeef".repeat(8);
const AGENT = TEST_IDENTITIES.charlie.pubkey;
const OUTSIDER_AGENT = TEST_IDENTITIES.outsider.pubkey;
const OTHER_CHANNEL_ID = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const IDS = {
  narrow: "a1000000-0000-4000-8000-000000000002",
  wide: "a1000000-0000-4000-8000-000000000001",
} as const;

function meetingSeed(
  meetingId: string,
  boardPhase: "board_pending" | "floor_ready",
) {
  const now = Date.now();
  const snapshot: MeetingSnapshot = {
    meetingId,
    title: "Agent activity review",
    description: "Observe Meeting-scoped ACP work without changing state.",
    summary: null,
    sourceChannelId: null,
    schemaVersion: 3,
    policy: "moderated-board-actions-v3",
    hostPubkey: CURRENT,
    moderatorPubkey: CURRENT,
    createEventId: "1".repeat(64),
    createdAt: Math.floor(now / 1_000) - 60,
    lifecycle: "active",
    phase: "moderator_control",
    stateRevision: 1,
    floorRevision: 1,
    intentRevision: 0,
    speechRevision: 0,
    currentSpeakerPubkey: null,
    currentOfferPubkey: null,
    floor: {
      stateEventId: "2".repeat(64),
      humanQueue: [],
      offer: null,
      grant: null,
    },
    host: {
      controlToken: "3".repeat(64),
      stateEventId: "2".repeat(64),
      controlEpoch: 1,
      decisionEpoch: 1,
      decisionDeadlineMs: boardPhase === "floor_ready" ? now + 120_000 : null,
      nextActionAtMs: null,
      consecutiveModeratorSpeeches: 0,
      forcedReturnToModerator: false,
      pendingIntents: [],
      openHandoffs: [],
      boardControl: {
        phase: boardPhase,
        controlEpoch: 1,
        boardWindow: 1,
        boardStartedAtMs: now - 10_000,
        boardDeadlineAtMs:
          boardPhase === "board_pending" ? now + 120_000 : now - 5_000,
        boardCompletedAtMs: boardPhase === "floor_ready" ? now - 6_000 : null,
        boardOutcome: boardPhase === "floor_ready" ? "updated" : null,
      },
      canSelect: boardPhase === "floor_ready",
      canClose: boardPhase === "floor_ready",
      canRecall: false,
    },
    participants: [
      {
        pubkey: CURRENT,
        participantType: "human",
        channelRole: "owner",
      },
      {
        pubkey: AGENT,
        participantType: "agent",
        channelRole: "bot",
      },
    ],
    board: {
      eventId: "4".repeat(64),
      format: "markdown",
      body: "# Goal\nReview Agent activity without losing Board state.",
      moderatorPubkey: CURRENT,
      updatedAt: Math.floor(now / 1_000) - 20,
      source: "projection",
    },
    action: null,
    end: null,
    latestSpeechAt: null,
  };
  const result: MeetingLoadResult = { status: "ready", snapshot };
  return { id: meetingId, title: snapshot.title, result, speeches: [] };
}

async function openMeeting(
  page: import("@playwright/test").Page,
  meetingId: string,
) {
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.getByTestId(`meeting-row-${meetingId}`).click();
  await expect(page.getByTestId("meeting-screen")).toBeVisible();
  await page.waitForFunction(
    () =>
      typeof (window as Window & { __BUZZ_E2E_SEED_ACTIVE_TURNS__?: unknown })
        .__BUZZ_E2E_SEED_ACTIVE_TURNS__ === "function",
  );
}

async function seedMeetingActivity(
  page: import("@playwright/test").Page,
  meetingId: string,
) {
  await page.evaluate(
    ({ agentPubkey, meeting, otherChannel, outsiderPubkey }) => {
      const win = window as Window & {
        __BUZZ_E2E_SEED_ACTIVE_TURNS__?: (input: {
          agentPubkey: string;
          channelId: string;
          turnId: string;
        }) => void;
        __BUZZ_E2E_SEED_OBSERVER_EVENTS__?: (input: {
          agentPubkey: string;
          events: Array<{
            agentIndex: number | null;
            channelId: string | null;
            kind: string;
            payload: unknown;
            seq: number;
            sessionId: string | null;
            timestamp: string;
            turnId: string | null;
          }>;
        }) => void;
      };
      win.__BUZZ_E2E_SEED_ACTIVE_TURNS__?.({
        agentPubkey,
        channelId: meeting,
        turnId: "meeting-turn",
      });
      win.__BUZZ_E2E_SEED_ACTIVE_TURNS__?.({
        agentPubkey: outsiderPubkey,
        channelId: meeting,
        turnId: "outsider-turn",
      });
      const timestamp = new Date().toISOString();
      win.__BUZZ_E2E_SEED_OBSERVER_EVENTS__?.({
        agentPubkey,
        events: [
          {
            agentIndex: 0,
            channelId: meeting,
            kind: "acp_read",
            payload: { marker: "meeting-only-observer" },
            seq: 10,
            sessionId: "meeting-session",
            timestamp,
            turnId: "meeting-turn",
          },
          {
            agentIndex: 0,
            channelId: otherChannel,
            kind: "acp_read",
            payload: { marker: "other-channel-observer" },
            seq: 11,
            sessionId: "other-session",
            timestamp,
            turnId: "other-turn",
          },
        ],
      });
    },
    {
      agentPubkey: AGENT,
      meeting: meetingId,
      otherChannel: OTHER_CHANNEL_ID,
      outsiderPubkey: OUTSIDER_AGENT,
    },
  );
  await expect(page.getByTestId("meeting-agent-activity-row")).toBeVisible();
}

async function openAgentActivity(page: import("@playwright/test").Page) {
  await page.getByTestId("bot-activity-composer-trigger").click();
  await expect(
    page.getByTestId(`bot-activity-composer-item-${OUTSIDER_AGENT}`),
  ).toHaveCount(0);
  await page
    .getByTestId(`bot-activity-composer-item-${AGENT}`)
    .click({ force: true });
}

test("Meeting Agent activity is roster-scoped and preserves the Board draft", async ({
  page,
}) => {
  await installMockBridge(page, {
    managedAgents: [
      { pubkey: AGENT, name: "Meeting Agent", status: "running" },
      {
        pubkey: OUTSIDER_AGENT,
        name: "Non-roster Agent",
        status: "running",
      },
    ],
    meetings: [meetingSeed(IDS.wide, "board_pending")],
  });
  await openMeeting(page, IDS.wide);

  const boardDraft = page.getByTestId("meeting-board-editor");
  await boardDraft.fill("# Unsaved Board draft\nKeep this while observing.");
  await seedMeetingActivity(page, IDS.wide);
  await openAgentActivity(page);

  const activity = page.getByTestId("meeting-agent-activity-wide");
  await expect(activity).toBeVisible();
  await expect(page.getByTestId("meeting-board-wide")).toHaveCount(0);
  await expect(activity.getByTestId("agent-session-scope-label")).toHaveText(
    "Meeting · Agent activity review",
  );

  await activity.getByTestId("agent-session-settings-menu-trigger").click();
  await expect(page.getByTestId("agent-session-stop-turn")).toHaveCount(0);
  await page.getByTestId("agent-session-toggle-raw-feed").click();
  await page.keyboard.press("Escape");
  await expect(activity).toContainText("meeting-only-observer");
  await expect(activity).not.toContainText("other-channel-observer");

  await activity.getByTestId("agent-session-back").click();
  await expect(page.getByTestId("meeting-board-wide")).toBeVisible();
  await expect(boardDraft).toHaveValue(
    "# Unsaved Board draft\nKeep this while observing.",
  );

  await page.getByTestId("meeting-board-trigger").click();
  await expect(page.getByTestId("meeting-board-wide")).toHaveCount(0);
  await openAgentActivity(page);
  await page
    .getByTestId("meeting-agent-activity-wide")
    .getByTestId("auxiliary-panel-close")
    .click();
  await expect(page.getByTestId("meeting-agent-activity-wide")).toHaveCount(0);
  await expect(page.getByTestId("meeting-board-wide")).toHaveCount(0);
});

test.describe("narrow Meeting Agent activity", () => {
  test.use({ viewport: { width: 1100, height: 800 } });

  test("Activity and Board use mutually exclusive Sheets", async ({ page }) => {
    await installMockBridge(page, {
      managedAgents: [
        { pubkey: AGENT, name: "Meeting Agent", status: "running" },
      ],
      meetings: [meetingSeed(IDS.narrow, "floor_ready")],
    });
    await openMeeting(page, IDS.narrow);
    await seedMeetingActivity(page, IDS.narrow);
    await openAgentActivity(page);

    await expect(
      page.getByTestId("meeting-agent-activity-sheet"),
    ).toBeVisible();
    await expect(page.locator("#meeting-board-overlay-panel")).toHaveCount(0);

    await page.getByTestId("agent-session-back").click();
    await expect(page.locator("#meeting-board-overlay-panel")).toBeVisible();
    await expect(page.getByTestId("meeting-agent-activity-sheet")).toHaveCount(
      0,
    );
  });
});
