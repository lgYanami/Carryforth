import { expect, test } from "@playwright/test";

import type {
  MeetingActionState,
  MeetingHostState,
  MeetingLoadResult,
  MeetingSnapshot,
} from "../../src/shared/api/tauriMeetings";
import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

const CURRENT = "deadbeef".repeat(8);
const HUMAN = TEST_IDENTITIES.bob.pubkey;
const AGENT = TEST_IDENTITIES.charlie.pubkey;
const IDS = {
  lifecycle: "50000000-0000-4000-8000-000000000001",
  recovery: "50000000-0000-4000-8000-000000000002",
  returnRunnable: "50000000-0000-4000-8000-000000000003",
  returnBlocked: "50000000-0000-4000-8000-000000000004",
  retry: "50000000-0000-4000-8000-000000000005",
  deadline: "50000000-0000-4000-8000-000000000006",
  agentHost: "50000000-0000-4000-8000-000000000007",
  abort: "50000000-0000-4000-8000-000000000008",
  discussionControl: "50000000-0000-4000-8000-000000000009",
} as const;

function meetingSeed(input: {
  id: string;
  title: string;
  lifecycle?: "active" | "finalizing_actions";
  condition?: "runnable" | "blocked";
  agentHost?: boolean;
  expired?: boolean;
  phase?: "moderator_idle" | "moderator_control";
}) {
  const now = Date.now();
  const lifecycle = input.lifecycle ?? "active";
  const condition = input.condition ?? "runnable";
  const moderatorPubkey = input.agentHost ? AGENT : CURRENT;
  const boardEventId = "6".repeat(64);
  const action: MeetingActionState | null =
    lifecycle === "finalizing_actions"
      ? {
          actionRunId: input.id.replace("0001", "1001"),
          boardEventId,
          actionWindowEpoch: 1,
          condition,
          terminalStatus: null,
          completionEventId: null,
          actionDeadlineAtMs:
            condition === "runnable"
              ? now + (input.expired ? -10_000 : 180_000)
              : null,
          progressSeq: 2,
          lastProgressStage: "waiting_human",
          lastProgressAtMs: now - 5_000,
          operatorHardDeadlineMs: now + 3_600_000,
          createdAtMs: now - 30_000,
          lastErrorCode:
            condition === "blocked" ? "external_operation_failed" : null,
        }
      : null;
  const host: MeetingHostState = {
    controlToken: "4".repeat(64),
    stateEventId: "1".repeat(64),
    controlEpoch: 3,
    decisionEpoch: 4,
    decisionDeadlineMs: lifecycle === "active" ? now + 90_000 : null,
    nextActionAtMs: null,
    consecutiveModeratorSpeeches: 0,
    forcedReturnToModerator: false,
    pendingIntents: [],
    openHandoffs: [],
    boardControl: {
      phase:
        lifecycle === "finalizing_actions"
          ? "finalizing_actions"
          : "floor_ready",
      controlEpoch: 3,
      boardWindow: 2,
      boardStartedAtMs: now - 20_000,
      boardDeadlineAtMs: now - 15_000,
      boardCompletedAtMs: now - 14_000,
      boardOutcome: "updated",
    },
    canSelect: lifecycle === "active" && !input.agentHost,
    canClose: lifecycle === "active" && !input.agentHost,
    canRecall: false,
  };
  const snapshot: MeetingSnapshot = {
    meetingId: input.id,
    title: input.title,
    description: "Record the final Board actions without a Meeting Plan.",
    sourceChannelId: null,
    schemaVersion: 3,
    policy: "moderated-board-actions-v3",
    hostPubkey: moderatorPubkey,
    moderatorPubkey,
    createEventId: "5".repeat(64),
    createdAt: Math.floor(now / 1_000) - 60,
    lifecycle,
    phase: input.phase ?? "moderator_idle",
    stateRevision: 10,
    floorRevision: 4,
    intentRevision: 2,
    speechRevision: 3,
    currentSpeakerPubkey: null,
    currentOfferPubkey: null,
    floor: {
      stateEventId: "1".repeat(64),
      humanQueue: [],
      offer: null,
      grant: null,
    },
    host,
    participants: [
      {
        pubkey: moderatorPubkey,
        participantType: input.agentHost ? "agent" : "human",
        channelRole: "owner",
      },
      ...(input.agentHost
        ? [
            {
              pubkey: CURRENT,
              participantType: "human" as const,
              channelRole: "member",
            },
          ]
        : []),
      { pubkey: HUMAN, participantType: "human", channelRole: "member" },
      ...(input.agentHost
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
      eventId: boardEventId,
      format: "markdown",
      body: "# Goal\nRecord agreed actions.\n\n## Actions\n- Update the relevant system if needed.",
      moderatorPubkey,
      updatedAt: Math.floor(now / 1_000) - 14,
      source: "projection",
    },
    action,
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

test("Human host enters action finalization, visits Project View, and atomically closes", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [meetingSeed({ id: IDS.lifecycle, title: "Action lifecycle" })],
  });
  await openMeeting(page, IDS.lifecycle);

  await expect(page.getByTestId("meeting-host-close")).toBeVisible();
  await expect(page.getByTestId("meeting-host-begin-actions")).toBeVisible();
  await page.getByTestId("meeting-host-begin-actions").click();
  await expect(
    page.getByTestId("meeting-host-begin-actions-dialog"),
  ).toContainText("does not create a Plan or Step");
  await page.getByTestId("meeting-host-begin-actions-confirm").click();

  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-lifecycle",
    "finalizing_actions",
  );
  await expect(
    page.getByTestId("meeting-action-finalization-card"),
  ).toBeVisible();
  await expect(page.getByTestId("meeting-floor-request")).toHaveCount(0);

  await page.getByTestId("meeting-action-open-view").click();
  await expect(page).toHaveURL(/\/view(?:\?|$)/);
  await page.goBack();
  await expect(
    page.getByTestId("meeting-action-finalization-card"),
  ).toBeVisible();

  await page.getByTestId("meeting-action-confirm").click();
  await expect(page.getByTestId("meeting-action-confirm-dialog")).toContainText(
    "or that no new recording is required",
  );
  await page.getByTestId("meeting-action-confirm-submit").click();
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-lifecycle",
    "closed",
  );
  await expect(page.getByTestId("meeting-terminal-summary")).toContainText(
    "Action output confirmed",
  );

  const actions = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter((entry) => entry.command === "submit_meeting_action_finalization")
      .map((entry) => entry.payload.input.action.type),
  );
  expect(actions).toEqual(["begin", "confirm"]);
});

