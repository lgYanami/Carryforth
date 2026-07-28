import {
  AlertTriangle,
  Ban,
  Boxes,
  CircleOff,
  LoaderCircle,
  RefreshCw,
} from "lucide-react";

import { Button } from "@/shared/ui/button";

type StateProps = {
  description: string;
  icon: React.ReactNode;
  title: string;
  action?: React.ReactNode;
};

function ProjectViewState({ action, description, icon, title }: StateProps) {
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
        {action ? <div className="mt-4">{action}</div> : null}
      </div>
    </div>
  );
}

export function ProjectViewLoadingState() {
  return (
    <ProjectViewState
      description="Reading and verifying the Relay-authored project snapshot."
      icon={<LoaderCircle className="h-5 w-5 animate-spin" />}
      title="Loading View"
    />
  );
}

export function ProjectViewUnsupportedState() {
  return (
    <ProjectViewState
      description="This Relay does not advertise the Project View protocol. Existing Projects and other Buzz features are unaffected."
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

export function ProjectViewUninitializedState() {
  return (
    <ProjectViewState
      description="This Community supports View, but its project profile and first goal have not been initialized yet. Initialization will arrive in the next delivery slice."
      icon={<Boxes className="h-5 w-5" />}
      title="This View has not been initialized"
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
