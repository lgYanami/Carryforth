import type * as React from "react";
import {
  ArrowRight,
  Inbox,
  LayoutDashboard,
  MessageSquareText,
} from "lucide-react";

import type { CommunityContinueTarget } from "@/features/communities/communityContinueTarget";
import { TopChromeInsetHeader } from "@/shared/layout/TopChromeInsetHeader";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Skeleton } from "@/shared/ui/skeleton";

type CommunityOverviewHeaderProps = {
  communityIconUrl?: string | null;
  communityName: string;
  communityRole?: string;
  continueStatus: "pending" | "ready" | "invalid";
  continueTarget: CommunityContinueTarget;
  onContinue: () => void;
  projectStatus?: React.ReactNode;
};

export function CommunityOverviewHeader({
  communityIconUrl,
  communityName,
  communityRole,
  continueStatus,
  continueTarget,
  onContinue,
  projectStatus,
}: CommunityOverviewHeaderProps) {
  const continueTitle =
    continueStatus === "pending"
      ? "The last channel is still being verified. Open Inbox now."
      : continueTarget.label;

  return (
    <TopChromeInsetHeader flush>
      <header
        className="flex h-12 items-center gap-2 px-3 sm:gap-3 sm:px-5"
        data-tauri-drag-region
        data-testid="community-space-header"
      >
        {communityIconUrl ? (
          <img
            alt=""
            className="h-6 w-6 shrink-0 rounded-lg object-cover"
            src={communityIconUrl}
          />
        ) : (
          <LayoutDashboard className="h-4 w-4 text-muted-foreground" />
        )}
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold">{communityName}</div>
          <div className="hidden text-2xs text-muted-foreground sm:block">
            Community · Project space
          </div>
        </div>
        {communityRole ? (
          <Badge className="hidden capitalize sm:inline-flex" variant="outline">
            {communityRole}
          </Badge>
        ) : null}
        {projectStatus}
        <Button
          aria-label={continueTarget.label}
          className="max-w-56 shrink-0"
          data-destination-status={continueStatus}
          data-testid="community-continue-work"
          onClick={onContinue}
          size="sm"
          title={continueTitle}
          type="button"
          variant="outline"
        >
          {continueTarget.kind === "channel" ? (
            <MessageSquareText />
          ) : (
            <Inbox />
          )}
          <span className="truncate">{continueTarget.label}</span>
          <ArrowRight className="hidden sm:block" />
        </Button>
      </header>
    </TopChromeInsetHeader>
  );
}

type OverviewStateProps = {
  description: string;
  title: string;
  action?: React.ReactNode;
  diagnostic?: string;
  icon: React.ReactNode;
  testId: string;
};

export function CommunityOverviewState({
  action,
  description,
  diagnostic,
  icon,
  testId,
  title,
}: OverviewStateProps) {
  return (
    <section
      className="rounded-2xl border border-border/70 bg-card/60 p-4 shadow-xs"
      data-testid={testId}
    >
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start">
        <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-border/70 bg-muted/30 text-muted-foreground">
          {icon}
        </div>
        <div className="min-w-0 flex-1">
          <h2 className="text-base font-semibold">{title}</h2>
          <p className="mt-1 max-w-3xl text-sm leading-relaxed text-muted-foreground">
            {description}
          </p>
        </div>
        {action ? <div className="shrink-0">{action}</div> : null}
      </div>
      {diagnostic ? (
        <details className="mt-3 rounded-lg border border-border/70 bg-muted/20 px-3 py-2 text-left">
          <summary className="cursor-pointer text-xs font-medium">
            Diagnostic detail
          </summary>
          <code className="mt-2 block whitespace-pre-wrap break-words text-xs text-muted-foreground">
            {diagnostic}
          </code>
        </details>
      ) : null}
    </section>
  );
}

export function CommunityOverviewLoading() {
  return (
    <div
      aria-busy="true"
      className="grid gap-3"
      data-testid="community-project-loading"
      role="status"
    >
      <span className="sr-only">
        Reading and verifying the Community project snapshot.
      </span>
      <section className="rounded-2xl border border-border/70 bg-card/60 p-4">
        <Skeleton className="h-5 w-28 rounded-full" />
        <Skeleton className="mt-3 h-7 w-64 max-w-full" />
        <Skeleton className="mt-2 h-4 w-full max-w-3xl" />
      </section>
      <div className="grid gap-3 xl:grid-cols-2">
        {["focus", "roles"].map((item) => (
          <section
            className="rounded-2xl border border-border/70 bg-card/60 p-4"
            key={item}
          >
            <Skeleton className="h-5 w-32" />
            <div className="mt-3 grid grid-cols-2 gap-2">
              <Skeleton className="h-20 rounded-xl" />
              <Skeleton className="h-20 rounded-xl" />
            </div>
          </section>
        ))}
      </div>
    </div>
  );
}
