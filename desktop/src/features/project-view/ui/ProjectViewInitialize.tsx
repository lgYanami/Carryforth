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
  ProjectViewMutationResult,
} from "@/shared/api/tauriProjectView";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { Textarea } from "@/shared/ui/textarea";

type GoalDraft = ProjectGoalData & { key: string };

const EMPTY_PROFILE: ProjectProfileData = {
  name: "",
  positioning: "",
  purpose: "",
  problem: "",
  scope: "",
};

function newGoal(): GoalDraft {
  return {
    key: globalThis.crypto.randomUUID(),
    title: "",
    desiredOutcome: "",
    directions: [],
  };
}

function isComplete(profile: ProjectProfileData, goals: GoalDraft[]) {
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

function normalizeGoal(goal: GoalDraft): ProjectGoalData {
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
  goals: GoalDraft[];
  onChange: (goals: GoalDraft[]) => void;
}) {
  const update = (index: number, patch: Partial<GoalDraft>) =>
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
  goals: GoalDraft[];
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

export function ProjectViewInitialize({
  onReviewLatest,
}: {
  onReviewLatest: () => void;
}) {
  const mutation = useProjectViewMutation();
  const [profile, setProfile] =
    React.useState<ProjectProfileData>(EMPTY_PROFILE);
  const [goals, setGoals] = React.useState<GoalDraft[]>(() => [newGoal()]);
  const [reviewing, setReviewing] = React.useState(false);
  const [conflict, setConflict] = React.useState<
    Extract<ProjectViewMutationResult, { status: "conflict" }> | undefined
  >();

  const submit = async () => {
    if (mutation.isPending || !isComplete(profile, goals)) return;
    setConflict(undefined);
    try {
      const result = await mutation.mutateAsync({
        operation: "initialize",
        profile: normalizeProfile(profile),
        goals: goals.map(normalizeGoal),
      });
      if (result.status === "conflict") {
        setConflict(result);
        return;
      }
      toast.success("Project View initialized");
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
              conflict={conflict}
              onReviewLatest={onReviewLatest}
            />
          </div>
        ) : null}

        <div className="rounded-2xl border border-border/70 bg-card/60 p-5 shadow-xs">
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
                disabled={mutation.isPending}
                onClick={() => void submit()}
                type="button"
              >
                {mutation.isPending ? (
                  <LoaderCircle className="animate-spin" />
                ) : (
                  <Check />
                )}
                {mutation.isPending ? "Initializing…" : "Initialize View"}
              </Button>
            ) : (
              <Button
                disabled={!isComplete(profile, goals)}
                onClick={() => setReviewing(true)}
                type="button"
              >
                Review foundation
              </Button>
            )}
          </div>
        </div>
      </div>
    </main>
  );
}
