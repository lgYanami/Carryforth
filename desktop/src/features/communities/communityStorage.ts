import type { Community } from "./types";
import { homeDir } from "@tauri-apps/api/path";
import { setLocalStorageItemWithRecovery } from "@/shared/lib/localStorageQuota";
import {
  CANONICAL_LOCAL_RELAY_URL,
  LOCAL_COMMUNITY_NAME,
} from "@/shared/runtime/desktopNetworkMode";

const COMMUNITIES_KEY = "buzz-communities";
const ACTIVE_COMMUNITY_KEY = "buzz-active-community-id";
const LEGACY_WORKSPACES_KEY = "buzz-workspaces";
const LEGACY_ACTIVE_WORKSPACE_KEY = "buzz-active-workspace-id";

/**
 * Expand a leading `~` to the user's home directory. The backend rejects
 * `~`-prefixed paths (`std::fs` does not expand the shell tilde), so the UI
 * resolves it before save. Returns non-`~` input unchanged. Empty/whitespace
 * input returns `undefined` so callers can clear the override.
 */
export async function expandTilde(input: string): Promise<string | undefined> {
  const trimmed = input.trim();
  if (!trimmed) {
    return undefined;
  }
  if (trimmed === "~") {
    return homeDir();
  }
  if (trimmed.startsWith("~/")) {
    const home = await homeDir();
    const base = home.endsWith("/") ? home.slice(0, -1) : home;
    return `${base}/${trimmed.slice(2)}`;
  }
  return trimmed;
}

export function migrateLegacyCommunityStorage(
  storage: Storage = localStorage,
): void {
  if (storage.getItem(COMMUNITIES_KEY) === null) {
    const legacyCommunities = storage.getItem(LEGACY_WORKSPACES_KEY);
    if (legacyCommunities !== null) {
      storage.setItem(COMMUNITIES_KEY, legacyCommunities);
    }
  }
  if (storage.getItem(ACTIVE_COMMUNITY_KEY) === null) {
    const legacyActiveCommunity = storage.getItem(LEGACY_ACTIVE_WORKSPACE_KEY);
    if (legacyActiveCommunity !== null) {
      storage.setItem(ACTIVE_COMMUNITY_KEY, legacyActiveCommunity);
    }
  }
}

export function loadCommunities(storage: Storage = localStorage): Community[] {
  try {
    migrateLegacyCommunityStorage(storage);
    const raw = storage.getItem(COMMUNITIES_KEY);
    if (!raw) {
      return [];
    }
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      return [];
    }
    // Migration: older builds stored the user's `nsec` in localStorage and
    // re-applied it to the backend on every reload, which silently overwrote
    // any `import_identity` result with the original generated key. The
    // on-disk `identity.key` file is the only source of truth now. Strip
    // any lingering `nsec` from existing entries on read and persist the
    // cleaned list back so it cannot leak into future sessions.
    let didStrip = false;
    const cleaned = (parsed as Array<Record<string, unknown>>).map((entry) => {
      if (entry && typeof entry === "object" && "nsec" in entry) {
        const { nsec: _nsec, ...rest } = entry;
        didStrip = true;
        return rest;
      }
      return entry;
    }) as Community[];
    if (didStrip) {
      persistStorageValue(storage, COMMUNITIES_KEY, JSON.stringify(cleaned));
    }
    return cleaned;
  } catch {
    return [];
  }
}

function persistStorageValue(
  storage: Storage,
  key: string,
  value: string,
): boolean {
  if (typeof window !== "undefined" && storage === window.localStorage) {
    return setLocalStorageItemWithRecovery(key, value);
  }

  try {
    storage.setItem(key, value);
    return true;
  } catch {
    return false;
  }
}

export function saveCommunities(
  communities: Community[],
  storage: Storage = localStorage,
): boolean {
  return persistStorageValue(
    storage,
    COMMUNITIES_KEY,
    JSON.stringify(communities),
  );
}

export function clearCommunityStorage(storage: Storage = localStorage): void {
  storage.removeItem(COMMUNITIES_KEY);
  storage.removeItem(ACTIVE_COMMUNITY_KEY);
  storage.removeItem(LEGACY_WORKSPACES_KEY);
  storage.removeItem(LEGACY_ACTIVE_WORKSPACE_KEY);
}

