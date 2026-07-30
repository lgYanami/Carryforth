import {
  ArrowRightLeft,
  Check,
  Clock3,
  Crown,
  History,
  ShieldAlert,
  UserPlus,
  X,
} from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import type { UserProfileLookup } from "@/features/profile/lib/identity";
import {
  useProjectViewRoleHistory,
  useProjectViewRoleMutation,
} from "@/features/project-view/hooks";
import { ProjectViewActor } from "@/features/project-view/ui/ProjectViewActor";
import {
  EMPTY_ROLE_CONTINUITY_DRAFT,
  ProjectRoleCheckpointDialog,
  ProjectRoleHandoffDialog,
} from "@/features/project-view/ui/ProjectRoleContinuityDialogs";
import { ProjectRoleDirectory } from "@/features/project-view/ui/ProjectRoleDirectory";
import {
  findActiveProjectRoleAssignment,
  formatProjectRoleDateTime,
} from "@/features/project-view/ui/projectRoleFormatting";
import type {
  ProjectRoleDefinition,
  ProjectRoleProposal,
  ProjectViewRoleContinuity,
  ProjectViewRoleMutationIntent,
} from "@/shared/api/tauriProjectView";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";
import { Textarea } from "@/shared/ui/textarea";

function effectiveProposalStatus(proposal: ProjectRoleProposal, now: number) {
  return proposal.status === "open" &&
    new Date(proposal.expiresAt).getTime() <= now
    ? "expired"
    : proposal.status;
}

function memberLabel(pubkey: string, profiles?: UserProfileLookup) {
  const profile = profiles?.[pubkey.toLowerCase()];
  return profile?.displayName ?? profile?.name ?? truncatePubkey(pubkey);
}

function uniqueBy<T>(items: T[], key: (item: T) => string) {
  return [...new Map(items.map((item) => [key(item), item])).values()];
}

