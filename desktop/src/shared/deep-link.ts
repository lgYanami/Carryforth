import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Payload emitted by the native handler for `carryforth://message?…`. */
export type MessageDeepLinkPayload = {
  channelId: string;
  messageId: string;
  threadRootId: string | null;
};

/** Register the only supported Carryforth deep-link surface. */
export function listenForMessageDeepLinks(
  onOpen: (payload: MessageDeepLinkPayload) => void,
): Promise<UnlistenFn> {
  return listen<MessageDeepLinkPayload>("deep-link-message", (event) => {
    onOpen(event.payload);
  });
}
