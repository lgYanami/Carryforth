import { clearSavedCommunitySnapshot } from "@/features/agents/activeAgentTurnsStore";
import { removeChannelSnapshotForRelay } from "@/features/channels/channelSnapshot";
import { removeMessageSnapshotsForRelay } from "@/features/messages/lib/messageSnapshot";
import { removeSelfProfileCachesForRelay } from "@/features/profile/lib/selfProfileStorage";

import { removeCachedCommunityIcon } from "./communityIconCache";
import { removeCommunityDestination } from "./communityNavigationStorage";
import { isLocalDesktopRelayUrl } from "./communityStorage";
import type { Community } from "./types";

/** Remove relay-scoped stores that do not expose their own GC helper. */
export function removeRelayScopedStorageKeys(
  relayUrl: string,
  storage: Storage = window.localStorage,
): void {
  // A discarded duplicate localhost entry still points at the retained local
  // Relay. Never treat that alias as remote state: its relay-scoped snapshots
  // belong to the same canonical Community data the user is keeping.
  if (isLocalDesktopRelayUrl(relayUrl)) return;

  const normalized = relayUrl.trim().replace(/\/+$/, "").toLowerCase();
  if (!normalized) return;
  const encoded = encodeURIComponent(normalized);
  const keys = Array.from({ length: storage.length }, (_, index) =>
    storage.key(index),
  ).filter((key): key is string => key !== null);

  for (const key of keys) {
    if (key.includes(normalized) || key.includes(encoded)) {
      storage.removeItem(key);
    }
  }
}

/**
 * Remove browser-side state that is scoped to discarded Buzz remote
 * Communities. Local Relay data and the Desktop identity are untouched.
 */
export function purgeRemovedCommunityClientState(
  removedCommunities: Community[],
): void {
  for (const community of removedCommunities) {
    // These two stores are keyed by the discarded Community id and can be
    // removed even for a duplicate localhost record.
    clearSavedCommunitySnapshot(community.id);
    removeCommunityDestination(community.id);

    if (isLocalDesktopRelayUrl(community.relayUrl)) continue;

    removeSelfProfileCachesForRelay(community.relayUrl);
    removeChannelSnapshotForRelay(community.relayUrl);
    removeMessageSnapshotsForRelay(community.relayUrl);
    removeCachedCommunityIcon(community.relayUrl);
    removeRelayScopedStorageKeys(community.relayUrl);
  }
}
