import type { ReactNode } from "react";
import {
  Bot,
  CheckCircle2,
  ExternalLink,
  FileCheck2,
  History,
  Link,
  MoreHorizontal,
  OctagonX,
  Presentation,
  ShieldCheck,
  UserRound,
  UsersRound,
  XCircle,
} from "lucide-react";

import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import type { UserProfileSummary } from "@/shared/api/types";
import type {
  MeetingLifecycle,
  MeetingSnapshot,
} from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";

function lifecycleLabel(lifecycle: MeetingLifecycle): string {
  switch (lifecycle) {
    case "initializing":
      return "Starting";
    case "active":
      return "In progress";
    case "finalizing_actions":
      return "Recording actions";
    case "closed":
      return "Completed";
    case "aborted":
      return "Aborted";
  }
}

function lifecycleBadgeVariant(
  lifecycle: MeetingLifecycle,
): "secondary" | "info" | "warning" | "success" | "destructive" {
  switch (lifecycle) {
    case "initializing":
      return "secondary";
    case "active":
      return "info";
    case "finalizing_actions":
      return "warning";
    case "closed":
      return "success";
    case "aborted":
      return "destructive";
  }
}

function profileName(
  pubkey: string,
  profiles: Record<string, UserProfileSummary>,
): string {
  return (
    profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey)
  );
}

function LifecycleIcon({ lifecycle }: { lifecycle: MeetingLifecycle }) {
  if (lifecycle === "closed") {
    return <CheckCircle2 className="mr-1 size-3" />;
  }
  if (lifecycle === "aborted") {
    return <XCircle className="mr-1 size-3" />;
  }
  return <ShieldCheck className="mr-1 size-3" />;
}

