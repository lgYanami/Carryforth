import {
  ArrowLeft,
  Check,
  LoaderCircle,
  Plus,
  Sparkles,
  Trash2,
} from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import { useProjectViewMutation } from "@/features/project-view/hooks";
import {
  ProjectViewConflictNotice,
  ProjectViewField,
  ProjectViewListField,
} from "@/features/project-view/ui/ProjectViewFormFields";
import type {
  ProjectGoalData,
  ProjectProfileData,
  ProjectViewLoadResult,
  ProjectViewMutationResult,
} from "@/shared/api/tauriProjectView";
import { getProjectView } from "@/shared/api/tauriProjectView";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { Textarea } from "@/shared/ui/textarea";

export type ProjectViewGoalDraft = ProjectGoalData & { key: string };

const EMPTY_PROFILE: ProjectProfileData = {
  name: "",
  positioning: "",
  purpose: "",
  problem: "",
  scope: "",
};

function newGoal(): ProjectViewGoalDraft {
  return {
    key: globalThis.crypto.randomUUID(),
    title: "",
    desiredOutcome: "",
    directions: [],
  };
}

function isComplete(
  profile: ProjectProfileData,
  goals: ProjectViewGoalDraft[],
) {
  return (
    Object.values(profile).every((value) => value.trim().length > 0) &&
    goals.length > 0 &&
    goals.every(
      (goal) =>
        goal.title.trim().length > 0 && goal.desiredOutcome.trim().length > 0,
    )
  );
}

function normalizeProfile(profile: ProjectProfileData): ProjectProfileData {
  return {
    name: profile.name.trim(),
    positioning: profile.positioning.trim(),
    purpose: profile.purpose.trim(),
    problem: profile.problem.trim(),
    scope: profile.scope.trim(),
  };
}

function normalizeGoal(goal: ProjectViewGoalDraft): ProjectGoalData {
  return {
    title: goal.title.trim(),
    desiredOutcome: goal.desiredOutcome.trim(),
    directions: goal.directions.map((item) => item.trim()).filter(Boolean),
  };
}

function ProfileFields({
  onChange,
  profile,
}: {
  onChange: (profile: ProjectProfileData) => void;
  profile: ProjectProfileData;
}) {
  const set = (field: keyof ProjectProfileData, value: string) =>
    onChange({ ...profile, [field]: value });
  return (
    <section className="space-y-4">
      <div>
        <h2 className="text-base font-semibold">Project Profile</h2>
        <p className="mt-1 text-xs text-muted-foreground">
          Define the shared frame Humans and Agents should use.
        </p>
      </div>
      <ProjectViewField label="Project name" required>
        <Input
          autoFocus
          onChange={(event) => set("name", event.target.value)}
          value={profile.name}
        />
      </ProjectViewField>
      <ProjectViewField label="Positioning" required>
        <Textarea
          onChange={(event) => set("positioning", event.target.value)}
          value={profile.positioning}
        />
      </ProjectViewField>
      <ProjectViewField label="Purpose" required>
        <Textarea
          onChange={(event) => set("purpose", event.target.value)}
          value={profile.purpose}
        />
      </ProjectViewField>
      <ProjectViewField label="Problem" required>
        <Textarea
          onChange={(event) => set("problem", event.target.value)}
          value={profile.problem}
        />
      </ProjectViewField>
      <ProjectViewField label="Scope" required>
        <Textarea
          onChange={(event) => set("scope", event.target.value)}
          value={profile.scope}
        />
      </ProjectViewField>
    </section>
  );
}