export function loadActiveCommunityId(
  storage: Storage = localStorage,
): string | null {
  migrateLegacyCommunityStorage(storage);
  return storage.getItem(ACTIVE_COMMUNITY_KEY);
}

export function saveActiveCommunityId(
  id: string,
  storage: Storage = localStorage,
): boolean {
  return persistStorageValue(storage, ACTIVE_COMMUNITY_KEY, id);
}

export function clearActiveCommunityId(storage: Storage = localStorage): void {
  storage.removeItem(ACTIVE_COMMUNITY_KEY);
}

export function normalizeRelayUrl(url: string): string {
  if (!url.startsWith("ws://") && !url.startsWith("wss://")) {
    return `wss://${url}`;
  }
  return url;
}

function isLocalRelayHost(hostname: string): boolean {
  return ["localhost", "127.0.0.1", "[::1]", "0.0.0.0"].includes(hostname);
}

/** Whether a stored relay points at the Desktop's canonical local port. */
export function isLocalDesktopRelayUrl(relayUrl: string): boolean {
  try {
    const parsed = new URL(relayUrl.trim());
    return (
      (parsed.protocol === "ws:" || parsed.protocol === "wss:") &&
      isLocalRelayHost(parsed.hostname) &&
      parsed.port === "3000"
    );
  } catch {
    return false;
  }
}

function findReusableLocalCommunity(
  communities: Community[],
): Community | undefined {
  // Keep the first historical loopback record rather than preferring a later
  // duplicate that already uses the canonical spelling. The older record may
  // carry the stable Community id, reposDir, or local invite token that the
  // user has been developing against. Its URL is canonicalized before use.
  return communities.find((community) =>
    isLocalDesktopRelayUrl(community.relayUrl),
  );
}

function canonicalizeLocalCommunity(
  community: Community,
  pubkey?: string,
): Community {
  if (
    community.relayUrl === CANONICAL_LOCAL_RELAY_URL &&
    community.name === LOCAL_COMMUNITY_NAME &&
    (pubkey === undefined || community.pubkey === pubkey)
  ) {
    return community;
  }

  return {
    ...community,
    name: LOCAL_COMMUNITY_NAME,
    relayUrl: CANONICAL_LOCAL_RELAY_URL,
    ...(pubkey === undefined ? {} : { pubkey }),
  };
}

export type LocalOnlyCommunityResolution = {
  storedCommunities: Community[];
  connectableCommunity: Community;
  activeId: string;
  didChangeCommunities: boolean;
  didChangeActiveId: boolean;
  removedCommunities: Community[];
};

/**
 * Resolve Carryforth Desktop storage to one canonical local Community.
 *
 * A reusable loopback record retains its stable id and local configuration.
 * Every remote or duplicate record is deliberately removed: Carryforth has no
 * remote-community mode, so carrying those Buzz coordinates forward would be
 * misleading state rather than compatibility.
 */
export function resolveLocalOnlyCommunityState(
  communities: Community[],
  activeId: string | null,
  createId: () => string = () => crypto.randomUUID(),
  now: () => string = () => new Date().toISOString(),
  pubkey?: string,
): LocalOnlyCommunityResolution {
  const reusable = findReusableLocalCommunity(communities);
  const connectableCommunity = reusable
    ? canonicalizeLocalCommunity(reusable, pubkey)
    : {
        id: createId(),
        name: LOCAL_COMMUNITY_NAME,
        relayUrl: CANONICAL_LOCAL_RELAY_URL,
        ...(pubkey === undefined ? {} : { pubkey }),
        addedAt: now(),
      };

  const storedCommunities = [connectableCommunity];
  const removedCommunities = communities.filter(
    (community) => community.id !== reusable?.id,
  );
  const didChangeCommunities =
    communities.length !== 1 || communities[0] !== connectableCommunity;

  return {
    storedCommunities,
    connectableCommunity,
    activeId: connectableCommunity.id,
    didChangeCommunities,
    didChangeActiveId: activeId !== connectableCommunity.id,
    removedCommunities,
  };
}

/**
 * Project persisted communities into the only coordinate the running Desktop
 * may connect to. Remote records are removed by bootstrap, and this guard
 * prevents stale in-memory state from becoming connectable before that write.
 */
export function projectConnectableCommunities(
  communities: Community[],
): Community[] {
  const local = findReusableLocalCommunity(communities);
  return local ? [canonicalizeLocalCommunity(local)] : [];
}

