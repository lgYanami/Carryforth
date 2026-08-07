const STORAGE_KEY = "buzz:meeting-pending-commands:v1";
const MAX_PENDING_COMMANDS = 64;

export type MeetingPendingCommandLane = "action" | "floor" | "host";

type PendingCommandInput = {
  meetingId: string;
  submissionId: string;
};

type PendingCommandEntry = {
  input: unknown;
  lane: MeetingPendingCommandLane;
  scopeKey: string;
  updatedAt: number;
};

type PendingCommandStore = {
  entries: PendingCommandEntry[];
  version: 1;
};

function sessionStore(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readStore(storage: Storage): PendingCommandStore {
  try {
    const parsed: unknown = JSON.parse(storage.getItem(STORAGE_KEY) ?? "null");
    if (
      !isRecord(parsed) ||
      parsed.version !== 1 ||
      !Array.isArray(parsed.entries)
    ) {
      return { entries: [], version: 1 };
    }
    const entries = parsed.entries.filter(
      (entry): entry is PendingCommandEntry =>
        isRecord(entry) &&
        typeof entry.scopeKey === "string" &&
        (entry.lane === "action" ||
          entry.lane === "floor" ||
          entry.lane === "host") &&
        typeof entry.updatedAt === "number" &&
        "input" in entry,
    );
    return { entries: entries.slice(-MAX_PENDING_COMMANDS), version: 1 };
  } catch {
    return { entries: [], version: 1 };
  }
}

function writeStore(storage: Storage, store: PendingCommandStore): void {
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(store));
  } catch {
    // Exact retry remains protected by the native pending-event binding. If
    // session storage is unavailable, the current mounted controller still
    // retains the input in React state.
  }
}

function isPendingCommandInput(
  value: unknown,
  meetingId: string,
): value is PendingCommandInput {
  return (
    isRecord(value) &&
    value.meetingId === meetingId &&
    typeof value.submissionId === "string" &&
    value.submissionId.length > 0
  );
}

/**
 * Restore only the exact unresolved command for one
 * `{Community, identity, Meeting, lane}` scope.
 */
export function readMeetingPendingCommand<T extends PendingCommandInput>(
  scopeKey: string,
  lane: MeetingPendingCommandLane,
  meetingId: string,
): T | null {
  const storage = sessionStore();
  if (!storage) return null;
  const entries = readStore(storage).entries;
  let entry: PendingCommandEntry | undefined;
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const candidate = entries[index];
    if (candidate.scopeKey === scopeKey && candidate.lane === lane) {
      entry = candidate;
      break;
    }
  }
  return entry && isPendingCommandInput(entry.input, meetingId)
    ? (entry.input as T)
    : null;
}

/** Preserve a public mutation payload for exact retry; no signing key is stored. */
export function writeMeetingPendingCommand<T extends PendingCommandInput>(
  scopeKey: string,
  lane: MeetingPendingCommandLane,
  input: T,
): void {
  const storage = sessionStore();
  if (!storage) return;
  const current = readStore(storage);
  const entries = current.entries.filter(
    (entry) => entry.scopeKey !== scopeKey || entry.lane !== lane,
  );
  entries.push({ input, lane, scopeKey, updatedAt: Date.now() });
  writeStore(storage, {
    entries: entries.slice(-MAX_PENDING_COMMANDS),
    version: 1,
  });
}

export function clearMeetingPendingCommand(
  scopeKey: string,
  lane: MeetingPendingCommandLane,
): void {
  const storage = sessionStore();
  if (!storage) return;
  const current = readStore(storage);
  const entries = current.entries.filter(
    (entry) => entry.scopeKey !== scopeKey || entry.lane !== lane,
  );
  if (entries.length === current.entries.length) return;
  writeStore(storage, { entries, version: 1 });
}