function GoalsFields({
  goals,
  onChange,
}: {
  goals: ProjectViewGoalDraft[];
  onChange: (goals: ProjectViewGoalDraft[]) => void;
}) {
  const update = (index: number, patch: Partial<ProjectViewGoalDraft>) =>
    onChange(
      goals.map((goal, goalIndex) =>
        goalIndex === index ? { ...goal, ...patch } : goal,
      ),
    );
  return (
    <section className="space-y-4">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h2 className="text-base font-semibold">Initial Goals</h2>
          <p className="mt-1 text-xs text-muted-foreground">
            Initialization requires at least one meaningful outcome.
          </p>
        </div>
        <Button
          disabled={goals.length >= 32}
          onClick={() => onChange([...goals, newGoal()])}
          size="sm"
          type="button"
          variant="outline"
        >
          <Plus />
          Add Goal
        </Button>
      </div>
      {goals.map((goal, index) => (
        <article
          className="space-y-3 rounded-xl border border-border/70 bg-background p-4"
          key={goal.key}
        >
          <div className="flex items-center justify-between gap-3">
            <div className="text-sm font-semibold">Goal {index + 1}</div>
            <Button
              aria-label={`Remove Goal ${index + 1}`}
              disabled={goals.length === 1}
              onClick={() =>
                onChange(goals.filter((_, goalIndex) => goalIndex !== index))
              }
              size="icon"
              type="button"
              variant="ghost"
            >
              <Trash2 />
            </Button>
          </div>
          <ProjectViewField label="Title" required>
            <Input
              onChange={(event) => update(index, { title: event.target.value })}
              value={goal.title}
            />
          </ProjectViewField>
          <ProjectViewField label="Desired outcome" required>
            <Textarea
              onChange={(event) =>
                update(index, { desiredOutcome: event.target.value })
              }
              value={goal.desiredOutcome}
            />
          </ProjectViewField>
          <ProjectViewListField
            label="Directions"
            onChange={(value) =>
              update(index, { directions: value.split("\n") })
            }
            value={goal.directions.join("\n")}
          />
        </article>
      ))}
    </section>
  );
}

function Review({
  goals,
  profile,
}: {
  goals: ProjectViewGoalDraft[];
  profile: ProjectProfileData;
}) {
  return (
    <div className="space-y-5">
      <section className="rounded-xl border border-border/70 bg-background p-4">
        <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
          Project Profile
        </div>
        <h2 className="mt-2 text-lg font-semibold">{profile.name}</h2>
        {[
          ["Positioning", profile.positioning],
          ["Purpose", profile.purpose],
          ["Problem", profile.problem],
          ["Scope", profile.scope],
        ].map(([label, value]) => (
          <div className="mt-3" key={label}>
            <div className="text-xs font-semibold">{label}</div>
            <p className="mt-1 whitespace-pre-wrap text-sm text-muted-foreground">
              {value}
            </p>
          </div>
        ))}
      </section>
      <section className="space-y-2">
        <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
          {goals.length} Initial {goals.length === 1 ? "Goal" : "Goals"}
        </div>
        {goals.map((goal) => (
          <article
            className="rounded-xl border border-border/70 bg-background p-4"
            key={goal.key}
          >
            <h3 className="text-sm font-semibold">{goal.title}</h3>
            <p className="mt-1 text-sm text-muted-foreground">
              {goal.desiredOutcome}
            </p>
          </article>
        ))}
      </section>
    </div>
  );
}

export type ProjectViewInitializationDraft = {
  goals: ProjectViewGoalDraft[];
  profile: ProjectProfileData;
  reviewing: boolean;
};

export function createProjectViewInitializationDraft(): ProjectViewInitializationDraft {
  return {
    goals: [newGoal()],
    profile: { ...EMPTY_PROFILE },
    reviewing: false,
  };
}

export function isProjectViewInitializationDraftDirty(
  draft: ProjectViewInitializationDraft,
) {
  return (
    draft.reviewing ||
    Object.values(draft.profile).some((value) => value.length > 0) ||
    draft.goals.length !== 1 ||
    draft.goals.some(
      (goal) =>
        goal.title.length > 0 ||
        goal.desiredOutcome.length > 0 ||
        goal.directions.length > 0,
    )
  );
}

