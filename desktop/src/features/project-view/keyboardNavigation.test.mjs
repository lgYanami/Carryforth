import assert from "node:assert/strict";
import test from "node:test";

import { nextProjectViewObjectIndex } from "./keyboardNavigation.ts";

test("forward map navigation advances and wraps", () => {
  assert.equal(nextProjectViewObjectIndex(0, 3, "ArrowDown"), 1);
  assert.equal(nextProjectViewObjectIndex(2, 3, "ArrowRight"), 0);
});

test("backward map navigation retreats and wraps", () => {
  assert.equal(nextProjectViewObjectIndex(2, 3, "ArrowUp"), 1);
  assert.equal(nextProjectViewObjectIndex(0, 3, "ArrowLeft"), 2);
});

test("Home and End jump to map boundaries", () => {
  assert.equal(nextProjectViewObjectIndex(1, 3, "Home"), 0);
  assert.equal(nextProjectViewObjectIndex(1, 3, "End"), 2);
});

test("invalid or empty map positions do not invent a target", () => {
  assert.equal(nextProjectViewObjectIndex(-1, 3, "ArrowDown"), undefined);
  assert.equal(nextProjectViewObjectIndex(0, 0, "Home"), undefined);
});
