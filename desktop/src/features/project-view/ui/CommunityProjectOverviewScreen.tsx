import * as React from "react";
import {
  AlertCircle,
  ArrowRight,
  Boxes,
  CircleDot,
  Flag,
  GitBranch,
  LayoutDashboard,
  Map as MapIcon,
  RefreshCw,
  ShieldCheck,
  UserRoundX,
  WifiOff,
} from "lucide-react";

import { useMyRelayMembershipLookupQuery } from "@/features/community-members/hooks";
import { useActiveCommunityIcon } from "@/features/communities/useCommunityIcons";
import { useCommunityContinueTarget } from "@/features/communities/useCommunityContinueTarget";
import { useCommunities } from "@/features/communities/useCommunities";
import {
  useProjectViewLiveSync,
  useProjectViewQuery,
} from "@/features/project-view/hooks";
import {
  countProjectViewFocus,
  formatProjectViewTerm,
  projectViewObjectDescription,
  projectViewObjectPriority,
  projectViewObjectStatus,
  projectViewObjectTitle,
} from "@/features/project-view/model";
import {
  CommunityOverviewHeader,
  CommunityOverviewLoading,
  CommunityOverviewState,
} from "@/features/project-view/ui/CommunityProjectOverviewChrome";
import { ProjectRoleCard } from "@/features/project-view/ui/ProjectRoleCard";
import { useProjectViewActors } from "@/features/project-view/useProjectViewActors";
import type {
  ProjectRoleAssignment,
  ProjectRoleDefinition,
  ProjectView,
  ProjectViewLoadResult,
  ProjectViewObject,
  ProjectViewObjectOf,
  ProjectViewRoleContinuity,
} from "@/shared/api/tauriProjectView";
import { isProjectViewIntegrityError } from "@/shared/api/tauriProjectView";
import {
  isRelayConnectionDegraded,
  useRelayConnection,
} from "@/shared/api/useRelayConnection";
import { useFeatureEnabled } from "@/shared/features";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

type CommunityProjectOverviewScreenProps = {
  onOpenChannel: (channelId: string) => void;
  onOpenExperiments: () => void;
  onOpenFullView: () => void;
  onOpenInbox: () => void;
  onOpenObject: (objectId: string) => void;
};

type AttentionItem = {
  detail: string;
  label: string;
  objectId: string;
  tone: "warning" | "danger";
};

function FocusMetric({
  icon,
  label,
  value,
}: {
  icon: React.ReactNode;
  label: string;
  value: number;
}) {
  return (
    <div className="flex items-center gap-2 rounded-xl border border-border/70 bg-background/50 px-2.5 py-2">
      <div className="flex min-w-0 flex-1 items-center gap-1.5 text-muted-foreground">
        {icon}
        <span className="truncate text-2xs font-semibold uppercase tracking-wider">
          {label}
        </span>
      </div>
      <div className="text-lg font-semibold tabular-nums">{value}</div>
    </div>
  );
}

function isCurrentFocusObject(object: ProjectViewObject) {
  const status = projectViewObjectStatus(object);
  return (
    (object.objectType === "plan" && status === "active") ||
    (object.objectType === "stage" && status === "active") ||
    (object.objectType === "requirement" &&
      (status === "ready" || status === "in_progress")) ||
    (object.objectType === "issue" &&
      (status === "open" || status === "in_progress")) ||
    (object.objectType === "work" && status === "in_progress")
  );
}

function priorityRank(object: ProjectViewObject) {
  switch (projectViewObjectPriority(object)) {
    case "urgent":
      return 0;
    case "high":
      return 1;
    case "normal":
      return 2;
    case "low":
      return 3;
    default:
      return 2;
  }
}

function isTerminalAttentionObject(object: ProjectViewObject) {
  const status = projectViewObjectStatus(object);
  switch (object.objectType) {
    case "requirement":
      return status === "satisfied" || status === "withdrawn";
    case "issue":
      return status === "resolved" || status === "closed";
    case "work":
      return (
        status === "submitted" ||
        status === "completed" ||
        status === "cancelled"
      );
    default:
      return true;
  }
}