export function loadConnectableCommunities(
  storage: Storage = localStorage,
): Community[] {
  return projectConnectableCommunities(loadCommunities(storage));
}

/**
 * Idempotently seed and activate the canonical local community before React
 * providers mount. Remote records are discarded after both canonical writes
 * succeed.
 */
export type LocalCommunityBootstrapResult = {
  community: Community;
  removedCommunities: Community[];
};

export function ensureLocalOnlyCommunityStorage(
  storage: Storage = localStorage,
  pubkey?: string,
): LocalCommunityBootstrapResult | null {
  const communities = loadCommunities(storage);
  const previousCommunitiesRaw = storage.getItem(COMMUNITIES_KEY);
  const previousActiveId = loadActiveCommunityId(storage);
  const resolution = resolveLocalOnlyCommunityState(
    communities,
    previousActiveId,
    undefined,
    undefined,
    pubkey,
  );

  if (
    resolution.didChangeCommunities &&
    !saveCommunities(resolution.storedCommunities, storage)
  ) {
    return null;
  }

  if (
    resolution.didChangeActiveId &&
    !saveActiveCommunityId(resolution.activeId, storage)
  ) {
    if (resolution.didChangeCommunities) {
      try {
        if (previousCommunitiesRaw === null) {
          storage.removeItem(COMMUNITIES_KEY);
        } else {
          storage.setItem(COMMUNITIES_KEY, previousCommunitiesRaw);
        }
      } catch {
        // Persistence is already unavailable. The caller keeps React unmounted
        // and shows the local setup error instead of connecting anywhere.
      }
    }
    return null;
  }

  // The old workspace keys are not a compatibility source in Carryforth. Do
  // this only after both canonical writes succeeded so a quota failure never
  // destroys the last recoverable local record.
  storage.removeItem(LEGACY_WORKSPACES_KEY);
  storage.removeItem(LEGACY_ACTIVE_WORKSPACE_KEY);

  return {
    community: resolution.connectableCommunity,
    removedCommunities: resolution.removedCommunities,
  };
}

export function deriveCommunityName(relayUrl: string): string {
  try {
    const url = new URL(
      relayUrl.replace("ws://", "http://").replace("wss://", "https://"),
    );
    const host = url.hostname;
    if (isLocalRelayHost(host)) {
      return "Local Dev";
    }
    const parts = host.split(".");
    // Detect staging environments (e.g. buzz-oss.stage.blox.sqprod.co)
    if (parts.some((p) => p === "stage" || p === "staging")) {
      return "Buzz (staging)";
    }
    // Use the first subdomain segment or the domain itself
    if (parts.length >= 2) {
      return parts[0] === "relay" ? parts[1] : parts[0];
    }
    return host;
  } catch {
    return "Community";
  }
}

export function initFirstCommunity(
  relayUrl: string,
  pubkey: string,
  name?: string,
): Community | null {
  if (relayUrl !== CANONICAL_LOCAL_RELAY_URL) {
    return null;
  }
  const normalizedUrl = CANONICAL_LOCAL_RELAY_URL;
  const trimmedName = name?.trim();
  const community: Community = {
    id: crypto.randomUUID(),
    name: trimmedName || deriveCommunityName(normalizedUrl),
    relayUrl: normalizedUrl,
    // Local bootstrap is token-less. Membership is established through the
    // signed owner claim or an explicit local invite, never a hosted fallback.
    pubkey,
    addedAt: new Date().toISOString(),
  };
  const previousActiveCommunityId = localStorage.getItem(ACTIVE_COMMUNITY_KEY);
  const didSaveActiveCommunity = saveActiveCommunityId(community.id);
  if (!didSaveActiveCommunity) {
    return null;
  }

  if (!saveCommunities([community])) {
    // A failed setItem leaves the existing communities value untouched. Roll
    // back only the active-ID write so inconsistent pre-existing data is never
    // destroyed while recovering from a quota failure.
    try {
      if (previousActiveCommunityId === null) {
        localStorage.removeItem(ACTIVE_COMMUNITY_KEY);
      } else {
        localStorage.setItem(ACTIVE_COMMUNITY_KEY, previousActiveCommunityId);
      }
    } catch {
      // Best effort: persistence is already unavailable, and callers will stay
      // on setup instead of reloading.
    }
    return null;
  }

  return community;
}
