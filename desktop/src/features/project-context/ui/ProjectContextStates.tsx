import {
  AlertTriangle,
  Ban,
  CircleOff,
  RefreshCw,
  ShieldAlert,
  TimerReset,
} from "lucide-react";

import type { ProjectContextFailureKind } from "@/features/project-context/state";
import { Button } from "@/shared/ui/button";
import { Skeleton } from "@/shared/ui/skeleton";

type StateProps = {
  description: string;
  icon: React.ReactNode;
  testId: string;
  title: string;
  diagnostic?: string;
  onRetry?: () => void;
  retrying?: boolean;
};

function ProjectContextState({
  description,
  diagnostic,
  icon,
  onRetry,
  retrying = false,
  testId,
  title,
}: StateProps) {
  return (
    <main
      className="flex min-h-0 flex-1 items-center justify-center p-6"
      data-testid={testId}
    >
      <div className="max-w-lg text-center">
        <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-2xl border border-border/70 bg-muted/30 text-muted-foreground">
          {icon}
        </div>
        <h1 className="mt-4 text-lg font-semibold">{title}</h1>
        <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
          {description}
        </p>
        {diagnostic ? (
          <details className="mt-4 rounded-lg border border-border/70 bg-muted/20 px-3 py-2 text-left">
            <summary className="cursor-pointer text-xs font-medium">
              Diagnostic detail
            </summary>
            <code className="mt-2 block whitespace-pre-wrap break-words text-xs text-muted-foreground">
              {diagnostic}
            </code>
          </details>
        ) : null}
        {onRetry ? (
          <Button
            className="mt-4"
            disabled={retrying}
            onClick={onRetry}
            size="sm"
            type="button"
            variant="outline"
          >
            <RefreshCw className={retrying ? "animate-spin" : undefined} />
            {retrying ? "Verifying…" : "Verify again"}
          </Button>
        ) : null}
      </div>
    </main>
  );
}

export function ProjectContextLoadingState() {
  return (
    <main
      aria-busy="true"
      className="min-h-0 flex-1 p-4 sm:p-6"
      data-testid="project-context-loading"
      role="status"
    >
      <span className="sr-only">
        Reading and verifying the complete Project Context snapshot.
      </span>
      <div className="mx-auto grid h-full max-w-6xl gap-4">
        <section className="rounded-2xl border border-border/70 bg-card/60 p-4">
          <Skeleton className="h-4 w-28" />
          <div className="mt-4 flex gap-3">
            <Skeleton className="h-16 flex-1 rounded-xl" />
            <Skeleton className="h-16 flex-1 rounded-xl" />
            <Skeleton className="hidden h-16 flex-1 rounded-xl sm:block" />
          </div>
        </section>
        <Skeleton className="min-h-72 rounded-2xl" />
      </div>
    </main>
  );
}

export function ProjectContextEmptyState() {
  return (
    <ProjectContextState
      description="This verified Context catalog currently has no active Context Edges. That does not imply the Project is missing context."
      icon={<CircleOff className="h-5 w-5" />}
      testId="project-context-empty"
      title="No Context Edges recorded yet"
    />
  );
}

export function ProjectContextFailureState({
  diagnostic,
  kind,
  onRetry,
  retrying,
}: {
  diagnostic: string;
  kind: ProjectContextFailureKind;
  onRetry: () => void;
  retrying: boolean;
}) {
  const content = {
    unsupported: {
      description:
        "This Relay or Project does not provide the Project View and Document capabilities required by Project Context.",
      icon: <CircleOff className="h-5 w-5" />,
      title: "Project Context is not supported",
    },
    restricted: {
      description:
        "Your current identity is not permitted to read this Community's Project Context.",
      icon: <Ban className="h-5 w-5" />,
      title: "Project Context access denied",
    },
    unavailable: {
      description:
        "No verified Project Context projection is currently available. An advertised capability alone is not treated as an empty catalog.",
      icon: <TimerReset className="h-5 w-5" />,
      title: "Project Context is not available yet",
    },
    snapshot_conflict: {
      description:
        "The Project Context changed while Desktop was assembling it. No mixed or partial snapshot is being shown.",
      icon: <RefreshCw className="h-5 w-5" />,
      title: "Project Context changed while loading",
    },
    verification_failed: {
      description:
        "Desktop rejected the projection because its signed sources do not form one safe, consistent Context result. No partial graph is being shown.",
      icon: <ShieldAlert className="h-5 w-5" />,
      title: "Project Context verification failed",
    },
    error: {
      description:
        "Desktop could not complete the trusted Project Context read boundary.",
      icon: <AlertTriangle className="h-5 w-5" />,
      title: "Project Context could not be read",
    },
  }[kind];

  return (
    <ProjectContextState
      description={content.description}
      diagnostic={diagnostic}
      icon={content.icon}
      onRetry={onRetry}
      retrying={retrying}
      testId={`project-context-${kind.replace("_", "-")}`}
      title={content.title}
    />
  );
}
