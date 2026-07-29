import type * as React from "react";

import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Textarea } from "@/shared/ui/textarea";

export type RoleContinuityDraft = {
  summary: string;
  currentFocus: string;
  progress: string;
  blockers: string;
  risks: string;
  openQuestions: string;
  nextSteps: string;
};

export const EMPTY_ROLE_CONTINUITY_DRAFT: RoleContinuityDraft = {
  summary: "",
  currentFocus: "",
  progress: "",
  blockers: "",
  risks: "",
  openQuestions: "",
  nextSteps: "",
};

type SharedDialogProps = {
  draft: RoleContinuityDraft;
  onDraftChange: React.Dispatch<React.SetStateAction<RoleContinuityDraft>>;
  onOpenChange: (open: boolean) => void;
  onSubmit: () => void;
  open: boolean;
  pending: boolean;
  roleId: string;
};

const checkpointFields: Array<{
  key: Exclude<keyof RoleContinuityDraft, "summary">;
  label: string;
}> = [
  { key: "currentFocus", label: "Current focus" },
  { key: "progress", label: "Progress" },
  { key: "blockers", label: "Blockers" },
  { key: "risks", label: "Risks" },
  { key: "openQuestions", label: "Open questions" },
  { key: "nextSteps", label: "Next steps" },
];

export function ProjectRoleCheckpointDialog({
  draft,
  onDraftChange,
  onOpenChange,
  onSubmit,
  open,
  pending,
  roleId,
}: SharedDialogProps) {
  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add Role Checkpoint</DialogTitle>
          <DialogDescription>
            Record durable role context for this Assignment and its future
            successors. Use one line per item in the structured fields.
          </DialogDescription>
        </DialogHeader>
        <div className="max-h-[60vh] space-y-3 overflow-y-auto pr-1">
          <label
            className="space-y-1.5 text-sm"
            htmlFor={`project-role-checkpoint-summary-${roleId}`}
          >
            <span className="font-medium">Situation summary</span>
            <Textarea
              data-testid="project-role-checkpoint-summary"
              id={`project-role-checkpoint-summary-${roleId}`}
              onChange={(event) =>
                onDraftChange((current) => ({
                  ...current,
                  summary: event.target.value,
                }))
              }
              placeholder="What should the next turn or successor know first?"
              value={draft.summary}
            />
          </label>
          {checkpointFields.map((field) => (
            <label
              className="space-y-1.5 text-sm"
              htmlFor={`project-role-checkpoint-${field.key}-${roleId}`}
              key={field.key}
            >
              <span className="font-medium">{field.label}</span>
              <Textarea
                id={`project-role-checkpoint-${field.key}-${roleId}`}
                onChange={(event) =>
                  onDraftChange((current) => ({
                    ...current,
                    [field.key]: event.target.value,
                  }))
                }
                placeholder="One item per line"
                value={draft[field.key]}
              />
            </label>
          ))}
        </div>
        <DialogFooter>
          <Button
            onClick={() => onOpenChange(false)}
            type="button"
            variant="outline"
          >
            Cancel
          </Button>
          <Button
            data-testid="project-role-checkpoint-submit"
            disabled={!draft.summary.trim() || pending}
            onClick={onSubmit}
            type="button"
          >
            {pending ? "Submitting…" : "Append Checkpoint"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function ProjectRoleHandoffDialog({
  draft,
  hasCheckpoint,
  onDraftChange,
  onOpenChange,
  onSubmit,
  open,
  pending,
  roleId,
}: SharedDialogProps & { hasCheckpoint: boolean }) {
  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add Handoff note</DialogTitle>
          <DialogDescription>
            Preserve planned transition context without ending your Assignment.
            Only governance can end or replace the tenure.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          <label
            className="space-y-1.5 text-sm"
            htmlFor={`project-role-handoff-summary-${roleId}`}
          >
            <span className="font-medium">Transition summary</span>
            <Textarea
              data-testid="project-role-handoff-summary"
              id={`project-role-handoff-summary-${roleId}`}
              onChange={(event) =>
                onDraftChange((current) => ({
                  ...current,
                  summary: event.target.value,
                }))
              }
              placeholder="What is being handed over?"
              value={draft.summary}
            />
          </label>
          <label
            className="space-y-1.5 text-sm"
            htmlFor={`project-role-handoff-unresolved-${roleId}`}
          >
            <span className="font-medium">Unresolved items</span>
            <Textarea
              id={`project-role-handoff-unresolved-${roleId}`}
              onChange={(event) =>
                onDraftChange((current) => ({
                  ...current,
                  openQuestions: event.target.value,
                }))
              }
              placeholder="One item per line"
              value={draft.openQuestions}
            />
          </label>
          {hasCheckpoint ? (
            <p className="text-xs text-muted-foreground">
              The latest Checkpoint will be carried into this Handoff.
            </p>
          ) : null}
        </div>
        <DialogFooter>
          <Button
            onClick={() => onOpenChange(false)}
            type="button"
            variant="outline"
          >
            Cancel
          </Button>
          <Button
            data-testid="project-role-handoff-submit"
            disabled={!draft.summary.trim() || pending}
            onClick={onSubmit}
            type="button"
          >
            {pending ? "Submitting…" : "Append Handoff"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
