import assert from "node:assert/strict";
import test from "node:test";

import { validateBaseRoleBriefV3 } from "./tauriProjectViewRoleV3.ts";

function baseBrief() {
  return {
    project_view_schema_version: 3,
    project_revision: 7,
    projection_generation: 3,
    source_revisions: {
      meta_event_id: "a".repeat(64),
      meta_change_id: "b".repeat(64),
      membership_event_id: "c".repeat(64),
      project_updated_at: "2026-08-01T00:00:00Z",
      document_metadata: { state: "not_required" },
    },
    context: {
      availability: { state: "not_advertised_empty" },
      resources: [],
      live_documents: [],
      pinned_documents: [],
      truncation: {
        truncated: false,
        omitted_resources: 0,
        omitted_live_documents: 0,
        omitted_pinned_documents: 0,
      },
    },
  };
}

test("accepts the strict empty base RoleBriefV3 contract", () => {
  assert.deepEqual(validateBaseRoleBriefV3(baseBrief(), 7, 3), {
    availability: "not_advertised_empty",
  });
});

test("rejects a v2 discriminator on the v3 path", () => {
  const raw = baseBrief();
  raw.project_view_schema_version = 2;
  assert.throws(
    () => validateBaseRoleBriefV3(raw, 7, 3),
    /does not match the verified Project snapshot/,
  );
});

test("rejects hydrated Context before the Context capability", () => {
  const raw = baseBrief();
  raw.context.resources.push({ resource_id: "unexpected" });
  assert.throws(
    () => validateBaseRoleBriefV3(raw, 7, 3),
    /must not hydrate or truncate Context/,
  );
});

test("rejects a Document metadata source on the stage-5 base surface", () => {
  const raw = baseBrief();
  raw.source_revisions.document_metadata = {
    state: "verified",
    catalog_revision: 1,
  };
  assert.throws(
    () => validateBaseRoleBriefV3(raw, 7, 3),
    /document_metadata:not_required/,
  );
});
