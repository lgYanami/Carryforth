import { expect, type Locator, type Page, test } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge } from "../helpers/bridge";
import {
  denseProjectContextFixture,
  denseProjectDocumentFixture,
  denseProjectViewFixture,
  installWorkspaceSemanticResult,
  openProjectContextWorkspace,
  type DenseProjectContextFixture,
} from "../helpers/projectContextWorkspace";

const SHOTS = "test-results/project-context-workspace";

type ViewportObservation = {
  worldX: number;
  worldY: number;
  zoom: number;
};

async function seedTheme(page: Page, theme: "buzz" | "buzz-dark") {
  await page.addInitScript(
    ({ key, value }) => window.localStorage.setItem(key, value),
    { key: "buzz-theme", value: theme },
  );
}

async function openDenseWorkspace(
  page: Page,
  input: {
    height?: number;
    theme?: "buzz" | "buzz-dark";
    width?: number;
  } = {},
): Promise<DenseProjectContextFixture> {
  const dense = denseProjectContextFixture();
  await page.setViewportSize({
    width: input.width ?? 1440,
    height: input.height ?? 900,
  });
  await seedTheme(page, input.theme ?? "buzz");
  await installMockBridge(page, {
    projectContext: dense.result,
    projectDocument: denseProjectDocumentFixture(dense),
    projectView: denseProjectViewFixture(dense),
  });
  await openProjectContextWorkspace(page);
  const graph = page.getByTestId("project-context-graph");
  await expect(graph).toHaveAttribute("data-chrome-ready", "true");
  await expect(graph).toHaveAttribute("data-auto-fit-count", "1");
  return dense;
}

async function projectContextCallCounts(page: Page) {
  return page.evaluate(() => ({
    context: window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length ?? 0,
    semantic: window.__BUZZ_E2E_PROJECT_CONTEXT_SEMANTIC_CALLS__?.length ?? 0,
  }));
}

async function graphGeometry(page: Page) {
  return page
    .getByTestId("project-context-graph")
    .locator(
      '[data-context-graph-kind="coordinate"], [data-context-graph-kind="edge"]',
    )
    .evaluateAll((items) =>
      items
        .map((item) => {
          const node = item.closest(".react-flow__node");
          return {
            id: item.getAttribute("data-testid"),
            style: node?.getAttribute("style") ?? "",
          };
        })
        .sort((left, right) => (left.id ?? "").localeCompare(right.id ?? "")),
    );
}

async function observeViewport(graph: Locator): Promise<ViewportObservation> {
  return graph.evaluate((root) => {
    const viewport = root.querySelector<HTMLElement>(".react-flow__viewport");
    if (!viewport) throw new Error("React Flow viewport is unavailable.");
    const matrix = new DOMMatrixReadOnly(getComputedStyle(viewport).transform);
    return {
      worldX: (root.clientWidth / 2 - matrix.e) / matrix.a,
      worldY: (root.clientHeight / 2 - matrix.f) / matrix.d,
      zoom: matrix.a,
    };
  });
}

async function increaseRootTextScale(page: Page, steps: number) {
  await page.evaluate((count) => {
    const applePlatform = /mac|iphone|ipad|ipod/i.test(navigator.platform);
    for (let index = 0; index < count; index += 1) {
      window.dispatchEvent(
        new KeyboardEvent("keydown", {
          bubbles: true,
          cancelable: true,
          code: "Equal",
          ctrlKey: !applePlatform,
          key: "+",
          metaKey: applePlatform,
          shiftKey: true,
        }),
      );
    }
  }, steps);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          Number.parseFloat(
            getComputedStyle(document.documentElement).fontSize,
          ) / 16,
      ),
    )
    .toBeCloseTo(1 + steps / 10, 5);
}

async function increaseRootTextScaleWhenFitBecomesPending(page: Page) {
  const observedPending = await page.evaluate(
    () =>
      new Promise<boolean>((resolve, reject) => {
        const graph = document.querySelector<HTMLElement>(
          '[data-testid="project-context-graph"]',
        );
        const fitAll = document.querySelector<HTMLElement>(
          '[data-testid="project-context-fit-all-canvas"]',
        );
        if (!graph || !fitAll) {
          reject(new Error("Fit race controls are unavailable."));
          return;
        }
        let timeout = 0;
        const observer = new MutationObserver(() => {
          if (
            graph.getAttribute("data-viewport-authority-pending") !== "true"
          ) {
            return;
          }
          observer.disconnect();
          window.clearTimeout(timeout);
          const applePlatform = /mac|iphone|ipad|ipod/i.test(
            navigator.platform,
          );
          window.dispatchEvent(
            new KeyboardEvent("keydown", {
              bubbles: true,
              cancelable: true,
              code: "Equal",
              ctrlKey: !applePlatform,
              key: "+",
              metaKey: applePlatform,
              shiftKey: true,
            }),
          );
          resolve(true);
        });
        observer.observe(graph, {
          attributes: true,
          attributeFilter: ["data-viewport-authority-pending"],
        });
        timeout = window.setTimeout(() => {
          observer.disconnect();
          reject(new Error("Manual Fit did not become pending."));
        }, 1_000);
        fitAll.click();
      }),
  );
  expect(observedPending).toBe(true);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          Number.parseFloat(
            getComputedStyle(document.documentElement).fontSize,
          ) / 16,
      ),
    )
    .toBeCloseTo(1.1, 5);
}

function expectViewportClose(
  actual: ViewportObservation,
  expected: ViewportObservation,
) {
  expect(actual.zoom).toBeCloseTo(expected.zoom, 3);
  expect(Math.abs(actual.worldX - expected.worldX)).toBeLessThanOrEqual(2);
  expect(Math.abs(actual.worldY - expected.worldY)).toBeLessThanOrEqual(2);
}

async function settleWorkspaceChrome(page: Page) {
  const graph = page.getByTestId("project-context-graph");
  await expect(graph).toHaveAttribute("data-chrome-ready", "true");
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
}

