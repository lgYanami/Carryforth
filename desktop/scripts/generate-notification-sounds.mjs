#!/usr/bin/env node

// Generates Carryforth's notification sounds and their picker waveforms from
// code-only synthesis parameters. This script deliberately has no decoder,
// network access, sample-pack input, or dependency on the retired MP3 files.
//
// The synthesis contract is integer/fixed-point throughout the sample path:
// mono 16-bit PCM at 48 kHz, a 24-bit phase accumulator, Q15 envelopes, and
// explicit little-endian WAV writes. The SVG bars are derived directly from
// the generated PCM so `--check` can enforce byte-for-byte reproducibility.

import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SAMPLE_RATE = 48_000;
const CHANNELS = 1;
const BITS_PER_SAMPLE = 16;
const BYTES_PER_SAMPLE = BITS_PER_SAMPLE / 8;
const PHASE_BITS = 24;
const PHASE_MODULUS = 2 ** PHASE_BITS;
const PHASE_HALF = PHASE_MODULUS / 2;
const Q15_ONE = 32_768;
const TARGET_PEAK = 22_000;
const TAIL_MS = 24;

const SVG_BARS = 24;
const SVG_WIDTH = 192;
const SVG_HEIGHT = 64;
const SVG_BAR_WIDTH = 4;
const SVG_MIN_BAR_HEIGHT = 4;
const SVG_MAX_BAR_HEIGHT = 60;

const SOUND_IDS = [
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
];

const soundsDirectory = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../public/sounds",
);

const soundDefinitions = [
  {
    id: "bong",
    notes: [
      { startMs: 0, durationMs: 470, frequencyHz: 294, gainQ15: 25_000 },
      { startMs: 90, durationMs: 390, frequencyHz: 440, gainQ15: 13_000 },
    ],
  },
  {
    id: "boo",
    notes: [
      { startMs: 0, durationMs: 220, frequencyHz: 349, gainQ15: 21_000 },
      { startMs: 135, durationMs: 300, frequencyHz: 523, gainQ15: 21_000 },
    ],
  },
  {
    id: "dng",
    notes: [
      { startMs: 0, durationMs: 340, frequencyHz: 740, gainQ15: 26_000 },
      { startMs: 0, durationMs: 250, frequencyHz: 1110, gainQ15: 8_000 },
    ],
  },
  {
    id: "doo",
    notes: [{ startMs: 0, durationMs: 410, frequencyHz: 523, gainQ15: 25_000 }],
  },
  {
    id: "doodone",
    notes: [
      { startMs: 0, durationMs: 210, frequencyHz: 440, gainQ15: 20_000 },
      { startMs: 115, durationMs: 240, frequencyHz: 659, gainQ15: 21_000 },
      { startMs: 245, durationMs: 370, frequencyHz: 880, gainQ15: 23_000 },
    ],
  },
  {
    id: "doong",
    notes: [
      { startMs: 0, durationMs: 260, frequencyHz: 466, gainQ15: 22_000 },
      { startMs: 150, durationMs: 410, frequencyHz: 311, gainQ15: 24_000 },
    ],
  },
  {
    id: "doop",
    notes: [
      { startMs: 0, durationMs: 230, frequencyHz: 587, gainQ15: 25_000 },
      { startMs: 52, durationMs: 155, frequencyHz: 784, gainQ15: 9_000 },
    ],
  },
  {
    id: "flirl",
    notes: [
      { startMs: 0, durationMs: 180, frequencyHz: 659, gainQ15: 19_000 },
      { startMs: 78, durationMs: 190, frequencyHz: 784, gainQ15: 20_000 },
      { startMs: 162, durationMs: 250, frequencyHz: 988, gainQ15: 22_000 },
    ],
  },
  {
    id: "flutter",
    notes: [
      { startMs: 0, durationMs: 125, frequencyHz: 880, gainQ15: 18_000 },
      { startMs: 76, durationMs: 125, frequencyHz: 1047, gainQ15: 18_000 },
      { startMs: 152, durationMs: 135, frequencyHz: 880, gainQ15: 18_000 },
      { startMs: 228, durationMs: 185, frequencyHz: 1175, gainQ15: 20_000 },
    ],
  },
  {
    id: "oh-no",
    notes: [
      { startMs: 0, durationMs: 270, frequencyHz: 523, gainQ15: 22_000 },
      { startMs: 180, durationMs: 420, frequencyHz: 349, gainQ15: 25_000 },
    ],
  },
  {
    id: "ping",
    notes: [
      { startMs: 0, durationMs: 330, frequencyHz: 1175, gainQ15: 25_000 },
      { startMs: 0, durationMs: 210, frequencyHz: 1762, gainQ15: 7_000 },
    ],
  },
  {
    id: "unison",
    notes: [
      { startMs: 0, durationMs: 430, frequencyHz: 523, gainQ15: 19_000 },
      { startMs: 0, durationMs: 430, frequencyHz: 659, gainQ15: 17_000 },
      { startMs: 0, durationMs: 430, frequencyHz: 784, gainQ15: 15_000 },
    ],
  },
];

