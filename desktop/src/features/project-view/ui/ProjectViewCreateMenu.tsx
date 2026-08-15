import { ChevronDown, CirclePlus, Plus } from "lucide-react";

import {
  projectViewCreateActions,
  type ProjectViewCreateAction,
} from "@/features/project-view/projectViewCreateActions";
import type { ProjectViewCreateContext } from "@/features/project-view/model";
import type {
  ProjectViewObject,
  ProjectViewObjectType,
} from "@/shared/api/tauriProjectView";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";

type CreateRequest = (
  initialType?: Exclude<ProjectViewObjectType, "project_profile">,
  context?: ProjectViewCreateContext,
) => void;

function CreateItems({
  actions,
  onCreate,
}: {
  actions: ProjectViewCreateAction[];
  onCreate: CreateRequest;
}) {
  return actions.map((action) => (
    <DropdownMenuItem
      key={action.id}
      onSelect={() => onCreate(action.initialType, action.context)}
    >
      {action.relation === "related" ? <CirclePlus /> : <Plus />}
      {action.label}
    </DropdownMenuItem>
  ));
}

/** Explicit contextual and global create entry points for the current object. */
export function ProjectViewCreateMenu({
  canCreateRole,
  object,
  onCreate,
}: {
  canCreateRole: boolean;
  object: ProjectViewObject;
  onCreate: CreateRequest;
}) {
  const actions = projectViewCreateActions(object).filter(
    (action) => action.initialType !== "role" || canCreateRole,
  );
  const structural = actions.filter(
    (action) => action.relation === "structural",
  );
  const related = actions.filter((action) => action.relation === "related");
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button data-testid="project-view-add" size="sm" type="button">
          <Plus />
          Add
          <ChevronDown />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        {structural.length > 0 ? (
          <>
            <DropdownMenuLabel>Direct next layer</DropdownMenuLabel>
            <CreateItems actions={structural} onCreate={onCreate} />
            <DropdownMenuSeparator />
          </>
        ) : null}
        <DropdownMenuLabel>Related to current object</DropdownMenuLabel>
        <CreateItems actions={related} onCreate={onCreate} />
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => onCreate()}>
          <Plus />
          Add another object…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