function ProjectIdentitySummary({
  communityName,
  onOpenFullView,
  onOpenObject,
  result,
}: {
  communityName: string;
  onOpenFullView: () => void;
  onOpenObject: (objectId: string) => void;
  result: Extract<ProjectViewLoadResult, { status: "ready" }>;
}) {
  const profile = result.view.profile;
  return (
    <section
      className="overflow-hidden rounded-2xl border border-border/70 bg-card/60 shadow-xs"
      data-testid="community-project-summary"
    >
      <div className="p-4">
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="outline">Project space</Badge>
          <Badge variant="success">
            <ShieldCheck className="mr-1 h-3 w-3" />
            Verified
          </Badge>
          <span className="text-2xs text-muted-foreground">
            Revision {result.projectRevision}
          </span>
        </div>
        <div className="mt-2 flex flex-col gap-3 sm:flex-row sm:items-start">
          <button
            aria-label={`Inspect Project Profile ${profile.data.name}`}
            className="min-w-0 flex-1 text-left focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
            data-object-id={profile.id}
            onClick={() => onOpenObject(profile.id)}
            type="button"
          >
            <div className="text-xs font-medium text-muted-foreground">
              {communityName}
            </div>
            <h1 className="mt-0.5 text-xl font-semibold tracking-tight">
              {profile.data.name}
            </h1>
            <p className="mt-1 max-w-4xl text-sm leading-relaxed text-muted-foreground">
              {profile.data.positioning}
            </p>
            <p className="mt-1.5 max-w-4xl text-sm leading-relaxed">
              {profile.data.purpose}
            </p>
          </button>
          <Button
            className="shrink-0"
            data-testid="open-full-project-view"
            onClick={onOpenFullView}
            type="button"
          >
            Open full View
            <ArrowRight />
          </Button>
        </div>
      </div>
      {result.view.goals.length > 0 ? (
        <div className="border-t border-border/70 px-4 py-3">
          <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Current direction
          </div>
          <div className="mt-2 grid gap-2 lg:grid-cols-3">
            {result.view.goals.slice(0, 3).map(({ goal }) => (
              <button
                className="rounded-xl border border-border/60 bg-background/40 px-3 py-2 text-left transition-colors hover:border-primary/40 hover:bg-background/70 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                data-object-id={goal.id}
                key={goal.id}
                onClick={() => onOpenObject(goal.id)}
                type="button"
              >
                <div className="text-sm font-semibold">{goal.data.title}</div>
                <p className="mt-0.5 line-clamp-1 text-xs leading-relaxed text-muted-foreground">
                  {goal.data.desiredOutcome}
                </p>
              </button>
            ))}
          </div>
        </div>
      ) : null}
    </section>
  );
}

function ProjectRoleFallbackCard({
  object,
  onSelect,
}: {
  object: ProjectViewObjectOf<"role">;
  onSelect: (objectId: string) => void;
}) {
  return (
    <button
      className="w-full rounded-xl border border-border/70 bg-background/50 p-3 text-left shadow-xs transition-colors hover:border-primary/40 hover:bg-background/80 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
      data-object-id={object.id}
      onClick={() => onSelect(object.id)}
      type="button"
    >
      <div className="flex flex-wrap items-center gap-1.5">
        <Badge variant="outline">Role</Badge>
        <Badge variant={object.data.active ? "success" : "secondary"}>
          {object.data.active ? "Active" : "Inactive"}
        </Badge>
      </div>
      <h3 className="mt-2 text-sm font-semibold">{object.data.name}</h3>
      <p className="mt-1 line-clamp-2 text-xs leading-relaxed text-muted-foreground">
        {object.data.purpose}
      </p>
      <p className="mt-3 border-t border-border/60 pt-2 text-xs text-muted-foreground">
        Assignment continuity is unavailable in this projection.
      </p>
    </button>
  );
}

