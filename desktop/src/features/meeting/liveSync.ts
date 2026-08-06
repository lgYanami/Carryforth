import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_MEETING_END,
  KIND_MEETING_STATE,
  KIND_STREAM_MESSAGE,
} from "@/shared/constants/kinds";

export const MEETING_LIVE_LOOKBACK_SECONDS = 5;
export const MEETING_LIVE_INVALIDATION_DELAY_MS = 150;
export const MEETING_LIVE_RETRY_BASE_MS = 500;
export const MEETING_LIVE_RETRY_MAX_MS = 5_000;

export type MeetingLiveSignal = "initial" | number;
export type MeetingLiveDispose = () => Promise<void>;
export type MeetingLiveSubscribe = (
  filter: RelaySubscriptionFilter,
  onEvent: (event: RelayEvent) => void,
) => Promise<MeetingLiveDispose>;

export function meetingLiveFilter(
  meetingId: string,
  nowSeconds = Math.floor(Date.now() / 1_000),
): RelaySubscriptionFilter {
  const normalizedMeetingId = meetingId.trim();
  if (!normalizedMeetingId) {
    throw new Error("Meeting live subscriptions require one channel ID");
  }

  return {
    kinds: [KIND_STREAM_MESSAGE, KIND_MEETING_STATE, KIND_MEETING_END],
    "#h": [normalizedMeetingId],
    limit: 256,
    since: Math.max(0, nowSeconds - MEETING_LIVE_LOOKBACK_SECONDS),
  };
}

/**
 * Coalesce a burst of live hints into one canonical read, retaining any hints
 * that arrive while the previous read is running.
 */
export class MeetingLiveInvalidationScheduler {
  private readonly pending = new Set<MeetingLiveSignal>();
  private timer: number | null = null;
  private running = false;
  private disposed = false;
  private readonly refresh: (
    signals: ReadonlySet<MeetingLiveSignal>,
  ) => Promise<unknown> | unknown;
  private readonly delayMs: number;
  private readonly setTimeoutFn: (callback: () => void, ms: number) => number;
  private readonly clearTimeoutFn: (id: number) => void;

  constructor(
    refresh: (
      signals: ReadonlySet<MeetingLiveSignal>,
    ) => Promise<unknown> | unknown,
    delayMs = MEETING_LIVE_INVALIDATION_DELAY_MS,
    setTimeoutFn: (callback: () => void, ms: number) => number,
    clearTimeoutFn: (id: number) => void,
  ) {
    this.refresh = refresh;
    this.delayMs = delayMs;
    this.setTimeoutFn = setTimeoutFn;
    this.clearTimeoutFn = clearTimeoutFn;
  }

  signal(signal: MeetingLiveSignal): void {
    if (this.disposed) return;
    this.pending.add(signal);
    if (this.running) return;
    this.schedule();
  }

  dispose(): void {
    this.disposed = true;
    this.pending.clear();
    if (this.timer !== null) {
      this.clearTimeoutFn(this.timer);
      this.timer = null;
    }
  }

  private schedule(): void {
    if (this.disposed || this.running) return;
    if (this.timer !== null) this.clearTimeoutFn(this.timer);
    this.timer = this.setTimeoutFn(() => {
      this.timer = null;
      void this.run();
    }, this.delayMs);
  }

  private async run(): Promise<void> {
    if (this.disposed || this.running || this.pending.size === 0) return;
    const signals = new Set(this.pending);
    this.pending.clear();
    this.running = true;
    try {
      await this.refresh(signals);
    } catch {
      // React Query owns the visible read error. A later signal or fallback
      // poll must remain able to retry the full trusted snapshot.
    } finally {
      this.running = false;
      if (!this.disposed && this.pending.size > 0) this.schedule();
    }
  }
}

type MeetingSubscriptionEntry = {
  meetingId: string;
  connecting: boolean;
  retryAttempt: number;
  retryTimer: number | null;
  dispose: MeetingLiveDispose | null;
};

type MeetingLiveSubscriptionManagerOptions = {
  subscribe: MeetingLiveSubscribe;
  onSignal: (meetingId: string, signal: MeetingLiveSignal) => void;
  onError?: (meetingId: string, error: unknown, retryInMs: number) => void;
  nowSeconds?: () => number;
  setTimeoutFn: (callback: () => void, ms: number) => number;
  clearTimeoutFn: (id: number) => void;
  retryBaseMs?: number;
  retryMaxMs?: number;
};

