import assert from "node:assert/strict";
import test from "node:test";

import { selectStarterPersonas, STARTER_PERSONAS } from "./starterPersonas.ts";

function persona(id, displayName) {
  return { id, displayName };
}

test("starter personas keep their stable ids and order", () => {
  assert.deepEqual(
    STARTER_PERSONAS.map(({ id }) => id),
    ["builtin:fizz", "builtin:honey", "builtin:bumble"],
  );

  assert.deepEqual(
    selectStarterPersonas([
      persona("builtin:bumble", "Renamed Bumble"),
      persona("custom:fizz", "Fizz"),
      persona("builtin:fizz", "Renamed Fizz"),
      persona("builtin:honey", "Renamed Honey"),
    ]).map(({ id, displayName }) => ({ id, displayName })),
    [
      { id: "builtin:fizz", displayName: "Renamed Fizz" },
      { id: "builtin:honey", displayName: "Renamed Honey" },
      { id: "builtin:bumble", displayName: "Renamed Bumble" },
    ],
  );
});