export function ProjectRoleInspector({
  actorProfiles,
  continuity,
  currentPubkey,
  definition,
  projectionGeneration,
  projectRevision,
}: {
  actorProfiles?: UserProfileLookup;
  continuity: ProjectViewRoleContinuity;
  currentPubkey?: string;
  definition: ProjectRoleDefinition;
  projectionGeneration: number;
  projectRevision: number;
}) {
  const mutation = useProjectViewRoleMutation();
  const roleHistory = useProjectViewRoleHistory({
    roleId: definition.roleId,
    projectRevision,
    projectionGeneration,
  });
  const [offerOpen, setOfferOpen] = React.useState(false);
  const [endOpen, setEndOpen] = React.useState(false);
  const [checkpointOpen, setCheckpointOpen] = React.useState(false);
  const [handoffOpen, setHandoffOpen] = React.useState(false);
  const [candidatePubkey, setCandidatePubkey] = React.useState("");
  const [reason, setReason] = React.useState("");
  const [continuityDraft, setContinuityDraft] = React.useState(
    EMPTY_ROLE_CONTINUITY_DRAFT,
  );
  const [renderedAt] = React.useState(Date.now);
  const historyItems =
    roleHistory.data?.pages.flatMap((page) => page.items) ?? [];
  const historicalProposals = historyItems.flatMap((item) =>
    item.entityType === "proposal" ? [item.entity] : [],
  );
  const historicalAssignments = historyItems.flatMap((item) =>
    item.entityType === "assignment" ? [item.entity] : [],
  );
  const historicalCheckpoints = historyItems.flatMap((item) =>
    item.entityType === "checkpoint" ? [item.entity] : [],
  );
  const historicalHandoffs = historyItems.flatMap((item) =>
    item.entityType === "handoff" ? [item.entity] : [],
  );
  const normalizedCurrentPubkey = currentPubkey?.toLowerCase();
  const currentAssignment = findActiveProjectRoleAssignment(
    continuity.assignments,
    definition.roleId,
  );
  const currentBrief = currentAssignment
    ? continuity.briefs.find(
        (brief) =>
          brief.state.status === "assigned" &&
          brief.state.assignmentId === currentAssignment.assignmentId,
      )
    : undefined;
  const viewerMembership = continuity.members.find(
    (member) => member.pubkey.toLowerCase() === normalizedCurrentPubkey,
  );
  const viewerAssignment = continuity.assignments.find(
    (assignment) =>
      !assignment.endedAt &&
      assignment.memberPubkey.toLowerCase() === normalizedCurrentPubkey,
  );
  const viewerRole = viewerAssignment
    ? continuity.roles.find((role) => role.roleId === viewerAssignment.roleId)
    : undefined;
  const isOwner = viewerMembership?.role === "owner";
  const isLeader = viewerRole?.level === "admin";
  const canGovernRole = Boolean(
    isOwner || (isLeader && definition.level === "member"),
  );
  const actingAssignmentId = isOwner
    ? undefined
    : isLeader
      ? viewerAssignment?.assignmentId
      : undefined;
  const proposals = uniqueBy(
    [
      ...continuity.proposals.filter(
        (proposal) => proposal.roleId === definition.roleId,
      ),
      ...historicalProposals,
    ],
    (proposal) => proposal.proposalId,
  ).sort((left, right) => right.createdAt.localeCompare(left.createdAt));
  const history = uniqueBy(
    [
      ...continuity.assignments.filter(
        (assignment) =>
          assignment.roleId === definition.roleId &&
          Boolean(assignment.endedAt),
      ),
      ...historicalAssignments.filter((assignment) =>
        Boolean(assignment.endedAt),
      ),
    ],
    (assignment) => assignment.assignmentId,
  ).sort((left, right) =>
    (right.endedAt ?? "").localeCompare(left.endedAt ?? ""),
  );
  const handoffs = uniqueBy(
    [
      ...continuity.handoffs.filter(
        (handoff) => handoff.roleId === definition.roleId,
      ),
      ...historicalHandoffs,
    ],
    (handoff) => handoff.handoffId,
  ).sort(
    (left, right) =>
      right.projectRevision - left.projectRevision ||
      right.handoffId.localeCompare(left.handoffId),
  );
  const checkpoints = uniqueBy(
    [
      ...continuity.checkpoints.filter(
        (checkpoint) => checkpoint.roleId === definition.roleId,
      ),
      ...historicalCheckpoints,
    ],
    (checkpoint) => checkpoint.checkpointId,
  ).sort(
    (left, right) =>
      right.projectRevision - left.projectRevision ||
      right.checkpointId.localeCompare(left.checkpointId),
  );
  const timeline = [
    ...checkpoints.map((checkpoint) => ({
      id: checkpoint.checkpointId,
      createdAt: checkpoint.createdAt,
      item: checkpoint,
      type: "checkpoint" as const,
    })),
    ...handoffs.map((handoff) => ({
      id: handoff.handoffId,
      createdAt: handoff.createdAt,
      item: handoff,
      type: "handoff" as const,
    })),
  ].sort(
    (left, right) =>
      right.item.projectRevision - left.item.projectRevision ||
      right.id.localeCompare(left.id),
  );
  const viewerIsCurrentAssignee =
    currentAssignment?.memberPubkey.toLowerCase() === normalizedCurrentPubkey;
  const hasOpenViewerProposal = proposals.some(
    (proposal) =>
      effectiveProposalStatus(proposal, renderedAt) === "open" &&
      proposal.candidatePubkey.toLowerCase() === normalizedCurrentPubkey,
  );
  const canRequest =
    Boolean(viewerMembership) &&
    definition.active &&
    !viewerIsCurrentAssignee &&
    !hasOpenViewerProposal;

  async function submit(intent: ProjectViewRoleMutationIntent) {
    try {
      const result = await mutation.mutateAsync(intent);
      if (result.status === "conflict") {
        toast.error(
          "The Project changed before this Role action was applied. The latest state is loading; review it before trying again.",
        );
        return false;
      }
      toast.success("Role state updated");
      return true;
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : "Could not update the Role",
      );
      return false;
    }
  }

  async function submitOffer() {
    if (!candidatePubkey.trim()) return;
    const applied = await submit({
      operation: "offer_role",
      expectedProjectRevision: projectRevision,
      actingAssignmentId,
      roleId: definition.roleId,
      candidatePubkey: candidatePubkey.trim(),
      expiresInHours: 72,
      reason: reason.trim() || undefined,
    });
    if (applied) {
      setOfferOpen(false);
      setCandidatePubkey("");
      setReason("");
    }
  }

  async function submitEnd() {
    if (!currentAssignment) return;
    const applied = await submit({
      operation: "end_assignment",
      expectedProjectRevision: projectRevision,
      actingAssignmentId,
      assignmentId: currentAssignment.assignmentId,
      reason: reason.trim() || undefined,
    });
    if (applied) {
      setEndOpen(false);
      setReason("");
    }
  }

  function lines(value: string) {
    return value
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean);
  }

  function clearContinuityDraft() {
    setContinuityDraft(EMPTY_ROLE_CONTINUITY_DRAFT);
  }

  async function submitCheckpoint() {
    if (!currentAssignment || !continuityDraft.summary.trim()) return;
    const applied = await submit({
      operation: "append_checkpoint",
      expectedProjectRevision: projectRevision,
      basedOnProjectRevision: projectRevision,
      actingAssignmentId: currentAssignment.assignmentId,
      content: {
        summary: continuityDraft.summary.trim(),
        currentFocus: lines(continuityDraft.currentFocus),
        progress: lines(continuityDraft.progress),
        blockers: lines(continuityDraft.blockers),
        risks: lines(continuityDraft.risks),
        openQuestions: lines(continuityDraft.openQuestions),
        nextSteps: lines(continuityDraft.nextSteps),
        references: [],
      },
    });
    if (applied) {
      setCheckpointOpen(false);
      clearContinuityDraft();
    }
  }

  async function submitHandoff() {
    if (!currentAssignment || !continuityDraft.summary.trim()) return;
    const applied = await submit({
      operation: "append_handoff",
      expectedProjectRevision: projectRevision,
      actingAssignmentId: currentAssignment.assignmentId,
      checkpointId: checkpoints[0]?.checkpointId,
      cause: "planned",
      content: {
        summary: continuityDraft.summary.trim(),
        unresolvedItems: lines(continuityDraft.openQuestions),
        references: [],
      },
    });
    if (applied) {
      setHandoffOpen(false);
      clearContinuityDraft();
    }
  }

  return (
    <section
      className="space-y-4 border-t border-border/70 pt-4"
      data-testid="project-role-continuity"
    >
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-xs font-semibold">Role continuity</h3>
        <Badge variant={definition.level === "admin" ? "info" : "outline"}>
          {definition.level === "admin" ? (
            <Crown className="mr-1 h-3 w-3" />
          ) : null}
          {definition.level === "admin" ? "Leader · admin" : "Role · member"}
        </Badge>
        {currentAssignment ? (
          <Badge variant="success">Assigned</Badge>
        ) : (
          <Badge variant="warning">Vacant</Badge>
        )}
      </div>

      <div className="rounded-xl border border-border/70 bg-muted/20 p-3">
        <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
          Current tenure
        </div>
        {currentAssignment ? (
          <div className="mt-2 space-y-2">
            <ProjectViewActor
              currentPubkey={currentPubkey}
              profiles={actorProfiles}
              pubkey={currentAssignment.memberPubkey}
            />
            <div className="text-xs text-muted-foreground">
              Since {formatProjectRoleDateTime(currentAssignment.startedAt)}
            </div>
            {currentAssignment.replacementRequestedAt ? (
              <div className="rounded-lg border border-amber-500/40 bg-amber-500/10 px-2.5 py-2 text-xs">
                Replacement requested
                {currentAssignment.replacementRequestReason
                  ? ` · ${currentAssignment.replacementRequestReason}`
                  : ""}
              </div>
            ) : null}
            {currentAssignment.unableReportedAt ? (
              <div className="rounded-lg border border-destructive/30 bg-destructive/10 px-2.5 py-2 text-xs">
                Assignee reported that it cannot continue
                {currentAssignment.unableReportReason
                  ? ` · ${currentAssignment.unableReportReason}`
                  : ""}
              </div>
            ) : null}
          </div>
        ) : (
          <p className="mt-2 text-xs text-muted-foreground">
            This responsibility is available for a new Assignment.
          </p>
        )}
      </div>

      {currentBrief ? (
        <div
          className="rounded-xl border border-border/70 bg-muted/20 p-3"
          data-testid="project-role-brief"
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Verified Role Brief
            </div>
            <Badge variant="outline">
              Revision {currentBrief.projectRevision}
            </Badge>
          </div>
          <div className="mt-2 text-sm font-medium">
            {currentBrief.project.name}
          </div>
          <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
            {currentBrief.project.purpose}
          </p>
          <div className="mt-3 text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Goals
          </div>
          <ul className="mt-1.5 space-y-1 text-xs">
            {currentBrief.project.goals.map((goal) => (
              <li key={goal.title}>
                <span className="font-medium">{goal.title}</span>
                <span className="text-muted-foreground">
                  {" "}
                  · {goal.desiredOutcome}
                </span>
              </li>
            ))}
          </ul>
          <ProjectRoleDirectory
            actorProfiles={actorProfiles}
            currentPubkey={currentPubkey}
            directory={currentBrief.roleDirectory}
          />
          <div className="mt-3 text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Responsible Work
          </div>
          {currentBrief.responsibleWork.length === 0 ? (
            <p className="mt-1.5 text-xs text-muted-foreground">
              No non-terminal Work is assigned to this Role.
            </p>
          ) : (
            <ul className="mt-1.5 space-y-2 text-xs">
              {currentBrief.responsibleWork.map((work) => (
                <li
                  className="rounded-lg border border-border/70 px-2.5 py-2"
                  key={work.workId}
                >
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <span className="font-medium">{work.title}</span>
                    <Badge
                      variant={
                        work.commitment.status === "committed"
                          ? "success"
                          : "warning"
                      }
                    >
                      {work.commitment.status === "committed"
                        ? "Committed"
                        : "Waiting for continuation"}
                    </Badge>
                  </div>
                  <div className="mt-1 capitalize text-muted-foreground">
                    {work.status.replaceAll("_", " ")}
                  </div>
                </li>
              ))}
            </ul>
          )}
          {currentBrief.relatedObjects.length > 0 ? (
            <p className="mt-3 text-xs text-muted-foreground">
              {currentBrief.relatedObjects.length} related Issue/Work{" "}
              {currentBrief.relatedObjects.length === 1 ? "object" : "objects"}
            </p>
          ) : null}
          {currentBrief.latestCheckpoint ? (
            <div
              className="mt-3 rounded-lg border border-border/70 px-2.5 py-2"
              data-testid="project-role-latest-checkpoint"
            >
              <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                Latest Checkpoint
              </div>
              <p className="mt-1 text-xs font-medium">
                {currentBrief.latestCheckpoint.content.summary}
              </p>
              {currentBrief.latestCheckpoint.content.blockers.length > 0 ? (
                <p className="mt-1 text-xs text-destructive">
                  Blocked ·{" "}
                  {currentBrief.latestCheckpoint.content.blockers.join(" · ")}
                </p>
              ) : null}
              {currentBrief.latestCheckpoint.content.risks.length > 0 ? (
                <p className="mt-1 text-xs text-muted-foreground">
                  Risks ·{" "}
                  {currentBrief.latestCheckpoint.content.risks.join(" · ")}
                </p>
              ) : null}
              {currentBrief.latestCheckpoint.content.nextSteps.length > 0 ? (
                <p className="mt-1 text-xs text-muted-foreground">
                  Next ·{" "}
                  {currentBrief.latestCheckpoint.content.nextSteps.join(" · ")}
                </p>
              ) : null}
            </div>
          ) : (
            <p className="mt-3 text-xs text-muted-foreground">
              No Role Checkpoint has been recorded yet.
            </p>
          )}
          <p className="mt-2 text-2xs text-muted-foreground">
            Projection generation {currentBrief.projectionGeneration} · built
            from the same verified projection model used by assigned Agents.
          </p>
        </div>
      ) : null}

      {canGovernRole || canRequest ? (
        <div className="flex flex-wrap gap-2">
          {canRequest ? (
            <Button
              data-testid="project-role-request"
              disabled={mutation.isPending}
              onClick={() =>
                void submit({
                  operation: "request_role",
                  expectedProjectRevision: projectRevision,
                  actingAssignmentId: viewerAssignment?.assignmentId,
                  roleId: definition.roleId,
                  expiresInHours: 72,
                })
              }
              size="sm"
              type="button"
              variant="outline"
            >
              <UserPlus />
              Request Role
            </Button>
          ) : null}
          {canGovernRole && definition.active ? (
            <Button
              data-testid="project-role-offer"
              disabled={mutation.isPending}
              onClick={() => {
                setReason("");
                setOfferOpen(true);
              }}
              size="sm"
              type="button"
              variant="outline"
            >
              {currentAssignment ? <ArrowRightLeft /> : <UserPlus />}
              {currentAssignment ? "Replace" : "Assign"}
            </Button>
          ) : null}
          {canGovernRole && definition.active ? (
            <Button
              data-testid="project-role-end"
              disabled={
                !currentAssignment ||
                viewerIsCurrentAssignee ||
                mutation.isPending
              }
              onClick={() => {
                setReason("");
                setEndOpen(true);
              }}
              size="sm"
              type="button"
              variant="outline"
            >
              <ShieldAlert />
              End tenure
            </Button>
          ) : null}
        </div>
      ) : null}
      {viewerIsCurrentAssignee ? (
        <div className="flex flex-wrap gap-2">
          <Button
            data-testid="project-role-checkpoint"
            disabled={mutation.isPending}
            onClick={() => {
              clearContinuityDraft();
              setCheckpointOpen(true);
            }}
            size="sm"
            type="button"
            variant="outline"
          >
            <Clock3 />
            Add Checkpoint
          </Button>
          <Button
            data-testid="project-role-handoff"
            disabled={mutation.isPending}
            onClick={() => {
              clearContinuityDraft();
              setHandoffOpen(true);
            }}
            size="sm"
            type="button"
            variant="outline"
          >
            <ArrowRightLeft />
            Add Handoff note
          </Button>
        </div>
      ) : null}

      <div>
        <div className="flex items-center gap-2">
          <Clock3 className="h-3.5 w-3.5 text-muted-foreground" />
          <h4 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Proposals
          </h4>
        </div>
        {proposals.length === 0 ? (
          <p className="mt-2 text-xs text-muted-foreground">
            No proposals yet.
          </p>
        ) : (
          <div className="mt-2 space-y-2">
            {proposals.map((proposal) => {
              const status = effectiveProposalStatus(proposal, renderedAt);
              const isCandidate =
                proposal.candidatePubkey.toLowerCase() ===
                normalizedCurrentPubkey;
              const isCreator =
                proposal.createdBy.toLowerCase() === normalizedCurrentPubkey;
              return (
                <div
                  className="rounded-lg border border-border/70 p-2.5"
                  key={proposal.proposalId}
                >
                  <div className="flex flex-wrap items-center gap-1.5">
                    <Badge variant="outline">{proposal.proposalType}</Badge>
                    <Badge
                      variant={
                        status === "open"
                          ? "warning"
                          : status === "consumed"
                            ? "success"
                            : "secondary"
                      }
                    >
                      {status}
                    </Badge>
                  </div>
                  <div className="mt-2 text-xs">
                    Candidate{" "}
                    <ProjectViewActor
                      compact
                      currentPubkey={currentPubkey}
                      profiles={actorProfiles}
                      pubkey={proposal.candidatePubkey}
                    />
                  </div>
                  <div className="mt-1 text-2xs text-muted-foreground">
                    Expires {formatProjectRoleDateTime(proposal.expiresAt)}
                  </div>
                  {proposal.reason ? (
                    <p className="mt-1 text-xs text-muted-foreground">
                      {proposal.reason}
                    </p>
                  ) : null}
                  {status === "open" ? (
                    <div className="mt-2 flex flex-wrap gap-1.5">
                      {isCandidate && !proposal.candidateAcceptedAt ? (
                        <>
                          <Button
                            disabled={mutation.isPending}
                            onClick={() =>
                              void submit({
                                operation: "accept_proposal",
                                expectedProjectRevision: projectRevision,
                                actingAssignmentId:
                                  viewerAssignment?.assignmentId,
                                proposalId: proposal.proposalId,
                              })
                            }
                            size="sm"
                            type="button"
                            variant="outline"
                          >
                            <Check />
                            Accept
                          </Button>
                          <Button
                            disabled={mutation.isPending}
                            onClick={() =>
                              void submit({
                                operation: "reject_proposal",
                                expectedProjectRevision: projectRevision,
                                actingAssignmentId:
                                  viewerAssignment?.assignmentId,
                                proposalId: proposal.proposalId,
                              })
                            }
                            size="sm"
                            type="button"
                            variant="ghost"
                          >
                            <X />
                            Reject
                          </Button>
                        </>
                      ) : null}
                      {canGovernRole &&
                      proposal.proposalType === "request" &&
                      !proposal.authorizedAt ? (
                        <Button
                          disabled={mutation.isPending}
                          onClick={() =>
                            void submit({
                              operation: "authorize_proposal",
                              expectedProjectRevision: projectRevision,
                              actingAssignmentId,
                              proposalId: proposal.proposalId,
                            })
                          }
                          size="sm"
                          type="button"
                          variant="outline"
                        >
                          <Check />
                          Authorize
                        </Button>
                      ) : null}
                      {isCreator ? (
                        <Button
                          disabled={mutation.isPending}
                          onClick={() =>
                            void submit({
                              operation: "withdraw_proposal",
                              expectedProjectRevision: projectRevision,
                              actingAssignmentId:
                                viewerAssignment?.assignmentId,
                              proposalId: proposal.proposalId,
                            })
                          }
                          size="sm"
                          type="button"
                          variant="ghost"
                        >
                          Withdraw
                        </Button>
                      ) : null}
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        )}
      </div>

      <div>
        <div className="flex items-center gap-2">
          <History className="h-3.5 w-3.5 text-muted-foreground" />
          <h4 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Tenure history
          </h4>
        </div>
        {history.length === 0 ? (
          <p className="mt-2 text-xs text-muted-foreground">
            No ended tenures.
          </p>
        ) : (
          <div className="mt-2 space-y-2">
            {history.map((assignment) => (
              <div
                className="rounded-lg border border-border/70 p-2.5 text-xs"
                key={assignment.assignmentId}
              >
                <ProjectViewActor
                  compact
                  currentPubkey={currentPubkey}
                  profiles={actorProfiles}
                  pubkey={assignment.memberPubkey}
                />
                <div className="mt-1 text-2xs text-muted-foreground">
                  {formatProjectRoleDateTime(assignment.startedAt)} –{" "}
                  {assignment.endedAt
                    ? formatProjectRoleDateTime(assignment.endedAt)
                    : "current"}{" "}
                  · {assignment.endedReason ?? "ended"}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      <div data-testid="project-role-timeline">
        <div className="flex items-center gap-2">
          <History className="h-3.5 w-3.5 text-muted-foreground" />
          <h4 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Continuity timeline
          </h4>
        </div>
        {timeline.length === 0 ? (
          <p className="mt-2 text-xs text-muted-foreground">
            No Checkpoints or Handoffs yet.
          </p>
        ) : (
          <div className="mt-2 space-y-2">
            {timeline.map((entry) =>
              entry.type === "checkpoint" ? (
                <div
                  className="rounded-lg border border-border/70 p-2.5 text-xs"
                  key={`checkpoint-${entry.id}`}
                >
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <Badge variant="outline">Checkpoint</Badge>
                    <span className="text-2xs text-muted-foreground">
                      {formatProjectRoleDateTime(entry.createdAt)}
                    </span>
                  </div>
                  <p className="mt-1.5 font-medium">
                    {entry.item.content.summary}
                  </p>
                  {entry.item.content.openQuestions.length > 0 ? (
                    <p className="mt-1 text-muted-foreground">
                      Open · {entry.item.content.openQuestions.join(" · ")}
                    </p>
                  ) : null}
                </div>
              ) : (
                <div
                  className="rounded-lg border border-border/70 p-2.5 text-xs"
                  key={`handoff-${entry.id}`}
                >
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <Badge variant="secondary">
                      Handoff · {entry.item.cause}
                    </Badge>
                    <span className="text-2xs text-muted-foreground">
                      {formatProjectRoleDateTime(entry.createdAt)}
                    </span>
                  </div>
                  {entry.item.content.summary ? (
                    <p className="mt-1.5 font-medium">
                      {entry.item.content.summary}
                    </p>
                  ) : null}
                  {entry.item.content.unresolvedItems.length > 0 ? (
                    <p className="mt-1 text-muted-foreground">
                      Unresolved ·{" "}
                      {entry.item.content.unresolvedItems.join(" · ")}
                    </p>
                  ) : null}
                </div>
              ),
            )}
            {roleHistory.hasNextPage ? (
              <Button
                data-testid="project-role-timeline-more"
                disabled={roleHistory.isFetchingNextPage}
                onClick={() => void roleHistory.fetchNextPage()}
                size="sm"
                type="button"
                variant="ghost"
              >
                {roleHistory.isFetchingNextPage ? "Loading…" : "Load more"}
              </Button>
            ) : null}
            {roleHistory.isError ? (
              <p className="text-xs text-destructive">
                Role history could not be loaded.
              </p>
            ) : null}
          </div>
        )}
      </div>

      <Dialog onOpenChange={setOfferOpen} open={offerOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {currentAssignment ? "Replace assignee" : "Assign Role"}
            </DialogTitle>
            <DialogDescription>
              This creates a 72-hour offer. The candidate must accept it; any
              replacement is committed atomically at acceptance.
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3">
            <label
              className="space-y-1.5 text-sm"
              htmlFor={`project-role-candidate-${definition.roleId}`}
            >
              <span className="font-medium">Candidate</span>
              <Input
                data-testid="project-role-candidate"
                id={`project-role-candidate-${definition.roleId}`}
                list={`project-role-members-${definition.roleId}`}
                onChange={(event) => setCandidatePubkey(event.target.value)}
                placeholder="Public key or npub"
                value={candidatePubkey}
              />
              <datalist id={`project-role-members-${definition.roleId}`}>
                {continuity.members.map((member) => (
                  <option key={member.pubkey} value={member.pubkey}>
                    {memberLabel(member.pubkey, actorProfiles)}
                  </option>
                ))}
              </datalist>
            </label>
            <label
              className="space-y-1.5 text-sm"
              htmlFor={`project-role-reason-${definition.roleId}`}
            >
              <span className="font-medium">Context (optional)</span>
              <Textarea
                id={`project-role-reason-${definition.roleId}`}
                onChange={(event) => setReason(event.target.value)}
                placeholder="Why this Assignment should change"
                value={reason}
              />
            </label>
          </div>
          <DialogFooter>
            <Button
              onClick={() => setOfferOpen(false)}
              type="button"
              variant="outline"
            >
              Cancel
            </Button>
            <Button
              data-testid="project-role-offer-submit"
              disabled={!candidatePubkey.trim() || mutation.isPending}
              onClick={() => void submitOffer()}
              type="button"
            >
              {mutation.isPending ? "Submitting…" : "Create offer"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog onOpenChange={setEndOpen} open={endOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>End current tenure?</DialogTitle>
            <DialogDescription>
              The Assignment remains in history. The Role becomes vacant; the
              assignee cannot self-end through this flow.
            </DialogDescription>
          </DialogHeader>
          <Textarea
            onChange={(event) => setReason(event.target.value)}
            placeholder="Reason (optional)"
            value={reason}
          />
          <DialogFooter>
            <Button
              onClick={() => setEndOpen(false)}
              type="button"
              variant="outline"
            >
              Cancel
            </Button>
            <Button
              data-testid="project-role-end-confirm"
              disabled={mutation.isPending}
              onClick={() => void submitEnd()}
              type="button"
              variant="destructive"
            >
              {mutation.isPending ? "Ending…" : "End tenure"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ProjectRoleCheckpointDialog
        draft={continuityDraft}
        onDraftChange={setContinuityDraft}
        onOpenChange={setCheckpointOpen}
        onSubmit={() => void submitCheckpoint()}
        open={checkpointOpen}
        pending={mutation.isPending}
        roleId={definition.roleId}
      />
      <ProjectRoleHandoffDialog
        draft={continuityDraft}
        hasCheckpoint={Boolean(checkpoints[0])}
        onDraftChange={setContinuityDraft}
        onOpenChange={setHandoffOpen}
        onSubmit={() => void submitHandoff()}
        open={handoffOpen}
        pending={mutation.isPending}
        roleId={definition.roleId}
      />
    </section>
  );
}
