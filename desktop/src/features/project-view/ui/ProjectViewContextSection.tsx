import { ArrowRight, FileText, Link2, Plus, Trash2 } from "lucide-react";
import * as React from "react";
import { Link } from "@tanstack/react-router";

import {
  useProjectDocumentMeta,
  useProjectDocuments,
} from "@/features/project-documents/hooks";
import {
  canonicalizeProjectViewContextReferences,
  type ProjectViewContextReference,
  type ProjectViewObject,
} from "@/shared/api/tauriProjectView";
import { useProjectViewMutation } from "@/features/project-view/hooks";
import { projectViewObjectTitle } from "@/features/project-view/model";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";

type TargetKind = "resource" | "live_document" | "pinned_document";

function referenceKey(reference: ProjectViewContextReference): string {
  return reference.referenceType === "resource"
    ? `resource:${reference.resourceId}`
    : `document:${reference.documentId}:${reference.mode}:${reference.documentRevision ?? 0}`;
}

function contextLabel(
  reference: ProjectViewContextReference,
  objectsById: ReadonlyMap<string, ProjectViewObject>,
  documents: ReadonlyMap<string, string>,
): string {
  if (reference.referenceType === "resource") {
    const resource = objectsById.get(reference.resourceId);
    return resource ? projectViewObjectTitle(resource) : reference.resourceId;
  }
  return documents.get(reference.documentId) ?? reference.documentId;
}

