import {
  AlertCircle,
  FileText,
  RefreshCw,
  ShieldCheck,
  Wifi,
} from "lucide-react";
import * as React from "react";

import {
  identityFromMeta,
  useProjectDocument,
  useProjectDocumentLiveSync,
  useProjectDocumentMeta,
  useProjectDocuments,
} from "@/features/project-documents/hooks";
import { DocumentEditor } from "@/features/project-documents/ui/DocumentEditor";
import { DocumentListPane } from "@/features/project-documents/ui/DocumentListPane";
import { DocumentViewer } from "@/features/project-documents/ui/DocumentViewer";
import { ProjectDocumentError } from "@/shared/api/tauriProjectDocument";
import { TopChromeInsetHeader } from "@/shared/layout/TopChromeInsetHeader";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

export function ProjectDocumentsScreen({
  onSelectDocument,
  onShowInProjectContext,
  selectedDocumentId,
  selectedRevision,
}: {
  onSelectDocument: (documentId?: string, revision?: number) => void;
  onShowInProjectContext?: (documentId: string) => void;
  selectedDocumentId?: string;
  selectedRevision?: number;
}) {
  const metaQuery = useProjectDocumentMeta();
  const listQuery = useProjectDocuments(metaQuery.data);
  const identity = metaQuery.data
    ? identityFromMeta(metaQuery.data)
    : undefined;
  const currentQuery = useProjectDocument({
    identity,
    documentId: selectedDocumentId,
  });
  const pinnedQuery = useProjectDocument({
    identity,
    documentId: selectedDocumentId,
    revision: selectedRevision,
    enabled: selectedRevision !== undefined,
  });
  const liveStatus = useProjectDocumentLiveSync(metaQuery.data);
  const [editorMode, setEditorMode] = React.useState<"create" | "update">();
  const displayedDocument = selectedRevision
    ? pinnedQuery.data
    : currentQuery.data;
  const selectedListItem = listQuery.data?.documents.find(
    (document) => document.documentId === selectedDocumentId,
  );
  const fatalError = metaQuery.error ?? listQuery.error;
  const isUnsupported =
    fatalError instanceof ProjectDocumentError &&
    fatalError.code === "unsupported";
  const isRestricted =
    fatalError instanceof ProjectDocumentError &&
    fatalError.code === "restricted";

  function selectDocument(documentId?: string, revision?: number) {
    setEditorMode(undefined);
    onSelectDocument(documentId, revision);
  }

  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-testid="project-documents-screen"
    >
      <TopChromeInsetHeader flush>
        <header
          className="flex h-12 items-center gap-2 px-3 sm:gap-3 sm:px-5"
          data-tauri-drag-region
        >
          <FileText className="h-4 w-4 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <div className="text-sm font-semibold">Documents</div>
            <div className="hidden text-2xs text-muted-foreground sm:block">
              Verified, revisioned project Markdown
            </div>
          </div>
          {metaQuery.data ? (
            <Badge variant="success">
              <ShieldCheck className="mr-1 h-3 w-3" />
              Verified
            </Badge>
          ) : null}
          {liveStatus === "live" ? (
            <Badge className="hidden sm:inline-flex" variant="outline">
              <Wifi className="mr-1 h-3 w-3" />
              Live
            </Badge>
          ) : liveStatus === "retrying" ? (
            <Badge className="hidden sm:inline-flex" variant="warning">
              Reconnecting
            </Badge>
          ) : null}
          <Button
            aria-label="Refresh Documents"
            disabled={metaQuery.isFetching || listQuery.isFetching}
            onClick={() => void metaQuery.refetch()}
            size="icon"
            type="button"
            variant="ghost"
          >
            <RefreshCw
              className={`h-4 w-4 ${metaQuery.isFetching || listQuery.isFetching ? "animate-spin" : ""}`}
            />
          </Button>
        </header>
      </TopChromeInsetHeader>

      {metaQuery.isLoading || (metaQuery.data && listQuery.isLoading) ? (
        <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
          Verifying Document metadata…
        </div>
      ) : isUnsupported || isRestricted ? (
        <div className="flex flex-1 items-center justify-center p-6">
          <div className="max-w-md rounded-2xl border border-border/70 bg-card/60 p-6 text-center">
            <AlertCircle className="mx-auto h-8 w-8 text-muted-foreground" />
            <h1 className="mt-3 text-base font-semibold">
              {isUnsupported
                ? "Documents are not enabled"
                : "Documents are restricted"}
            </h1>
            <p className="mt-2 text-sm text-muted-foreground">
              {fatalError instanceof Error
                ? fatalError.message
                : "The verified Document catalog is unavailable."}
            </p>
          </div>
        </div>
      ) : fatalError ? (
        <div className="flex flex-1 items-center justify-center p-6">
          <div className="max-w-lg rounded-2xl border border-destructive/30 bg-destructive/10 p-6 text-center">
            <AlertCircle className="mx-auto h-8 w-8 text-destructive" />
            <h1 className="mt-3 text-base font-semibold">
              Document verification failed
            </h1>
            <p className="mt-2 text-sm text-muted-foreground">
              {fatalError instanceof Error
                ? fatalError.message
                : "The Relay returned an invalid Project Document response."}
            </p>
            <Button
              className="mt-4"
              onClick={() => void metaQuery.refetch()}
              size="sm"
              type="button"
              variant="outline"
            >
              Try again
            </Button>
          </div>
        </div>
      ) : metaQuery.data && listQuery.data && identity ? (
        <div className="flex min-h-0 flex-1">
          <DocumentListPane
            documents={listQuery.data.documents}
            isFetching={listQuery.isFetching}
            onCreate={() => {
              onSelectDocument(undefined, undefined);
              setEditorMode("create");
            }}
            onSelect={(documentId) => selectDocument(documentId)}
            selectedDocumentId={selectedDocumentId}
          />

          {editorMode === "create" ? (
            <DocumentEditor
              identity={identity}
              onApplied={(documentId) => selectDocument(documentId)}
              onCancel={() => setEditorMode(undefined)}
            />
          ) : editorMode === "update" && currentQuery.data ? (
            <DocumentEditor
              base={currentQuery.data}
              identity={identity}
              latest={currentQuery.data}
              onApplied={(documentId) => selectDocument(documentId)}
              onCancel={() => setEditorMode(undefined)}
            />
          ) : !selectedDocumentId ? (
            <div className="flex min-w-0 flex-1 items-center justify-center p-6 text-center">
              <div className="max-w-sm">
                <FileText className="mx-auto h-10 w-10 text-muted-foreground" />
                <h1 className="mt-3 text-base font-semibold">
                  Select a Document
                </h1>
                <p className="mt-1 text-sm text-muted-foreground">
                  The catalog above contains metadata only. Markdown is fetched
                  and verified after you choose one Document.
                </p>
              </div>
            </div>
          ) : currentQuery.isLoading ||
            (selectedRevision !== undefined && pinnedQuery.isLoading) ? (
            <div className="flex min-w-0 flex-1 items-center justify-center text-sm text-muted-foreground">
              Verifying Document revision…
            </div>
          ) : displayedDocument ? (
            <DocumentViewer
              currentRevision={
                currentQuery.data?.documentRevision ??
                selectedListItem?.documentRevision ??
                displayedDocument.documentRevision
              }
              document={displayedDocument}
              identity={identity}
              onDeleted={() => selectDocument(undefined)}
              onEdit={() => setEditorMode("update")}
              onSelectRevision={(revision) =>
                onSelectDocument(selectedDocumentId, revision)
              }
              onShowInProjectContext={onShowInProjectContext}
              selectedRevision={selectedRevision}
            />
          ) : (
            <div className="flex min-w-0 flex-1 items-center justify-center p-6 text-center">
              <div className="max-w-lg rounded-2xl border border-destructive/30 bg-destructive/10 p-5">
                <AlertCircle className="mx-auto h-7 w-7 text-destructive" />
                <h1 className="mt-2 text-sm font-semibold">
                  Document verification failed
                </h1>
                <p className="mt-1 text-xs text-muted-foreground">
                  {(selectedRevision
                    ? pinnedQuery.error
                    : currentQuery.error) instanceof Error
                    ? (selectedRevision
                        ? pinnedQuery.error
                        : currentQuery.error
                      )?.message
                    : "The requested signed revision could not be verified."}
                </p>
              </div>
            </div>
          )}
        </div>
      ) : null}
    </div>
  );
}