async function assertNoNodeChromeOverlap(page: Page) {
  const overlaps = await page
    .getByTestId("project-context-graph")
    .evaluate((graph) => {
      const boxes = (selector: string) =>
        [...graph.querySelectorAll<HTMLElement>(selector)].map((element) => {
          const box = element.getBoundingClientRect();
          return {
            bottom: box.bottom,
            left: box.left,
            right: box.right,
            top: box.top,
          };
        });
      const nodes = [
        ...graph.querySelectorAll<HTMLElement>(
          '[data-context-graph-kind="coordinate"]',
        ),
      ].map((content) => {
        const element = content.closest<HTMLElement>(".react-flow__node");
        if (!element)
          throw new Error("Coordinate is missing its React Flow node");
        const box = element.getBoundingClientRect();
        return {
          bottom: box.bottom,
          left: box.left,
          right: box.right,
          top: box.top,
        };
      });
      const rail = document.querySelector<HTMLElement>(
        '[data-testid="project-context-tools-rail"]',
      );
      const railBox = rail?.getBoundingClientRect();
      const chrome = [
        ...boxes("[data-project-context-chrome-contributor]"),
        ...(railBox
          ? [
              {
                bottom: railBox.bottom,
                left: railBox.left,
                right: railBox.right,
                top: railBox.top,
              },
            ]
          : []),
      ];
      return {
        chromeCount: chrome.length,
        nodeCount: nodes.length,
        intersections: nodes.flatMap((node, nodeIndex) =>
          chrome.flatMap((contributor, chromeIndex) => {
            const width =
              Math.min(node.right, contributor.right) -
              Math.max(node.left, contributor.left);
            const height =
              Math.min(node.bottom, contributor.bottom) -
              Math.max(node.top, contributor.top);
            return width > 1 && height > 1 ? [{ chromeIndex, nodeIndex }] : [];
          }),
        ),
      };
    });
  expect(overlaps.nodeCount).toBe(44);
  expect(overlaps.chromeCount).toBeGreaterThan(0);
  expect(overlaps.intersections).toEqual([]);
}

for (const viewport of [
  { width: 1280, height: 800 },
  { width: 1440, height: 900 },
]) {
  test(`default ${viewport.width}x${viewport.height} workspace is a fitted full canvas`, async ({
    page,
  }) => {
    const dense = await openDenseWorkspace(page, viewport);
    const workspace = page.getByTestId("project-context-workspace");
    const graphSlot = page.getByTestId("project-context-graph-slot");
    const graph = page.getByTestId("project-context-graph");
    const rail = page.getByTestId("project-context-tools-rail");

    await expect(workspace).toBeVisible();
    await expect(rail).toHaveCount(1);
    await expect(page.getByTestId("project-context-tool-panel")).toHaveCount(0);
    for (const tool of ["structure", "semantic", "details"] as const) {
      await expect(
        page.getByTestId(`project-context-tool-${tool}`),
      ).toHaveAttribute("aria-expanded", "false");
    }
    await expect(page.getByTestId("project-context-query-bar")).toHaveCount(0);
    await expect(
      page.getByTestId("project-context-semantic-query-bar"),
    ).toHaveCount(0);
    await expect(page.getByTestId("project-context-canvas-hud")).toContainText(
      "44",
    );
    await expect(page.getByTestId("project-context-canvas-hud")).toContainText(
      "21",
    );
    await expect(page.getByTestId("project-context-canvas-hud")).toContainText(
      "22",
    );
    await expect(
      page.getByTestId("project-context-island-summary"),
    ).toContainText("2 context islands");

    await expect(
      graph.locator('[data-context-graph-kind="coordinate"]'),
    ).toHaveCount(dense.coordinateKeys.length);
    await expect(graph.locator('[data-context-graph-kind="edge"]')).toHaveCount(
      dense.edgeKeys.length,
    );
    const bounds = await Promise.all([
      workspace.boundingBox(),
      graphSlot.boundingBox(),
      graph.boundingBox(),
    ]);
    expect(bounds.every(Boolean)).toBe(true);
    const [workspaceBox, slotBox, graphBox] = bounds;
    if (!workspaceBox || !slotBox || !graphBox) {
      throw new Error("Full-canvas bounds are unavailable.");
    }
    expect(Math.abs(slotBox.y - workspaceBox.y)).toBeLessThanOrEqual(1);
    expect(
      Math.abs(
        slotBox.y + slotBox.height - (workspaceBox.y + workspaceBox.height),
      ),
    ).toBeLessThanOrEqual(1);
    expect(Math.abs(graphBox.y - slotBox.y)).toBeLessThanOrEqual(1);
    expect(
      Math.abs(graphBox.y + graphBox.height - (slotBox.y + slotBox.height)),
    ).toBeLessThanOrEqual(1);
    const overflow = await page.evaluate(() => ({
      body: document.body.scrollHeight - window.innerHeight,
      document: document.documentElement.scrollHeight - window.innerHeight,
    }));
    expect(overflow.body).toBeLessThanOrEqual(1);
    expect(overflow.document).toBeLessThanOrEqual(1);
    await assertNoNodeChromeOverlap(page);

    const initialTransform = await graph
      .locator(".react-flow__viewport")
      .getAttribute("style");
    await page.getByRole("button", { name: "Zoom in" }).click();
    await expect
      .poll(() => graph.locator(".react-flow__viewport").getAttribute("style"))
      .not.toBe(initialTransform);
    await page.getByTestId("project-context-fit-all-canvas").click();
    await page.getByTestId("project-context-fit-island-2").click();
    await expect(graph).toHaveAttribute("data-auto-fit-count", "1");

    if (viewport.width === 1440) {
      await page.getByTestId("project-context-fit-all-canvas").click();
      await waitForAnimations(page);
      await workspace.screenshot({ path: `${SHOTS}/01-dense-light.png` });
    }
  });
}

