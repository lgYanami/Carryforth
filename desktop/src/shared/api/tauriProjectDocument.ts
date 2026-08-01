import { invokeTauri, TauriInvokeError } from "@/shared/api/tauri";

export type ProjectDocumentState = "active" | "deleted";

export type ProjectDocumentIdentity = {
  communityKey: string;
  projectId: string;
  relayPubkey: string;
  projectionGeneration: number;
};

export type ProjectDocumentMeta = ProjectDocumentIdentity & {
  catalogRevision: number;
  activeDocumentCount: number;
  updatedAt: string;
  metaEventId: string;
};

export type ProjectDocumentListItem = {
  documentId: string;
  title: string;
  summary?: string;
  documentRevision: number;
  updatedAt: string;
  updatedBy: string;
  headEventId: string;
};

export type ProjectDocumentList = ProjectDocumentIdentity & {
  catalogRevision: number;
  documents: ProjectDocumentListItem[];
};

export type ProjectDocument = ProjectDocumentIdentity & {
  documentId: string;
  documentRevision: number;
  state: ProjectDocumentState;
  title?: string;
  summary?: string;
  contentMarkdown?: string;
  createdAt: string;
  createdBy: string;
  revisionAt: string;
  revisionBy: string;
  revisionEventId: string;
  headEventId?: string;
  sourceEventId: string;
};

export type ProjectDocumentHistoryItem = {
  documentRevision: number;
  state: ProjectDocumentState;
  actor: string;
  canonicalAt: string;
  revisionEventId: string;
};

export type ProjectDocumentHistory = ProjectDocumentIdentity & {
  documentId: string;
  maxDocumentRevision: number;
  revisions: ProjectDocumentHistoryItem[];
};

export type ProjectDocumentMutation =
  | {
      type: "create";
      documentId?: string;
      title: string;
      summary?: string;
      contentMarkdown: string;
    }
  | {
      type: "update";
      documentId: string;
      expectedDocumentRevision: number;
      title: string;
      summary?: string;
      contentMarkdown: string;
    }
  | {
      type: "delete";
      documentId: string;
      expectedDocumentRevision: number;
    };

export type ProjectDocumentMutationResult =
  | {
      status: "applied";
      communityKey: string;
      documentId: string;
      documentRevision: number;
      catalogRevision: number;
      eventId: string;
      confirmation: "receipt_and_readback" | "readback";
      state: ProjectDocumentState;
    }
  | {
      status: "conflict";
      communityKey: string;
      documentId: string;
      expectedDocumentRevision: number;
      currentDocumentRevision?: number;
    };

export type ProjectDocumentErrorCode =
  | "snapshot_conflict"
  | "revision_conflict"
  | "unavailable"
  | "delivery_unknown"
  | "restricted"
  | "unsupported"
  | "not_found"
  | "invalid_input"
  | "internal";

type ProjectDocumentErrorPayload = {
  code: ProjectDocumentErrorCode;
  message: string;
  status?: number;
  retryable: boolean;
  retryAfterSeconds?: number;
  eventId?: string;
};

export class ProjectDocumentError extends Error {
  readonly code: ProjectDocumentErrorCode;
  readonly status?: number;
  readonly retryable: boolean;
  readonly retryAfterSeconds?: number;
  readonly eventId?: string;

  constructor(payload: ProjectDocumentErrorPayload) {
    super(payload.message);
    this.name = "ProjectDocumentError";
    this.code = payload.code;
    this.status = payload.status;
    this.retryable = payload.retryable;
    this.retryAfterSeconds = payload.retryAfterSeconds;
    this.eventId = payload.eventId;
  }
}

function isErrorPayload(value: unknown): value is ProjectDocumentErrorPayload {
  return (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    typeof value.code === "string" &&
    "message" in value &&
    typeof value.message === "string" &&
    "retryable" in value &&
    typeof value.retryable === "boolean"
  );
}

async function invokeDocument<T>(
  command: string,
  args: Record<string, unknown>,
): Promise<T> {
  try {
    return await invokeTauri<T>(command, args);
  } catch (error) {
    const payload =
      error instanceof TauriInvokeError ? error.payload : (error as unknown);
    if (isErrorPayload(payload)) throw new ProjectDocumentError(payload);
    throw error;
  }
}

function requireCommunity<T extends { communityKey: string }>(
  expectedCommunityKey: string,
  value: T,
): T {
  if (value.communityKey !== expectedCommunityKey) {
    throw new ProjectDocumentError({
      code: "snapshot_conflict",
      message: "The active Community changed while Documents were loading.",
      retryable: true,
    });
  }
  return value;
}

export function documentIdentity(
  meta: ProjectDocumentMeta,
): ProjectDocumentIdentity {
  return {
    communityKey: meta.communityKey,
    projectId: meta.projectId,
    relayPubkey: meta.relayPubkey,
    projectionGeneration: meta.projectionGeneration,
  };
}

export async function getProjectDocumentMeta(
  communityKey: string,
): Promise<ProjectDocumentMeta> {
  return requireCommunity(
    communityKey,
    await invokeDocument<ProjectDocumentMeta>("get_project_document_meta", {
      communityKey,
    }),
  );
}

export async function listProjectDocuments(
  meta: ProjectDocumentMeta,
): Promise<ProjectDocumentList> {
  return requireCommunity(
    meta.communityKey,
    await invokeDocument<ProjectDocumentList>("list_project_documents", {
      input: {
        ...documentIdentity(meta),
        catalogRevision: meta.catalogRevision,
      },
    }),
  );
}

export async function getProjectDocument(input: {
  identity: ProjectDocumentIdentity;
  documentId: string;
  revision?: number;
}): Promise<ProjectDocument> {
  return requireCommunity(
    input.identity.communityKey,
    await invokeDocument<ProjectDocument>("get_project_document", {
      input: {
        ...input.identity,
        documentId: input.documentId,
        revision: input.revision,
      },
    }),
  );
}

export async function getProjectDocumentHistory(input: {
  identity: ProjectDocumentIdentity;
  documentId: string;
  maxDocumentRevision: number;
}): Promise<ProjectDocumentHistory> {
  return requireCommunity(
    input.identity.communityKey,
    await invokeDocument<ProjectDocumentHistory>(
      "get_project_document_history",
      {
        input: {
          ...input.identity,
          documentId: input.documentId,
          maxDocumentRevision: input.maxDocumentRevision,
        },
      },
    ),
  );
}

export async function mutateProjectDocument(input: {
  identity: ProjectDocumentIdentity;
  mutation: ProjectDocumentMutation;
}): Promise<ProjectDocumentMutationResult> {
  return requireCommunity(
    input.identity.communityKey,
    await invokeDocument<ProjectDocumentMutationResult>(
      "mutate_project_document",
      {
        input: {
          ...input.identity,
          mutation: input.mutation,
        },
      },
    ),
  );
}
