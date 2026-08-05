import { expect, type Page, test } from "@playwright/test";

import type {
  MeetingSnapshot,
  MeetingSpeech,
} from "../../src/shared/api/tauriMeetings";
import { waitForAnimations } from "../helpers/animations";
import { installMockBridge } from "../helpers/bridge";

const CURRENT = "deadbeef".repeat(8);
const PARTICIPANT = "cafebabe".repeat(8);
const RELAY_A = "ws://localhost:3000";
const RELAY_B = "ws://localhost:3001";
const MEETING_A = "60000000-0000-4000-8000-000000000001";
const MEETING_B = "60000000-0000-4000-8000-000000000002";

type RelayConnectionState =
  | "connected"
  | "connecting"
  | "disconnected"
  | "idle"
  | "reconnecting"
  | "stalled";

function meetingSeed(
  id: string,
  title: string,
  boardBody: string,
  speeches: MeetingSpeech[] = [],
) {
  const now = Date.now();
  const snapshot: MeetingSnapshot = {
    meetingId: id,
    title,
    description: "Recovery boundary fixture",
    sourceChannelId: null,
    schemaVersion: 3,
    policy: "moderated-board-actions-v2",
    hostPubkey: CURRENT,
    moderatorPubkey: CURRENT,
    createEventId: "1".repeat(64),
    createdAt: Math.floor(now / 1_000) - 60,
    lifecycle: "active",
    phase: "moderator_control",
    stateRevision: 1,
    floorRevision: 1,
    intentRevision: 0,
    speechRevision: speeches.length,
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
      decisionDeadlineMs: null,
      nextActionAtMs: null,
      consecutiveModeratorSpeeches: 0,
      forcedReturnToModerator: false,
      pendingIntents: [],
      openHandoffs: [],
      boardControl: {
        phase: "board_pending",
        controlEpoch: 1,
        boardWindow: 1,
        boardStartedAtMs: now - 5_000,
        boardDeadlineAtMs: now + 120_000,
        boardCompletedAtMs: null,
        boardOutcome: null,
      },
      canSelect: false,
      canClose: false,
      canRecall: false,
    },
    participants: [
      {
        pubkey: CURRENT,
        participantType: "human",
        channelRole: "owner",
      },
      {
        pubkey: PARTICIPANT,
        participantType: "human",
        channelRole: "member",
      },
    ],
    board: {
      eventId: "4".repeat(64),
      format: "markdown",
      body: boardBody,
      moderatorPubkey: CURRENT,
      updatedAt: Math.floor(now / 1_000) - 20,
      source: "projection",
    },
    action: null,
    end: null,
    latestSpeechAt: speeches.at(-1)?.createdAt ?? null,
  };
  return {
    id,
    title,
    result: { status: "ready" as const, snapshot },
    speeches,
  };
}

function meetingSpeech(revision: number): MeetingSpeech {
  return {
    eventId: revision.toString(16).padStart(64, "0"),
    authorPubkey: PARTICIPANT,
    content: `Formal Speech ${revision}: preserve the current timeline position while the Board view changes.`,
    createdAt: 1_785_800_000 + revision,
    speechRevision: revision,
    grantEventId: "5".repeat(64),
    mentions: [],
    authorParticipantType: "human",
    authorIsModerator: false,
    handoff: null,
  };
}

async function setRelayConnectionState(
  page: Page,
  state: RelayConnectionState,
) {
  if (state !== "connected") {
    await page.waitForFunction(() => {
      const testWindow = window as Window & {
        __BUZZ_E2E_GET_RELAY_CONNECTION_STATE__?: () => string;
      };
      return (
        testWindow.__BUZZ_E2E_GET_RELAY_CONNECTION_STATE__?.() === "connected"
      );
    });
  }
  await page.evaluate((nextState) => {
    const testWindow = window as Window & {
      __BUZZ_E2E_SET_RELAY_CONNECTION_STATE__?: (
        value: RelayConnectionState,
      ) => void;
    };
    if (!testWindow.__BUZZ_E2E_SET_RELAY_CONNECTION_STATE__) {
      throw new Error("Missing relay connection E2E seam.");
    }
    testWindow.__BUZZ_E2E_SET_RELAY_CONNECTION_STATE__(nextState);
  }, state);
}

async function setSnapshotError(page: Page, error: string | null) {
  await page.evaluate((nextError) => {
    const testWindow = window as Window & {
      __BUZZ_E2E__?: {
        mock?: { meetingSnapshotError?: string };
      };
    };
    if (!testWindow.__BUZZ_E2E__?.mock) {
      throw new Error("Missing Meeting mock configuration.");
    }
    if (nextError === null) {
      delete testWindow.__BUZZ_E2E__.mock.meetingSnapshotError;
    } else {
      testWindow.__BUZZ_E2E__.mock.meetingSnapshotError = nextError;
    }
  }, error);
}