const definitionIds = soundDefinitions.map(({ id }) => id);

function fail(message) {
  throw new Error(message);
}

function millisecondsToSamples(milliseconds) {
  return Math.trunc((milliseconds * SAMPLE_RATE) / 1000);
}

function multiplyQ15(value, multiplier) {
  return Math.trunc((value * multiplier) / Q15_ONE);
}

function triangleSample(phase) {
  const rising = phase < PHASE_HALF ? phase : PHASE_MODULUS - phase;
  return Math.trunc((rising * 65_535) / PHASE_HALF) - 32_768;
}

function envelopeQ15(sampleIndex, sampleCount) {
  const attackSamples = Math.min(millisecondsToSamples(8), sampleCount);
  const attack = Math.min(
    Q15_ONE,
    Math.trunc((sampleIndex * Q15_ONE) / Math.max(1, attackSamples)),
  );
  const remaining = sampleCount - sampleIndex - 1;
  const linearDecay = Math.max(
    0,
    Math.trunc((remaining * Q15_ONE) / Math.max(1, sampleCount - 1)),
  );
  const curvedDecay = multiplyQ15(linearDecay, linearDecay);
  return Math.min(attack, curvedDecay);
}

function renderNote(output, note) {
  const start = millisecondsToSamples(note.startMs);
  const count = millisecondsToSamples(note.durationMs);
  const fundamentalStep = Math.max(
    1,
    Math.trunc((note.frequencyHz * PHASE_MODULUS) / SAMPLE_RATE),
  );
  const overtoneStep = fundamentalStep * 2;
  let fundamentalPhase = 0;
  let overtonePhase = PHASE_MODULUS / 4;

  for (let index = 0; index < count; index += 1) {
    fundamentalPhase = (fundamentalPhase + fundamentalStep) % PHASE_MODULUS;
    overtonePhase = (overtonePhase + overtoneStep) % PHASE_MODULUS;

    const fundamental = triangleSample(fundamentalPhase);
    const overtone = triangleSample(overtonePhase);
    const timbre = Math.trunc((fundamental * 7 + overtone) / 8);
    const enveloped = multiplyQ15(timbre, envelopeQ15(index, count));
    output[start + index] += multiplyQ15(enveloped, note.gainQ15);
  }
}

function normalizeSamples(mixedSamples) {
  let peak = 0;
  for (const sample of mixedSamples) {
    peak = Math.max(peak, Math.abs(sample));
  }
  if (peak === 0) fail("synthesis produced silence");

  const normalized = new Int16Array(mixedSamples.length);
  for (let index = 0; index < mixedSamples.length; index += 1) {
    const scaled = Math.trunc((mixedSamples[index] * TARGET_PEAK) / peak);
    normalized[index] = Math.max(-32_768, Math.min(32_767, scaled));
  }
  return normalized;
}

function synthesize(definition) {
  const lastSample = Math.max(
    ...definition.notes.map((note) => note.startMs + note.durationMs),
  );
  const sampleCount = millisecondsToSamples(lastSample + TAIL_MS);
  const mixedSamples = new Int32Array(sampleCount);
  for (const note of definition.notes) renderNote(mixedSamples, note);
  return normalizeSamples(mixedSamples);
}