test("dark full canvas keeps the same collapsed information architecture", async ({
  page,
}) => {
  await openDenseWorkspace(page, { theme: "buzz-dark" });
  await expect(page.locator("html")).toHaveClass(/dark/);
  await expect(page.getByTestId("project-context-tools-rail")).toHaveCount(1);
  await expect(page.getByTestId("project-context-tool-panel")).toHaveCount(0);
  await waitForAnimations(page);
  await page.getByTestId("project-context-workspace").screenshot({
    path: `${SHOTS}/02-dense-dark.png`,
  });
});

test("tool switches preserve controlled drafts and never remount or refetch the graph", async ({
  page,
}) => {
  const dense = await openDenseWorkspace(page, { width: 1600 });
  const graph = page.getByTestId("project-context-graph");
  const initialCalls = await projectContextCallCounts(page);
  const initialGeometry = await graphGeometry(page);
  const initialAutoFit = await graph.getAttribute("data-auto-fit-count");
  await graph.evaluate((element) => {
    element.setAttribute("data-e2e-dom-identity", "preserved");
  });

  await page.getByTestId("project-context-tool-structure").click();
  await expect(page.getByTestId("project-context-tool-panel")).toHaveAttribute(
    "data-presentation",
    "docked",
  );
  await page.getByTestId("project-context-mode-contains_all").click();
  await page.getByTestId("project-context-coordinate-picker").click();
  await page
    .getByTestId("project-context-coordinate-search")
    .fill("Workspace requirement 1");
  await page.getByTestId("project-context-coordinate-search").press("Enter");
  await expect(page.getByTestId("project-context-query-chips")).toContainText(
    "Workspace requirement 1",
  );

  await page.getByTestId("project-context-tool-semantic").click();
  const problem = "Which dense Context path should be prioritized next?";
  await page.getByTestId("project-context-semantic-problem").fill(problem);
  await page.getByText("Optional graph inputs", { exact: true }).click();
  await page.getByTestId("project-context-semantic-initial-picker").click();
  await page
    .getByTestId("project-context-semantic-initial-search")
    .fill("Workspace work 2");
  await page
    .getByTestId("project-context-semantic-initial-search")
    .press("Enter");
  await page.keyboard.press("Escape");
  await page.getByTestId("project-context-semantic-context-picker").click();
  await page
    .getByTestId("project-context-semantic-context-search")
    .fill("Workspace resource 3");
  await page
    .getByTestId("project-context-semantic-context-search")
    .press("Enter");
  await page.keyboard.press("Escape");
  await expect(
    page.getByTestId("project-context-semantic-initial-chips"),
  ).toContainText("Workspace work 2");
  await expect(
    page.getByTestId("project-context-semantic-context-chips"),
  ).toContainText("Workspace resource 3");

  await page.getByTestId("project-context-tools-collapse").click();
  await expect(page.getByTestId("project-context-tool-panel")).toHaveCount(0);
  await page.getByTestId("project-context-tool-semantic").click();
  await expect(
    page.getByTestId("project-context-semantic-problem"),
  ).toHaveValue(problem);
  await expect(
    page.getByTestId("project-context-semantic-initial-chips"),
  ).toContainText("Workspace work 2");
  await expect(
    page.getByTestId("project-context-semantic-context-chips"),
  ).toContainText("Workspace resource 3");

  await page.getByTestId("project-context-tool-structure").click();
  await expect(page.getByTestId("project-context-query-chips")).toContainText(
    "Workspace requirement 1",
  );
  await page.getByTestId("project-context-coordinate-picker").click();
  await expect(
    page.getByTestId("project-context-coordinate-search"),
  ).toBeVisible();
  await page
    .getByTestId("project-context-tool-semantic")
    .click({ force: true });
  await expect(
    page.getByTestId("project-context-coordinate-search"),
  ).toHaveCount(0);

  await page.getByTestId("project-context-tools-collapse").click();
  await settleWorkspaceChrome(page);

  await expect(graph).toHaveAttribute("data-e2e-dom-identity", "preserved");
  expect(await graphGeometry(page)).toEqual(initialGeometry);
  expect(await projectContextCallCounts(page)).toEqual(initialCalls);
  await expect(graph).toHaveAttribute(
    "data-auto-fit-count",
    initialAutoFit ?? "1",
  );
  await expect(
    graph.locator('[data-context-graph-kind="coordinate"]'),
  ).toHaveCount(dense.coordinateKeys.length);
});