export function MeetingHeader({
  abortDisabled,
  boardControl,
  canAbort,
  onAbort,
  onCopyLink,
  onOpenActivity,
  onOpenOutcome,
  onOpenParticipants,
  onOpenSource,
  profiles,
  snapshot,
}: {
  abortDisabled: boolean;
  boardControl: ReactNode;
  canAbort: boolean;
  onAbort: () => void;
  onCopyLink: () => void;
  onOpenActivity: () => void;
  onOpenOutcome: () => void;
  onOpenParticipants: () => void;
  onOpenSource?: () => void;
  profiles: Record<string, UserProfileSummary>;
  snapshot: MeetingSnapshot;
}) {
  const host = snapshot.participants.find(
    (participant) => participant.pubkey === snapshot.moderatorPubkey,
  );
  const hostName = profileName(snapshot.moderatorPubkey, profiles);
  const hostProfile = profiles[snapshot.moderatorPubkey.toLowerCase()];
  const visibleParticipants = snapshot.participants.slice(0, 3);
  const remainingParticipants =
    snapshot.participants.length - visibleParticipants.length;
  const terminal =
    snapshot.lifecycle === "closed" || snapshot.lifecycle === "aborted";
  const abortTestId =
    snapshot.lifecycle === "finalizing_actions"
      ? "meeting-action-abort"
      : "meeting-host-abort";

  return (
    <header
      className="flex h-14 items-center gap-2 px-3 sm:px-5"
      data-tauri-drag-region
      data-testid="meeting-header"
    >
      <div
        aria-hidden="true"
        className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"
        data-testid="meeting-header-icon"
      >
        <Presentation className="size-4" />
      </div>

      <div className="min-w-0 flex-1">
        <h1 className="truncate text-sm font-semibold">{snapshot.title}</h1>
        <p className="hidden truncate text-2xs text-muted-foreground sm:block">
          {snapshot.description || "Moderated Meeting"}
        </p>
      </div>

      <Badge
        className="shrink-0"
        data-testid="meeting-lifecycle-badge"
        variant={lifecycleBadgeVariant(snapshot.lifecycle)}
      >
        <LifecycleIcon lifecycle={snapshot.lifecycle} />
        <span className="hidden md:inline">
          {lifecycleLabel(snapshot.lifecycle)}
        </span>
        <span className="sr-only md:hidden">
          {lifecycleLabel(snapshot.lifecycle)}
        </span>
      </Badge>

      <div
        className="flex min-w-0 shrink-0 items-center gap-2"
        data-testid="meeting-host-identity"
        title={`Host: ${hostName} (${host?.participantType ?? "unknown"})`}
      >
        <span className="sr-only">
          Host {hostName}, {host?.participantType ?? "unknown"}
        </span>
        <ProfileAvatar
          avatarUrl={hostProfile?.avatarUrl ?? null}
          className="size-7 ring-1 ring-border"
          label={hostName}
          testId="meeting-host-avatar"
        />
        <div className="hidden min-w-0 lg:block">
          <p className="max-w-28 truncate text-xs font-medium">{hostName}</p>
          <p className="flex items-center gap-1 text-2xs capitalize text-muted-foreground">
            {host?.participantType === "agent" ? (
              <Bot className="size-3" />
            ) : (
              <UserRound className="size-3" />
            )}
            {host?.participantType ?? "unknown"} host
          </p>
        </div>
      </div>

      <Button
        aria-label={`View ${snapshot.participants.length} Meeting participants`}
        className="shrink-0 px-2"
        data-testid="meeting-participants-trigger"
        onClick={onOpenParticipants}
        size="sm"
        title="View Meeting participants"
        variant="ghost"
      >
        <span
          aria-hidden="true"
          className="hidden items-center py-px -space-x-1.5 sm:flex"
        >
          {visibleParticipants.map((participant, index) => {
            const name = profileName(participant.pubkey, profiles);
            const profile = profiles[participant.pubkey.toLowerCase()];
            return (
              <span
                className="relative inline-flex"
                key={participant.pubkey}
                style={{ zIndex: visibleParticipants.length - index }}
              >
                <ProfileAvatar
                  avatarUrl={profile?.avatarUrl ?? null}
                  className="size-6 ring-2 ring-background"
                  label={name}
                  testId={`meeting-header-participant-${participant.pubkey}`}
                />
              </span>
            );
          })}
        </span>
        {remainingParticipants > 0 ? (
          <span className="hidden text-2xs sm:inline">
            +{remainingParticipants}
          </span>
        ) : null}
        <UsersRound className="size-4 sm:hidden" />
        <span className="text-xs">{snapshot.participants.length}</span>
      </Button>

      {boardControl}

      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            aria-label="More Meeting actions"
            className="shrink-0"
            data-testid="meeting-more-trigger"
            size="icon"
            title="More Meeting actions"
            variant="ghost"
          >
            <MoreHorizontal className="size-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" data-testid="meeting-more-menu">
          <DropdownMenuLabel>Meeting</DropdownMenuLabel>
          <DropdownMenuItem
            data-testid="meeting-menu-participants"
            onSelect={onOpenParticipants}
          >
            <UsersRound />
            View participants
          </DropdownMenuItem>
          {onOpenSource ? (
            <DropdownMenuItem
              data-testid="meeting-menu-source"
              onSelect={onOpenSource}
            >
              <ExternalLink />
              Open source context
            </DropdownMenuItem>
          ) : null}
          <DropdownMenuItem
            data-testid="meeting-menu-activity"
            onSelect={onOpenActivity}
          >
            <History />
            Meeting activity
          </DropdownMenuItem>
          <DropdownMenuItem
            data-testid="meeting-menu-copy-link"
            onSelect={onCopyLink}
          >
            <Link />
            Copy Meeting link
          </DropdownMenuItem>
          {terminal ? (
            <DropdownMenuItem
              data-testid="meeting-menu-outcome"
              onSelect={onOpenOutcome}
            >
              <FileCheck2 />
              View Meeting outcome
            </DropdownMenuItem>
          ) : null}
          {canAbort ? (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                data-testid={abortTestId}
                disabled={abortDisabled}
                onSelect={onAbort}
              >
                <OctagonX />
                Abort meeting…
              </DropdownMenuItem>
            </>
          ) : null}
        </DropdownMenuContent>
      </DropdownMenu>
    </header>
  );
}