function wavFromSamples(samples) {
  const dataSize = samples.length * BYTES_PER_SAMPLE;
  const wav = Buffer.alloc(44 + dataSize);
  wav.write("RIFF", 0, "ascii");
  wav.writeUInt32LE(36 + dataSize, 4);
  wav.write("WAVE", 8, "ascii");
  wav.write("fmt ", 12, "ascii");
  wav.writeUInt32LE(16, 16);
  wav.writeUInt16LE(1, 20);
  wav.writeUInt16LE(CHANNELS, 22);
  wav.writeUInt32LE(SAMPLE_RATE, 24);
  wav.writeUInt32LE(SAMPLE_RATE * CHANNELS * BYTES_PER_SAMPLE, 28);
  wav.writeUInt16LE(CHANNELS * BYTES_PER_SAMPLE, 32);
  wav.writeUInt16LE(BITS_PER_SAMPLE, 34);
  wav.write("data", 36, "ascii");
  wav.writeUInt32LE(dataSize, 40);
  for (let index = 0; index < samples.length; index += 1) {
    wav.writeInt16LE(samples[index], 44 + index * BYTES_PER_SAMPLE);
  }
  return wav;
}

function waveformFromSamples(samples) {
  const bucketSize = Math.ceil(samples.length / SVG_BARS);
  const peaks = [];
  for (let bar = 0; bar < SVG_BARS; bar += 1) {
    const from = bar * bucketSize;
    const to = Math.min(samples.length, from + bucketSize);
    let peak = 0;
    for (let index = from; index < to; index += 1) {
      peak = Math.max(peak, Math.abs(samples[index]));
    }
    peaks.push(peak);
  }
  const largestPeak = Math.max(...peaks, 1);
  const heightSteps = (SVG_MAX_BAR_HEIGHT - SVG_MIN_BAR_HEIGHT) / 2;
  const bars = peaks
    .map((peak, index) => {
      const scaledSteps = Math.trunc(
        (peak * heightSteps + Math.trunc(largestPeak / 2)) / largestPeak,
      );
      const height = SVG_MIN_BAR_HEIGHT + scaledSteps * 2;
      const x = index * (SVG_WIDTH / SVG_BARS) + 2;
      const y = (SVG_HEIGHT - height) / 2;
      return `<rect x="${x}" y="${y}" width="${SVG_BAR_WIDTH}" height="${height}" rx="2"/>`;
    })
    .join("");
  return Buffer.from(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${SVG_WIDTH} ${SVG_HEIGHT}" fill="currentColor" aria-hidden="true">${bars}</svg>\n`,
    "utf8",
  );
}

function generateOutputs() {
  const outputs = new Map();
  for (const definition of soundDefinitions) {
    const samples = synthesize(definition);
    outputs.set(`${definition.id}.wav`, wavFromSamples(samples));
    outputs.set(`${definition.id}.svg`, waveformFromSamples(samples));
  }
  return outputs;
}