/**
 * Maintains exactly one Relay subscription per live Meeting channel. Entries
 * are diffed by ID so unrelated Meetings are not churned when one terminates.
 */
export class MeetingLiveSubscriptionManager {
  private readonly entries = new Map<string, MeetingSubscriptionEntry>();
  private destroyed = false;
  private readonly subscribe: MeetingLiveSubscribe;
  private readonly onSignal: (
    meetingId: string,
    signal: MeetingLiveSignal,
  ) => void;
  private readonly onError?: (
    meetingId: string,
    error: unknown,
    retryInMs: number,
  ) => void;
  private readonly nowSeconds: () => number;
  private readonly setTimeoutFn: (callback: () => void, ms: number) => number;
  private readonly clearTimeoutFn: (id: number) => void;
  private readonly retryBaseMs: number;
  private readonly retryMaxMs: number;

  constructor(options: MeetingLiveSubscriptionManagerOptions) {
    this.subscribe = options.subscribe;
    this.onSignal = options.onSignal;
    this.onError = options.onError;
    this.nowSeconds =
      options.nowSeconds ?? (() => Math.floor(Date.now() / 1_000));
    this.setTimeoutFn = options.setTimeoutFn;
    this.clearTimeoutFn = options.clearTimeoutFn;
    this.retryBaseMs = options.retryBaseMs ?? MEETING_LIVE_RETRY_BASE_MS;
    this.retryMaxMs = options.retryMaxMs ?? MEETING_LIVE_RETRY_MAX_MS;
  }

  sync(meetingIds: readonly string[]): void {
    if (this.destroyed) return;
    const desired = new Set(
      meetingIds.map((meetingId) => meetingId.trim()).filter(Boolean),
    );

    for (const [meetingId, entry] of this.entries) {
      if (desired.has(meetingId)) continue;
      this.entries.delete(meetingId);
      this.disposeEntry(entry);
    }

    for (const meetingId of desired) {
      if (this.entries.has(meetingId)) continue;
      const entry: MeetingSubscriptionEntry = {
        meetingId,
        connecting: false,
        retryAttempt: 0,
        retryTimer: null,
        dispose: null,
      };
      this.entries.set(meetingId, entry);
      void this.connect(entry);
    }
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    for (const entry of this.entries.values()) this.disposeEntry(entry);
    this.entries.clear();
  }

  private async connect(entry: MeetingSubscriptionEntry): Promise<void> {
    if (!this.isCurrent(entry) || entry.connecting || entry.dispose) return;
    entry.connecting = true;
    try {
      const dispose = await this.subscribe(
        meetingLiveFilter(entry.meetingId, this.nowSeconds()),
        (event) => {
          if (this.isCurrent(entry)) {
            this.onSignal(entry.meetingId, event.kind);
          }
        },
      );
      entry.connecting = false;
      if (!this.isCurrent(entry)) {
        void dispose().catch(() => {});
        return;
      }
      entry.dispose = dispose;
      entry.retryAttempt = 0;
      // Close the first canonical read → live subscription race.
      this.onSignal(entry.meetingId, "initial");
    } catch (error) {
      entry.connecting = false;
      if (!this.isCurrent(entry)) return;
      const retryInMs = Math.min(
        this.retryMaxMs,
        this.retryBaseMs * 2 ** Math.min(entry.retryAttempt, 8),
      );
      entry.retryAttempt += 1;
      this.onError?.(entry.meetingId, error, retryInMs);
      entry.retryTimer = this.setTimeoutFn(() => {
        entry.retryTimer = null;
        void this.connect(entry);
      }, retryInMs);
    }
  }

  private isCurrent(entry: MeetingSubscriptionEntry): boolean {
    return !this.destroyed && this.entries.get(entry.meetingId) === entry;
  }

  private disposeEntry(entry: MeetingSubscriptionEntry): void {
    if (entry.retryTimer !== null) {
      this.clearTimeoutFn(entry.retryTimer);
      entry.retryTimer = null;
    }
    const dispose = entry.dispose;
    entry.dispose = null;
    if (dispose) void dispose().catch(() => {});
  }
}
