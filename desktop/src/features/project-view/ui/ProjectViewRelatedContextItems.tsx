import type {
  ProjectViewDocumentSummaryItem,
  ProjectViewObjectSummaryItem,
} from "@/features/project-view/explorerModel";
import { ProjectViewSummaryGroup } from "@/features/project-view/ui/ProjectViewSummaryGroup";
import type { ProjectViewSummaryEntry } from "@/features/project-view/ui/ProjectViewSummaryItem";

/** Read-only Project View Context presentation; mutations live in a separate management surface. */
export function ProjectViewRelatedContextItems({
  documents,
  onSelect,
  relatedIssues,
  relatedResources,
}: {
  documents: ProjectViewDocumentSummaryItem[];
  onSelect: (item: ProjectViewSummaryEntry) => void;
  relatedIssues: ProjectViewObjectSummaryItem[];
  relatedResources: ProjectViewObjectSummaryItem[];
}) {
  if (
    relatedIssues.length === 0 &&
    relatedResources.length === 0 &&
    documents.length === 0
  ) {
    return null;
  }
  return (
    <section
      className="space-y-6 border-t border-border/70 pt-6"
      data-testid="project-view-related-items"
    >
      <ProjectViewSummaryGroup
        items={relatedIssues}
        label="Related Issues"
        onSelect={onSelect}
      />
      <ProjectViewSummaryGroup
        items={relatedResources}
        label="Related Resources"
        onSelect={onSelect}
      />
      <ProjectViewSummaryGroup
        items={documents}
        label="Documents"
        onSelect={onSelect}
      />
    </section>
  );
}
