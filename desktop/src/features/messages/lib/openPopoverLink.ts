import { isMessageLink, parseMessageLink } from "./messageLink";
import type { ParsedMessageLink } from "./messageLink";

/**
 * Open a link the same way the rendered-message link path does:
 * `carryforth://message?…` deep-links navigate in-app, everything else goes
 * to the OS opener. Mirrors `markdown.tsx`'s `a`
 * renderer so the composer popover and the rendered link behave identically.
 */
export function openPopoverLink(
  url: string,
  handlers: {
    openExternal: (url: string) => void;
    openMessageLink: (link: ParsedMessageLink) => void;
  },
): void {
  if (isMessageLink(url)) {
    const parsed = parseMessageLink(url);
    if (parsed.ok) {
      handlers.openMessageLink(parsed.value);
      return;
    }
  }
  handlers.openExternal(url);
}
