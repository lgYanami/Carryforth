import {
  AlertTriangle,
  ClipboardCopy,
  ClipboardList,
  ShieldCheck,
} from "lucide-react";

import type { MeetingBoard } from "@/shared/api/tauriMeetings";
import type { MeetingStaleBoardDraft } from "@/features/meeting/useMeetingBoardDraft";
import { copyTextToClipboard } from "@/shared/lib/clipboard";
import { cn } from "@/shared/lib/cn";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Markdown } from "@/shared/ui/markdown";
import { Textarea } from "@/shared/ui/textarea";

export type MeetingBoardEditor = {
  disabled: boolean;
  value: string;
  onChange: (value: string) => void;
};

export function MeetingBoardPanel({
  board,
  className,
  editor,
  onDismissStaleDraft,
  staleDraft,
}: {
  board: MeetingBoard;
  className?: string;
  editor?: MeetingBoardEditor;
  onDismissStaleDraft?: () => void;
  staleDraft?: MeetingStaleBoardDraft | null;
}) {
  return (
    <section
      className={cn("flex min-h-0 flex-col bg-background", className)}
      data-testid="meeting-board"
    >
      <div className="flex h-12 shrink-0 items-center gap-2 border-b px-4">
        <ClipboardList className="size-4" />
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold">Meeting board</h2>
          <p className="truncate text-2xs text-muted-foreground">
            Maintained by the host
          </p>
        </div>
        <Badge variant={editor ? "warning" : "success"}>
          {editor ? null : <ShieldCheck className="mr-1 size-3" />}
          {editor ? "Editing" : "Current"}
        </Badge>
      </div>
      {staleDraft ? (
        <div
          className="m-3 flex items-start gap-2 rounded-lg border border-amber-500/35 bg-amber-500/5 p-3 text-xs"
          data-testid="meeting-stale-board-draft"
        >
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-600" />
          <div className="min-w-0 flex-1">
            <p className="font-medium">An unsubmitted Board draft was kept</p>
            <p className="mt-1 text-muted-foreground">
              Its control window ended. Copy it if useful; it cannot be
              submitted against a later window.
            </p>
            <div className="mt-2 flex gap-2">
              <Button
                onClick={() =>
                  copyTextToClipboard(staleDraft.body, "Board draft copied")
                }
                size="sm"
                variant="outline"
              >
                <ClipboardCopy className="size-4" />
                Copy draft
              </Button>
              <Button onClick={onDismissStaleDraft} size="sm" variant="ghost">
                Dismiss
              </Button>
            </div>
          </div>
        </div>
      ) : null}
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-5 py-4">
        {editor ? (
          <Textarea
            aria-label="Meeting Board Markdown"
            className="min-h-72 flex-1 resize-none font-mono text-sm leading-relaxed"
            data-testid="meeting-board-editor"
            disabled={editor.disabled}
            onChange={(event) => editor.onChange(event.target.value)}
            spellCheck
            value={editor.value}
          />
        ) : (
          <Markdown
            className="max-w-full text-sm"
            content={board.body}
            interactive
          />
        )}
      </div>
      <div className="shrink-0 border-t px-4 py-2 text-2xs text-muted-foreground">
        {editor
          ? "This editor is bound to the current Board window."
          : `Updated ${new Date(board.updatedAt * 1_000).toLocaleString()}`}
      </div>
    </section>
  );
}
