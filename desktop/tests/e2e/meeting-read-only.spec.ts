import { expect, test } from "@playwright/test";

import type {
  MeetingActivity,
  MeetingLifecycle,
  MeetingLoadResult,
  MeetingSnapshot,
  MeetingSpeech,
} from "../../src/shared/api/tauriMeetings";
import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

const HOST = TEST_IDENTITIES.alice.pubkey;
const HUMAN = TEST_IDENTITIES.tyler.pubkey;
const AGENT = TEST_IDENTITIES.charlie.pubkey;
const NOW = 1_785_800_000;
const STAR_STORAGE_KEY = `buzz-channel-stars.v1:${"deadbeef".repeat(8)}`;
const IDS = {
  active: "10000000-0000-4000-8000-000000000001",
  actions: "10000000-0000-4000-8000-000000000002",
  closed: "10000000-0000-4000-8000-000000000003",
  aborted: "10000000-0000-4000-8000-000000000004",
  forbidden: "10000000-0000-4000-8000-000000000005",
  unsupported: "10000000-0000-4000-8000-000000000006",
} as const;

function speech(revision: number, content: string): MeetingSpeech {
  return {
    eventId: revision.toString(16).padStart(64, "0"),
    authorPubkey: revision % 2 === 0 ? HOST : AGENT,
    content,
    createdAt: NOW + revision,
    speechRevision: revision,
    grantEventId: "a".repeat(64),
    mentions: [],
  };
}

function meetingActivities(): MeetingActivity[] {
  const kinds: Array<{
    kind: MeetingActivity["kind"];
    summary: string;
    actor?: string;
    target?: string;
  }> = [
    {
      kind: "meeting_closed",
      summary: "The host closed the meeting after confirming its outcome.",
      actor: HOST,
    },
    {
      kind: "action_retried",
      summary: "The host retried recording the meeting actions.",
      actor: HOST,
    },
    {
      kind: "action_blocked",
      summary: "Recording the meeting actions became blocked.",
      actor: HOST,
    },
    {
      kind: "action_finalization_started",
      summary: "The meeting entered action finalization.",
      actor: HOST,
    },
    {
      kind: "handoff_resolved",
      summary: "A directed handoff was answered by the accepted Speech.",
      actor: AGENT,
    },
    {
      kind: "handoff_attempted",
      summary: "The host offered the floor for a directed handoff.",
      actor: HOST,
      target: HUMAN,
    },
    {
      kind: "floor_yielded",
      summary: "The participant yielded the floor.",
      actor: AGENT,
    },
    {
      kind: "floor_granted",
      summary: "The participant accepted the offer and received the floor.",
      actor: AGENT,
      target: AGENT,
    },
    {
      kind: "offer_declined",
      summary: "The participant declined the floor offer.",
      actor: HUMAN,
      target: HUMAN,
    },
    {
      kind: "floor_offered",
      summary: "The host offered the floor to a participant.",
      actor: HOST,
      target: AGENT,
    },
    {
      kind: "board_timed_out",
      summary: "The Board maintenance window timed out.",
    },
    {
      kind: "board_updated",
      summary: "The host updated the Meeting Board.",
      actor: HOST,
    },
  ];
  const filler = Array.from({ length: 22 }, (_, index) => ({
    kind: "board_unchanged" as const,
    summary: `The host completed Board maintenance without changes (${index + 1}).`,
    actor: HOST,
  }));
  return [...kinds, ...filler].map((activity, index) => ({
    activityId: `verified-activity-${index + 1}`,
    kind: activity.kind,
    occurredAtMs: (NOW + 200 - index) * 1_000,
    actorPubkey: activity.actor ?? null,
    targetPubkey: activity.target ?? null,
    summary: activity.summary,
  }));
}

