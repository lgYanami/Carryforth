import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import {
  KIND_PROJECT_CONTEXT_EDGE_BINDING,
  KIND_PROJECT_CONTEXT_META,
  KIND_PROJECT_DOCUMENT_HEAD,
  KIND_PROJECT_DOCUMENT_META,
  KIND_PROJECT_VIEW_META,
  KIND_PROJECT_VIEW_OBJECT,
} from "@/shared/constants/kinds";

export const PROJECT_CONTEXT_LIVE_LOOKBACK_SECONDS = 5;
export const PROJECT_CONTEXT_LIVE_INVALIDATION_DELAY_MS = 150;

export type ProjectContextInvalidationScope =
  | "context"
  | "project_view"
  | "document_catalog"
  | "documents";

const CONTEXT_KINDS = new Set([
  KIND_PROJECT_CONTEXT_EDGE_BINDING,
  KIND_PROJECT_CONTEXT_META,
]);
const PROJECT_VIEW_KINDS = new Set([
  KIND_PROJECT_VIEW_OBJECT,
  KIND_PROJECT_VIEW_META,
]);
const DOCUMENT_KINDS = new Set([
  KIND_PROJECT_DOCUMENT_HEAD,
  KIND_PROJECT_DOCUMENT_META,
]);

const LIVE_KINDS = [
  KIND_PROJECT_CONTEXT_EDGE_BINDING,
  KIND_PROJECT_CONTEXT_META,
  KIND_PROJECT_VIEW_OBJECT,
  KIND_PROJECT_VIEW_META,
  KIND_PROJECT_DOCUMENT_HEAD,
  KIND_PROJECT_DOCUMENT_META,
];

function observedSeconds(value: string | undefined): number | undefined {
  if (!value) return undefined;
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? undefined : Math.floor(parsed / 1_000);
}

/**
 * Subscribe only to projection events authored by the verified Relay, with an
 * overlap that closes all three snapshot-to-subscription race windows.
 */
export function projectContextLiveFilter(input: {
  relayPubkey: string;
  contextUpdatedAt?: string;
  projectViewUpdatedAt?: string;
  documentUpdatedAt?: string;
  nowSeconds?: number;
}): RelaySubscriptionFilter {
  const nowSeconds = input.nowSeconds ?? Math.floor(Date.now() / 1_000);
  const observed = [
    observedSeconds(input.contextUpdatedAt),
    observedSeconds(input.projectViewUpdatedAt),
    observedSeconds(input.documentUpdatedAt),
  ].filter((value): value is number => value !== undefined);
  const oldestObserved =
    observed.length > 0 ? Math.min(...observed) : nowSeconds;

  return {
    authors: [input.relayPubkey.trim().toLowerCase()],
    kinds: LIVE_KINDS,
    limit: 512,
    since: Math.max(0, oldestObserved - PROJECT_CONTEXT_LIVE_LOOKBACK_SECONDS),
  };
}

/** Map an untrusted projection hint to the trusted reads it can invalidate. */
export function projectContextInvalidationScopesForKind(
  kind: number,
): ProjectContextInvalidationScope[] {
  if (CONTEXT_KINDS.has(kind)) return ["context"];
  if (PROJECT_VIEW_KINDS.has(kind)) return ["context", "project_view"];
  if (DOCUMENT_KINDS.has(kind)) return ["context", "documents"];
  return [];
}

/**
 * Coalesce projection bursts into complete trusted refreshes. Signals received
 * during an active refresh are retained for one trailing refresh.
 */
export class ProjectContextInvalidationScheduler {
  private timer: number | null = null;
  private running = false;
  private disposed = false;
  private readonly pending = new Set<ProjectContextInvalidationScope>();
  private readonly refresh: (
    scopes: ReadonlySet<ProjectContextInvalidationScope>,
  ) => Promise<unknown> | unknown;
  private readonly delayMs: number;
  private readonly setTimeoutFn: (callback: () => void, ms: number) => number;
  private readonly clearTimeoutFn: (id: number) => void;

  constructor(
    refresh: (
      scopes: ReadonlySet<ProjectContextInvalidationScope>,
    ) => Promise<unknown> | unknown,
    delayMs = PROJECT_CONTEXT_LIVE_INVALIDATION_DELAY_MS,
    setTimeoutFn: (callback: () => void, ms: number) => number,
    clearTimeoutFn: (id: number) => void,
  ) {
    this.refresh = refresh;
    this.delayMs = delayMs;
    this.setTimeoutFn = setTimeoutFn;
    this.clearTimeoutFn = clearTimeoutFn;
  }

  signal(
    scopes:
      | ProjectContextInvalidationScope
      | Iterable<ProjectContextInvalidationScope>,
  ): void {
    if (this.disposed) return;
    if (typeof scopes === "string") {
      this.pending.add(scopes);
    } else {
      for (const scope of scopes) this.pending.add(scope);
    }
    if (!this.running) this.schedule();
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
    if (this.disposed || this.pending.size === 0) return;
    if (this.timer !== null) this.clearTimeoutFn(this.timer);
    this.timer = this.setTimeoutFn(() => {
      this.timer = null;
      void this.run();
    }, this.delayMs);
  }

  private async run(): Promise<void> {
    if (this.disposed || this.running || this.pending.size === 0) return;
    const scopes = new Set(this.pending);
    this.pending.clear();
    this.running = true;
    try {
      await this.refresh(scopes);
    } catch {
      // React Query owns user-visible refresh errors. Keep the scheduler alive
      // so a later projection can re-enter the complete native boundary.
    } finally {
      this.running = false;
      if (!this.disposed && this.pending.size > 0) this.schedule();
    }
  }
}