test("Coordinate, Hub, and Spoke selection share one Details panel and preserve return state", async ({
  page,
}) => {
  const dense = await openDenseWorkspace(page, { width: 1440 });
  const selected = encodeURIComponent(`coordinate:${dense.coordinateKeys[0]}`);
  await page.goto(`/#/project-context?selected=${selected}`);
  await page.reload();
  const graph = page.getByTestId("project-context-graph");
  await expect(graph).toHaveAttribute("data-chrome-ready", "true");
  const coordinate = page.getByTestId(
    `project-context-coordinate-${dense.coordinateKeys[0]}`,
  );
  const coordinateButton = coordinate.getByRole("button");
  const announcement = page.getByTestId(
    "project-context-workspace-announcement",
  );
  await expect(
    page
      .getByTestId("project-context-workspace")
      .locator('[aria-live="polite"]'),
  ).toHaveCount(1);

  await expect(page).toHaveURL(/selected=coordinate/);
  await expect(
    page.getByTestId("project-context-tool-details"),
  ).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByTestId("project-context-tool-panel")).toHaveAttribute(
    "data-presentation",
    "docked",
  );
  await expect(page.getByTestId("project-context-tool-details")).toBeFocused();
  await expect(
    page.getByTestId("project-context-coordinate-inspector"),
  ).toContainText("Workspace requirement 1");
  await expect(coordinateButton).toHaveAttribute("aria-pressed", "true");
  await page.getByTestId("project-context-focus-selection").focus();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await waitForAnimations(page);
  await page.getByTestId("project-context-workspace").screenshot({
    path: `${SHOTS}/03-details-docked.png`,
  });

  await page.getByTestId("project-context-tools-collapse").click();
  await expect(page.getByTestId("project-context-tool-panel")).toHaveCount(0);
  await expect(page).toHaveURL(/selected=coordinate/);
  await expect(coordinateButton).toHaveAttribute("aria-pressed", "true");
  await page.getByTestId("project-context-tool-details").click();
  await expect(
    page.getByTestId("project-context-coordinate-inspector"),
  ).toContainText("Workspace requirement 1");

  await page.getByTestId("project-context-tool-semantic").click();
  await expect(page).toHaveURL(/selected=coordinate/);
  await coordinateButton.click();
  await expect(page).not.toHaveURL(/selected=/);
  await expect(
    page.getByTestId("project-context-semantic-query-bar"),
  ).toBeVisible();
  await coordinateButton.click();
  await expect(announcement).toHaveText("Coordinate details selected.");
  const coordinateAnnouncement = await announcement
    .locator("span")
    .elementHandle();
  await page.getByTestId("project-context-details-close").click();
  await expect(page).not.toHaveURL(/selected=/);

  const hub = page.getByTestId(`project-context-edge-${dense.edgeKeys[0]}`);
  await hub.getByRole("button").click();
  await expect(page).toHaveURL(/selected=edge/);
  await expect(announcement).toHaveText("Context Edge details selected.");
  expect(
    await coordinateAnnouncement?.evaluate((element) => element.isConnected),
  ).toBe(false);
  await expect(
    page.getByTestId("project-context-edge-inspector"),
  ).toBeVisible();
  await page
    .getByTestId("project-context-graph")
    .locator(".react-flow__pane")
    .click({ force: true, position: { x: 8, y: 8 } });
  await expect(page).not.toHaveURL(/selected=/);
  await expect(
    page.getByTestId("project-context-semantic-query-bar"),
  ).toBeVisible();

  const spokeId = `spoke:${dense.edgeKeys[0]}:${dense.coordinateKeys[0]}`;
  await page
    .locator(
      `.react-flow__edge[data-id="${spokeId}"] .react-flow__edge-interaction`,
    )
    .dispatchEvent("click");
  await expect(
    page.getByTestId("project-context-edge-inspector"),
  ).toBeVisible();
  await page.getByTestId("project-context-details-close").click();
  await expect(page).not.toHaveURL(/selected=/);
  await expect(
    page.getByTestId("project-context-semantic-query-bar"),
  ).toBeVisible();

  await coordinateButton.click();
  await page.keyboard.press("Escape");
  await expect(page).not.toHaveURL(/selected=/);
  await expect(coordinateButton).toBeFocused();
});

test("panel open, resize, switch, and collapse preserve zoom and the world-center point", async ({
  page,
}) => {
  await openDenseWorkspace(page, { width: 1600 });
  const graph = page.getByTestId("project-context-graph");
  const pane = graph.locator(".react-flow__pane");
  const paneBox = await pane.boundingBox();
  if (!paneBox) throw new Error("Graph pane bounds are unavailable.");

  await page.mouse.move(
    paneBox.x + paneBox.width / 2,
    paneBox.y + paneBox.height / 2,
  );
  await page.mouse.down();
  await page.mouse.move(
    paneBox.x + paneBox.width / 2 + 96,
    paneBox.y + paneBox.height / 2 + 48,
    { steps: 4 },
  );
  await page.mouse.up();
  await page.getByRole("button", { name: "Zoom in" }).click();
  await page.waitForTimeout(250);
  const authoredViewport = await observeViewport(graph);
  const autoFitCount = await graph.getAttribute("data-auto-fit-count");
  const authorityGeneration = Number(
    await graph.getAttribute("data-viewport-authority-generation"),
  );

  await page.getByTestId("project-context-tool-structure").click();
  await settleWorkspaceChrome(page);
  expectViewportClose(await observeViewport(graph), authoredViewport);

  const resizeHandle = page.getByTestId("project-context-tools-resize-handle");
  const handleBox = await resizeHandle.boundingBox();
  if (!handleBox)
    throw new Error("Docked resize handle bounds are unavailable.");
  await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + 40);
  await page.mouse.down();
  await page.mouse.move(handleBox.x - 80, handleBox.y + 40, { steps: 6 });
  await page.mouse.up();
  await settleWorkspaceChrome(page);
  expectViewportClose(await observeViewport(graph), authoredViewport);

  await page.getByTestId("project-context-tool-semantic").click();
  await settleWorkspaceChrome(page);
  expectViewportClose(await observeViewport(graph), authoredViewport);
  await page.getByTestId("project-context-tools-collapse").click();
  await settleWorkspaceChrome(page);
  expectViewportClose(await observeViewport(graph), authoredViewport);
  await expect(graph).toHaveAttribute(
    "data-auto-fit-count",
    autoFitCount ?? "1",
  );
  expect(
    Number(await graph.getAttribute("data-viewport-correction-count")),
  ).toBeGreaterThan(0);
  expect(
    Number(await graph.getAttribute("data-viewport-authority-generation")),
  ).toBe(authorityGeneration);

  await page.getByTestId("project-context-tool-structure").click();
  await settleWorkspaceChrome(page);
  const correctionCountBeforeResizeRace = Number(
    await graph.getAttribute("data-viewport-correction-count"),
  );
  const humanGenerationBeforeResizeRace = Number(
    await graph.getAttribute("data-human-viewport-generation"),
  );
  const authorityBeforeResizeRace = Number(
    await graph.getAttribute("data-viewport-authority-generation"),
  );
  const resizedCanvas = await page.evaluate(
    () =>
      new Promise<{ afterWidth: number; beforeWidth: number }>(
        (resolve, reject) => {
          const graphRoot = document.querySelector<HTMLElement>(
            '[data-testid="project-context-graph"]',
          );
          const handle = document.querySelector<HTMLElement>(
            '[data-testid="project-context-tools-resize-handle"]',
          );
          const zoomIn = graphRoot?.querySelector<HTMLElement>(
            'button[aria-label="Zoom in"]',
          );
          if (!graphRoot || !handle || !zoomIn) {
            reject(new Error("Viewport race controls are unavailable."));
            return;
          }
          const beforeWidth = graphRoot.clientWidth;
          const timeout = window.setTimeout(() => {
            observer.disconnect();
            reject(new Error("Docked resize did not reach ResizeObserver."));
          }, 1_000);
          const observer = new ResizeObserver(() => {
            const afterWidth = graphRoot.clientWidth;
            if (Math.abs(afterWidth - beforeWidth) < 1) return;
            observer.disconnect();
            window.clearTimeout(timeout);
            // The app observer has queued its two-RAF correction. Claim Human
            // authority in the same observer delivery, before either RAF runs.
            zoomIn.click();
            resolve({ afterWidth, beforeWidth });
          });
          observer.observe(graphRoot);
          const box = handle.getBoundingClientRect();
          const pointer = {
            bubbles: true,
            cancelable: true,
            clientX: box.x + box.width / 2,
            clientY: box.y + 40,
            isPrimary: true,
            pointerId: 91,
            pointerType: "mouse",
          };
          handle.dispatchEvent(
            new PointerEvent("pointerdown", {
              ...pointer,
              button: 0,
              buttons: 1,
            }),
          );
          window.dispatchEvent(
            new PointerEvent("pointermove", {
              ...pointer,
              buttons: 1,
              clientX: pointer.clientX - 72,
            }),
          );
          window.dispatchEvent(
            new PointerEvent("pointerup", {
              ...pointer,
              button: 0,
              buttons: 0,
              clientX: pointer.clientX - 72,
            }),
          );
        },
      ),
  );
  expect(Math.abs(resizedCanvas.afterWidth - resizedCanvas.beforeWidth)).toBe(
    72,
  );
  await expect
    .poll(() => graph.getAttribute("data-viewport-authority-pending"), {
      timeout: 1_000,
    })
    .toBe("false");
  expect(
    Number(await graph.getAttribute("data-human-viewport-generation")),
  ).toBeGreaterThan(humanGenerationBeforeResizeRace);
  expect(
    Number(await graph.getAttribute("data-viewport-authority-generation")),
  ).toBeGreaterThan(authorityBeforeResizeRace);
  expect(
    Number(await graph.getAttribute("data-viewport-correction-count")),
  ).toBe(correctionCountBeforeResizeRace);
  const humanResizeViewport = await observeViewport(graph);
  await page.waitForTimeout(350);
  expectViewportClose(await observeViewport(graph), humanResizeViewport);

  const authorityBeforeFitRace = Number(
    await graph.getAttribute("data-viewport-authority-generation"),
  );
  const humanGenerationBeforeFitRace = Number(
    await graph.getAttribute("data-human-viewport-generation"),
  );
  await page.getByTestId("project-context-fit-all-canvas").click();
  await expect(graph).toHaveAttribute(
    "data-viewport-authority-pending",
    "true",
  );
  await graph.getByRole("button", { name: "Zoom in" }).click();
  expect(
    Number(await graph.getAttribute("data-human-viewport-generation")),
  ).toBeGreaterThan(humanGenerationBeforeFitRace);
  expect(
    Number(await graph.getAttribute("data-viewport-authority-generation")),
  ).toBeGreaterThan(authorityBeforeFitRace + 1);
  await expect
    .poll(() => graph.getAttribute("data-viewport-authority-pending"), {
      timeout: 1_000,
    })
    .toBe("false");
  const humanFitViewport = await observeViewport(graph);
  await page.waitForTimeout(500);
  expectViewportClose(await observeViewport(graph), humanFitViewport);
});