export function ProjectViewContextSection({
  contextCapability,
  object,
  objectsById,
  onRefresh,
  onSelectObject,
  projectRevision,
}: {
  contextCapability: boolean;
  object: ProjectViewObject;
  objectsById: ReadonlyMap<string, ProjectViewObject>;
  onRefresh: () => Promise<unknown>;
  onSelectObject: (objectId: string) => void;
  projectRevision: number;
}) {
  const mutation = useProjectViewMutation();
  const metaQuery = useProjectDocumentMeta(contextCapability);
  const documentsQuery = useProjectDocuments(
    contextCapability ? metaQuery.data : undefined,
  );
  const references = object.contextReferences ?? [];
  const [targetKind, setTargetKind] = React.useState<TargetKind>(
    object.objectType === "resource" ? "live_document" : "resource",
  );
  const [targetId, setTargetId] = React.useState("");
  const [pinnedRevision, setPinnedRevision] = React.useState("");
  const [message, setMessage] = React.useState<string>();
  const documents = React.useMemo(
    () =>
      new Map(
        documentsQuery.data?.documents.map((document) => [
          document.documentId,
          document.title,
        ]) ?? [],
      ),
    [documentsQuery.data?.documents],
  );
  const resourceOptions = React.useMemo(
    () =>
      [...objectsById.values()].filter(
        (candidate) => candidate.objectType === "resource",
      ),
    [objectsById],
  );
  const documentOptions = documentsQuery.data?.documents ?? [];
  const isDocumentTarget = targetKind !== "resource";
  const canAdd =
    contextCapability &&
    Boolean(targetId) &&
    (targetKind !== "pinned_document" ||
      (Number.isSafeInteger(Number(pinnedRevision)) &&
        Number(pinnedRevision) > 0));

  async function replace(next: ProjectViewContextReference[]) {
    setMessage(undefined);
    try {
      const result = await mutation.mutateAsync({
        operation: "context",
        expectedProjectRevision: projectRevision,
        objectType: object.objectType,
        objectId: object.id,
        contextReferences: canonicalizeProjectViewContextReferences(next),
      });
      if (result.status === "conflict") {
        setMessage(
          "Project View changed. Refresh the verified snapshot and retry.",
        );
        return;
      }
      setTargetId("");
      setPinnedRevision("");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  }

  function addReference() {
    if (!canAdd) return;
    const reference: ProjectViewContextReference =
      targetKind === "resource"
        ? { referenceType: "resource", resourceId: targetId }
        : {
            referenceType: "document",
            documentId: targetId,
            mode: targetKind === "pinned_document" ? "pinned" : "live",
            ...(targetKind === "pinned_document"
              ? { documentRevision: Number(pinnedRevision) }
              : {}),
          };
    if (
      references.some(
        (candidate) => referenceKey(candidate) === referenceKey(reference),
      )
    ) {
      setMessage("That exact Context coordinate is already present.");
      return;
    }
    void replace([...references, reference]);
  }

  function removeReference(reference: ProjectViewContextReference) {
    void replace(
      references.filter(
        (candidate) => referenceKey(candidate) !== referenceKey(reference),
      ),
    );
  }

  return (
    <section className="space-y-3" data-testid="project-view-context">
      <div className="flex items-center justify-between gap-2">
        <div>
          <h3 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Context
          </h3>
          <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
            Verified coordinates only. References do not grant permission or run
            content.
          </p>
        </div>
        <Badge variant={contextCapability ? "success" : "outline"}>
          {contextCapability ? "Ready" : "Unavailable"}
        </Badge>
      </div>

      {references.length === 0 ? (
        <p className="rounded-lg border border-dashed border-border px-3 py-2 text-xs text-muted-foreground">
          No Context References.
        </p>
      ) : (
        <div className="space-y-2">
          {references.map((reference) => {
            const label = contextLabel(reference, objectsById, documents);
            return (
              <div
                className="flex items-center gap-2 rounded-lg border border-border/70 bg-muted/20 px-2.5 py-2"
                data-testid={`context-${referenceKey(reference)}`}
                key={referenceKey(reference)}
              >
                {reference.referenceType === "resource" ? (
                  <Link2 className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                ) : (
                  <FileText className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                )}
                <div className="min-w-0 flex-1">
                  <div className="truncate text-xs font-medium">{label}</div>
                  <div className="truncate text-2xs text-muted-foreground">
                    {reference.referenceType === "resource"
                      ? "Resource"
                      : reference.mode === "live"
                        ? "Live Document"
                        : `Pinned Document · revision ${reference.documentRevision}`}
                  </div>
                </div>
                {reference.referenceType === "resource" ? (
                  <Button
                    aria-label={`Open Resource ${label}`}
                    onClick={() => onSelectObject(reference.resourceId)}
                    size="icon"
                    type="button"
                    variant="ghost"
                  >
                    <ArrowRight />
                  </Button>
                ) : (
                  <Button asChild size="icon" variant="ghost">
                    <Link
                      aria-label={`Open Document ${label}`}
                      search={{
                        document: reference.documentId,
                        revision:
                          reference.mode === "pinned"
                            ? reference.documentRevision
                            : undefined,
                      }}
                      to="/documents"
                    >
                      <ArrowRight />
                    </Link>
                  </Button>
                )}
                <Button
                  aria-label={`Remove Context ${label}`}
                  disabled={mutation.isPending}
                  onClick={() => removeReference(reference)}
                  size="icon"
                  title={
                    contextCapability
                      ? "Remove Context Reference"
                      : "Remove preserved coordinate while Context is unavailable"
                  }
                  type="button"
                  variant="ghost"
                >
                  <Trash2 />
                </Button>
              </div>
            );
          })}
        </div>
      )}

      {contextCapability ? (
        <div className="space-y-2 rounded-lg border border-border/70 p-3">
          <div className="grid gap-2 sm:grid-cols-2">
            <select
              aria-label="Context target type"
              className="h-9 rounded-md border border-input bg-background px-3 text-xs"
              onChange={(event) => {
                setTargetKind(event.target.value as TargetKind);
                setTargetId("");
                setPinnedRevision("");
              }}
              value={targetKind}
            >
              {object.objectType === "resource" ? null : (
                <option value="resource">Resource</option>
              )}
              <option value="live_document">Live Document</option>
              <option value="pinned_document">Pinned Document</option>
            </select>
            <select
              aria-label="Context target"
              className="h-9 min-w-0 rounded-md border border-input bg-background px-3 text-xs"
              disabled={isDocumentTarget && !documentsQuery.data}
              onChange={(event) => setTargetId(event.target.value)}
              value={targetId}
            >
              <option value="">Select target…</option>
              {targetKind === "resource"
                ? resourceOptions.map((resource) => (
                    <option key={resource.id} value={resource.id}>
                      {projectViewObjectTitle(resource)}
                    </option>
                  ))
                : documentOptions.map((document) => (
                    <option
                      key={document.documentId}
                      value={document.documentId}
                    >
                      {document.title}
                    </option>
                  ))}
            </select>
          </div>
          {targetKind === "pinned_document" ? (
            <Input
              aria-label="Pinned Document revision"
              min={1}
              onChange={(event) => setPinnedRevision(event.target.value)}
              placeholder="Exact revision"
              type="number"
              value={pinnedRevision}
            />
          ) : null}
          <Button
            disabled={!canAdd || mutation.isPending}
            onClick={addReference}
            size="sm"
            type="button"
            variant="outline"
          >
            <Plus />
            Add Context
          </Button>
          {documentsQuery.isError && isDocumentTarget ? (
            <p className="text-xs text-destructive">
              Document metadata is unavailable; existing coordinates remain
              readable.
            </p>
          ) : null}
        </div>
      ) : references.length > 0 ? (
        <p className="rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs text-muted-foreground">
          Context is temporarily unavailable. Preserved coordinates remain
          visible and may be removed, but cannot be added or retargeted.
        </p>
      ) : null}

      {message ? (
        <div className="space-y-2 rounded-lg border border-destructive/40 bg-destructive/5 px-3 py-2 text-xs">
          <p>{message}</p>
          <Button
            onClick={() => void onRefresh()}
            size="sm"
            type="button"
            variant="outline"
          >
            Refresh verified View
          </Button>
        </div>
      ) : null}
    </section>
  );
}