function readyMeeting(input: {
  id: string;
  title: string;
  lifecycle: MeetingLifecycle;
  phase?: string;
  outcome?: "closed" | "aborted";
}): {
  result: MeetingLoadResult;
  speeches: MeetingSpeech[];
  activities: MeetingActivity[];
} {
  const speeches = [
    speech(1, "I recommend shipping the verified read path first."),
    speech(2, "Agreed. The Board captures the acceptance boundary."),
  ];
  const end = input.outcome
    ? {
        eventId: "e".repeat(64),
        outcome: input.outcome,
        reasonCode: input.outcome === "aborted" ? "insufficient_context" : null,
        reason:
          input.outcome === "aborted"
            ? "The required evidence was not available."
            : null,
        endedBy: HOST,
        endedAt: NOW + 20,
        actionsAttested: input.outcome === "closed",
      }
    : null;
  const snapshot: MeetingSnapshot = {
    meetingId: input.id,
    title: input.title,
    description: "Review the Meeting Desktop lifecycle and safe room split.",
    sourceChannelId: "1c7e1c02-87bb-5e88-b2da-5a7a9432d0c9",
    schemaVersion: 3,
    policy: "moderated-board-actions-v2",
    hostPubkey: HOST,
    moderatorPubkey: HOST,
    createEventId: "c".repeat(64),
    createdAt: NOW - 300,
    lifecycle: input.lifecycle,
    phase: input.phase ?? "moderator_control",
    stateRevision: 8,
    floorRevision: 5,
    intentRevision: 3,
    speechRevision: 2,
    currentSpeakerPubkey: input.phase === "granted" ? AGENT : null,
    currentOfferPubkey: null,
    floor: null,
    host: null,
    participants: [
      { pubkey: HOST, participantType: "agent", channelRole: "owner" },
      { pubkey: HUMAN, participantType: "human", channelRole: "member" },
      { pubkey: AGENT, participantType: "agent", channelRole: "bot" },
    ],
    board: {
      eventId: "b".repeat(64),
      format: "markdown",
      body: "# Goal\nDeliver a trustworthy read-only Meeting room.\n\n## Agenda\n- Verify room isolation\n- Review canonical Speech",
      moderatorPubkey: HOST,
      updatedAt: NOW + 3,
      source: "projection",
    },
    action:
      input.lifecycle === "finalizing_actions"
        ? {
            actionRunId: "20000000-0000-4000-8000-000000000001",
            boardEventId: "b".repeat(64),
            actionWindowEpoch: 1,
            condition: "runnable",
            terminalStatus: null,
            completionEventId: null,
            actionDeadlineAtMs: (NOW + 600) * 1_000,
            lastErrorCode: null,
          }
        : null,
    end,
    latestSpeechAt: NOW + 2,
  };
  return {
    result: { status: "ready", snapshot },
    speeches,
    activities: meetingActivities(),
  };
}

function meetingSeeds() {
  return [
    {
      id: IDS.active,
      title: "Desktop lifecycle review",
      ...readyMeeting({
        id: IDS.active,
        title: "Desktop lifecycle review",
        lifecycle: "active",
        phase: "granted",
      }),
    },
    {
      id: IDS.actions,
      title: "Action recording",
      ...readyMeeting({
        id: IDS.actions,
        title: "Action recording",
        lifecycle: "finalizing_actions",
      }),
    },
    {
      id: IDS.closed,
      title: "Completed requirements review",
      ...readyMeeting({
        id: IDS.closed,
        title: "Completed requirements review",
        lifecycle: "closed",
        outcome: "closed",
      }),
    },
    {
      id: IDS.aborted,
      title: "Aborted evidence review",
      ...readyMeeting({
        id: IDS.aborted,
        title: "Aborted evidence review",
        lifecycle: "aborted",
        outcome: "aborted",
      }),
    },
    {
      id: IDS.forbidden,
      title: "Private roster meeting",
      result: { status: "forbidden" } as const,
    },
    {
      id: IDS.unsupported,
      title: "Legacy meeting",
      result: {
        status: "unsupported_protocol",
        meeting_id: IDS.unsupported,
        schema_version: "2",
        policy: "moderated-baton-v1",
      } as const,
    },
  ];
}

function seedStaleMeetingStar(
  page: import("@playwright/test").Page,
  meetingId: string,
) {
  return page.addInitScript(
    ({ key, id }) => {
      localStorage.setItem(
        key,
        JSON.stringify({
          version: 1,
          channels: {
            [id]: { starred: true, updatedAt: 1_700_000_000 },
          },
        }),
      );
    },
    { key: STAR_STORAGE_KEY, id: meetingId },
  );
}

test("Meeting rooms are isolated and render verified Board and Speech", async ({
  page,
}) => {
  await seedStaleMeetingStar(page, IDS.active);
  await installMockBridge(page, { meetings: meetingSeeds() });
  await page.goto("/");

  await expect(page.getByTestId("meetings-section")).toBeVisible();
  await expect(page.getByTestId("meeting-active-list")).toContainText(
    "Desktop lifecycle review",
  );
  await expect(page.getByTestId("stream-list")).not.toContainText(
    "Desktop lifecycle review",
  );
  await expect(page.getByTestId("starred-list")).toHaveCount(0);

  await page.getByTestId("section-actions-channels-quick-create").click();
  await expect(page.getByTestId("channel-browser-search")).toBeVisible();
  await expect(
    page.getByTestId("browse-channel-Desktop lifecycle review"),
  ).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("channel-browser-search")).toHaveCount(0);

  await page.getByTestId(`meeting-row-${IDS.active}`).click();
  await expect(page.getByTestId("meeting-screen")).toBeVisible();
  await expect(page.getByTestId("meeting-board").first()).toContainText(
    "Deliver a trustworthy read-only Meeting room",
  );
  await expect(page.getByTestId("meeting-speech-timeline")).toContainText(
    "verified read path first",
  );
  const statusStrip = page.getByTestId("meeting-status-strip");
  await expect(statusStrip).toContainText("has the floor");
  await expect(statusStrip).not.toContainText(/Speech r\d+|State r\d+/);
  await expect(page.getByTestId("message-composer")).toHaveCount(0);
  await expect(page.getByTestId("channel-composer-overlay")).toHaveCount(0);
  await expect(page.getByTestId("channel-drop-zone")).toHaveCount(0);
  await expect(page.getByTestId("message-thread-body")).toHaveCount(0);
  await expect(page.getByTestId("message-reactions")).toHaveCount(0);
  await expect(page.getByTestId("channel-management-trigger")).toHaveCount(0);

  await page.getByTestId("meeting-participants-trigger").click();
  await expect(page.getByTestId("meeting-participants")).toContainText("alice");
  await expect(
    page.getByTestId("meeting-participants").getByLabel("Host"),
  ).toBeVisible();
});

