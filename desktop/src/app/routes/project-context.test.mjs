import assert from "node:assert/strict";
import test from "node:test";

import { validateProjectContextSearch } from "./project-context.tsx";

test("stage-two route normalizes omitted and foreign search to All Context", () => {
  assert.deepEqual(validateProjectContextSearch({}), {});
  assert.deepEqual(
    validateProjectContextSearch({
      mode: "exact",
      coordinates: ["untrusted-coordinate-token"],
      selected: "edge:untrusted",
    }),
    {},
  );
});
