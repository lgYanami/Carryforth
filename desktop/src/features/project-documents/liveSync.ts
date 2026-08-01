import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import {
  KIND_PROJECT_DOCUMENT_HEAD,
  KIND_PROJECT_DOCUMENT_META,
} from "@/shared/constants/kinds";

export const PROJECT_DOCUMENT_LIVE_LOOKBACK_SECONDS = 5;
export const PROJECT_DOCUMENT_INVALIDATION_DELAY_MS = 150;

export function projectDocumentLiveFilter(input: {
  relayPubkey: string;
  snapshotUpdatedAt?: string;
  nowSeconds?: number;
}): RelaySubscriptionFilter {
  const nowSeconds = input.nowSeconds ?? Math.floor(Date.now() / 1_000);
  const parsed = input.snapshotUpdatedAt
    ? Date.parse(input.snapshotUpdatedAt)
    : Number.NaN;
  const snapshotSeconds = Number.isNaN(parsed)
    ? nowSeconds
    : Math.floor(parsed / 1_000);
  return {
    authors: [input.relayPubkey.trim().toLowerCase()],
    kinds: [KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META],
    limit: 256,
    since: Math.max(
      0,
      snapshotSeconds - PROJECT_DOCUMENT_LIVE_LOOKBACK_SECONDS,
    ),
  };
}

/** Coalesces projection bursts into trusted native refreshes. */
export class ProjectDocumentInvalidationScheduler {
  private timer: number | null = null;
  private running = false;
  private trailing = false;
  private disposed = false;
  private readonly refresh: () => Promise<unknown> | unknown;
  private readonly delayMs: number;
  private readonly setTimeoutFn: (
    callback: () => void,
    delayMs: number,
  ) => number;
  private readonly clearTimeoutFn: (id: number) => void;

  constructor(
    refresh: () => Promise<unknown> | unknown,
    delayMs = PROJECT_DOCUMENT_INVALIDATION_DELAY_MS,
    setTimeoutFn: (callback: () => void, delayMs: number) => number,
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
    if (this.timer !== null) this.clearTimeoutFn(this.timer);
    this.timer = this.setTimeoutFn(() => {
      this.timer = null;
      void this.run();
    }, this.delayMs);
  }

  dispose(): void {
    this.disposed = true;
    this.trailing = false;
    if (this.timer !== null) this.clearTimeoutFn(this.timer);
    this.timer = null;
  }

  private async run(): Promise<void> {
    if (this.disposed || this.running) return;
    this.running = true;
    try {
      await this.refresh();
    } catch {
      // React Query owns the visible error and later signals remain usable.
    } finally {
      this.running = false;
      if (this.trailing && !this.disposed) {
        this.trailing = false;
        this.signal();
      }
    }
  }
}