test("action begin waits for the idle Human-host decision point", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [
      meetingSeed({
        id: IDS.discussionControl,
        title: "Discussion control",
        phase: "moderator_control",
      }),
    ],
  });
  await openMeeting(page, IDS.discussionControl);

  await expect(page.getByTestId("meeting-host-close")).toBeVisible();
  await expect(page.getByTestId("meeting-host-begin-actions")).toHaveCount(0);
});

test("blocked action recording retries into a fresh runnable window before close", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [
      meetingSeed({
        id: IDS.recovery,
        title: "Action recovery",
        lifecycle: "finalizing_actions",
      }),
    ],
  });
  await openMeeting(page, IDS.recovery);

  await page.getByTestId("meeting-action-block").click();
  await page
    .getByLabel("Action block category")
    .selectOption("external_state_conflict");
  await page
    .getByLabel("Action block explanation")
    .fill("The target changed while the Board was being recorded.");
  await page.getByTestId("meeting-action-block-confirm").click();
  await expect(page.getByTestId("meeting-action-blocked")).toContainText(
    "External state conflicts",
  );
  await expect(page.getByTestId("meeting-action-confirm")).toHaveCount(0);

  await page.getByTestId("meeting-action-retry").click();
  await expect(page.getByTestId("meeting-action-blocked")).toHaveCount(0);
  await expect(page.getByTestId("meeting-action-confirm")).toBeVisible();
  await page.getByTestId("meeting-action-confirm").click();
  await page.getByTestId("meeting-action-confirm-submit").click();
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-lifecycle",
    "closed",
  );

  const actions = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter((entry) => entry.command === "submit_meeting_action_finalization")
      .map((entry) => entry.payload.input.action.type),
  );
  expect(actions).toEqual(["block", "retry", "confirm"]);
  const blockAction = await page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
        .filter(
          (entry) => entry.command === "submit_meeting_action_finalization",
        )
        .find((entry) => entry.payload.input.action.type === "block")?.payload
        .input.action,
  );
  expect(blockAction).toMatchObject({
    type: "block",
    reasonCode: "external_state_conflict",
  });
});

test("Human Action renewal initial failure is visible and retries the exact window", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetingActionRenewalErrors: ["mock action renewal unavailable"],
    meetings: [
      meetingSeed({
        id: IDS.recovery,
        title: "Action renewal recovery",
        lifecycle: "finalizing_actions",
      }),
    ],
  });
  await openMeeting(page, IDS.recovery);

  await expect(page.getByTestId("meeting-action-renewal-error")).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window.__BUZZ_E2E_COMMAND_LOG__ ?? []).filter(
            (entry) => entry.command === "ensure_meeting_action_renewal",
          ).length,
      ),
    )
    .toBeGreaterThanOrEqual(2);
  await expect(page.getByTestId("meeting-action-renewal-error")).toHaveCount(0);

  const renewalInputs = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter((entry) => entry.command === "ensure_meeting_action_renewal")
      .map((entry) => entry.payload.input),
  );
  expect(renewalInputs[1]).toEqual(renewalInputs[0]);
});

