import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { StartCommunityOnboardingInput } from "@/features/onboarding/communityOnboarding";

export type AddCommunityDeepLinkPayload = {
  relayUrl: string;
  name?: string;
};

export interface DeepLinkDeps {
  startCommunityOnboarding: (input: StartCommunityOnboardingInput) => boolean;
  openAddCommunity: (
    payload: AddCommunityDeepLinkPayload & { requestId: string },
  ) => boolean;
  onAddCommunityAvailable: (listener: () => void) => () => void;
}

/**
 * Payload emitted by the Rust deep-link handler for `buzz://message?…`.
 * Field names match the JSON shape produced in `desktop/src-tauri/src/lib.rs`.
 */
export type MessageDeepLinkPayload = {
  channelId: string;
  messageId: string;
  threadRootId: string | null;
};

export type NostrBindDeepLinkPayload = {
  challengeId: string;
  nonce: string;
  verificationCode: string;
  audience: "buzz:nostr-identity";
  action: "bind_nostr_identity";
  protocol: "buzz-nostr-identity";
  version: "1";
  origin: string;
  expiresAt: string;
  returnMode: "clipboard" | "browser_fragment_v1";
  callbackUrl?: string;
};

/**
 * Register listeners for deep-link events emitted by the Rust backend.
 *
 * When a `buzz://connect?relay=<url>` link is opened, the handler
 * adds a community for the relay (deduplicating by URL) and switches
 * to it. Returns an unlisten function to tear down all listeners.
 *
 * When a `buzz://join?relay=<url>&code=<invite>` link is opened (relay
 * invite landing page), the handler first claims the invite against the
 * relay's HTTP API — signed by this app's identity key — and only adds and
 * switches to the community once the relay has admitted the key.
 *
 * `buzz://message?…` is handled separately by `listenForMessageDeepLinks`,
 * because it needs to dispatch into the router which only exists below the
 * `RouterProvider` in the component tree.
 */
export async function listenForDeepLinks(
  deps: DeepLinkDeps,
): Promise<UnlistenFn> {
  // Community deep links are a remote-community entry point. Carryforth does
  // not consume Rust's pending queue or register any of those listeners.
  void deps;
  return () => {};
}

/**
 * Register a listener for `deep-link-message` events. Must be called from
 * inside the router tree (e.g. AppShell) because the navigation callback
 * uses TanStack Router state.
 */
export function listenForMessageDeepLinks(
  onOpen: (payload: MessageDeepLinkPayload) => void,
): Promise<UnlistenFn> {
  return listen<MessageDeepLinkPayload>("deep-link-message", (event) => {
    onOpen(event.payload);
  });
}

export function listenForNostrBindDeepLinks(
  onOpen: (payload: NostrBindDeepLinkPayload) => void,
): Promise<UnlistenFn> {
  // Nostr binding is the browser/Builderlab account hand-off. A local-only
  // Desktop must not surface or sign that remote binding request.
  void onOpen;
  return Promise.resolve(() => {});
}
