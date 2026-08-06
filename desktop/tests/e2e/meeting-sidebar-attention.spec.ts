import { expect, test } from "@playwright/test";

import type {
  MeetingLifecycle,
  MeetingSnapshot,
} from "../../src/shared/api/tauriMeetings";
import { installMockBridge } from "../helpers/bridge";

const CURRENT = "deadbeef".repeat(8);
const OTHER = "cafebabe".repeat(8);
const NOW = 1_785_800_000;
const IDS = {
  aborted: "70000000-0000-4000-8000-000000000001",
  hostBoard: "70000000-0000-4000-8000-000000000002",
  offer: "70000000-0000-4000-8000-000000000003",
  recent: "70000000-0000-4000-8000-000000000004",
} as const;

function snapshot(input: {
  id: string;
  title: string;
  lifecycle?: MeetingLifecycle;
  updatedAt: number;
  host?: string;
  offered?: boolean;
  unread?: boolean;
}): MeetingSnapshot {
  const lifecycle = input.lifecycle ?? "active";
  const host = input.host ?? OTHER;
  const offered = input.offered ?? false;
  const hostBoard = host === CURRENT;
  const aborted = lifecycle === "aborted";
  return {
    meetingId: input.id,
    title: input.title,
    description: "Sidebar attention acceptance fixture",
    sourceChannelId: null,
    schemaVersion: 3,
    policy: "moderated-board-actions-v3",
    hostPubkey: host,
    moderatorPubkey: host,
    createEventId: "1".repeat(64),
    createdAt: NOW - 1_000,
    lifecycle,
    phase: aborted ? "ended" : offered ? "offered" : "moderator_control",
    stateRevision: 4,
    floorRevision: offered ? 3 : 2,
    intentRevision: 0,
    speechRevision: input.unread ? 1 : 0,
    currentSpeakerPubkey: null,
    currentOfferPubkey: offered ? CURRENT : null,
    floor: aborted
      ? null
      : {
          stateEventId: "2".repeat(64),
          humanQueue: [],
          offer: offered
            ? {
                offerId: "3".repeat(64),
                targetPubkey: CURRENT,
                targetParticipantType: "human",
                allocationSource: "moderator_select",
                turnRole: "participant",
                selectionReason: "The Human requested the Floor.",
                sourceIntentId: null,
                sourceRequestId: null,
                sourceHandoffId: null,
                sourceSpeechEventId: null,
                handoffContext: null,
                createdAtMs: input.updatedAt * 1_000,
                ackDeadlineMs: (input.updatedAt + 60) * 1_000,
              }
            : null,
          grant: null,
        },
    host: aborted
      ? null
      : {
          controlToken: "4".repeat(64),
          stateEventId: "2".repeat(64),
          controlEpoch: 2,
          decisionEpoch: 2,
          decisionDeadlineMs: null,
          nextActionAtMs: null,
          consecutiveModeratorSpeeches: 0,
          forcedReturnToModerator: false,
          pendingIntents: [],
          openHandoffs: [],
          boardControl: {
            phase: hostBoard ? "board_pending" : "floor_ready",
            controlEpoch: 2,
            boardWindow: 2,
            boardStartedAtMs: input.updatedAt * 1_000,
            boardDeadlineAtMs: hostBoard
              ? (input.updatedAt + 60) * 1_000
              : null,
            boardCompletedAtMs: hostBoard ? null : input.updatedAt * 1_000,
            boardOutcome: hostBoard ? null : "unchanged",
          },
          canSelect: !hostBoard && !offered,
          canClose: !hostBoard && !offered,
          canRecall: offered,
        },
    participants: [
      {
        pubkey: host,
        participantType: host === CURRENT ? "human" : "agent",
        channelRole: "owner",
      },
      ...(host === CURRENT
        ? [
            {
              pubkey: OTHER,
              participantType: "agent" as const,
              channelRole: "bot",
            },
          ]
        : [
            {
              pubkey: CURRENT,
              participantType: "human" as const,
              channelRole: "member",
            },
          ]),
    ],
    board: {
      eventId: "5".repeat(64),
      format: "markdown",
      body: "# Goal\nVerify Meeting sidebar attention.",
      moderatorPubkey: host,
      updatedAt: input.updatedAt,
      source: "projection",
    },
    action: null,
    end: aborted
      ? {
          eventId: "6".repeat(64),
          outcome: "aborted",
          reasonCode: "discussion_blocked",
          reason: "The meeting could not reach a safe conclusion.",
          endedBy: host,
          endedAt: input.updatedAt,
          actionsAttested: false,
          terminationSource: "host",
        }
      : null,
    latestSpeechAt: input.unread ? input.updatedAt + 1 : null,
  };
}

function meetings() {
  const inputs = [
    {
      id: IDS.aborted,
      title: "Aborted decision",
      lifecycle: "aborted" as const,
      updatedAt: NOW + 400,
    },
    {
      id: IDS.hostBoard,
      title: "Host Board work",
      host: CURRENT,
      updatedAt: NOW + 300,
    },
    {
      id: IDS.offer,
      title: "Your Floor offer",
      offered: true,
      updatedAt: NOW + 200,
    },
    {
      id: IDS.recent,
      title: "Recent Speech only",
      unread: true,
      updatedAt: NOW + 500,
    },
  ];
  return inputs.map((input) => ({
    id: input.id,
    title: input.title,
    result: { status: "ready" as const, snapshot: snapshot(input) },
    speeches: [],
  }));
}

test("Meeting sidebar separates identity attention from Speech unread and acknowledges aborts", async ({
  page,
}) => {
  await installMockBridge(page, { meetings: meetings() });
  await page.goto("/");

  const rows = page
    .getByTestId("meeting-active-list")
    .locator('[data-testid^="meeting-row-"]');
  await expect(rows).toHaveCount(4);
  await expect(rows).toHaveText([
    /Host Board work/,
    /Your Floor offer/,
    /Aborted decision/,
    /Recent Speech only/,
  ]);

  await expect(
    page.getByTestId(`meeting-attention-${IDS.hostBoard}`),
  ).toHaveAttribute("aria-label", "Complete Board maintenance");
  await expect(
    page.getByTestId(`meeting-attention-${IDS.offer}`),
  ).toHaveAttribute("aria-label", "Respond to the Floor offer");
  await expect(page.getByTestId(`meeting-unread-${IDS.offer}`)).toHaveCount(0);
  await expect(page.getByTestId(`meeting-unread-${IDS.recent}`)).toBeVisible();
  await expect(page.getByTestId(`meeting-attention-${IDS.recent}`)).toHaveCount(
    0,
  );

  await page.getByTestId(`meeting-row-${IDS.aborted}`).click();
  await expect(page.getByTestId("meeting-screen")).toHaveAttribute(
    "data-meeting-lifecycle",
    "aborted",
  );
  await expect(page.getByTestId(`meeting-row-${IDS.aborted}`)).toHaveCount(0);
  await expect(page.getByTestId(`meeting-unread-${IDS.recent}`)).toBeVisible();

  await page.reload();
  await expect(page.getByTestId(`meeting-row-${IDS.aborted}`)).toHaveCount(0);
  await page.getByTestId("meeting-history-trigger").click();
  await expect(page.getByTestId("meeting-history-list")).toContainText(
    "Aborted decision",
  );
  await expect(
    page.getByTestId(`meeting-history-attention-${IDS.aborted}`),
  ).toHaveCount(0);
  await expect(page.getByTestId(`meeting-unread-${IDS.recent}`)).toBeVisible();
});