test("root text scale fences an in-flight animated Fit", async ({ page }) => {
  await openDenseWorkspace(page, { width: 1600 });
  const graph = page.getByTestId("project-context-graph");
  const pane = graph.locator(".react-flow__pane");
  const paneBox = await pane.boundingBox();
  if (!paneBox) throw new Error("Graph pane bounds are unavailable.");
  await page.mouse.move(
    paneBox.x + paneBox.width / 2,
    paneBox.y + paneBox.height / 2,
  );
  await page.mouse.down();
  await page.mouse.move(
    paneBox.x + paneBox.width / 2 + 96,
    paneBox.y + paneBox.height / 2 + 48,
    { steps: 4 },
  );
  await page.mouse.up();
  await graph.getByRole("button", { name: "Zoom in" }).click();
  await page.waitForTimeout(250);
  const beforeScale = await observeViewport(graph);
  const authorityBeforeFit = Number(
    await graph.getAttribute("data-viewport-authority-generation"),
  );

  await increaseRootTextScaleWhenFitBecomesPending(page);
  await expect
    .poll(() => graph.getAttribute("data-viewport-authority-pending"), {
      timeout: 1_000,
    })
    .toBe("false");
  expect(
    Number(await graph.getAttribute("data-viewport-authority-generation")),
  ).toBeGreaterThan(authorityBeforeFit + 1);

  const textScaleViewport = await observeViewport(graph);
  expect(textScaleViewport.zoom).toBeCloseTo(beforeScale.zoom, 3);
  expect(
    Math.abs(textScaleViewport.worldX - beforeScale.worldX * 1.1),
  ).toBeLessThanOrEqual(2);
  expect(
    Math.abs(textScaleViewport.worldY - beforeScale.worldY * 1.1),
  ).toBeLessThanOrEqual(2);
  await page.waitForTimeout(500);
  expectViewportClose(await observeViewport(graph), textScaleViewport);
});

