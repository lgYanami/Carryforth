import type { ProjectContextRouteSelection } from "@/features/project-context/routeState";

/** Focus the real Coordinate/Hub control for one stable graph selection. */
export function focusProjectContextGraphTarget(
  selection: ProjectContextRouteSelection,
): boolean {
  if (typeof document === "undefined") return false;
  const selector =
    selection.kind === "coordinate"
      ? ".project-context-coordinate[data-coordinate-key]"
      : ".project-context-hub[data-edge-key]";
  const dataKey = selection.kind === "coordinate" ? "coordinateKey" : "edgeKey";
  const container = [...document.querySelectorAll<HTMLElement>(selector)].find(
    (element) => element.dataset[dataKey] === selection.key,
  );
  const button = container?.querySelector<HTMLButtonElement>("button");
  button?.focus();
  return Boolean(button);
}
