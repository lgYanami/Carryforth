import {
  AlertTriangle,
  Ban,
  CircleOff,
  RefreshCw,
  ShieldX,
} from "lucide-react";

import { Button } from "@/shared/ui/button";
import { Skeleton } from "@/shared/ui/skeleton";

type StateProps = {
  description: string;
  icon: React.ReactNode;
  title: string;
  action?: React.ReactNode;
  diagnostic?: string;
};

function ProjectViewState({
  action,
  description,
  diagnostic,
  icon,
  title,
}: StateProps) {
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center p-8">
      <div className="max-w-md text-center">
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
        {action ? <div className="mt-4">{action}</div> : null}
      </div>
    </div>
  );
}

export function ProjectViewLoadingState() {
  return (
    <main
      aria-busy="true"
      className="min-h-0 flex-1 overflow-hidden"
      data-testid="project-view-loading-skeleton"
      role="status"
    >
      <span className="sr-only">
        Reading and verifying the Relay-authored project snapshot.
      </span>
      <div className="mx-auto max-w-7xl space-y-6 p-3 pb-12 sm:p-5">
        <section className="rounded-2xl border border-border/70 bg-card/60 p-5">
          <Skeleton className="h-5 w-28 rounded-full" />
          <Skeleton className="mt-4 h-7 w-64 max-w-full" />
          <Skeleton className="mt-3 h-4 w-full max-w-3xl" />
          <Skeleton className="mt-2 h-4 w-4/5 max-w-2xl" />
          <div className="mt-5 grid gap-4 border-t border-border/70 pt-4 sm:grid-cols-3">
            {["purpose", "problem", "scope"].map((item) => (
              <div className="space-y-2" key={item}>
                <Skeleton className="h-3 w-16" />
                <Skeleton className="h-4 w-full" />
                <Skeleton className="h-4 w-4/5" />
              </div>
            ))}
          </div>
        </section>
        <section className="grid grid-cols-2 gap-2 lg:grid-cols-4">
          {["plans", "stages", "issues", "work"].map((item) => (
            <div
              className="rounded-xl border border-border/70 bg-card/60 p-3"
              key={item}
            >
              <Skeleton className="h-3 w-20" />
              <Skeleton className="mt-3 h-6 w-10" />
            </div>
          ))}
        </section>
        <section className="space-y-3 rounded-2xl border border-border/70 p-4">
          <Skeleton className="h-5 w-32" />
          <Skeleton className="h-28 w-full rounded-xl" />
          <div className="grid gap-3 lg:grid-cols-2">
            <Skeleton className="h-36 w-full rounded-xl" />
            <Skeleton className="h-36 w-full rounded-xl" />
          </div>
        </section>
      </div>
    </main>
  );
}

export function ProjectViewUnsupportedState() {
  return (
    <ProjectViewState
      description="This Relay does not advertise the Project View protocol. Existing Projects and other Carryforth features are unaffected."
      icon={<CircleOff className="h-5 w-5" />}
      title="View is not supported by this Relay"
    />
  );
}

export function ProjectViewForbiddenState() {
  return (
    <ProjectViewState
      description="Your current identity cannot read this Community's Project View."
      icon={<Ban className="h-5 w-5" />}
      title="View access denied"
    />
  );
}

export function ProjectViewErrorState({
  message,
  onRetry,
  retrying,
}: {
  message: string;
  onRetry: () => void;
  retrying: boolean;
}) {
  return (
    <ProjectViewState
      action={
        <Button disabled={retrying} onClick={onRetry} type="button">
          <RefreshCw className={retrying ? "animate-spin" : undefined} />
          {retrying ? "Retrying…" : "Retry"}
        </Button>
      }
      description={message}
      icon={<AlertTriangle className="h-5 w-5" />}
      title="View could not be verified"
    />
  );
}

export function ProjectViewIntegrityFailureState({
  diagnostic,
  onRetry,
  retrying,
}: {
  diagnostic: string;
  onRetry: () => void;
  retrying: boolean;
}) {
  return (
    <ProjectViewState
      action={
        <Button disabled={retrying} onClick={onRetry} type="button">
          <RefreshCw className={retrying ? "animate-spin" : undefined} />
          {retrying ? "Checking again…" : "Verify again"}
        </Button>
      }
      description="The Relay rejected the snapshot because its verified metadata and assembled objects do not describe one safe, consistent View. No partial project data is being shown."
      diagnostic={diagnostic}
      icon={<ShieldX className="h-5 w-5" />}
      title="View integrity check failed"
    />
  );
}