test("1.5 root text scale preserves the authored center while docked becomes Drawer", async ({
  page,
}) => {
  await openDenseWorkspace(page, { width: 1600 });
  const graph = page.getByTestId("project-context-graph");
  await page.getByTestId("project-context-tool-structure").click();
  const panel = page.getByTestId("project-context-tool-panel");
  await expect(panel).toHaveAttribute("data-presentation", "docked");
  await settleWorkspaceChrome(page);

  const pane = graph.locator(".react-flow__pane");
  const paneBox = await pane.boundingBox();
  if (!paneBox) throw new Error("Graph pane bounds are unavailable.");
  await page.mouse.move(
    paneBox.x + paneBox.width / 2,
    paneBox.y + paneBox.height / 2,
  );
  await page.mouse.down();
  await page.mouse.move(
    paneBox.x + paneBox.width / 2 + 112,
    paneBox.y + paneBox.height / 2 + 56,
    { steps: 4 },
  );
  await page.mouse.up();
  await graph.getByRole("button", { name: "Zoom in" }).click();
  await page.waitForTimeout(250);

  const beforeScale = await observeViewport(graph);
  const beforeCanvasWidth = await graph.evaluate((root) => root.clientWidth);
  await increaseRootTextScale(page, 5);
  await expect(panel).toHaveAttribute("data-presentation", "drawer");
  await expect
    .poll(() => graph.getAttribute("data-viewport-authority-pending"), {
      timeout: 1_000,
    })
    .toBe("false");
  await settleWorkspaceChrome(page);

  const afterCanvasWidth = await graph.evaluate((root) => root.clientWidth);
  expect(afterCanvasWidth).toBeGreaterThan(beforeCanvasWidth);
  const afterScale = await observeViewport(graph);
  expect(afterScale.zoom).toBeCloseTo(beforeScale.zoom, 3);
  expect(
    Math.abs(afterScale.worldX - beforeScale.worldX * 1.5),
  ).toBeLessThanOrEqual(2);
  expect(
    Math.abs(afterScale.worldY - beforeScale.worldY * 1.5),
  ).toBeLessThanOrEqual(2);
});

test("semantic result persists through collapse, Details, and tool switches until HUD Clear", async ({
  page,
}) => {
  const dense = await openDenseWorkspace(page, { width: 1600 });
  await installWorkspaceSemanticResult(page, dense, {
    budgetExhausted: true,
    partialCoverage: true,
  });
  const graph = page.getByTestId("project-context-graph");
  const semanticTool = page.getByTestId("project-context-tool-semantic");
  const semanticProblem = page.getByTestId("project-context-semantic-problem");
  await semanticTool.click();
  await semanticProblem.fill(
    "Which dense Context path should be prioritized next?",
  );
  await semanticProblem.focus();
  await page.getByTestId("project-context-tools-collapse").click();
  await expect(page.getByTestId("project-context-tool-panel")).toHaveCount(0);
  await expect(semanticTool).toBeFocused();
  await semanticTool.click();
  await expect(semanticProblem).toBeFocused();
  await page.getByTestId("project-context-semantic-run").click();
  await expect(graph).toHaveAttribute("data-semantic-overlay", "active");
  await expect(
    page.getByTestId(`project-context-coordinate-${dense.coordinateKeys[0]}`),
  ).toHaveAttribute("data-semantic-root", "true");
  await expect(
    page.getByTestId(`project-context-edge-${dense.edgeKeys[0]}`),
  ).toHaveAttribute("data-semantic-emphasis", "route");

  await page.getByTestId("project-context-tools-collapse").click();
  const semanticHud = page.getByTestId("project-context-semantic-session-hud");
  await expect(semanticHud).toContainText("1 path");
  await expect(semanticHud).toContainText("1 root");
  await expect(semanticHud).toContainText("42");
  await expect(
    page.getByTestId("project-context-semantic-partial-coverage"),
  ).toHaveText("· Partial coverage");
  await expect(
    page.getByTestId("project-context-semantic-budget-exhausted"),
  ).toHaveText("· Budget exhausted");
  await expect(
    page.getByTestId("project-context-fit-semantic-paths"),
  ).toBeVisible();

  await page
    .getByTestId(`project-context-coordinate-${dense.coordinateKeys[1]}`)
    .getByRole("button")
    .click();
  await expect(
    page.getByTestId("project-context-coordinate-inspector"),
  ).toBeVisible();
  await expect(graph).toHaveAttribute("data-semantic-overlay", "active");
  await page.getByTestId("project-context-tool-structure").click();
  await expect(page.getByTestId("project-context-run-query")).toBeDisabled();
  await expect(graph).toHaveAttribute("data-semantic-overlay", "active");
  await page.getByTestId("project-context-tools-collapse").click();
  await page.mouse.move(1, 1);
  await page.getByTestId("project-context-fit-semantic-paths").focus();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await waitForAnimations(page);
  await page.getByTestId("project-context-workspace").screenshot({
    path: `${SHOTS}/04-semantic-selection.png`,
  });

  await page.getByTestId("project-context-fit-semantic-paths").click();
  await expect(graph).toHaveAttribute("data-semantic-overlay", "active");
  expect((await projectContextCallCounts(page)).semantic).toBe(1);

  const advanced = structuredClone(dense.result);
  advanced.context.contextRevision += 1;
  advanced.context.updatedAt = "2026-08-12T08:01:00Z";
  advanced.context.metaEventId = "f".repeat(64);
  await page.evaluate((result) => {
    window.__BUZZ_E2E_SET_PROJECT_CONTEXT__?.(result);
  }, advanced);
  await page.getByTestId("project-context-refresh").click();
  await expect(page.getByText("Revision 43", { exact: true })).toHaveText(
    "Revision 43",
  );
  await expect(graph).not.toHaveAttribute("data-semantic-overlay", "active");
  await expect(graph).toHaveAttribute("data-semantic-freshness", "stale");
  await expect(
    page.getByTestId(`project-context-coordinate-${dense.coordinateKeys[0]}`),
  ).toHaveAttribute("data-semantic-root", "false");
  await expect(
    page.getByTestId(`project-context-edge-${dense.edgeKeys[0]}`),
  ).toHaveAttribute("data-semantic-emphasis", "none");
  await expect(semanticHud).toContainText("Stale semantic snapshot");
  await expect(semanticHud).toContainText("Revision 42");
  await expect(
    page.getByTestId("project-context-fit-semantic-paths"),
  ).toBeDisabled();
  await expect(
    page.getByTestId("project-context-clear-semantic-result"),
  ).toBeVisible();
  const clearSemanticResult = page.getByTestId(
    "project-context-clear-semantic-result",
  );
  await page.mouse.move(1, 1);
  await clearSemanticResult.focus();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await waitForAnimations(page);
  await page.getByTestId("project-context-workspace").screenshot({
    path: `${SHOTS}/06-semantic-stale.png`,
  });

  await clearSemanticResult.click();
  await expect(graph).not.toHaveAttribute("data-semantic-overlay", "active");
  await expect(graph).not.toHaveAttribute("data-semantic-freshness");
  await expect(semanticHud).toHaveCount(0);
});

