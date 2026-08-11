import {
  AlertTriangle,
  Clipboard,
  Eye,
  FilePenLine,
  Save,
  X,
} from "lucide-react";
import * as React from "react";

import { useProjectDocumentMutation } from "@/features/project-documents/hooks";
import { DocumentDiff } from "@/features/project-documents/ui/DocumentDiff";
import type {
  ProjectDocument,
  ProjectDocumentIdentity,
} from "@/shared/api/tauriProjectDocument";
import { copyTextToClipboard } from "@/shared/lib/clipboard";
import { Alert, AlertDescription, AlertTitle } from "@/shared/ui/alert";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { Markdown } from "@/shared/ui/markdown";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/shared/ui/tabs";
import { Textarea } from "@/shared/ui/textarea";

type DocumentSnapshot = {
  title: string;
  summary: string;
  contentMarkdown: string;
};

function snapshotOf(document?: ProjectDocument): DocumentSnapshot {
  return {
    title: document?.title ?? "",
    summary: document?.summary ?? "",
    contentMarkdown: document?.contentMarkdown ?? "",
  };
}

export function DocumentEditor({
  base,
  identity,
  latest,
  onApplied,
  onCancel,
}: {
  base?: ProjectDocument;
  identity: ProjectDocumentIdentity;
  latest?: ProjectDocument;
  onApplied: (documentId: string) => void;
  onCancel: () => void;
}) {
  const initial = React.useMemo(() => snapshotOf(base), [base]);
  const [workingBase, setWorkingBase] = React.useState(() => ({
    revision: base?.documentRevision ?? 0,
    snapshot: initial,
  }));
  const [draft, setDraft] = React.useState<DocumentSnapshot>(initial);
  const [conflict, setConflict] = React.useState<{
    currentRevision?: number;
    local: DocumentSnapshot;
  }>();
  const [showConflictDiff, setShowConflictDiff] = React.useState(false);
  const mutation = useProjectDocumentMutation();
  const isCreate = !base;
  const latestActive = latest?.state === "active" ? latest : undefined;
  const verifiedConflictLatest =
    conflict &&
    latestActive &&
    latestActive.documentRevision > workingBase.revision &&
    (conflict.currentRevision === undefined ||
      latestActive.documentRevision >= conflict.currentRevision)
      ? latestActive
      : undefined;

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    const submittedDraft = { ...draft };
    const title = submittedDraft.title.trim();
    if (!title) return;
    const summary = submittedDraft.summary.trim() || undefined;
    try {
      const result = await mutation.mutateAsync({
        identity,
        mutation: isCreate
          ? {
              type: "create",
              title,
              summary,
              contentMarkdown: submittedDraft.contentMarkdown,
            }
          : {
              type: "update",
              documentId: base.documentId,
              expectedDocumentRevision: workingBase.revision,
              title,
              summary,
              contentMarkdown: submittedDraft.contentMarkdown,
            },
      });
      if (result.status === "conflict") {
        setDraft(submittedDraft);
        setConflict({
          currentRevision: result.currentDocumentRevision,
          local: submittedDraft,
        });
        return;
      }
      onApplied(result.documentId);
    } catch {
      // React Query owns the visible error; keep the complete local draft.
    }
  }

  function reloadLatest() {
    if (!verifiedConflictLatest) return;
    const next = snapshotOf(verifiedConflictLatest);
    setDraft(next);
    setWorkingBase({
      revision: verifiedConflictLatest.documentRevision,
      snapshot: next,
    });
    setConflict(undefined);
    setShowConflictDiff(false);
  }

  function useLatestAsBase() {
    if (!verifiedConflictLatest) return;
    setWorkingBase({
      revision: verifiedConflictLatest.documentRevision,
      snapshot: snapshotOf(verifiedConflictLatest),
    });
    setConflict(undefined);
    setShowConflictDiff(false);
  }

  return (
    <form
      className="flex min-h-0 flex-1 flex-col"
      data-testid="document-editor"
      onSubmit={(event) => void submit(event)}
    >
      <div className="flex items-center gap-2 border-b border-border/70 px-5 py-3">
        <FilePenLine className="h-4 w-4 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="text-sm font-semibold">
            {isCreate
              ? "New Document"
              : `Edit revision ${workingBase.revision}`}
          </div>
          <div className="text-2xs text-muted-foreground">
            Every save creates one immutable full Markdown revision.
          </div>
        </div>
        <Button onClick={onCancel} size="sm" type="button" variant="ghost">
          <X className="h-4 w-4" />
          Cancel
        </Button>
        <Button
          data-testid="document-save"
          disabled={mutation.isPending || !draft.title.trim()}
          size="sm"
          type="submit"
        >
          <Save className="h-4 w-4" />
          {mutation.isPending ? "Saving…" : "Save revision"}
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto p-5">
        <Alert className="mb-4 border-warning/40 bg-warning/10">
          <AlertTriangle className="h-4 w-4 text-warning" />
          <AlertTitle>Documents are not a Secret Store</AlertTitle>
          <AlertDescription>
            Do not paste passwords, access tokens, private keys, credentials, or
            other secrets. Document revisions are durable project records.
          </AlertDescription>
        </Alert>

        {conflict ? (
          <Alert
            className="mb-4 border-destructive/40 bg-destructive/10"
            data-testid="document-conflict"
          >
            <AlertTriangle className="h-4 w-4 text-destructive" />
            <AlertTitle>Your draft was preserved</AlertTitle>
            <AlertDescription>
              <p>
                You edited revision {workingBase.revision}; the latest verified
                revision is{" "}
                {verifiedConflictLatest?.documentRevision ??
                  conflict.currentRevision ??
                  "loading"}
                . Carryforth did not overwrite or automatically rebase your
                content.
              </p>
              <div className="mt-3 flex flex-wrap gap-2">
                <Button
                  onClick={() =>
                    copyTextToClipboard(
                      conflict.local.contentMarkdown,
                      "Local Markdown copied",
                    )
                  }
                  size="sm"
                  type="button"
                  variant="outline"
                >
                  <Clipboard className="h-4 w-4" />
                  Copy local content
                </Button>
                <Button
                  disabled={!verifiedConflictLatest}
                  onClick={reloadLatest}
                  size="sm"
                  type="button"
                  variant="outline"
                >
                  Reload latest
                </Button>
                <Button
                  onClick={() => setShowConflictDiff((shown) => !shown)}
                  size="sm"
                  type="button"
                  variant="outline"
                >
                  <Eye className="h-4 w-4" />
                  {showConflictDiff ? "Hide diff" : "View diff"}
                </Button>
                <Button
                  disabled={!verifiedConflictLatest}
                  onClick={useLatestAsBase}
                  size="sm"
                  type="button"
                  variant="outline"
                >
                  Edit on latest
                </Button>
              </div>
            </AlertDescription>
          </Alert>
        ) : null}

        {showConflictDiff && conflict ? (
          <div className="mb-4 grid gap-3 xl:grid-cols-2">
            <DocumentDiff
              after={conflict.local.contentMarkdown}
              before={workingBase.snapshot.contentMarkdown}
              label={`Base r${workingBase.revision} → local draft`}
            />
            {verifiedConflictLatest ? (
              <DocumentDiff
                after={conflict.local.contentMarkdown}
                before={verifiedConflictLatest.contentMarkdown ?? ""}
                label={`Latest r${verifiedConflictLatest.documentRevision} → local draft`}
              />
            ) : null}
          </div>
        ) : null}

        {mutation.error instanceof Error ? (
          <p className="mb-4 rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {mutation.error.message}
          </p>
        ) : null}

        <div className="grid gap-4">
          <label
            className="grid gap-1.5 text-xs font-medium"
            htmlFor="project-document-title"
          >
            Title
            <Input
              data-testid="document-title-input"
              disabled={mutation.isPending}
              id="project-document-title"
              maxLength={200}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  title: event.target.value,
                }))
              }
              placeholder="Document title"
              value={draft.title}
            />
          </label>
          <label
            className="grid gap-1.5 text-xs font-medium"
            htmlFor="project-document-summary"
          >
            Summary <span className="text-muted-foreground">(optional)</span>
            <Input
              data-testid="document-summary-input"
              disabled={mutation.isPending}
              id="project-document-summary"
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  summary: event.target.value,
                }))
              }
              placeholder="What this document is for"
              value={draft.summary}
            />
          </label>
          <Tabs defaultValue="write">
            <TabsList>
              <TabsTrigger value="write">Write</TabsTrigger>
              <TabsTrigger value="preview">Preview</TabsTrigger>
            </TabsList>
            <TabsContent value="write">
              <Textarea
                aria-label="Markdown content"
                className="min-h-96 resize-y font-mono text-sm"
                data-testid="document-content-input"
                disabled={mutation.isPending}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    contentMarkdown: event.target.value,
                  }))
                }
                placeholder="Write Markdown…"
                value={draft.contentMarkdown}
              />
            </TabsContent>
            <TabsContent value="preview">
              <div
                className="min-h-96 rounded-xl border border-border/70 bg-card/50 p-4"
                data-testid="document-preview"
              >
                {draft.contentMarkdown ? (
                  <Markdown
                    content={draft.contentMarkdown}
                    interactive={false}
                  />
                ) : (
                  <p className="text-sm text-muted-foreground">
                    Nothing to preview yet.
                  </p>
                )}
              </div>
            </TabsContent>
          </Tabs>
        </div>
      </div>
    </form>
  );
}
