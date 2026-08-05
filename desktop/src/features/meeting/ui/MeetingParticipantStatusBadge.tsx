import type { MeetingParticipantStatus } from "@/features/meeting/participantPresentation";
import { Badge } from "@/shared/ui/badge";

const statusVariants = {
  speaking: "info",
  waiting_for_ack: "warning",
  floor_requested: "outline",
  intent_pending: "outline",
  idle: "secondary",
} as const;

export function MeetingParticipantStatusBadge({
  status,
  testId,
}: {
  status: MeetingParticipantStatus;
  testId?: string;
}) {
  return (
    <div className="flex shrink-0 flex-col items-end gap-1">
      <Badge data-testid={testId} variant={statusVariants[status.kind]}>
        {status.label}
      </Badge>
      {status.detail ? (
        <span className="text-2xs text-muted-foreground">{status.detail}</span>
      ) : null}
    </div>
  );
}
