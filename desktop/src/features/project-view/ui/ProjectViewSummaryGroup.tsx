import { ProjectViewSummaryItem } from "@/features/project-view/ui/ProjectViewSummaryItem";
import type { ProjectViewSummaryEntry } from "@/features/project-view/ui/ProjectViewSummaryItem";

/** Render one named direct-child or related-item group without deriving more data. */
export function ProjectViewSummaryGroup({
  items,
  label,
  onSelect,
}: {
  items: ProjectViewSummaryEntry[];
  label: string;
  onSelect: (item: ProjectViewSummaryEntry) => void;
}) {
  if (items.length === 0) return null;
  return (
    <section className="space-y-3" data-summary-group={label}>
      <h2 className="text-sm font-semibold">{label}</h2>
      <div className="grid gap-3 md:grid-cols-2">
        {items.map((item) => (
          <ProjectViewSummaryItem
            item={item}
            key={item.occurrenceKey}
            onSelect={onSelect}
          />
        ))}
      </div>
    </section>
  );
}
