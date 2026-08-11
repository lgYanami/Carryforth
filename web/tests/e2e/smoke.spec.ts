import { createHash } from "node:crypto";
import { expect, test, type Page } from "@playwright/test";

const SOURCE_BUILD_URL =
  "https://github.com/lgYanami/Carryforth#build-and-run-from-source";

async function installNip07(page: Page, pubkey = "ab".repeat(32)) {
  await page.addInitScript((extensionPubkey) => {
    (
      window as Window & {
        nostr?: {
          getPublicKey(): Promise<string>;
          signEvent(
            event: Record<string, unknown>,
          ): Promise<Record<string, unknown>>;
        };
      }
    ).nostr = {
      async getPublicKey() {
        return extensionPubkey;
      },
      async signEvent(event) {
        return {
          ...event,
          id: "cd".repeat(32),
          pubkey: extensionPubkey,
          sig: "ef".repeat(64),
        };
      },
    };
  }, pubkey);
}

test("home page loads with Carryforth branding", async ({ page }) => {
  await page.goto("/");
  await expect(page).toHaveTitle("Carryforth");
  await expect(
    page.getByRole("main").getByRole("img", { name: "Carryforth" }),
  ).toBeVisible();
});

test("repository pages use source-build guidance without an app deep link", async ({
  page,
}) => {
  await page.goto("/?preview=repositories");
  await expect(page.getByText("Repositories")).toBeVisible();
  const sourceBuildLink = page.getByRole("link", {
    name: "Build Carryforth from source",
  });
  await expect(sourceBuildLink).toHaveAttribute("href", SOURCE_BUILD_URL);
  await expect(
    page.locator('a[href^="buzz://"], a[href^="carryforth://"]'),
  ).toHaveCount(0);
});

test("browser invite requires age and Carryforth legal consent", async ({
  page,
}) => {
  await installNip07(page);
  await page.route("**/api/join-policy", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        policy: {
          terms_markdown: "# Terms",
          privacy_markdown: "# Privacy",
          age_attestation_required: true,
          version: "policy-v1",
        },
      }),
    });
  });
  await page.goto("/invite/demo-code");

  await expect(
    page.getByRole("link", { name: "Build it from source" }),
  ).toHaveAttribute("href", SOURCE_BUILD_URL);

  const ageConfirmation = page.getByLabel("I am 18 years of age or older.");
  const agreementConfirmation = page.getByLabel(
    "I agree to the Carryforth Terms of Service and Privacy Policy.",
  );
  const joinInBrowser = page.getByRole("button", {
    name: "Join in browser",
  });

  await expect(ageConfirmation).toBeVisible();
  await expect(agreementConfirmation).toBeVisible();
  await expect(joinInBrowser).toBeDisabled();

  const termsLink = page.getByRole("button", { name: "Terms of Service" });
  const privacyLink = page.getByRole("button", { name: "Privacy Policy" });
  await expect(termsLink).toHaveCSS("text-decoration-line", "none");
  await expect(privacyLink).toHaveCSS("text-decoration-line", "none");
  await termsLink.hover();
  await expect(termsLink).toHaveCSS("text-decoration-line", "underline");
  await page.mouse.move(0, 0);
  await privacyLink.hover();
  await expect(privacyLink).toHaveCSS("text-decoration-line", "underline");

  await page
    .locator("label")
    .filter({ hasText: "I am 18 years of age or older." })
    .click();
  await expect(ageConfirmation).toBeChecked();
  await expect(joinInBrowser).toBeDisabled();
  await page
    .locator("label")
    .filter({
      hasText: "I agree to the Carryforth Terms of Service and Privacy Policy.",
    })
    .click({ position: { x: 8, y: 8 } });
  await expect(agreementConfirmation).toBeChecked();
  await expect(joinInBrowser).toBeEnabled();

  const consentBox = await page
    .getByTestId("invite-join-policy-notice")
    .boundingBox();
  const joinButtonBox = await joinInBrowser.boundingBox();
  expect(consentBox?.y).toBeLessThan(joinButtonBox?.y ?? 0);
  expect(consentBox?.width).toBe(joinButtonBox?.width);
});

test("invite can enroll a NIP-07 identity for browser access", async ({
  page,
}) => {
  const pubkey = "ab".repeat(32);
  await installNip07(page, pubkey);
  await page.route("**/api/join-policy", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ policy: null }),
    });
  });

  let claimObserved = false;
  await page.route("**/api/invites/claim", async (route) => {
    claimObserved = true;
    const request = route.request();
    const body = request.postData() ?? "";
    expect(JSON.parse(body)).toEqual({ code: "browser-code" });

    const authorization = request.headers().authorization;
    expect(authorization).toMatch(/^Nostr /);
    const event = JSON.parse(
      Buffer.from(authorization.slice("Nostr ".length), "base64").toString(
        "utf8",
      ),
    ) as {
      pubkey: string;
      tags: string[][];
    };
    expect(event.pubkey).toBe(pubkey);
    expect(event.tags).toContainEqual(["u", request.url()]);
    expect(event.tags).toContainEqual(["method", "POST"]);
    expect(event.tags).toContainEqual([
      "payload",
      createHash("sha256").update(body).digest("hex"),
    ]);

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        status: "joined",
        community_id: "community-id",
        host: "127.0.0.1",
        role: "member",
      }),
    });
  });

  await page.goto("/invite/browser-code");
  await page.getByRole("button", { name: "Join in browser" }).click();
  await expect(page).toHaveURL("/");
  expect(claimObserved).toBe(true);
});

test("invite fails closed without NIP-07 or a supported desktop handoff", async ({
  page,
}) => {
  const legacyRequests: string[] = [];
  page.on("request", (request) => {
    if (
      request.url().includes("api.github.com/repos/block/buzz") ||
      request.url().includes("github.com/block/buzz/releases")
    ) {
      legacyRequests.push(request.url());
    }
  });
  await page.route("**/api/join-policy", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ policy: null }),
    });
  });

  await page.goto("/invite/source-only-code");

  await expect(
    page.getByTestId("desktop-invite-handoff-unavailable"),
  ).toContainText("Desktop invite handoff is not available");
  await expect(
    page.getByRole("link", { name: "Build it from source" }),
  ).toHaveAttribute("href", SOURCE_BUILD_URL);
  await expect(
    page.getByRole("button", { name: "Join in browser" }),
  ).toHaveCount(0);
  await expect(
    page.locator('a[href^="buzz://"], a[href^="carryforth://"]'),
  ).toHaveCount(0);
  expect(legacyRequests).toEqual([]);
});
