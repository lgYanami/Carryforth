export type ProjectViewNavigationKey =
  | "ArrowDown"
  | "ArrowLeft"
  | "ArrowRight"
  | "ArrowUp"
  | "End"
  | "Home";

/** Resolve the next card index for deterministic Project View map navigation. */
export function nextProjectViewObjectIndex(
  currentIndex: number,
  itemCount: number,
  key: ProjectViewNavigationKey,
): number | undefined {
  if (itemCount < 1 || currentIndex < 0 || currentIndex >= itemCount) {
    return undefined;
  }
  if (key === "Home") return 0;
  if (key === "End") return itemCount - 1;
  if (key === "ArrowDown" || key === "ArrowRight") {
    return (currentIndex + 1) % itemCount;
  }
  return (currentIndex - 1 + itemCount) % itemCount;
}