async function openMeeting(page: Page, meetingId: string) {
  await page.getByTestId(`meeting-row-${meetingId}`).click();
  await expect(page.getByTestId("meeting-screen")).toBeVisible();
}

test("Meeting stays visible but non-authoritative until reconnect snapshot succeeds", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await installMockBridge(page, {
    meetingSnapshotDelayMs: 150,
    meetings: [meetingSeed(MEETING_A, "Recovery review", "# Goal\nStay exact")],
  });
  await page.goto("/");
  await openMeeting(page, MEETING_A);

  const editor = page.locator('[data-testid="meeting-board-editor"]:visible');
  await editor.fill("# Goal\nKeep this unsent draft");
  await setRelayConnectionState(page, "disconnected");

  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-authority",
    "stale",
  );
  await expect(page.getByTestId("meeting-authority-banner")).toContainText(
    "last verified Meeting state",
  );
  await expect(editor).toBeDisabled();
  await expect(editor).toHaveValue("# Goal\nKeep this unsent draft");
  await expect(page.getByTestId("meeting-read-only-floor")).toContainText(
    "latest authoritative state",
  );

  await setSnapshotError(page, "relay snapshot unavailable");
  await setRelayConnectionState(page, "connected");
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-authority",
    "resyncing",
  );
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-authority",
    "stale",
  );
  await expect(page.getByTestId("meeting-screen")).toBeVisible();
  await expect(editor).toBeDisabled();

  await setSnapshotError(page, null);
  await page.getByTestId("meeting-authority-retry").click();
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-authority",
    "current",
  );
  await expect(page.getByTestId("meeting-authority-banner")).toHaveCount(0);
  await expect(editor).toBeEnabled();
  await expect(editor).toHaveValue("# Goal\nKeep this unsent draft");
});

test("wide Meeting Board collapse preserves its draft, width, and timeline position", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await installMockBridge(page, {
    meetings: [
      meetingSeed(
        MEETING_A,
        "Wide Board review",
        "# Wide Board",
        Array.from({ length: 36 }, (_, index) => meetingSpeech(index + 1)),
      ),
    ],
  });
  await page.goto("/");
  await openMeeting(page, MEETING_A);

  const boardTrigger = page.getByTestId("meeting-board-trigger");
  const timelineScroll = page.getByTestId("meeting-timeline-scroll");
  await expect(boardTrigger).toHaveAttribute("aria-expanded", "true");
  await page.getByTestId("meeting-board-resize-handle").press("ArrowLeft");
  await expect(page.getByTestId("meeting-board-wide")).toHaveCSS(
    "width",
    "416px",
  );
  await page
    .getByTestId("meeting-board-editor")
    .fill("# Wide unsent Board draft");
  await timelineScroll.evaluate((element) => {
    element.scrollTop = 300;
  });
  await expect
    .poll(() => timelineScroll.evaluate((element) => element.scrollTop))
    .toBe(300);

  await boardTrigger.click();
  await expect(boardTrigger).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByTestId("meeting-board-wide")).toBeHidden();
  await expect(page.getByTestId("meeting-speech-timeline")).toContainText(
    "Formal Speech 36",
  );
  await expect
    .poll(() => timelineScroll.evaluate((element) => element.scrollTop))
    .toBe(300);

  await boardTrigger.click();
  await expect(boardTrigger).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByTestId("meeting-board-wide")).toHaveCSS(
    "width",
    "416px",
  );
  await expect(page.getByTestId("meeting-board-editor")).toHaveValue(
    "# Wide unsent Board draft",
  );
  await expect
    .poll(() => timelineScroll.evaluate((element) => element.scrollTop))
    .toBe(300);
});

