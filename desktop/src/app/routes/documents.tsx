import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { usePreviewFeatureWarning } from "@/shared/features";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";

const ProjectDocumentsScreen = React.lazy(async () => {
  const module = await import(
    "@/features/project-documents/ui/ProjectDocumentsScreen"
  );
  return { default: module.ProjectDocumentsScreen };
});

type DocumentsRouteSearch = {
  document?: string;
  revision?: number;
};

function validateDocumentsSearch(
  search: Record<string, unknown>,
): DocumentsRouteSearch {
  const revision =
    typeof search.revision === "number"
      ? search.revision
      : typeof search.revision === "string"
        ? Number(search.revision)
        : undefined;
  return {
    document:
      typeof search.document === "string" && search.document.length > 0
        ? search.document
        : undefined,
    revision:
      revision !== undefined && Number.isSafeInteger(revision) && revision > 0
        ? revision
        : undefined,
  };
}

export const Route = createFileRoute("/documents")({
  validateSearch: validateDocumentsSearch,
  component: DocumentsRouteComponent,
});

function DocumentsRouteComponent() {
  usePreviewFeatureWarning("projectView");
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const { goProjectContext } = useAppNavigation();
  return (
    <React.Suspense fallback={<ViewLoadingFallback kind="documents" />}>
      <ProjectDocumentsScreen
        onSelectDocument={(document, revision) =>
          void navigate({
            search: {
              document,
              revision: document ? revision : undefined,
            },
          })
        }
        onShowInProjectContext={(documentId) =>
          void goProjectContext({
            query: {
              type: "incident",
              coordinate: { type: "document", documentId },
            },
          })
        }
        selectedDocumentId={search.document}
        selectedRevision={search.revision}
      />
    </React.Suspense>
  );
}
