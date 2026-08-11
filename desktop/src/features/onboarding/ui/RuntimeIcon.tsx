import type { AcpRuntimeCatalogEntry } from "@/shared/api/types";
import { RuntimeGlyph } from "@/shared/ui/RuntimeGlyph";

function isBuiltInRuntime(runtime: AcpRuntimeCatalogEntry): boolean {
  return runtime.id.trim().toLowerCase() === "buzz-agent";
}

export function getRuntimeDisplayLabel(
  runtime: AcpRuntimeCatalogEntry,
): string {
  return isBuiltInRuntime(runtime) ? "Built-in Agent" : runtime.label;
}

export function RuntimeIcon({
  className = "h-8 w-8",
  runtime,
}: {
  className?: string;
  runtime: AcpRuntimeCatalogEntry;
}) {
  return (
    <RuntimeGlyph
      className={className}
      runtimeId={runtime.id}
      testId={`onboarding-runtime-glyph-${runtime.id}`}
    />
  );
}
