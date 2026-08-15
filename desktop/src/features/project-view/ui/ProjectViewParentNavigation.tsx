import { ArrowUp } from "lucide-react";

import type { ProjectViewExplorerParent } from "@/features/project-view/explorerModel";
import { Button } from "@/shared/ui/button";

/** The only parent affordance in Main: an up arrow and the parent title. */
export function ProjectViewParentNavigation({
  onSelect,
  parent,
}: {
  onSelect: (parent: ProjectViewExplorerParent) => void;
  parent?: ProjectViewExplorerParent;
}) {
  if (!parent) return null;
  return (
    <Button
      aria-label={`Go to parent: ${parent.title}`}
      className="max-w-[min(18rem,45vw)] shrink-0"
      data-testid="project-view-parent-navigation"
      onClick={() => onSelect(parent)}
      size="sm"
      title={parent.title}
      type="button"
      variant="outline"
    >
      <ArrowUp aria-hidden="true" />
      <span className="truncate">{parent.title}</span>
    </Button>
  );
}
