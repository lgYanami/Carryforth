import assert from "node:assert/strict";
import test from "node:test";

import {
  clampProjectContextPanelContentWidth,
  createProjectContextWorkspacePanelState,
  deriveProjectContextWorkspacePanelLayout,
  projectContextWorkspacePanelReducer,
  projectContextWorkspaceSelectionKey,
} from "./workspacePanelModel.ts";

const COORDINATE_A = "coordinate:requirement:a";
const COORDINATE_B = "coordinate:work:b";

function reduce(state, ...actions) {
  return actions.reduce(projectContextWorkspacePanelReducer, state);
}

test("starts collapsed on Structure and resets Community-scoped presentation", () => {
  const initial = createProjectContextWorkspacePanelState();
  assert.deepEqual(initial, {
    expanded: false,
    activeTool: "structure",
    returnTool: null,
    returnExpanded: false,
    panelContentWidthPx: 440,
    observedSelectionKey: null,
    pendingSelectionOrigin: null,
    openOrigin: null,
  });

  const changed = reduce(
    initial,
    { type: "tool_toggled", tool: "semantic" },
    { type: "panel_content_width_changed", widthPx: 512 },
    { type: "reset" },
  );
  assert.deepEqual(changed, initial);
});

test("deep-link selection starts expanded Details with a route origin", () => {
  const state = createProjectContextWorkspacePanelState({
    initialSelectionKey: COORDINATE_A,
  });
  assert.equal(state.expanded, true);
  assert.equal(state.activeTool, "details");
  assert.equal(state.returnTool, "structure");
  assert.equal(state.returnExpanded, false);
  assert.equal(state.observedSelectionKey, COORDINATE_A);
  assert.deepEqual(state.openOrigin, { kind: "route" });
});

test("tool disclosures are mutually exclusive and the active disclosure toggles closed", () => {
  const initial = createProjectContextWorkspacePanelState();
  const structure = reduce(initial, {
    type: "tool_toggled",
    tool: "structure",
  });
  assert.equal(structure.expanded, true);
  assert.equal(structure.activeTool, "structure");

  const semantic = reduce(structure, {
    type: "tool_toggled",
    tool: "semantic",
  });
  assert.equal(semantic.expanded, true);
  assert.equal(semantic.activeTool, "semantic");
  assert.deepEqual(semantic.openOrigin, {
    kind: "rail",
    tool: "semantic",
  });

  const collapsed = reduce(semantic, {
    type: "tool_toggled",
    tool: "semantic",
  });
  assert.equal(collapsed.expanded, false);
  assert.equal(collapsed.activeTool, "semantic");
  assert.equal(collapsed.openOrigin, null);
});

test("Details fails closed without a canonical selection", () => {
  const initial = createProjectContextWorkspacePanelState();
  assert.strictEqual(
    projectContextWorkspacePanelReducer(initial, {
      type: "tool_toggled",
      tool: "details",
    }),
    initial,
  );
});

test("matching graph intent is consumed once and records graph focus origin", () => {
  const state = reduce(
    createProjectContextWorkspacePanelState(),
    {
      type: "selection_open_intent",
      expectedSelectionKey: COORDINATE_A,
      origin: {
        kind: "graph",
        target: { kind: "coordinate", key: "requirement:a" },
      },
    },
    { type: "selection_observed", selectionKey: COORDINATE_A },
  );
  assert.equal(state.activeTool, "details");
  assert.equal(state.expanded, true);
  assert.equal(state.pendingSelectionOrigin, null);
  assert.deepEqual(state.openOrigin, {
    kind: "graph",
    target: { kind: "coordinate", key: "requirement:a" },
  });
});

test("mismatched, rejected, and redundant observations cannot leak an intent", () => {
  const pending = reduce(createProjectContextWorkspacePanelState(), {
    type: "selection_open_intent",
    expectedSelectionKey: COORDINATE_A,
    origin: {
      kind: "graph",
      target: { kind: "coordinate", key: "requirement:a" },
    },
  });
  const mismatch = reduce(pending, {
    type: "selection_observed",
    selectionKey: COORDINATE_B,
  });
  assert.equal(mismatch.pendingSelectionOrigin, null);
  assert.deepEqual(mismatch.openOrigin, { kind: "route" });

  const rejected = reduce(pending, { type: "selection_open_rejected" });
  assert.equal(rejected.pendingSelectionOrigin, null);

  const selected = createProjectContextWorkspacePanelState({
    initialSelectionKey: COORDINATE_A,
  });
  const redundant = reduce(
    selected,
    {
      type: "selection_open_intent",
      expectedSelectionKey: COORDINATE_A,
      origin: {
        kind: "graph",
        target: { kind: "coordinate", key: "requirement:a" },
      },
    },
    { type: "selection_observed", selectionKey: COORDINATE_A },
  );
  assert.equal(redundant.pendingSelectionOrigin, null);
  assert.deepEqual(redundant.openOrigin, { kind: "route" });
});