test("Meeting rooms, drafts, and panel preferences stay Community scoped", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "buzz-communities",
      JSON.stringify([
        {
          id: "meeting-a",
          name: "Meeting Alpha",
          relayUrl: "ws://localhost:3000",
          addedAt: "2026-01-01T00:00:00.000Z",
        },
        {
          id: "meeting-b",
          name: "Meeting Bravo",
          relayUrl: "ws://localhost:3001",
          addedAt: "2026-01-02T00:00:00.000Z",
        },
      ]),
    );
    window.localStorage.setItem("buzz-active-community-id", "meeting-a");
  });
  await installMockBridge(
    page,
    {
      meetingsByRelayUrl: {
        [RELAY_A]: [meetingSeed(MEETING_A, "Alpha decision", "# Alpha")],
        [RELAY_B]: [meetingSeed(MEETING_B, "Bravo decision", "# Bravo")],
      },
    },
    { skipCommunitySeed: true },
  );
  await page.goto("/");
  await openMeeting(page, MEETING_A);

  await page.getByTestId("meeting-board-editor").fill("# Alpha dirty draft");
  await page.getByTestId("meeting-board-resize-handle").press("ArrowLeft");
  await expect(page.getByTestId("meeting-board-wide")).toHaveCSS(
    "width",
    "416px",
  );

  await page.getByTestId("community-rail-button-meeting-b").click();
  await expect(page.getByTestId(`meeting-row-${MEETING_A}`)).toHaveCount(0);
  await openMeeting(page, MEETING_B);
  await expect(
    page.getByRole("heading", { name: "Bravo decision" }),
  ).toBeVisible();
  await expect(page.getByTestId("meeting-board-editor")).toHaveValue("# Bravo");
  await expect(page.getByTestId("meeting-board-wide")).toHaveCSS(
    "width",
    "384px",
  );

  await page.getByTestId("community-rail-button-meeting-a").click();
  const alphaHeading = page.getByRole("heading", { name: "Alpha decision" });
  if (!(await alphaHeading.isVisible())) {
    await openMeeting(page, MEETING_A);
  }
  await expect(alphaHeading).toBeVisible();
  await expect(page.getByTestId("meeting-board-editor")).toHaveValue("# Alpha");
  await expect(page.getByTestId("meeting-board-wide")).toHaveCSS(
    "width",
    "416px",
  );
});

test("large Meeting directories use bounded native batches and paged rendering", async ({
  page,
}) => {
  const meetings = Array.from({ length: 65 }, (_, index) =>
    meetingSeed(
      `61000000-0000-4000-8000-${String(index).padStart(12, "0")}`,
      `Bounded meeting ${index + 1}`,
      `# Meeting ${index + 1}`,
    ),
  );
  await installMockBridge(page, { meetings });
  await page.goto("/");

  const activeList = page.getByTestId("meeting-active-list");
  await expect(activeList.locator("[data-testid^='meeting-row-']")).toHaveCount(
    20,
  );
  await activeList
    .getByRole("button", { name: /Show 20 more meetings/ })
    .click();
  await expect(activeList.locator("[data-testid^='meeting-row-']")).toHaveCount(
    40,
  );

  const batchSizes = await page.evaluate(() =>
    (
      (
        window as Window & {
          __BUZZ_E2E_COMMAND_PAYLOADS__?: Array<{
            command: string;
            payload: { meetingIds?: string[] };
          }>;
        }
      ).__BUZZ_E2E_COMMAND_PAYLOADS__ ?? []
    )
      .filter((entry) => entry.command === "list_meetings")
      .map((entry) => entry.payload.meetingIds?.length ?? 0),
  );
  expect(batchSizes.length).toBeGreaterThanOrEqual(2);
  expect(Math.max(...batchSizes)).toBeLessThanOrEqual(64);
  expect(batchSizes).toContain(1);
});

test("narrow Meeting sheets preserve the Board draft and produce distinct scoped states", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 900, height: 800 });
  await installMockBridge(page, {
    meetings: [meetingSeed(MEETING_A, "Narrow review", "# Narrow Board")],
  });
  await page.goto("/");
  await openMeeting(page, MEETING_A);

  await expect(page.getByTestId("meeting-board-wide")).toBeHidden();
  const editor = page.locator('[data-testid="meeting-board-editor"]:visible');
  await expect(editor).toBeVisible();
  await editor.fill("# Narrow unsent draft");
  await waitForAnimations(page);
  const boardShot = await page
    .locator('[data-testid="meeting-board"]:visible')
    .screenshot({ path: testInfo.outputPath("meeting-narrow-board.png") });

  await page.keyboard.press("Escape");
  await page.getByTestId("meeting-participants-trigger").click();
  const participants = page.getByTestId("meeting-participants");
  await expect(participants).toBeVisible();
  await expect(participants).toContainText("human");
  await waitForAnimations(page);
  const participantShot = await participants.screenshot({
    path: testInfo.outputPath("meeting-narrow-participants.png"),
  });
  expect(boardShot.equals(participantShot)).toBe(false);

  await page.keyboard.press("Escape");
  await page.getByTestId("meeting-board-trigger").click();
  await expect(
    page.locator('[data-testid="meeting-board-editor"]:visible'),
  ).toHaveValue("# Narrow unsent draft");
});

test("Desktop preview gate hides discovery without turning an existing Meeting into chat", async ({
  page,
}) => {
  await installMockBridge(
    page,
    {
      meetings: [meetingSeed(MEETING_A, "Gated review", "# Gated Board")],
    },
    { seedPreviewFeatures: false },
  );
  await page.goto(`/#/channels/${MEETING_A}`);

  await expect(page.getByTestId("meetings-section")).toHaveCount(0);
  await expect(page.getByTestId("meeting-screen")).toBeVisible();
  await expect(page.getByTestId("message-composer")).toHaveCount(0);
  await expect(page.getByText(/Meetings is a preview feature/)).toBeVisible();
});