function ProjectRolesSummary({
  actorProfiles,
  currentAssignments,
  currentPubkey,
  definitions,
  onOpenObject,
  roles,
}: {
  actorProfiles: ReturnType<typeof useProjectViewActors>["actorProfiles"];
  currentAssignments: Map<string, ProjectRoleAssignment>;
  currentPubkey?: string;
  definitions: Map<string, ProjectRoleDefinition>;
  onOpenObject: (objectId: string) => void;
  roles: Array<ProjectViewObjectOf<"role">>;
}) {
  const visibleRoles = React.useMemo(
    () =>
      [...roles]
        .filter((role) => role.data.active)
        .sort((left, right) => {
          const leftDefinition = definitions.get(left.id);
          const rightDefinition = definitions.get(right.id);
          const leftAssignment = currentAssignments.get(left.id);
          const rightAssignment = currentAssignments.get(right.id);
          const current = currentPubkey?.toLowerCase();
          const leftIsMine =
            leftAssignment?.memberPubkey.toLowerCase() === current;
          const rightIsMine =
            rightAssignment?.memberPubkey.toLowerCase() === current;
          if (leftIsMine !== rightIsMine) return leftIsMine ? -1 : 1;
          const leftLeader = leftDefinition?.level === "admin";
          const rightLeader = rightDefinition?.level === "admin";
          if (leftLeader !== rightLeader) return leftLeader ? -1 : 1;
          if (Boolean(leftAssignment) !== Boolean(rightAssignment)) {
            return leftAssignment ? 1 : -1;
          }
          return left.data.name.localeCompare(right.data.name);
        })
        .slice(0, 2),
    [currentAssignments, currentPubkey, definitions, roles],
  );

  return (
    <section
      className="rounded-2xl border border-border/70 bg-card/60 p-4 shadow-xs"
      data-testid="community-role-summary"
    >
      <div className="flex items-center gap-2">
        <Flag className="h-4 w-4 text-muted-foreground" />
        <h2 className="text-sm font-semibold">Roles</h2>
        <span className="text-xs text-muted-foreground">
          Responsibility and continuity
        </span>
      </div>
      {visibleRoles.length > 0 ? (
        <div className="mt-3 grid gap-2 sm:grid-cols-2">
          {visibleRoles.map((role) => {
            const definition = definitions.get(role.id);
            return definition ? (
              <ProjectRoleCard
                actorProfiles={actorProfiles}
                currentAssignment={currentAssignments.get(role.id)}
                currentPubkey={currentPubkey}
                definition={definition}
                key={role.id}
                object={role}
                onSelect={onOpenObject}
              />
            ) : (
              <ProjectRoleFallbackCard
                key={role.id}
                object={role}
                onSelect={onOpenObject}
              />
            );
          })}
        </div>
      ) : (
        <div className="mt-3 rounded-xl border border-dashed border-border/70 p-4 text-sm text-muted-foreground">
          No active Project Roles are defined yet.
        </div>
      )}
      {roles.length > visibleRoles.length ? (
        <Button
          className="mt-3"
          onClick={() => onOpenObject(roles[visibleRoles.length].id)}
          size="sm"
          type="button"
          variant="ghost"
        >
          View all {roles.length} Roles
          <ArrowRight />
        </Button>
      ) : null}
    </section>
  );
}

function buildAttentionItems(
  view: ProjectView,
  roleContinuity: ProjectViewRoleContinuity | undefined,
  objects: Iterable<ProjectViewObject>,
  definitions: Map<string, ProjectRoleDefinition>,
  currentAssignments: Map<string, ProjectRoleAssignment>,
): AttentionItem[] {
  const items: AttentionItem[] = [];

  for (const object of objects) {
    const priority = projectViewObjectPriority(object);
    if (
      (priority === "high" || priority === "urgent") &&
      !isTerminalAttentionObject(object)
    ) {
      items.push({
        detail: `${formatProjectViewTerm(priority)} priority · ${formatProjectViewTerm(projectViewObjectStatus(object) ?? "active")}`,
        label: projectViewObjectTitle(object),
        objectId: object.id,
        tone: priority === "urgent" ? "danger" : "warning",
      });
    }
  }

  for (const role of view.roles) {
    const definition = definitions.get(role.id);
    if (definition?.active && !currentAssignments.has(role.id)) {
      items.push({
        detail: "Active Role has no current assignee",
        label: role.data.name,
        objectId: role.id,
        tone: "warning",
      });
    }
  }

  const waitingWork = new Map<string, string>();
  for (const brief of roleContinuity?.briefs ?? []) {
    for (const responsibleWork of brief.responsibleWork) {
      if (responsibleWork.commitment.status === "waiting_for_continuation") {
        waitingWork.set(responsibleWork.workId, responsibleWork.title);
      }
    }
  }
  for (const [workId, title] of waitingWork) {
    items.push({
      detail: "Responsible Work is waiting for continuation",
      label: title,
      objectId: workId,
      tone: "warning",
    });
  }

  const latestCheckpoints = new Map<
    string,
    ProjectViewRoleContinuity["checkpoints"][number]
  >();
  for (const checkpoint of roleContinuity?.checkpoints ?? []) {
    const current = latestCheckpoints.get(checkpoint.roleId);
    if (!current || current.createdAt < checkpoint.createdAt) {
      latestCheckpoints.set(checkpoint.roleId, checkpoint);
    }
  }
  for (const [roleId, checkpoint] of latestCheckpoints) {
    const roleName =
      view.roles.find((role) => role.id === roleId)?.data.name ?? "Role";
    for (const blocker of checkpoint.content.blockers.slice(0, 1)) {
      items.push({
        detail: `${roleName} checkpoint blocker`,
        label: blocker,
        objectId: roleId,
        tone: "danger",
      });
    }
    for (const risk of checkpoint.content.risks.slice(0, 1)) {
      items.push({
        detail: `${roleName} checkpoint risk`,
        label: risk,
        objectId: roleId,
        tone: "warning",
      });
    }
  }

  return items.slice(0, 4);
}

