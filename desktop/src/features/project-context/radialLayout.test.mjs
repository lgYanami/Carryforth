import assert from "node:assert/strict";
import test from "node:test";

import { layoutRadialComponent } from "./radialLayout.ts";

function coordinate(id) {
  return { id, kind: "coordinate", width: 224, height: 120 };
}

function hub(id) {
  return { id, kind: "hub", width: 76, height: 76 };
}

function bounds(node, position) {
  return {
    minX: position.centerX - node.width / 2,
    maxX: position.centerX + node.width / 2,
    minY: position.centerY - node.height / 2,
    maxY: position.centerY + node.height / 2,
  };
}

function overlap(left, right) {
  return (
    left.minX < right.maxX &&
    left.maxX > right.minX &&
    left.minY < right.maxY &&
    left.maxY > right.minY
  );
}

function assertNoOverlap(nodes, result) {
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  for (let leftIndex = 0; leftIndex < result.positions.length; leftIndex += 1) {
    for (
      let rightIndex = leftIndex + 1;
      rightIndex < result.positions.length;
      rightIndex += 1
    ) {
      const leftPosition = result.positions[leftIndex];
      const rightPosition = result.positions[rightIndex];
      assert.equal(
        overlap(
          bounds(nodeById.get(leftPosition.id), leftPosition),
          bounds(nodeById.get(rightPosition.id), rightPosition),
        ),
        false,
        `${leftPosition.id} overlaps ${rightPosition.id}`,
      );
    }
  }
}

test("radial layout is deterministic across input permutations", () => {
  const nodes = [
    hub("hub:b"),
    coordinate("coordinate:a"),
    coordinate("coordinate:c"),
  ];
  const links = [
    { sourceId: "hub:b", targetId: "coordinate:a" },
    { sourceId: "hub:b", targetId: "coordinate:c" },
  ];
  const input = {
    stableKey: "edge:b",
    nodes,
    links,
    centerIds: ["hub:b"],
    virtualCenter: false,
  };
  const regular = layoutRadialComponent(input);
  const permuted = layoutRadialComponent({
    ...input,
    nodes: [...nodes].reverse(),
    links: [...links].reverse(),
  });

  assert.ok(regular);
  assert.deepEqual(permuted, regular);
  assert.deepEqual(
    regular.positions.find((position) => position.id === "hub:b"),
    {
      id: "hub:b",
      centerX: 0,
      centerY: 0,
      depth: 0,
      band: 0,
    },
  );
});

test("non-tree component relaxes for a fixed budget and stays collision free", () => {
  const nodes = [
    hub("hub:a"),
    hub("hub:b"),
    coordinate("coordinate:a"),
    coordinate("coordinate:b"),
    coordinate("coordinate:c"),
  ];
  const links = [
    { sourceId: "hub:a", targetId: "coordinate:a" },
    { sourceId: "hub:a", targetId: "coordinate:b" },
    { sourceId: "hub:b", targetId: "coordinate:a" },
    { sourceId: "hub:b", targetId: "coordinate:b" },
    { sourceId: "hub:b", targetId: "coordinate:c" },
  ];
  const result = layoutRadialComponent({
    stableKey: "edge:a|edge:b",
    nodes,
    links,
    centerIds: ["hub:b"],
    virtualCenter: false,
  });

  assert.ok(result);
  assert.equal(result.diagnostics.ticks, 64);
  assert.ok(result.diagnostics.collisionPairs >= 0);
  assertNoOverlap(nodes, result);
  assert.equal(
    result.positions.every(
      (position) =>
        Number.isFinite(position.centerX) && Number.isFinite(position.centerY),
    ),
    true,
  );
});

function star(leafCount) {
  const nodes = [
    hub("hub:center"),
    ...Array.from({ length: leafCount }, (_value, index) =>
      coordinate(`coordinate:${String(index).padStart(4, "0")}`),
    ),
  ];
  const links = nodes.slice(1).map((node) => ({
    sourceId: "hub:center",
    targetId: node.id,
  }));
  return layoutRadialComponent({
    stableKey: "large-star",
    nodes,
    links,
    centerIds: ["hub:center"],
    virtualCenter: false,
  });
}

test("high fan-out splits into physical bands with sub-linear outer radius", () => {
  const fiveHundred = star(500);
  const oneThousand = star(1_000);
  assert.ok(fiveHundred);
  assert.ok(oneThousand);
  const radius = (result) =>
    Math.max(
      ...result.positions.map((position) =>
        Math.hypot(position.centerX, position.centerY),
      ),
    );
  assert.ok(radius(oneThousand) / radius(fiveHundred) < 1.75);
  assert.ok(
    new Set(oneThousand.positions.map((position) => position.band)).size > 1,
  );
  assert.equal(oneThousand.diagnostics.ticks, 0);
});