test("Sheet falls back to its Semantic Rail when remembered focus becomes stale-hidden", async ({
  page,
}) => {
  const dense = await openDenseWorkspace(page, { width: 1040, height: 800 });
  await installWorkspaceSemanticResult(page, dense);
  await page.setViewportSize({ width: 560, height: 800 });
  const graph = page.getByTestId("project-context-graph");
  const semanticTool = page.getByTestId("project-context-tool-semantic");
  await semanticTool.click();
  const panel = page.getByTestId("project-context-tool-panel");
  await expect(panel).toHaveAttribute("data-presentation", "sheet");
  await page
    .getByTestId("project-context-semantic-problem")
    .fill("Which dense Context path should be prioritized next?");
  await page.getByTestId("project-context-semantic-run").click();
  await expect(graph).toHaveAttribute("data-semantic-overlay", "active");

  const semanticFit = page.getByTestId("project-context-semantic-fit");
  await expect(semanticFit).toBeEnabled();
  await semanticFit.focus();
  await expect(semanticFit).toBeFocused();
  await page.getByTestId("project-context-tools-collapse").click();
  await expect(panel).toHaveCount(0);

  const advanced = structuredClone(dense.result);
  advanced.context.contextRevision += 1;
  advanced.context.updatedAt = "2026-08-12T08:01:00Z";
  advanced.context.metaEventId = "f".repeat(64);
  await page.evaluate((result) => {
    window.__BUZZ_E2E_SET_PROJECT_CONTEXT__?.(result);
  }, advanced);
  await page.getByTestId("project-context-refresh").click();
  await expect(page.getByText("Revision 43", { exact: true })).toHaveText(
    "Revision 43",
  );
  await expect(graph).toHaveAttribute("data-semantic-freshness", "stale");

  await semanticTool.click();
  const stalePanel = page.getByTestId("project-context-tool-panel");
  await expect(stalePanel).toHaveAttribute("data-presentation", "sheet");
  await expect(semanticFit).toHaveCount(0);
  await expect(
    page.getByTestId("project-context-semantic-active-badge"),
  ).toHaveText("Context changed");
  await expect(semanticTool).toBeFocused();
  const staleDialog = stalePanel.locator(
    'xpath=ancestor-or-self::*[@role="dialog"]',
  );
  expect(
    await staleDialog.evaluate(
      (dialog) =>
        dialog.contains(document.activeElement) &&
        document.activeElement instanceof HTMLElement &&
        document.activeElement.dataset.testid ===
          "project-context-tool-semantic",
    ),
  ).toBe(true);
});

test("Semantic pane Fit closes Sheet, restores graph-slot focus, and completes", async ({
  page,
}) => {
  const dense = await openDenseWorkspace(page, { width: 1040, height: 800 });
  await installWorkspaceSemanticResult(page, dense);
  await page.setViewportSize({ width: 560, height: 800 });
  const graph = page.getByTestId("project-context-graph");
  const graphSlot = page.getByTestId("project-context-graph-slot");
  await page.getByTestId("project-context-tool-semantic").click();
  const panel = page.getByTestId("project-context-tool-panel");
  await expect(panel).toHaveAttribute("data-presentation", "sheet");
  const announcement = page.getByTestId(
    "project-context-workspace-announcement",
  );
  await expect(announcement).toHaveCount(1);
  await expect(announcement).toHaveAttribute("aria-live", "polite");
  expect(
    await announcement.evaluate(
      (element) => element.closest('[role="dialog"]') !== null,
    ),
  ).toBe(true);
  const hiddenLiveOwnerAncestor = await announcement.evaluate(
    (announcement) => {
      const hidden = announcement.closest<HTMLElement>('[aria-hidden="true"]');
      if (!hidden) return null;
      const namedAncestor = hidden.closest<HTMLElement>("[data-testid]");
      return {
        hiddenId: hidden.id || null,
        hiddenTag: hidden.tagName.toLowerCase(),
        hiddenTestId: hidden.dataset.testid ?? null,
        namedAncestorTestId: namedAncestor?.dataset.testid ?? null,
      };
    },
  );
  expect(hiddenLiveOwnerAncestor).toBeNull();
  await page
    .getByTestId("project-context-semantic-problem")
    .fill("Which dense Context path should be prioritized next?");
  await page.getByTestId("project-context-semantic-run").click();
  await expect(graph).toHaveAttribute("data-semantic-overlay", "active");
  await expect(graph).toHaveAttribute(
    "data-viewport-authority-pending",
    "true",
  );

  const viewportBeforeFit = await observeViewport(graph);
  const authorityBeforeFit = Number(
    await graph.getAttribute("data-viewport-authority-generation"),
  );
  const paneFit = page.getByTestId("project-context-semantic-fit");
  await expect(paneFit).toBeEnabled();
  await paneFit.click();
  await expect(panel).toHaveCount(0);
  await expect(graphSlot).toBeFocused();
  expect(
    await page.evaluate(
      () =>
        document.activeElement instanceof HTMLElement &&
        document.activeElement.dataset.testid === "project-context-graph-slot",
    ),
  ).toBe(true);
  expect(
    Number(await graph.getAttribute("data-viewport-authority-generation")),
  ).toBeGreaterThan(authorityBeforeFit);
  await expect
    .poll(() => graph.getAttribute("data-viewport-authority-pending"), {
      timeout: 1_000,
    })
    .toBe("false");

  const viewportAfterFit = await observeViewport(graph);
  expect(
    Math.abs(viewportAfterFit.worldX - viewportBeforeFit.worldX) > 2 ||
      Math.abs(viewportAfterFit.worldY - viewportBeforeFit.worldY) > 2 ||
      Math.abs(viewportAfterFit.zoom - viewportBeforeFit.zoom) > 0.001,
  ).toBe(true);
});

