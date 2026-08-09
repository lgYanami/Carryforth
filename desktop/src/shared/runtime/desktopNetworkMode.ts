import { getDesktopNetworkMode as getNativeDesktopNetworkMode } from "@/shared/api/tauri";

export const CANONICAL_LOCAL_RELAY_URL = "ws://localhost:3000";
export const LOCAL_COMMUNITY_NAME = "Local Dev";

export type DesktopNetworkMode = "localOnly";

/** Load the native policy before mounting community-scoped providers. */
export async function bootstrapDesktopNetworkMode(): Promise<DesktopNetworkMode> {
  try {
    const mode = await getNativeDesktopNetworkMode();
    if (mode !== "localOnly") {
      console.warn(
        "Desktop reported an unsupported network mode; using local-only.",
      );
    }
  } catch (error) {
    console.warn(
      "Failed to load the Desktop network mode; staying local-only.",
      error,
    );
  }
  return "localOnly";
}

export function getDesktopNetworkMode(): DesktopNetworkMode {
  return "localOnly";
}

export function isDesktopLocalOnly(): boolean {
  return true;
}

/** Final synchronous guard for code paths that are handed a relay URL. */
export function isDesktopRelayUrlAllowed(relayUrl: string): boolean {
  return relayUrl === CANONICAL_LOCAL_RELAY_URL;
}