test("A to B in expanded Details preserves the original return state", () => {
  const structure = reduce(createProjectContextWorkspacePanelState(), {
    type: "tool_toggled",
    tool: "structure",
  });
  const selectedA = reduce(structure, {
    type: "selection_observed",
    selectionKey: COORDINATE_A,
  });
  const selectedB = reduce(selectedA, {
    type: "selection_observed",
    selectionKey: COORDINATE_B,
  });
  assert.equal(selectedB.activeTool, "details");
  assert.equal(selectedB.returnTool, "structure");
  assert.equal(selectedB.returnExpanded, true);
  assert.deepEqual(selectedB.openOrigin, { kind: "route" });
});

test("matching graph intent reopens collapsed Details without replacing return state", () => {
  const selectedA = reduce(
    createProjectContextWorkspacePanelState(),
    { type: "selection_observed", selectionKey: COORDINATE_A },
    { type: "collapse" },
  );
  assert.equal(selectedA.expanded, false);
  assert.equal(selectedA.returnExpanded, false);

  const selectedB = reduce(
    selectedA,
    {
      type: "selection_open_intent",
      expectedSelectionKey: COORDINATE_B,
      origin: {
        kind: "graph",
        target: { kind: "coordinate", key: "work:b" },
      },
    },
    { type: "selection_observed", selectionKey: COORDINATE_B },
  );
  assert.equal(selectedB.expanded, true);
  assert.equal(selectedB.activeTool, "details");
  assert.equal(selectedB.returnTool, "structure");
  assert.equal(selectedB.returnExpanded, false);
  assert.deepEqual(selectedB.openOrigin, {
    kind: "graph",
    target: { kind: "coordinate", key: "work:b" },
  });
});

test("route-driven A to B keeps collapsed Details collapsed", () => {
  const selectedA = reduce(
    createProjectContextWorkspacePanelState(),
    { type: "selection_observed", selectionKey: COORDINATE_A },
    { type: "collapse" },
  );
  const selectedB = reduce(selectedA, {
    type: "selection_observed",
    selectionKey: COORDINATE_B,
  });
  assert.equal(selectedB.expanded, false);
  assert.equal(selectedB.activeTool, "details");
  assert.equal(selectedB.returnExpanded, false);
});

test("manual Semantic becomes the next selection return target", () => {
  const selectedA = createProjectContextWorkspacePanelState({
    initialSelectionKey: COORDINATE_A,
  });
  const semantic = reduce(selectedA, {
    type: "tool_toggled",
    tool: "semantic",
  });
  assert.equal(semantic.activeTool, "semantic");
  assert.equal(semantic.returnTool, "semantic");
  assert.equal(semantic.returnExpanded, true);

  const selectedB = reduce(semantic, {
    type: "selection_observed",
    selectionKey: COORDINATE_B,
  });
  assert.equal(selectedB.activeTool, "details");
  assert.equal(selectedB.returnTool, "semantic");
  assert.equal(selectedB.returnExpanded, true);

  const cleared = reduce(selectedB, {
    type: "selection_observed",
    selectionKey: null,
  });
  assert.equal(cleared.activeTool, "semantic");
  assert.equal(cleared.expanded, true);
});

test("selection clear preserves a manually chosen pane and collapsed return state", () => {
  const semantic = reduce(
    createProjectContextWorkspacePanelState({
      initialSelectionKey: COORDINATE_A,
    }),
    { type: "tool_toggled", tool: "semantic" },
  );
  const manuallyCleared = reduce(semantic, {
    type: "selection_observed",
    selectionKey: null,
  });
  assert.equal(manuallyCleared.activeTool, "semantic");
  assert.equal(manuallyCleared.expanded, true);

  const collapsedDetails = reduce(
    createProjectContextWorkspacePanelState({
      initialSelectionKey: COORDINATE_A,
    }),
    { type: "collapse" },
    { type: "selection_observed", selectionKey: null },
  );
  assert.equal(collapsedDetails.activeTool, "structure");
  assert.equal(collapsedDetails.expanded, false);
});

