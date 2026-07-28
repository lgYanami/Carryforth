import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import {
  KIND_PROJECT_VIEW_META,
  KIND_PROJECT_VIEW_OBJECT,
} from "@/shared/constants/kinds";

export const PROJECT_VIEW_LIVE_LOOKBACK_SECONDS = 5;
export const PROJECT_VIEW_LIVE_INVALIDATION_DELAY_MS = 150;

export function projectViewLiveFilter(input: {
  relayPubkey: string;
  snapshotUpdatedAt?: string;
  nowSeconds?: number;
}): RelaySubscriptionFilter {
  const nowSeconds = input.nowSeconds ?? Math.floor(Date.now() / 1_000);
  const snapshotTime = input.snapshotUpdatedAt
    ? Date.parse(input.snapshotUpdatedAt)
    : Number.NaN;
  const snapshotSeconds = Number.isNaN(snapshotTime)
    ? nowSeconds
    : Math.floor(snapshotTime / 1_000);

  return {
    authors: [input.relayPubkey.trim().toLowerCase()],
    kinds: [KIND_PROJECT_VIEW_OBJECT, KIND_PROJECT_VIEW_META],
    limit: 256,
    since: Math.max(0, snapshotSeconds - PROJECT_VIEW_LIVE_LOOKBACK_SECONDS),
  };
}

/**
 * Coalesces a projection burst into one trusted snapshot refresh. If another
 * projection arrives while that refresh is running, one trailing refresh is
 * retained so the newest revision cannot be lost.
 */
export class ProjectViewInvalidationScheduler {
  private timer: number | null = null;
  private running = false;
  private trailing = false;
  private disposed = false;
  private readonly refresh: () => Promise<unknown> | unknown;
  private readonly delayMs: number;
  private readonly setTimeoutFn: (callback: () => void, ms: number) => number;
  private readonly clearTimeoutFn: (id: number) => void;

  constructor(
    refresh: () => Promise<unknown> | unknown,
    delayMs = PROJECT_VIEW_LIVE_INVALIDATION_DELAY_MS,
    setTimeoutFn: (callback: () => void, ms: number) => number,
    clearTimeoutFn: (id: number) => void,
  ) {
    this.refresh = refresh;
    this.delayMs = delayMs;
    this.setTimeoutFn = setTimeoutFn;
    this.clearTimeoutFn = clearTimeoutFn;
  }

  signal(): void {
    if (this.disposed) return;
    if (this.running) {
      this.trailing = true;
      return;
    }
    if (this.timer !== null) {
      this.clearTimeoutFn(this.timer);
    }
    this.timer = this.setTimeoutFn(() => {
      this.timer = null;
      void this.run();
    }, this.delayMs);
  }

  dispose(): void {
    this.disposed = true;
    this.trailing = false;
    if (this.timer !== null) {
      this.clearTimeoutFn(this.timer);
      this.timer = null;
    }
  }

  private async run(): Promise<void> {
    if (this.disposed || this.running) return;
    this.running = true;
    try {
      await this.refresh();
    } catch {
      // The React Query observer owns the user-visible refresh error. A failed
      // read must not break this coordinator or prevent a later signal from
      // retrying the complete trusted snapshot.
    } finally {
      this.running = false;
      if (this.trailing && !this.disposed) {
        this.trailing = false;
        this.signal();
      }
    }
  }
}
