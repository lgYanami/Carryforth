import { expect, test } from "@playwright/test";

import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

const GENERAL_CHANNEL_ID = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const HUMAN = TEST_IDENTITIES.bob.pubkey;
const AGENT = "42".repeat(32);
const ACTION_CAPABILITY = "meeting-v2-action-finalization-v2";

const creatableCapability = {
  status: "creatable" as const,
  relayPubkey: "ab".repeat(32),
  supportsDirectActions: true,
  canCreateDirectActions: true,
};

function directorySeed(agentCapabilities = [ACTION_CAPABILITY]) {
  return {
    searchProfiles: [
      {
        pubkey: HUMAN,
        displayName: "Bob Human",
        isAgent: false,
      },
      {
        pubkey: AGENT,
        displayName: "Action Agent",
        ownerPubkey: TEST_IDENTITIES.tyler.pubkey,
        isAgent: true,
      },
    ],
    relayAgents: [
      {
        pubkey: AGENT,
        name: "Action Agent",
        capabilities: agentCapabilities,
        status: "online" as const,
      },
    ],
  };
}

async function selectParticipant(
  page: import("@playwright/test").Page,
  query: string,
  pubkey: string,
) {
  await page.getByTestId("meeting-roster-search").fill(query);
  const candidate = page.getByTestId(`meeting-roster-candidate-${pubkey}`);
  await expect(candidate).toBeVisible();
  await candidate.click();
}

async function fillRequiredDraft(page: import("@playwright/test").Page) {
  await page
    .getByTestId("meeting-create-title")
    .fill("Desktop creation review");
  await page
    .getByTestId("meeting-create-goal")
    .fill("Agree on the Human Meeting creation boundary.");
}

test("Human starts a self-hosted Meeting with Human and compatible Agent participants", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetingCapability: creatableCapability,
    meetings: [],
    ...directorySeed(),
  });
  await page.goto("/");

  await page.getByTestId("meeting-create-trigger").click();
  await expect(page.getByRole("dialog")).toContainText("You · Human host");
  await fillRequiredDraft(page);
  await page.getByTestId("meeting-add-agenda").click();
  await page.getByTestId("meeting-agenda-0").fill("Review signed retry");
  await selectParticipant(page, "Bob Human", HUMAN);
  await selectParticipant(page, "Action Agent", AGENT);
  await expect(page.getByTestId("meeting-roster-selected")).toContainText(
    "Action ready",
  );
  await expect(page.getByTestId("meeting-create-board")).toHaveValue(
    /## Discussion goal[\s\S]*## Agenda[\s\S]*Review signed retry/u,
  );

  const submit = page.getByTestId("meeting-create-submit");
  await expect(submit).toBeEnabled();
  await submit.click();

  await expect(page.getByTestId("meeting-screen")).toBeVisible();
  await expect(page.getByTestId("meeting-board").first()).toContainText(
    "Agree on the Human Meeting creation boundary",
  );
  await expect(page.getByTestId("message-composer")).toHaveCount(0);

  const createPayload = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? []).findLast(
      (entry) => entry.command === "create_meeting",
    ),
  );
  expect(createPayload?.payload.input.participantPubkeys).toEqual([
    HUMAN,
    AGENT,
  ]);
  expect(createPayload?.payload.input.participantPubkeys).not.toContain(
    TEST_IDENTITIES.tyler.pubkey,
  );
  expect(createPayload?.payload.input.initialBoard).toContain(
    "1. Review signed retry",
  );
});

test("Channel entry prefills removable source and retries an indeterminate Create exactly", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetingCapability: creatableCapability,
    meetingCreateIndeterminateResponses: 1,
    meetings: [],
    ...directorySeed(),
  });
  await page.goto("/");

  await page.getByTestId("channel-general").click({ button: "right" });
  await page.getByTestId("start-meeting-from-channel-general").click();
  await expect(page.getByTestId("meeting-create-source")).toHaveValue(
    GENERAL_CHANNEL_ID,
  );
  await page.getByTestId("meeting-create-source").selectOption("");
  await expect(page.getByTestId("meeting-create-source")).toHaveValue("");
  await page
    .getByTestId("meeting-create-source")
    .selectOption(GENERAL_CHANNEL_ID);
  await fillRequiredDraft(page);
  await selectParticipant(page, "Bob Human", HUMAN);

  await page.getByTestId("meeting-create-submit").click();
  await expect(page.getByTestId("meeting-create-indeterminate")).toBeVisible();
  await expect(page.getByTestId("meeting-create-title")).toHaveValue(
    "Desktop creation review",
  );
  await expect(page.getByTestId("meeting-create-title")).toBeDisabled();

  const firstInput = await page.evaluate(() => {
    const calls = (window.__BUZZ_E2E_COMMAND_LOG__ ?? []).filter(
      (entry) => entry.command === "create_meeting",
    );
    return calls[0]?.payload.input;
  });
  await page.getByTestId("meeting-create-submit").click();
  await expect(page.getByTestId("meeting-screen")).toBeVisible();

  const inputs = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter((entry) => entry.command === "create_meeting")
      .map((entry) => entry.payload.input),
  );
  expect(inputs).toHaveLength(2);
  expect(inputs[1].submissionId).toBe(firstInput.submissionId);
  expect(inputs[1].initialBoard).toBe(firstInput.initialBoard);
  expect(inputs[1].sourceChannelId).toBe(GENERAL_CHANNEL_ID);
});

test("closed Relay create gate and incompatible Agent are explicit blockers", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetingCapability: {
      ...creatableCapability,
      status: "readable",
      canCreateDirectActions: false,
    },
    meetings: [],
    ...directorySeed([]),
  });
  await page.goto("/");
  await page.getByTestId("meeting-create-trigger").click();
  await expect(page.getByRole("dialog")).toContainText(
    "does not currently allow direct-action Meeting creation",
  );
  await expect(page.getByTestId("meeting-create-submit")).toBeDisabled();

  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    window.dispatchEvent(
      new CustomEvent("buzz:open-create-meeting", { detail: {} }),
    );
  });
  await fillRequiredDraft(page);
  await selectParticipant(page, "Action Agent", AGENT);
  await expect(page.getByTestId("meeting-roster-selected")).toContainText(
    "Missing action capability",
  );
  await expect(page.getByTestId("meeting-create-submit")).toBeDisabled();
});
