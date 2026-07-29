const PROJECT_VIEW_INTEGRITY_PREFIX = "Project View integrity error:";

/**
 * A defensive client-boundary failure raised when the native command returns
 * a self-contradictory Project View DTO. The native layer remains responsible
 * for signature and projection verification; this guard prevents an
 * accidentally mixed or malformed result from reaching React.
 */
export class ProjectViewIntegrityError extends Error {
  constructor(reason: string) {
    super(`${PROJECT_VIEW_INTEGRITY_PREFIX} ${reason}`);
    this.name = "ProjectViewIntegrityError";
  }
}

/** Returns whether an unknown command/query failure represents integrity. */
export function isProjectViewIntegrityError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error ?? "");
  return (
    error instanceof ProjectViewIntegrityError ||
    message.includes(PROJECT_VIEW_INTEGRITY_PREFIX)
  );
}