test("Collapse preserves selection while selection observation owns closing Details", () => {
  const selected = createProjectContextWorkspacePanelState({
    initialSelectionKey: COORDINATE_A,
  });
  const collapsed = reduce(selected, { type: "collapse" });
  assert.equal(collapsed.observedSelectionKey, COORDINATE_A);
  assert.equal(collapsed.expanded, false);

  const closed = reduce(selected, {
    type: "selection_observed",
    selectionKey: null,
  });
  assert.equal(closed.observedSelectionKey, null);
  assert.equal(closed.activeTool, "structure");
});

test("selection keys keep Coordinate and Edge namespaces distinct", () => {
  assert.equal(
    projectContextWorkspaceSelectionKey({ kind: "coordinate", key: "abc" }),
    "coordinate:abc",
  );
  assert.equal(
    projectContextWorkspaceSelectionKey({ kind: "edge", key: "abc" }),
    "edge:abc",
  );
});

test("responsive presentation honors exact sheet and dock thresholds", () => {
  const common = {
    expanded: true,
    panelContentWidthPx: 440,
    rootRemPx: 16,
  };
  assert.equal(
    deriveProjectContextWorkspacePanelLayout({
      ...common,
      workspaceWidthPx: 599,
    }).presentation,
    "sheet",
  );
  assert.equal(
    deriveProjectContextWorkspacePanelLayout({
      ...common,
      workspaceWidthPx: 600,
    }).presentation,
    "drawer",
  );
  assert.equal(
    deriveProjectContextWorkspacePanelLayout({
      ...common,
      workspaceWidthPx: 1127,
    }).presentation,
    "drawer",
  );
  const docked = deriveProjectContextWorkspacePanelLayout({
    ...common,
    workspaceWidthPx: 1128,
  });
  assert.equal(docked.presentation, "docked");
  assert.equal(docked.dockThresholdPx, 1128);
  assert.equal(docked.fitSuspended, false);
});

test("collapsed presentation ignores responsive mode and reserves the Rail only", () => {
  const layout = deriveProjectContextWorkspacePanelLayout({
    expanded: false,
    panelContentWidthPx: 440,
    rootRemPx: 16,
    workspaceWidthPx: 320,
  });
  assert.equal(layout.presentation, "collapsed");
  assert.equal(layout.railWidthPx, 48);
  assert.equal(layout.overlayWidthPx, 272);
  assert.equal(layout.fitSuspended, false);
});

test("root text scale raises responsive thresholds", () => {
  const base = deriveProjectContextWorkspacePanelLayout({
    expanded: true,
    panelContentWidthPx: 440,
    rootRemPx: 16,
    workspaceWidthPx: 1200,
  });
  const zoomed = deriveProjectContextWorkspacePanelLayout({
    expanded: true,
    panelContentWidthPx: 440,
    rootRemPx: 24,
    workspaceWidthPx: 1200,
  });
  assert.equal(base.presentation, "docked");
  assert.equal(zoomed.presentation, "drawer");
  assert.equal(zoomed.panelContentWidthPx, 540);
});

test("panel width clamp respects rem bounds and docked canvas space", () => {
  assert.equal(
    clampProjectContextPanelContentWidth({
      panelContentWidthPx: 100,
      rootRemPx: 16,
    }),
    360,
  );
  assert.equal(
    clampProjectContextPanelContentWidth({
      panelContentWidthPx: 900,
      rootRemPx: 16,
    }),
    560,
  );
  assert.equal(
    clampProjectContextPanelContentWidth({
      panelContentWidthPx: 560,
      preserveDockedCanvas: true,
      rootRemPx: 16,
      workspaceWidthPx: 1120,
    }),
    432,
  );
  assert.equal(
    clampProjectContextPanelContentWidth({
      panelContentWidthPx: 440,
      preserveDockedCanvas: true,
      rootRemPx: 16,
      workspaceWidthPx: 800,
    }),
    360,
  );
});

test("invalid dimensions fail closed to finite defaults", () => {
  const layout = deriveProjectContextWorkspacePanelLayout({
    expanded: true,
    panelContentWidthPx: Number.NaN,
    rootRemPx: 0,
    workspaceWidthPx: Number.NaN,
  });
  assert.equal(layout.presentation, "sheet");
  assert.equal(layout.panelContentWidthPx, 440);
  assert.equal(layout.overlayWidthPx, 0);
});