test("runnable and blocked action runs can return to a new Board window", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [
      meetingSeed({
        id: IDS.returnRunnable,
        title: "Runnable return",
        lifecycle: "finalizing_actions",
      }),
      meetingSeed({
        id: IDS.returnBlocked,
        title: "Blocked return",
        lifecycle: "finalizing_actions",
        condition: "blocked",
      }),
    ],
  });

  for (const meetingId of [IDS.returnRunnable, IDS.returnBlocked]) {
    await openMeeting(page, meetingId);
    const boardTrigger = page.getByTestId("meeting-board-trigger");
    await expect(boardTrigger).toHaveAttribute("aria-expanded", "true");
    await boardTrigger.click();
    await expect(page.getByTestId("meeting-board-wide")).toBeHidden();
    await page.getByTestId("meeting-action-return-board").click();
    await expect(
      page.getByTestId("meeting-action-return-dialog"),
    ).toContainText("external effects that already occurred will remain");
    await page.getByTestId("meeting-action-return-confirm").click();
    await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
      "data-meeting-lifecycle",
      "active",
    );
    await expect(boardTrigger).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByTestId("meeting-board-editor")).toBeVisible();
  }
});

test("indeterminate action command retries the exact signed submission", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetingHostIndeterminateResponses: 1,
    meetings: [meetingSeed({ id: IDS.retry, title: "Action exact retry" })],
  });
  await openMeeting(page, IDS.retry);

  await page.getByTestId("meeting-host-begin-actions").click();
  await page.getByTestId("meeting-host-begin-actions-confirm").click();
  await expect(page.getByTestId("meeting-action-indeterminate")).toBeVisible();
  await page.getByTestId("meeting-action-retry-exact").click();
  await expect(page.getByTestId("meeting-action-indeterminate")).toHaveCount(0);
  await expect(
    page.getByTestId("meeting-action-finalization-card"),
  ).toBeVisible();

  const inputs = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter((entry) => entry.command === "submit_meeting_action_finalization")
      .map((entry) => entry.payload.input),
  );
  expect(inputs).toHaveLength(2);
  expect(inputs[1]).toEqual(inputs[0]);
});

test("expired action window cannot be locally confirmed", async ({ page }) => {
  await installMockBridge(page, {
    meetings: [
      meetingSeed({
        id: IDS.deadline,
        title: "Expired action window",
        lifecycle: "finalizing_actions",
        expired: true,
      }),
    ],
  });
  await openMeeting(page, IDS.deadline);

  await expect(
    page.getByTestId("meeting-action-deadline-expired"),
  ).toBeVisible();
  await expect(page.getByTestId("meeting-action-confirm")).toBeDisabled();
  await expect(page.getByTestId("meeting-action-block")).toBeDisabled();
});

test("Agent-hosted action finalization remains read-only for Human participants", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [
      meetingSeed({
        id: IDS.agentHost,
        title: "Agent records actions",
        lifecycle: "finalizing_actions",
        agentHost: true,
      }),
    ],
  });
  await openMeeting(page, IDS.agentHost);

  const observation = page.getByTestId("meeting-host-observation");
  await expect(observation).toBeVisible();
  await expect(
    page.getByTestId("meeting-host-observation-phase"),
  ).toContainText("Action finalization");
  await expect(
    page.getByTestId("meeting-host-observation-action"),
  ).toContainText("Ready to record actions");
  await expect(
    observation.locator("button, input, select, textarea"),
  ).toHaveCount(0);
  await expect(page.getByTestId("meeting-read-only-floor")).toContainText(
    "host Agent is recording actions",
  );
  await expect(
    page.getByTestId("meeting-action-finalization-card"),
  ).toHaveCount(0);
  await expect(page.getByTestId("meeting-action-confirm")).toHaveCount(0);

  const writes = await page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMAND_LOG__ ?? []).filter(
        (entry) => entry.command === "submit_meeting_action_finalization",
      ).length,
  );
  expect(writes).toBe(0);
});

test("aborting during action finalization warns that external effects may remain", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [
      meetingSeed({
        id: IDS.abort,
        title: "Abort action recording",
        lifecycle: "finalizing_actions",
      }),
    ],
  });
  await openMeeting(page, IDS.abort);

  await page.getByTestId("meeting-more-trigger").click();
  await page.getByTestId("meeting-action-abort").click();
  await expect(page.getByTestId("meeting-action-abort-dialog")).toContainText(
    "may already have occurred",
  );
  await page
    .getByLabel("Meeting abort category")
    .selectOption("discussion_blocked");
  await page.getByTestId("meeting-action-abort-confirm").click();
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-lifecycle",
    "aborted",
  );
  await expect(page.getByTestId("meeting-terminal-summary")).toContainText(
    "External system effects may remain",
  );
});