function ReadyCommunityOverview({
  communityName,
  onOpenFullView,
  onOpenObject,
  result,
}: {
  communityName: string;
  onOpenFullView: () => void;
  onOpenObject: (objectId: string) => void;
  result: Extract<ProjectViewLoadResult, { status: "ready" }>;
}) {
  const { roleContinuity, view } = result;
  const { actorProfiles, currentPubkey, objectsById } = useProjectViewActors(
    view,
    roleContinuity,
  );
  const focus = React.useMemo(() => countProjectViewFocus(view), [view]);
  const definitions = React.useMemo(
    () =>
      new Map(
        roleContinuity?.roles.map((definition) => [
          definition.roleId,
          definition,
        ]) ?? [],
      ),
    [roleContinuity],
  );
  const currentAssignments = React.useMemo(
    () =>
      new Map(
        roleContinuity?.assignments
          .filter((assignment) => !assignment.endedAt)
          .map((assignment) => [assignment.roleId, assignment]) ?? [],
      ),
    [roleContinuity],
  );
  const focusObjects = React.useMemo(
    () =>
      [...objectsById.values()]
        .filter(isCurrentFocusObject)
        .sort((left, right) => {
          return (
            priorityRank(left) - priorityRank(right) ||
            projectViewObjectTitle(left).localeCompare(
              projectViewObjectTitle(right),
            )
          );
        })
        .slice(0, 4),
    [objectsById],
  );
  const attentionItems = React.useMemo(
    () =>
      buildAttentionItems(
        view,
        roleContinuity,
        objectsById.values(),
        definitions,
        currentAssignments,
      ),
    [currentAssignments, definitions, objectsById, roleContinuity, view],
  );

  return (
    <div className="grid gap-3" data-testid="community-project-overview">
      <ProjectIdentitySummary
        communityName={communityName}
        onOpenFullView={onOpenFullView}
        onOpenObject={onOpenObject}
        result={result}
      />

      <div className="grid gap-3 xl:grid-cols-2">
        <section
          className="rounded-2xl border border-border/70 bg-card/60 p-3.5 shadow-xs"
          data-testid="community-current-focus"
        >
          <div className="flex items-center gap-2">
            <CircleDot className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">Current focus</h2>
            <span className="text-xs text-muted-foreground">
              Explicit Project states
            </span>
          </div>
          <div className="mt-2.5 grid grid-cols-2 gap-2 lg:grid-cols-4">
            <FocusMetric
              icon={<GitBranch className="h-3.5 w-3.5" />}
              label="Plans"
              value={focus.activePlans}
            />
            <FocusMetric
              icon={<MapIcon className="h-3.5 w-3.5" />}
              label="Stages"
              value={focus.activeStages}
            />
            <FocusMetric
              icon={<CircleDot className="h-3.5 w-3.5" />}
              label="Issues"
              value={focus.openIssues}
            />
            <FocusMetric
              icon={<ShieldCheck className="h-3.5 w-3.5" />}
              label="Work"
              value={focus.inProgressWork}
            />
          </div>
          {focusObjects.length > 0 ? (
            <div className="mt-2 space-y-0.5">
              {focusObjects.map((object) => (
                <button
                  className="flex w-full items-center gap-3 rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-muted/50 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                  data-object-id={object.id}
                  key={object.id}
                  onClick={() => onOpenObject(object.id)}
                  type="button"
                >
                  <span className="min-w-0 flex-1 truncate text-sm font-medium">
                    {projectViewObjectTitle(object)}
                  </span>
                  <Badge variant="outline">
                    {formatProjectViewTerm(
                      projectViewObjectStatus(object) ?? object.objectType,
                    )}
                  </Badge>
                  <ArrowRight className="h-3.5 w-3.5 text-muted-foreground" />
                </button>
              ))}
            </div>
          ) : (
            <p className="mt-3 text-sm text-muted-foreground">
              No active Plan, Stage, Issue, Requirement, or Work is recorded.
            </p>
          )}
        </section>

        <ProjectRolesSummary
          actorProfiles={actorProfiles}
          currentAssignments={currentAssignments}
          currentPubkey={currentPubkey}
          definitions={definitions}
          onOpenObject={onOpenObject}
          roles={view.roles}
        />
      </div>

      <div className="grid gap-3 xl:grid-cols-2">
        {attentionItems.length > 0 ? (
          <section
            className="rounded-2xl border border-amber-500/30 bg-amber-500/5 p-4"
            data-testid="community-needs-attention"
          >
            <div className="flex items-center gap-2">
              <AlertCircle className="h-4 w-4 text-amber-600 dark:text-amber-400" />
              <h2 className="text-sm font-semibold">Needs attention</h2>
              <span className="text-xs text-muted-foreground">
                Explicit risks and continuity gaps
              </span>
            </div>
            <div className="mt-3 space-y-1">
              {attentionItems.map((item) => (
                <button
                  className="flex w-full items-start gap-3 rounded-lg px-2 py-2 text-left transition-colors hover:bg-amber-500/10 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                  key={`${item.objectId}-${item.label}-${item.detail}`}
                  onClick={() => onOpenObject(item.objectId)}
                  type="button"
                >
                  <AlertCircle
                    className={
                      item.tone === "danger"
                        ? "mt-0.5 h-4 w-4 shrink-0 text-destructive"
                        : "mt-0.5 h-4 w-4 shrink-0 text-amber-600 dark:text-amber-400"
                    }
                  />
                  <span className="min-w-0 flex-1">
                    <span className="block text-sm font-medium">
                      {item.label}
                    </span>
                    <span className="mt-0.5 block text-xs text-muted-foreground">
                      {item.detail}
                    </span>
                  </span>
                  <ArrowRight className="mt-0.5 h-3.5 w-3.5 text-muted-foreground" />
                </button>
              ))}
            </div>
          </section>
        ) : (
          <section
            className="rounded-2xl border border-border/70 bg-card/60 p-4 shadow-xs"
            data-testid="community-needs-attention"
          >
            <div className="flex items-center gap-2">
              <ShieldCheck className="h-4 w-4 text-muted-foreground" />
              <h2 className="text-sm font-semibold">Needs attention</h2>
            </div>
            <p className="mt-3 text-sm text-muted-foreground">
              No high-priority open object, vacant verified Role, waiting
              continuation, blocker, or risk is currently recorded.
            </p>
          </section>
        )}

        <section
          className="rounded-2xl border border-border/70 bg-card/60 p-4 shadow-xs"
          data-testid="community-resources"
        >
          <div className="flex items-center gap-2">
            <Boxes className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">Resources</h2>
            <span className="text-xs text-muted-foreground">
              Stable project entry points
            </span>
          </div>
          {view.resources.length > 0 ? (
            <div className="mt-3 grid gap-2 sm:grid-cols-2">
              {view.resources.slice(0, 4).map((resource) => (
                <button
                  className="rounded-xl border border-border/70 bg-background/50 p-3 text-left transition-colors hover:border-primary/40 hover:bg-background/80 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                  data-object-id={resource.id}
                  key={resource.id}
                  onClick={() => onOpenObject(resource.id)}
                  type="button"
                >
                  <Badge variant="outline">
                    {formatProjectViewTerm(resource.data.resourceKind)}
                  </Badge>
                  <h3 className="mt-2 text-sm font-semibold">
                    {resource.data.name}
                  </h3>
                  <p className="mt-1 line-clamp-2 text-xs leading-relaxed text-muted-foreground">
                    {projectViewObjectDescription(resource)}
                  </p>
                </button>
              ))}
            </div>
          ) : (
            <p className="mt-3 text-sm text-muted-foreground">
              No project Resources are registered yet.
            </p>
          )}
        </section>
      </div>

      <footer className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border/70 pt-4 text-2xs text-muted-foreground">
        <span>{result.activeObjectCount} verified objects</span>
        <span>Project revision {result.projectRevision}</span>
        <span>Projection generation {result.projectionGeneration}</span>
        <span>Updated {new Date(result.updatedAt).toLocaleString()}</span>
      </footer>
    </div>
  );
}