test("Drawer and Sheet keep one Rail, trap focus, layer Escape, and preserve drafts", async ({
  page,
}) => {
  const dense = await openDenseWorkspace(page, { width: 1040, height: 800 });
  const structureTool = page.getByTestId("project-context-tool-structure");
  const detailsTool = page.getByTestId("project-context-tool-details");
  await expect(detailsTool).toHaveAttribute("aria-disabled", "true");
  await detailsTool.focus();
  await expect(detailsTool).toBeFocused();
  await expect(page.getByTestId("project-context-tools-rail")).toHaveCount(1);

  await structureTool.click();
  const panel = page.getByTestId("project-context-tool-panel");
  await expect(panel).toHaveAttribute("data-presentation", "drawer");
  await expect(page.getByTestId("project-context-tools-rail")).toHaveCount(1);
  const dialog = panel.locator('xpath=ancestor-or-self::*[@role="dialog"]');
  await expect(dialog).toHaveAttribute("aria-modal", "true");
  const controlledId = await structureTool.getAttribute("aria-controls");
  expect(controlledId).toBeTruthy();
  await expect(panel).toHaveAttribute("id", controlledId ?? "");
  await expect(structureTool).toHaveAttribute("aria-pressed", "true");
  await expect(structureTool).toHaveAttribute("aria-expanded", "true");

  await page.getByTestId("project-context-mode-contains_all").click();
  await page.getByTestId("project-context-coordinate-picker").click();
  await page
    .getByTestId("project-context-coordinate-search")
    .fill("Workspace requirement 1");
  await page.getByTestId("project-context-coordinate-search").press("Enter");
  await page.getByTestId("project-context-coordinate-picker").click();
  await expect(
    page.getByTestId("project-context-coordinate-search"),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(
    page.getByTestId("project-context-coordinate-search"),
  ).toHaveCount(0);
  await expect(panel).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
  await expect(page.getByTestId("project-context-tools-rail")).toHaveCount(1);
  await expect(structureTool).toBeFocused();
  await structureTool.click();
  await expect(page.getByTestId("project-context-query-chips")).toContainText(
    "Workspace requirement 1",
  );
  await page.getByTestId("project-context-run-query").focus();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await waitForAnimations(page);
  await page.screenshot({ path: `${SHOTS}/05-narrow-drawer.png` });

  await page.keyboard.press("Escape");
  await page.setViewportSize({ width: 560, height: 800 });
  await page.getByTestId("project-context-tool-semantic").click();
  const sheet = page.getByTestId("project-context-tool-panel");
  await expect(sheet).toHaveAttribute("data-presentation", "sheet");
  await expect(page.getByTestId("project-context-tools-rail")).toHaveCount(1);
  const semanticTool = page.getByTestId("project-context-tool-semantic");
  await expect(semanticTool).toHaveAttribute("aria-pressed", "true");
  await expect(semanticTool).toHaveAttribute("aria-expanded", "true");
  await expect
    .poll(() =>
      page.evaluate(() => {
        const active = document.activeElement;
        const panel = document.querySelector(
          '[data-testid="project-context-tool-panel"]',
        );
        return (
          active instanceof HTMLElement &&
          (active.dataset.testid === "project-context-tool-semantic" ||
            (panel?.contains(active) === true &&
              active.dataset.testid !== "project-context-tool-structure"))
        );
      }),
    )
    .toBe(true);
  await expect(semanticTool).toBeFocused();
  await expect(
    page.getByTestId("project-context-semantic-problem"),
  ).toHaveAttribute("aria-invalid", "false");
  const sheetAnnouncement = page.getByTestId(
    "project-context-workspace-announcement",
  );
  await expect(sheetAnnouncement).toHaveCount(1);
  await expect(sheetAnnouncement).toHaveAttribute("aria-live", "polite");
  expect(
    await sheetAnnouncement.evaluate(
      (element) => element.closest('[role="dialog"]') !== null,
    ),
  ).toBe(true);
  expect(
    await sheetAnnouncement.evaluate(
      (element) => element.closest('[aria-hidden="true"]') === null,
    ),
  ).toBe(true);

  for (let index = 0; index < 8; index += 1) await page.keyboard.press("Tab");
  expect(
    await sheet.evaluate((element) => element.contains(document.activeElement)),
  ).toBe(true);
  await page.getByTestId("project-context-semantic-problem").focus();
  await page.getByTestId("project-context-semantic-problem").blur();
  await expect(
    page.getByTestId("project-context-semantic-problem"),
  ).toHaveAttribute("aria-invalid", "true");
  const horizontalOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth,
  );
  expect(horizontalOverflow).toBeLessThanOrEqual(1);
  await page.mouse.move(1, 1);
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await page.getByTestId("project-context-semantic-problem").focus();
  await page.keyboard.press("Escape");
  await expect(sheet).toHaveCount(0);
  await expect(semanticTool).toBeFocused();

  const coordinate = page
    .getByTestId(`project-context-coordinate-${dense.coordinateKeys[0]}`)
    .getByRole("button");
  await coordinate.click();
  const detailsSheet = page.getByTestId("project-context-tool-panel");
  await expect(detailsSheet).toHaveAttribute("data-presentation", "sheet");
  await expect
    .poll(() =>
      page.evaluate(() => {
        const active = document.activeElement;
        const panel = document.querySelector(
          '[data-testid="project-context-tool-panel"]',
        );
        return (
          active instanceof HTMLElement &&
          (active.dataset.testid === "project-context-tool-details" ||
            (panel?.contains(active) === true &&
              active.dataset.testid !== "project-context-tool-structure"))
        );
      }),
    )
    .toBe(true);
  await expect(detailsTool).toBeFocused();
  await page.getByTestId("project-context-details-close").click();
  await expect(detailsSheet).toHaveCount(0);
  await expect(page).not.toHaveURL(/selected=/);
});