export function ProjectViewInitialize({
  draft,
  onApplied,
  onChange,
  onConflict,
  onDiscardAndOpenLatest,
}: {
  draft: ProjectViewInitializationDraft;
  onApplied: () => void;
  onChange: (draft: ProjectViewInitializationDraft) => void;
  onConflict: (
    conflict: Extract<ProjectViewMutationResult, { status: "conflict" }>,
  ) => void;
  onDiscardAndOpenLatest: () => Promise<unknown>;
}) {
  const mutation = useProjectViewMutation();
  const { goals, profile, reviewing } = draft;
  const [conflict, setConflict] = React.useState<
    Extract<ProjectViewMutationResult, { status: "conflict" }> | undefined
  >();
  const [latestView, setLatestView] = React.useState<ProjectViewLoadResult>();
  const [reviewingLatest, setReviewingLatest] = React.useState(false);

  const setProfile = (nextProfile: ProjectProfileData) =>
    onChange({ ...draft, profile: nextProfile });
  const setGoals = (nextGoals: ProjectViewGoalDraft[]) =>
    onChange({ ...draft, goals: nextGoals });
  const setReviewing = (nextReviewing: boolean) =>
    onChange({ ...draft, reviewing: nextReviewing });

  const reviewLatest = async () => {
    if (reviewingLatest) return;
    setReviewingLatest(true);
    try {
      setLatestView(await getProjectView());
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : "The latest Project View could not be verified.",
      );
    } finally {
      setReviewingLatest(false);
    }
  };

  const discardAndOpenLatest = async () => {
    setConflict(undefined);
    setLatestView(undefined);
    await onDiscardAndOpenLatest();
  };

  const submit = async () => {
    if (mutation.isPending || conflict || !isComplete(profile, goals)) return;
    setConflict(undefined);
    try {
      const result = await mutation.mutateAsync({
        operation: "initialize",
        profile: normalizeProfile(profile),
        goals: goals.map(normalizeGoal),
      });
      if (result.status === "conflict") {
        setConflict(result);
        onConflict(result);
        void reviewLatest();
        return;
      }
      toast.success("Project View initialized");
      onApplied();
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : "Project View could not be initialized.",
      );
    }
  };

  return (
    <main className="min-h-0 flex-1 overflow-y-auto p-5">
      <div className="mx-auto max-w-4xl">
        <div className="mb-5 flex items-start gap-3">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-border/70 bg-muted/30">
            <Sparkles className="h-4 w-4 text-muted-foreground" />
          </div>
          <div>
            <h1 className="text-xl font-semibold">Initialize this View</h1>
            <p className="mt-1 max-w-2xl text-sm leading-relaxed text-muted-foreground">
              Establish one atomic Project Profile and its first Goal. Nothing
              is published until you review and confirm the whole foundation.
            </p>
          </div>
        </div>

        {conflict ? (
          <div className="mb-5">
            <ProjectViewConflictNotice
              comparison={
                <p className="text-xs leading-relaxed text-muted-foreground">
                  {latestView?.status === "ready"
                    ? `“${latestView.view.profile.data.name}” is now initialized. Initialization is atomic, so this preserved draft cannot overwrite it; use the latest View and apply any still-relevant fields individually.`
                    : "Initialization can only happen once. Keep this input for reference until the latest verified View is available."}
                </p>
              }
              conflict={conflict}
              latestProjectRevision={
                latestView?.status === "ready"
                  ? latestView.projectRevision
                  : undefined
              }
              onDiscardDraft={() => void discardAndOpenLatest()}
              onReviewLatest={() => void reviewLatest()}
              refreshing={reviewingLatest}
            />
          </div>
        ) : null}

        <form
          className="rounded-2xl border border-border/70 bg-card/60 p-5 shadow-xs"
          onSubmit={(event) => {
            event.preventDefault();
            if (reviewing) {
              void submit();
            } else if (isComplete(profile, goals)) {
              setReviewing(true);
            }
          }}
        >
          {reviewing ? (
            <Review goals={goals} profile={profile} />
          ) : (
            <div className="grid gap-8 lg:grid-cols-2">
              <ProfileFields onChange={setProfile} profile={profile} />
              <GoalsFields goals={goals} onChange={setGoals} />
            </div>
          )}

          <div className="mt-6 flex flex-wrap justify-end gap-2 border-t border-border/70 pt-4">
            {reviewing ? (
              <Button
                disabled={mutation.isPending}
                onClick={() => setReviewing(false)}
                type="button"
                variant="outline"
              >
                <ArrowLeft />
                Back to edit
              </Button>
            ) : null}
            {reviewing ? (
              <Button
                disabled={mutation.isPending || Boolean(conflict)}
                type="submit"
              >
                {mutation.isPending ? (
                  <LoaderCircle className="animate-spin" />
                ) : (
                  <Check />
                )}
                {mutation.isPending ? "Initializing…" : "Initialize View"}
              </Button>
            ) : (
              <Button disabled={!isComplete(profile, goals)} type="submit">
                Review foundation
              </Button>
            )}
          </div>
        </form>
      </div>
    </main>
  );
}