test("Meeting activity is bounded, product-level, and separate from canonical Speech", async ({
  page,
}) => {
  await installMockBridge(page, { meetings: meetingSeeds() });
  await page.goto("/");
  await page.getByTestId(`meeting-row-${IDS.active}`).click();

  const speechTimeline = page.getByTestId("meeting-speech-timeline");
  await expect(speechTimeline).toContainText("verified read path first");
  await page.getByTestId("meeting-activity-trigger").click();
  const panel = page.getByTestId("meeting-activity-panel");
  await expect(panel).toBeVisible();
  await expect(panel).toContainText("The host updated the Meeting Board.");
  await expect(panel).toContainText(
    "The participant accepted the offer and received the floor.",
  );
  await expect(panel).toContainText("The meeting entered action finalization.");
  await expect(panel.locator("[data-activity-kind]")).toHaveCount(30);
  await expect(panel).not.toContainText(
    /event.?id|state.?revision|floor.?revision|control.?epoch|lease|control.?token/i,
  );
  await expect(panel.getByTestId("message-reactions")).toHaveCount(0);
  await expect(speechTimeline).not.toContainText(
    "The host updated the Meeting Board.",
  );

  await page.getByTestId("meeting-activity-load-older").click();
  await expect(panel.locator("[data-activity-kind]")).toHaveCount(34);
  await expect(page.getByTestId("meeting-activity-load-older")).toHaveCount(0);
  const reads = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? []).filter(
      (entry) => entry.command === "get_meeting_activities",
    ),
  );
  expect(reads).toHaveLength(2);
});

test("action, history, forbidden, and unsupported Meeting states stay read-only", async ({
  page,
}) => {
  await installMockBridge(page, { meetings: meetingSeeds() });
  await page.goto("/");

  await page.getByTestId(`meeting-row-${IDS.actions}`).click();
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-lifecycle",
    "finalizing_actions",
  );
  await expect(page.getByTestId("meeting-status-strip")).toContainText(
    "recording the final Board actions",
  );

  await page.getByTestId("meeting-history-trigger").click();
  await expect(page.getByTestId("meeting-history-list")).toContainText(
    "Completed requirements review",
  );
  await expect(page.getByTestId("meeting-history-list")).toContainText(
    "Aborted evidence review",
  );
  await page
    .getByRole("button", { name: /Completed requirements review/ })
    .click();
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-lifecycle",
    "closed",
  );
  await expect(page.getByTestId("meeting-read-only-floor")).toContainText(
    "final Board",
  );
  await expect(page.getByTestId("meeting-terminal-summary")).toContainText(
    "Actions recorded",
  );

  await page.getByTestId("meeting-history-trigger").click();
  await page.getByRole("button", { name: /Aborted evidence review/ }).click();
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-lifecycle",
    "aborted",
  );
  await expect(page.getByTestId("meeting-status-strip")).toContainText(
    "required evidence was not available",
  );
  await expect(page.getByTestId("meeting-terminal-summary")).toContainText(
    "insufficient context",
  );

  await page.getByTestId(`meeting-row-${IDS.forbidden}`).click();
  await expect(page.getByTestId("meeting-load-state")).toContainText(
    "not a participant",
  );
  await expect(page.getByTestId("meeting-activity-trigger")).toHaveCount(0);
  await expect(page.getByTestId("message-composer")).toHaveCount(0);

  await page.getByTestId(`meeting-row-${IDS.unsupported}`).click();
  await expect(page.getByTestId("meeting-load-state")).toContainText(
    "compatibility required",
  );
  await expect(page.getByTestId("meeting-load-state")).toContainText(
    "moderated-baton-v1",
  );
  await expect(page.getByTestId("message-composer")).toHaveCount(0);
});
