const STORAGE_PREFIX = "buzz-meeting-attention-ack.v1";

function storageKey(communityId: string): string {
  return `${STORAGE_PREFIX}:${communityId}`;
}

export function readMeetingAttentionAcknowledgements(
  communityId: string,
): Set<string> {
  try {
    const value = window.localStorage.getItem(storageKey(communityId));
    if (!value) return new Set();
    const parsed: unknown = JSON.parse(value);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(
      parsed.filter((entry): entry is string => typeof entry === "string"),
    );
  } catch {
    return new Set();
  }
}

export function writeMeetingAttentionAcknowledgements(
  communityId: string,
  acknowledgements: ReadonlySet<string>,
): void {
  try {
    window.localStorage.setItem(
      storageKey(communityId),
      JSON.stringify([...acknowledgements].sort()),
    );
  } catch {
    // Acknowledgement is a UI convenience; storage denial must not block navigation.
  }
}
