import {
  AlertCircle,
  CheckCircle2,
  ClipboardCheck,
  Clock3,
  CornerDownRight,
  Hand,
  ListRestart,
  RefreshCw,
  ShieldAlert,
  XCircle,
} from "lucide-react";

import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import type { UserProfileSummary } from "@/shared/api/types";
import type {
  MeetingActivity,
  MeetingActivityKind,
} from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";

const activityTimeFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

function participantName(
  pubkey: string,
  profiles: Record<string, UserProfileSummary>,
): string {
  return (
    profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey)
  );
}

function activityIcon(kind: MeetingActivityKind) {
  if (kind.startsWith("board_")) return ClipboardCheck;
  if (kind.startsWith("handoff_")) return CornerDownRight;
  if (kind.startsWith("action_")) return ListRestart;
  if (kind === "meeting_closed") return CheckCircle2;
  if (kind === "meeting_aborted") return XCircle;
  if (kind.includes("expired") || kind.includes("timed_out")) return Clock3;
  if (kind === "offer_declined" || kind === "floor_recalled") {
    return ShieldAlert;
  }
  return Hand;
}

function ActivityActor({
  activity,
  profiles,
}: {
  activity: MeetingActivity;
  profiles: Record<string, UserProfileSummary>;
}) {
  const actor = activity.actorPubkey;
  const target = activity.targetPubkey;
  if (!actor && !target) return null;
  const avatarPubkey = actor ?? target;
  if (!avatarPubkey) return null;
  const profile = profiles[avatarPubkey.toLowerCase()];
  return (
    <div className="mt-1.5 flex items-center gap-1.5 text-xs text-muted-foreground">
      <ProfileAvatar
        avatarUrl={profile?.avatarUrl ?? null}
        className="size-4"
        label={participantName(avatarPubkey, profiles)}
      />
      <span>
        {actor ? participantName(actor, profiles) : "System"}
        {target && target !== actor
          ? ` → ${participantName(target, profiles)}`
          : ""}
      </span>
    </div>
  );
}

export function MeetingActivityPanel({
  activities,
  error,
  hasOlder,
  isFetching,
  isFetchingOlder,
  onFetchOlder,
  onRetry,
  profiles,
}: {
  activities: MeetingActivity[];
  error: boolean;
  hasOlder: boolean;
  isFetching: boolean;
  isFetchingOlder: boolean;
  onFetchOlder: () => void;
  onRetry: () => void;
  profiles: Record<string, UserProfileSummary>;
}) {
  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      data-testid="meeting-activity-panel"
    >
      <div className="border-b px-5 py-4">
        <p className="text-xs text-muted-foreground">
          Verified Board, floor, handoff, action, and end transitions. Formal
          Speech remains in the meeting timeline.
        </p>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
        {isFetching && activities.length === 0 ? (
          <div className="flex items-center gap-2 py-8 text-sm text-muted-foreground">
            <RefreshCw className="size-4 animate-spin" />
            Loading verified activity…
          </div>
        ) : error && activities.length === 0 ? (
          <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-4">
            <div className="flex items-center gap-2 text-sm font-medium text-destructive">
              <AlertCircle className="size-4" />
              Meeting activity could not be verified.
            </div>
            <Button
              className="mt-3"
              onClick={onRetry}
              size="sm"
              variant="outline"
            >
              <RefreshCw className="size-4" />
              Retry
            </Button>
          </div>
        ) : activities.length === 0 ? (
          <div className="py-8 text-sm text-muted-foreground">
            No product-level control activity has been recorded yet.
          </div>
        ) : (
          <ol className="space-y-1" data-testid="meeting-activity-list">
            {activities.map((activity) => {
              const Icon = activityIcon(activity.kind);
              return (
                <li
                  className="relative flex gap-3 rounded-lg px-2 py-3 hover:bg-muted/40"
                  data-activity-kind={activity.kind}
                  data-testid={`meeting-activity-${activity.activityId}`}
                  key={activity.activityId}
                >
                  <div className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-full border bg-background">
                    <Icon className="size-3.5 text-muted-foreground" />
                  </div>
                  <div className="min-w-0 flex-1">
                    <p className="text-sm leading-5">{activity.summary}</p>
                    <ActivityActor activity={activity} profiles={profiles} />
                    <time
                      className="mt-1 block text-2xs text-muted-foreground"
                      dateTime={new Date(activity.occurredAtMs).toISOString()}
                    >
                      {activityTimeFormatter.format(
                        new Date(activity.occurredAtMs),
                      )}
                    </time>
                  </div>
                </li>
              );
            })}
          </ol>
        )}
        {hasOlder ? (
          <Button
            className="mt-3 w-full"
            data-testid="meeting-activity-load-older"
            disabled={isFetchingOlder}
            onClick={onFetchOlder}
            size="sm"
            variant="outline"
          >
            {isFetchingOlder ? (
              <RefreshCw className="size-4 animate-spin" />
            ) : (
              <Clock3 className="size-4" />
            )}
            Load earlier activity
          </Button>
        ) : null}
        {error && activities.length > 0 ? (
          <div className="mt-3 flex items-center justify-between gap-3 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
            <span>Earlier activity could not be verified.</span>
            <Button onClick={onRetry} size="sm" variant="outline">
              Retry
            </Button>
          </div>
        ) : null}
      </div>
    </div>
  );
}
