import { Hand, Link2Off, Save } from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import { useProjectViewRoleMutation } from "@/features/project-view/hooks";
import type {
  ProjectViewRoleContinuity,
  ProjectViewRoleMutationIntent,
} from "@/shared/api/tauriProjectView";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

export function ProjectWorkContinuity({
  continuity,
  currentPubkey,
  projectRevision,
  workId,
}: {
  continuity: ProjectViewRoleContinuity;
  currentPubkey?: string;
  projectRevision: number;
  workId: string;
}) {
  const mutation = useProjectViewRoleMutation();
  const responsibility = continuity.workResponsibilities.find(
    (candidate) => candidate.workId === workId,
  );
  const responsibleRole = responsibility
    ? continuity.roles.find((role) => role.roleId === responsibility.roleId)
    : undefined;
  const activeCommitment = continuity.commitments.find(
    (commitment) => commitment.workId === workId && !commitment.endedAt,
  );
  const normalizedPubkey = currentPubkey?.toLowerCase();
  const viewerMembership = continuity.members.find(
    (member) => member.pubkey.toLowerCase() === normalizedPubkey,
  );
  const viewerAssignment = continuity.assignments.find(
    (assignment) =>
      !assignment.endedAt &&
      assignment.memberPubkey.toLowerCase() === normalizedPubkey,
  );
  const viewerRole = viewerAssignment
    ? continuity.roles.find((role) => role.roleId === viewerAssignment.roleId)
    : undefined;
  const isOwner = viewerMembership?.role === "owner";
  const isLeader =
    viewerMembership?.role === "admin" && viewerRole?.level === "admin";
  const canGovern = Boolean(isOwner || isLeader);
  const canAccept = Boolean(
    responsibility &&
      viewerAssignment?.roleId === responsibility.roleId &&
      !activeCommitment,
  );
  const canRelease =
    activeCommitment?.assignmentId === viewerAssignment?.assignmentId;
  const actingAssignmentId = viewerAssignment?.assignmentId;
  const activeRoles = continuity.roles.filter((role) => role.active);
  const [selectedRoleId, setSelectedRoleId] = React.useState(
    responsibility?.roleId ?? "",
  );

  React.useEffect(() => {
    setSelectedRoleId(responsibility?.roleId ?? "");
  }, [responsibility?.roleId]);

  async function submit(intent: ProjectViewRoleMutationIntent) {
    try {
      const result = await mutation.mutateAsync(intent);
      if (result.status === "conflict") {
        toast.error(
          "The Project changed before this Work action was applied. Review the refreshed state and try again.",
        );
        return;
      }
      toast.success("Work continuity updated");
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : "Could not update Work continuity",
      );
    }
  }

  return (
    <section
      className="space-y-3 border-t border-border/70 pt-4"
      data-testid="project-work-continuity"
    >
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-xs font-semibold">Work continuity</h3>
        {responsibleRole ? (
          <Badge variant="outline">{responsibleRole.name}</Badge>
        ) : (
          <Badge variant="warning">Unassigned</Badge>
        )}
        {activeCommitment ? (
          <Badge variant="success">Committed</Badge>
        ) : responsibility ? (
          <Badge variant="warning">Waiting for continuation</Badge>
        ) : null}
      </div>

      {canGovern ? (
        <div className="space-y-2 rounded-xl border border-border/70 bg-muted/20 p-3">
          <label
            className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground"
            htmlFor={`responsible-role-${workId}`}
          >
            Responsible Role
          </label>
          <select
            className="flex h-9 w-full rounded-md border border-input bg-background px-3 text-sm shadow-xs outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
            disabled={Boolean(activeCommitment) || mutation.isPending}
            id={`responsible-role-${workId}`}
            onChange={(event) => setSelectedRoleId(event.target.value)}
            value={selectedRoleId}
          >
            <option value="">No responsible Role</option>
            {activeRoles.map((role) => (
              <option key={role.roleId} value={role.roleId}>
                {role.name}
              </option>
            ))}
          </select>
          <div className="flex flex-wrap gap-2">
            <Button
              disabled={
                Boolean(activeCommitment) ||
                mutation.isPending ||
                selectedRoleId === (responsibility?.roleId ?? "")
              }
              onClick={() =>
                void submit({
                  operation: "set_work_responsibility",
                  expectedProjectRevision: projectRevision,
                  actingAssignmentId,
                  workId,
                  responsibleRoleId: selectedRoleId || undefined,
                })
              }
              size="sm"
              type="button"
              variant="outline"
            >
              {selectedRoleId ? <Save /> : <Link2Off />}
              Save responsibility
            </Button>
          </div>
          {activeCommitment ? (
            <p className="text-xs text-muted-foreground">
              Release or close the active Commitment before changing the
              responsible Role.
            </p>
          ) : null}
        </div>
      ) : null}

      {canAccept && viewerAssignment ? (
        <Button
          disabled={mutation.isPending}
          onClick={() =>
            void submit({
              operation: "accept_work",
              expectedProjectRevision: projectRevision,
              actingAssignmentId: viewerAssignment.assignmentId,
              workId,
            })
          }
          size="sm"
          type="button"
          variant="outline"
        >
          <Hand />
          Accept Work
        </Button>
      ) : null}

      {canRelease && viewerAssignment && activeCommitment ? (
        <Button
          disabled={mutation.isPending}
          onClick={() =>
            void submit({
              operation: "end_commitment",
              expectedProjectRevision: projectRevision,
              actingAssignmentId: viewerAssignment.assignmentId,
              commitmentId: activeCommitment.commitmentId,
            })
          }
          size="sm"
          type="button"
          variant="outline"
        >
          <Link2Off />
          Release Work
        </Button>
      ) : null}
    </section>
  );
}
