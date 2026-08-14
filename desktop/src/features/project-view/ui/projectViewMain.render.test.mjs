import assert from "node:assert/strict";
import test from "node:test";

import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { ProjectViewParentNavigation } from "./ProjectViewParentNavigation.tsx";
import { ProjectViewSummaryItem } from "./ProjectViewSummaryItem.tsx";

test("parent navigation renders only the up affordance and parent title", () => {
  const html = renderToStaticMarkup(
    React.createElement(ProjectViewParentNavigation, {
      onSelect() {},
      parent: {
        objectId: "goal-1",
        title: "Launch Goal",
      },
    }),
  );

  assert.match(html, /aria-label="Go to parent: Launch Goal"/);
  assert.match(html, />Launch Goal</);
  assert.doesNotMatch(html, /Goal summary|Revision|Status/);
});

test("project profile without a parent has no parent placeholder", () => {
  assert.equal(
    renderToStaticMarkup(
      React.createElement(ProjectViewParentNavigation, {
        onSelect() {},
      }),
    ),
    "",
  );
});

test("summary item ignores non-summary object metadata", () => {
  const html = renderToStaticMarkup(
    React.createElement(ProjectViewSummaryItem, {
      item: {
        kind: "object",
        occurrenceKey: "object:stage-1:canonical",
        objectId: "stage-1",
        objectType: "stage",
        typeLabel: "Stage",
        title: "Release",
        status: "active",
        priority: "urgent",
        description: "Must not be used as a summary fallback",
        objectRevision: 12,
      },
      onSelect() {},
    }),
  );

  assert.match(html, />Stage</);
  assert.match(html, />Release</);
  assert.match(html, /No summary provided\./);
  assert.match(html, /data-occurrence-key="object:stage-1:canonical"/);
  assert.doesNotMatch(html, />active</);
  assert.doesNotMatch(html, />urgent</);
  assert.doesNotMatch(html, /Must not/);
  assert.doesNotMatch(html, />12</);
});