export function CommunityProjectOverviewScreen({
  onOpenChannel,
  onOpenExperiments,
  onOpenFullView,
  onOpenInbox,
  onOpenObject,
}: CommunityProjectOverviewScreenProps) {
  const { activeCommunity } = useCommunities();
  const communityIconQuery = useActiveCommunityIcon(activeCommunity?.relayUrl);
  const membershipQuery = useMyRelayMembershipLookupQuery();
  const continueResolution = useCommunityContinueTarget();
  const projectViewEnabled = useFeatureEnabled("projectView");
  const query = useProjectViewQuery({ enabled: projectViewEnabled });
  const relayConnection = useRelayConnection();
  const relayPubkey =
    query.data?.status === "ready" ? query.data.relayPubkey : undefined;
  const snapshotUpdatedAt =
    query.data?.status === "ready" ? query.data.updatedAt : undefined;
  const liveStatus = useProjectViewLiveSync({
    relayPubkey: projectViewEnabled ? relayPubkey : undefined,
    snapshotUpdatedAt,
  });
  const degraded = isRelayConnectionDegraded(relayConnection);
  const fatalError = query.isError && !query.data ? query.error : undefined;
  const fatalErrorMessage =
    fatalError instanceof Error
      ? fatalError.message
      : "The Relay returned an unexpected Project View response.";
  const communityName = activeCommunity?.name ?? "Community";
  const refreshError =
    query.isError && query.data
      ? query.error instanceof Error
        ? query.error.message
        : "The latest Project View snapshot could not be verified."
      : undefined;
  const verifiedRevision =
    query.data?.status === "ready" ? query.data.projectRevision : undefined;
  const syncState: "refreshing" | "stale" | undefined = degraded
    ? "stale"
    : refreshError || liveStatus === "retrying"
      ? "stale"
      : query.data && (query.isFetching || liveStatus === "connecting")
        ? "refreshing"
        : undefined;
  const syncMessage =
    verifiedRevision === undefined || syncState === undefined
      ? undefined
      : degraded
        ? `Showing verified project revision ${verifiedRevision}. It may be stale while the Relay connection recovers.`
        : refreshError
          ? `Showing verified project revision ${verifiedRevision}. The latest refresh failed: ${refreshError}`
          : liveStatus === "retrying"
            ? `Showing verified project revision ${verifiedRevision} while the live update subscription reconnects.`
            : `Keeping verified project revision ${verifiedRevision} visible while a new complete snapshot is verified.`;
  const handleContinue = React.useCallback(() => {
    if (continueResolution.target.kind === "channel") {
      onOpenChannel(continueResolution.target.channelId);
      return;
    }
    onOpenInbox();
  }, [continueResolution.target, onOpenChannel, onOpenInbox]);

  return (
    <div className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
      <CommunityOverviewHeader
        communityIconUrl={communityIconQuery.data}
        communityName={communityName}
        communityRole={membershipQuery.data?.membership?.role}
        continueStatus={continueResolution.status}
        continueTarget={continueResolution.target}
        onContinue={handleContinue}
        projectStatus={
          <>
            {degraded && relayPubkey ? (
              <Badge variant="warning">
                <WifiOff className="mr-1 h-3 w-3" />
                Offline · may be stale
              </Badge>
            ) : null}
            {!degraded &&
            relayPubkey &&
            (query.isFetching || liveStatus === "connecting") ? (
              <Badge variant="secondary">
                <RefreshCw className="mr-1 h-3 w-3 animate-spin" />
                Syncing
              </Badge>
            ) : null}
            {!degraded && liveStatus === "retrying" ? (
              <Badge variant="warning">Live sync retrying</Badge>
            ) : null}
          </>
        }
      />

      <main className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-7xl space-y-3 p-3 pb-12 sm:p-5">
          {!projectViewEnabled ? (
            <CommunityOverviewState
              action={
                <Button
                  data-testid="enable-project-view"
                  onClick={onOpenExperiments}
                  type="button"
                >
                  Open Experiments
                  <ArrowRight />
                </Button>
              }
              description="Project Profile, current focus, Roles, and resources are hidden while this preview is off. Community navigation and your last work position remain available."
              icon={<LayoutDashboard className="h-5 w-5" />}
              testId="community-project-view-disabled"
              title="Project View preview is disabled"
            />
          ) : null}
          {projectViewEnabled && query.isPending ? (
            <CommunityOverviewLoading />
          ) : null}
          {projectViewEnabled &&
          fatalError &&
          isProjectViewIntegrityError(fatalError) ? (
            <CommunityOverviewState
              action={
                <Button
                  disabled={query.isFetching}
                  onClick={() => void query.refetch()}
                  type="button"
                >
                  <RefreshCw
                    className={query.isFetching ? "animate-spin" : undefined}
                  />
                  Verify again
                </Button>
              }
              description="Buzz rejected the snapshot because its verified metadata and assembled objects do not describe one safe, consistent View. No partial project summary is being shown."
              diagnostic={fatalErrorMessage}
              icon={<AlertCircle className="h-5 w-5" />}
              testId="community-project-integrity-failure"
              title="Project View integrity check failed"
            />
          ) : null}
          {projectViewEnabled &&
          fatalError &&
          !isProjectViewIntegrityError(fatalError) ? (
            <CommunityOverviewState
              action={
                <Button
                  disabled={query.isFetching}
                  onClick={() => void query.refetch()}
                  type="button"
                >
                  <RefreshCw
                    className={query.isFetching ? "animate-spin" : undefined}
                  />
                  Retry
                </Button>
              }
              description={fatalErrorMessage}
              icon={<AlertCircle className="h-5 w-5" />}
              testId="community-project-error"
              title="Project View could not be verified"
            />
          ) : null}
          {projectViewEnabled && query.data?.status === "unsupported" ? (
            <CommunityOverviewState
              description="This Relay does not advertise the Project View protocol. Inbox, Channels, Projects, and the rest of this Community remain available."
              icon={<MapIcon className="h-5 w-5" />}
              testId="community-project-unsupported"
              title="Project View is not supported by this Relay"
            />
          ) : null}
          {projectViewEnabled && query.data?.status === "forbidden" ? (
            <CommunityOverviewState
              description="Your current identity cannot read this Community's Project View. Other Community capabilities keep their own access rules."
              icon={<UserRoundX className="h-5 w-5" />}
              testId="community-project-forbidden"
              title="Project View access denied"
            />
          ) : null}
          {projectViewEnabled && query.data?.status === "uninitialized" ? (
            <CommunityOverviewState
              action={
                <Button
                  data-testid="open-project-view-v3-setup"
                  onClick={onOpenFullView}
                  type="button"
                >
                  Open v3 setup guide
                  <ArrowRight />
                </Button>
              }
              description="Desktop did not find an initialized canonical Project View. A Relay operator must prepare the v3 bootstrap before the Community owner initializes its Project Profile, Goal, and governance."
              icon={<Flag className="h-5 w-5" />}
              testId="community-project-uninitialized"
              title="Project View v3 requires owner initialization"
            />
          ) : null}
          {projectViewEnabled &&
          query.data?.status === "ready" &&
          syncMessage ? (
            <div
              className={
                syncState === "stale"
                  ? "flex items-start gap-2 rounded-xl border border-amber-500/40 bg-amber-500/10 px-3 py-2.5"
                  : "flex items-start gap-2 rounded-xl border border-border/70 bg-muted/30 px-3 py-2.5"
              }
              data-testid="community-project-sync-state"
              role="status"
            >
              {syncState === "stale" ? (
                <WifiOff className="mt-0.5 h-4 w-4 shrink-0 text-amber-600 dark:text-amber-400" />
              ) : (
                <RefreshCw className="mt-0.5 h-4 w-4 shrink-0 animate-spin text-muted-foreground" />
              )}
              <p className="text-xs leading-relaxed text-muted-foreground">
                {syncMessage}
              </p>
            </div>
          ) : null}
          {projectViewEnabled && query.data?.status === "ready" ? (
            <ReadyCommunityOverview
              communityName={communityName}
              onOpenFullView={onOpenFullView}
              onOpenObject={onOpenObject}
              result={query.data}
            />
          ) : null}
        </div>
      </main>
    </div>
  );
}
