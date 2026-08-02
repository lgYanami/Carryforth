export type DocumentDiffLine = {
  kind: "context" | "delete" | "insert";
  text: string;
  oldLine?: number;
  newLine?: number;
};

function lines(value: string): string[] {
  if (value.length === 0) return [];
  // Keep the terminal empty segment so adding/removing the final newline is a
  // visible exact change instead of being silently normalized away.
  return value.split("\n");
}

/** Exact line diff. Large matrices fall back to an exact, coarser middle block. */
export function diffDocumentLines(
  before: string,
  after: string,
): DocumentDiffLine[] {
  const oldLines = lines(before);
  const newLines = lines(after);
  const matrixCells = (oldLines.length + 1) * (newLines.length + 1);
  if (matrixCells > 250_000) return coarseExactDiff(oldLines, newLines);

  const width = newLines.length + 1;
  const matrix = new Uint32Array((oldLines.length + 1) * width);
  for (let oldIndex = oldLines.length - 1; oldIndex >= 0; oldIndex -= 1) {
    for (let newIndex = newLines.length - 1; newIndex >= 0; newIndex -= 1) {
      const index = oldIndex * width + newIndex;
      matrix[index] =
        oldLines[oldIndex] === newLines[newIndex]
          ? matrix[(oldIndex + 1) * width + newIndex + 1] + 1
          : Math.max(
              matrix[(oldIndex + 1) * width + newIndex],
              matrix[oldIndex * width + newIndex + 1],
            );
    }
  }

  const result: DocumentDiffLine[] = [];
  let oldIndex = 0;
  let newIndex = 0;
  while (oldIndex < oldLines.length || newIndex < newLines.length) {
    if (
      oldIndex < oldLines.length &&
      newIndex < newLines.length &&
      oldLines[oldIndex] === newLines[newIndex]
    ) {
      result.push({
        kind: "context",
        text: oldLines[oldIndex],
        oldLine: oldIndex + 1,
        newLine: newIndex + 1,
      });
      oldIndex += 1;
      newIndex += 1;
    } else if (
      newIndex >= newLines.length ||
      (oldIndex < oldLines.length &&
        matrix[(oldIndex + 1) * width + newIndex] >=
          matrix[oldIndex * width + newIndex + 1])
    ) {
      result.push({
        kind: "delete",
        text: oldLines[oldIndex],
        oldLine: oldIndex + 1,
      });
      oldIndex += 1;
    } else {
      result.push({
        kind: "insert",
        text: newLines[newIndex],
        newLine: newIndex + 1,
      });
      newIndex += 1;
    }
  }
  return result;
}

function coarseExactDiff(
  oldLines: string[],
  newLines: string[],
): DocumentDiffLine[] {
  let prefix = 0;
  while (
    prefix < oldLines.length &&
    prefix < newLines.length &&
    oldLines[prefix] === newLines[prefix]
  ) {
    prefix += 1;
  }
  let suffix = 0;
  while (
    suffix < oldLines.length - prefix &&
    suffix < newLines.length - prefix &&
    oldLines[oldLines.length - suffix - 1] ===
      newLines[newLines.length - suffix - 1]
  ) {
    suffix += 1;
  }
  const result: DocumentDiffLine[] = [];
  for (let index = 0; index < prefix; index += 1) {
    result.push({
      kind: "context",
      text: oldLines[index],
      oldLine: index + 1,
      newLine: index + 1,
    });
  }
  for (let index = prefix; index < oldLines.length - suffix; index += 1) {
    result.push({ kind: "delete", text: oldLines[index], oldLine: index + 1 });
  }
  for (let index = prefix; index < newLines.length - suffix; index += 1) {
    result.push({ kind: "insert", text: newLines[index], newLine: index + 1 });
  }
  for (let offset = suffix; offset > 0; offset -= 1) {
    const oldIndex = oldLines.length - offset;
    const newIndex = newLines.length - offset;
    result.push({
      kind: "context",
      text: oldLines[oldIndex],
      oldLine: oldIndex + 1,
      newLine: newIndex + 1,
    });
  }
  return result;
}
