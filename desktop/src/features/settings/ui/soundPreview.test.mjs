import assert from "node:assert/strict";
import test from "node:test";

import {
  listenForSoundPreviewStop,
  pauseSoundPreview,
} from "./soundPreview.ts";

test("sound preview pauses the active audio and tolerates no audio", () => {
  let pauseCalls = 0;
  const audio = { pause: () => (pauseCalls += 1) };

  pauseSoundPreview(audio);
  pauseSoundPreview(null);

  assert.equal(pauseCalls, 1);
});

test("sound preview stops on pause and ended with one-shot listeners", () => {
  const listeners = new Map();
  const audio = {
    addEventListener(name, listener, options) {
      listeners.set(name, { listener, options });
    },
  };
  let stopCalls = 0;

  listenForSoundPreviewStop(audio, () => (stopCalls += 1));

  assert.deepEqual([...listeners.keys()], ["ended", "pause"]);
  assert.equal(listeners.get("ended")?.options?.once, true);
  assert.equal(listeners.get("pause")?.options?.once, true);
  listeners.get("ended")?.listener();
  listeners.get("pause")?.listener();
  assert.equal(stopCalls, 2);
});
