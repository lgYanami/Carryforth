import { ClipboardList, ShieldCheck } from "lucide-react";

import type { MeetingBoard } from "@/shared/api/tauriMeetings";
import { cn } from "@/shared/lib/cn";
import { Badge } from "@/shared/ui/badge";
import { Markdown } from "@/shared/ui/markdown";

export function MeetingBoardPanel({
  board,
  className,
}: {
  board: MeetingBoard;
  className?: string;
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
        <Badge variant="success">
          <ShieldCheck className="mr-1 size-3" />
          Current
        </Badge>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
        <Markdown
          className="max-w-full text-sm"
          content={board.body}
          interactive
        />
      </div>
      <div className="shrink-0 border-t px-4 py-2 text-2xs text-muted-foreground">
        Updated {new Date(board.updatedAt * 1_000).toLocaleString()}
      </div>
    </section>
  );
}
