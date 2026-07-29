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
import { useProjectViewRoleMutation } from "@/features/project-view/hooks";
import { ProjectViewActor } from "@/features/project-view/ui/ProjectViewActor";
import type {
  ProjectRoleAssignment,
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

const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

function formatDateTime(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : dateTimeFormatter.format(date);
}

function activeAssignmentForRole(
  assignments: ProjectRoleAssignment[],
  roleId: string,
) {
  return assignments.find(
    (assignment) => assignment.roleId === roleId && !assignment.endedAt,
  );
}

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

export function ProjectRoleInspector({
  actorProfiles,
  continuity,
  currentPubkey,
  definition,
  projectRevision,
}: {
  actorProfiles?: UserProfileLookup;
  continuity: ProjectViewRoleContinuity;
  currentPubkey?: string;
  definition: ProjectRoleDefinition;
  projectRevision: number;
}) {
  const mutation = useProjectViewRoleMutation();
  const [offerOpen, setOfferOpen] = React.useState(false);
  const [endOpen, setEndOpen] = React.useState(false);
  const [candidatePubkey, setCandidatePubkey] = React.useState("");
  const [reason, setReason] = React.useState("");
  const [renderedAt] = React.useState(Date.now);
  const normalizedCurrentPubkey = currentPubkey?.toLowerCase();
  const currentAssignment = activeAssignmentForRole(
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
  const proposals = continuity.proposals
    .filter((proposal) => proposal.roleId === definition.roleId)
    .sort((left, right) => right.createdAt.localeCompare(left.createdAt));
  const history = continuity.assignments
    .filter(
      (assignment) =>
        assignment.roleId === definition.roleId && Boolean(assignment.endedAt),
    )
    .sort((left, right) =>
      (right.endedAt ?? "").localeCompare(left.endedAt ?? ""),
    );
  const handoffs = continuity.handoffs
    .filter((handoff) => handoff.roleId === definition.roleId)
    .sort((left, right) => right.createdAt.localeCompare(left.createdAt));
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
              Since {formatDateTime(currentAssignment.startedAt)}
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
          {currentBrief.relatedObjects.length > 0 ? (
            <p className="mt-3 text-xs text-muted-foreground">
              {currentBrief.relatedObjects.length} related Issue/Work{" "}
              {currentBrief.relatedObjects.length === 1 ? "object" : "objects"}
            </p>
          ) : null}
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
                    Expires {formatDateTime(proposal.expiresAt)}
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
                  {formatDateTime(assignment.startedAt)} –{" "}
                  {assignment.endedAt
                    ? formatDateTime(assignment.endedAt)
                    : "current"}{" "}
                  · {assignment.endedReason ?? "ended"}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {handoffs.length > 0 ? (
        <div>
          <h4 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Handoffs
          </h4>
          <div className="mt-2 space-y-2">
            {handoffs.map((handoff) => (
              <div
                className="rounded-lg border border-border/70 p-2.5 text-xs"
                key={handoff.handoffId}
              >
                {handoff.cause} · {formatDateTime(handoff.createdAt)}
              </div>
            ))}
          </div>
        </div>
      ) : null}

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
    </section>
  );
}
