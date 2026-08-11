import assert from "node:assert/strict";
import { after, test } from "node:test";

import { playNotificationSound, SOUND_NAMES } from "./sound.ts";

const originalAudio = globalThis.Audio;

after(() => {
  if (originalAudio === undefined) {
    delete globalThis.Audio;
  } else {
    globalThis.Audio = originalAudio;
  }
});

test("notification sound IDs remain the stable twelve-value set", () => {
  assert.deepEqual(SOUND_NAMES, [
    "bong",
    "boo",
    "dng",
    "doo",
    "doodone",
    "doong",
    "doop",
    "flirl",
    "flutter",
    "oh-no",
    "ping",
    "unison",
  ]);
});

test("notification playback uses generated WAV, resets, and caches audio", () => {
  const instances = [];
  class FakeAudio {
    constructor(source) {
      this.currentTime = 7;
      this.playCalls = 0;
      this.source = source;
      this.rejectionHandled = false;
      instances.push(this);
    }

    play() {
      this.playCalls += 1;
      return {
        catch: () => {
          this.rejectionHandled = true;
        },
      };
    }
  }
  globalThis.Audio = FakeAudio;

  const first = playNotificationSound("ping");
  assert.equal(first?.source, "/sounds/ping.wav");
  assert.equal(first?.currentTime, 0);
  assert.equal(first?.playCalls, 1);
  assert.equal(first?.rejectionHandled, true);

  first.currentTime = 5;
  const second = playNotificationSound("ping");
  assert.equal(second, first);
  assert.equal(second?.currentTime, 0);
  assert.equal(second?.playCalls, 2);
  assert.equal(instances.length, 1);
});

test("notification playback remains best-effort when Audio construction fails", () => {
  globalThis.Audio = class ThrowingAudio {
    constructor() {
      throw new Error("audio unavailable");
    }
  };

  assert.equal(playNotificationSound("bong"), null);
});