function validateWav(id, wav) {
  if (wav.subarray(0, 4).toString("ascii") !== "RIFF") {
    fail(`${id}.wav: missing RIFF header`);
  }
  if (wav.subarray(8, 12).toString("ascii") !== "WAVE") {
    fail(`${id}.wav: missing WAVE header`);
  }
  if (wav.subarray(36, 40).toString("ascii") !== "data") {
    fail(`${id}.wav: expected canonical 44-byte PCM header`);
  }
  if (wav.readUInt16LE(20) !== 1) fail(`${id}.wav: expected PCM format`);
  if (wav.readUInt16LE(22) !== CHANNELS) {
    fail(`${id}.wav: expected ${CHANNELS} channel`);
  }
  if (wav.readUInt32LE(24) !== SAMPLE_RATE) {
    fail(`${id}.wav: expected ${SAMPLE_RATE} Hz`);
  }
  if (wav.readUInt32LE(28) !== SAMPLE_RATE * CHANNELS * BYTES_PER_SAMPLE) {
    fail(`${id}.wav: invalid byte rate`);
  }
  if (wav.readUInt16LE(32) !== CHANNELS * BYTES_PER_SAMPLE) {
    fail(`${id}.wav: invalid block alignment`);
  }
  if (wav.readUInt16LE(34) !== BITS_PER_SAMPLE) {
    fail(`${id}.wav: expected ${BITS_PER_SAMPLE}-bit samples`);
  }
  if (wav.readUInt32LE(4) !== wav.length - 8) {
    fail(`${id}.wav: RIFF size does not match file length`);
  }
  if (wav.readUInt32LE(40) !== wav.length - 44) {
    fail(`${id}.wav: data size does not match file length`);
  }

  const sampleCount = (wav.length - 44) / BYTES_PER_SAMPLE;
  const durationMs = Math.trunc((sampleCount * 1000) / SAMPLE_RATE);
  if (durationMs < 200 || durationMs > 1_000) {
    fail(`${id}.wav: duration ${durationMs}ms is outside 200-1000ms`);
  }

  let peak = 0;
  let sum = 0;
  for (let index = 0; index < sampleCount; index += 1) {
    const sample = wav.readInt16LE(44 + index * BYTES_PER_SAMPLE);
    peak = Math.max(peak, Math.abs(sample));
    sum += sample;
  }
  if (peak < 18_000 || peak > 24_000) {
    fail(`${id}.wav: peak ${peak} is outside the safe target range`);
  }
  const dcOffset = Math.abs(sum / sampleCount);
  if (dcOffset > 64) {
    fail(`${id}.wav: DC offset ${dcOffset.toFixed(2)} exceeds 64 samples`);
  }
}

function validateGeneratedOutputs(outputs) {
  if (JSON.stringify(definitionIds) !== JSON.stringify(SOUND_IDS)) {
    fail(`sound definition IDs must remain: ${SOUND_IDS.join(", ")}`);
  }
  for (const definition of soundDefinitions) {
    validateWav(definition.id, outputs.get(`${definition.id}.wav`));
    const svg = outputs.get(`${definition.id}.svg`).toString("utf8");
    if (!svg.startsWith(`<svg xmlns="http://www.w3.org/2000/svg"`)) {
      fail(`${definition.id}.svg: missing SVG root`);
    }
    if (!svg.includes(`viewBox="0 0 ${SVG_WIDTH} ${SVG_HEIGHT}"`)) {
      fail(`${definition.id}.svg: expected ${SVG_WIDTH}x${SVG_HEIGHT} viewBox`);
    }
    if ((svg.match(/<rect /g) ?? []).length !== SVG_BARS) {
      fail(`${definition.id}.svg: expected ${SVG_BARS} waveform bars`);
    }
  }
}

function expectedFileNames() {
  return [...SOUND_IDS.flatMap((id) => [`${id}.svg`, `${id}.wav`])].sort();
}

function checkDirectory(outputs) {
  const actualFiles = readdirSync(soundsDirectory).sort();
  const expectedFiles = expectedFileNames();
  if (JSON.stringify(actualFiles) !== JSON.stringify(expectedFiles)) {
    fail(
      `sound directory must contain only the generated WAV/SVG set\nexpected: ${expectedFiles.join(", ")}\nactual:   ${actualFiles.join(", ")}`,
    );
  }

  for (const fileName of expectedFiles) {
    const actual = readFileSync(path.join(soundsDirectory, fileName));
    const expected = outputs.get(fileName);
    if (!actual.equals(expected)) {
      fail(`${fileName}: generated bytes differ; run pnpm generate:sounds`);
    }
  }
}

const mode = process.argv[2];
if (mode !== "--write" && mode !== "--check") {
  console.error(
    "usage: node scripts/generate-notification-sounds.mjs --write|--check",
  );
  process.exitCode = 1;
} else {
  try {
    const outputs = generateOutputs();
    validateGeneratedOutputs(outputs);
    if (mode === "--write") {
      for (const [fileName, bytes] of outputs) {
        writeFileSync(path.join(soundsDirectory, fileName), bytes);
      }
      console.log(
        `Generated ${SOUND_IDS.length} Carryforth notification sounds.`,
      );
    } else {
      checkDirectory(outputs);
      console.log(
        `Verified ${SOUND_IDS.length} deterministic Carryforth notification sounds.`,
      );
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
