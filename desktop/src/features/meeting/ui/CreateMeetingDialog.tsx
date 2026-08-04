import * as React from "react";
import {
  ArrowDown,
  ArrowUp,
  Plus,
  RotateCcw,
  Trash2,
  TriangleAlert,
} from "lucide-react";

import { useRelayAgentsQuery } from "@/features/agents/hooks";
import {
  useChannelMembersQuery,
  useChatRooms,
} from "@/features/channels/hooks";
import {
  MAX_MEETING_BOARD_BYTES,
  buildInitialMeetingBoard,
  checkMeetingSourceAccess,
  dedupeMeetingRosterCandidates,
  validateMeetingDraft,
} from "@/features/meeting/createMeetingModel";
import {
  useCreateMeetingMutation,
  useMeetingCapability,
} from "@/features/meeting/hooks";
import { useNewMessageRecipients } from "@/features/messages/ui/useNewMessageRecipients";
import { useProfileQuery } from "@/features/profile/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import type { CreateMeetingInput } from "@/shared/api/tauriMeetings";
import { normalizePubkey, truncatePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";
import { Textarea } from "@/shared/ui/textarea";

import { MeetingRosterPicker } from "./MeetingRosterPicker";

type AgendaItem = { id: string; value: string };

function newAgendaItem(): AgendaItem {
  return { id: crypto.randomUUID(), value: "" };
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}

export function CreateMeetingDialog({
  initialSourceChannelId,
  onCreated,
  onOpenChange,
  open,
  requestVersion,
}: {
  initialSourceChannelId: string | null;
  onCreated: (meetingId: string) => void;
  onOpenChange: (open: boolean) => void;
  open: boolean;
  requestVersion: number;
}) {
  const identityQuery = useIdentityQuery();
  const profileQuery = useProfileQuery();
  const relayAgentsQuery = useRelayAgentsQuery({ enabled: open });
  const channelsQuery = useChatRooms({ enabled: open });
  const capabilityQuery = useMeetingCapability();
  const createMutation = useCreateMeetingMutation();

  const currentPubkey = identityQuery.data?.pubkey ?? "";
  const [title, setTitle] = React.useState("");
  const [goal, setGoal] = React.useState("");
  const [agenda, setAgenda] = React.useState<AgendaItem[]>([]);
  const [background, setBackground] = React.useState("");
  const [references, setReferences] = React.useState("");
  const [sourceChannelId, setSourceChannelId] = React.useState("");
  const [customBoard, setCustomBoard] = React.useState<string | null>(null);
  const [pendingInput, setPendingInput] =
    React.useState<CreateMeetingInput | null>(null);
  const [indeterminateMessage, setIndeterminateMessage] = React.useState<
    string | null
  >(null);
  const [submitError, setSubmitError] = React.useState<string | null>(null);
  const [submitAttempted, setSubmitAttempted] = React.useState(false);

  const roster = useNewMessageRecipients({
    active: open,
    currentPubkey,
    includeAllRelayAgents: true,
    recipientLimit: 11,
  });

  React.useEffect(() => {
    if (requestVersion === 0 || !open || indeterminateMessage) {
      return;
    }
    setSourceChannelId(initialSourceChannelId ?? "");
  }, [indeterminateMessage, initialSourceChannelId, open, requestVersion]);

  const ordinarySourceChannels = React.useMemo(
    () =>
      (channelsQuery.data ?? []).filter(
        (channel) => channel.channelType !== "dm" && channel.roomKind === null,
      ),
    [channelsQuery.data],
  );
  const sourceChannel =
    ordinarySourceChannels.find((channel) => channel.id === sourceChannelId) ??
    null;
  const sourceSelectionUnavailable =
    sourceChannelId.length > 0 && sourceChannel === null;
  const sourceMembersQuery = useChannelMembersQuery(
    sourceChannel?.id ?? null,
    open && sourceChannel?.visibility === "private",
  );

  const generatedBoard = React.useMemo(
    () =>
      buildInitialMeetingBoard({
        title,
        goal,
        agenda: agenda.map((item) => item.value),
        background,
        references,
      }),
    [agenda, background, goal, references, title],
  );
  const board = customBoard ?? generatedBoard;
  const boardBytes = React.useMemo(
    () => new TextEncoder().encode(board).byteLength,
    [board],
  );

  const selectedCandidates = React.useMemo(
    () =>
      dedupeMeetingRosterCandidates(
        roster.selectedUsers,
        relayAgentsQuery.data ?? [],
      ),
    [relayAgentsQuery.data, roster.selectedUsers],
  );
  const selectedPubkeys = React.useMemo(
    () => new Set(selectedCandidates.map((candidate) => candidate.pubkey)),
    [selectedCandidates],
  );
  const directoryCandidates = React.useMemo(
    () =>
      dedupeMeetingRosterCandidates(
        roster.searchResults,
        relayAgentsQuery.data ?? [],
      ).filter((candidate) => !selectedPubkeys.has(candidate.pubkey)),
    [relayAgentsQuery.data, roster.searchResults, selectedPubkeys],
  );
  const rosterPubkeys = React.useMemo(
    () => [currentPubkey, ...selectedCandidates.map((item) => item.pubkey)],
    [currentPubkey, selectedCandidates],
  );
  const sourceAccess = checkMeetingSourceAccess({
    sourceVisibility: sourceChannel?.visibility ?? null,
    rosterPubkeys,
    memberPubkeys: sourceMembersQuery.data?.map((member) => member.pubkey),
    membersLoading: sourceMembersQuery.isLoading,
    membersUnavailable: sourceMembersQuery.isError,
  });
  const validationErrors = validateMeetingDraft({
    title,
    goal,
    participantPubkeys: selectedCandidates.map((candidate) => candidate.pubkey),
    board,
  });
  const incompatibleAgents = selectedCandidates.filter(
    (candidate) =>
      candidate.isAgent && candidate.actionCapability === "incompatible",
  );
  const relayCanCreate = Boolean(
    !capabilityQuery.error &&
      capabilityQuery.data?.status === "creatable" &&
      capabilityQuery.data.supportsDirectActions &&
      capabilityQuery.data.canCreateDirectActions,
  );
  const isExactRetry = indeterminateMessage !== null && pendingInput !== null;
  const fieldsDisabled = createMutation.isPending || isExactRetry;

  const markDirty = React.useCallback(() => {
    setSubmitError(null);
    setSubmitAttempted(false);
  }, []);

  const resetDraft = React.useCallback(() => {
    setTitle("");
    setGoal("");
    setAgenda([]);
    setBackground("");
    setReferences("");
    setSourceChannelId("");
    setCustomBoard(null);
    setPendingInput(null);
    setIndeterminateMessage(null);
    setSubmitError(null);
    setSubmitAttempted(false);
    roster.reset();
  }, [roster.reset]);

  const publish = React.useCallback(
    async (input: CreateMeetingInput) => {
      try {
        const result = await createMutation.mutateAsync(input);
        if (result.status === "indeterminate") {
          setIndeterminateMessage(result.message);
          setSubmitError(null);
          return;
        }
        resetDraft();
        onOpenChange(false);
        onCreated(result.meetingId);
      } catch (error) {
        // A definitive refusal releases the native pending event. A later
        // attempt deliberately receives a fresh submission/event identity.
        setPendingInput(null);
        setIndeterminateMessage(null);
        setSubmitError(errorMessage(error, "Failed to create Meeting."));
      }
    },
    [createMutation.mutateAsync, onCreated, onOpenChange, resetDraft],
  );

  const handleSubmit = React.useCallback(async () => {
    setSubmitAttempted(true);
    setSubmitError(null);
    if (isExactRetry) {
      await publish(pendingInput);
      return;
    }
    if (!currentPubkey) {
      setSubmitError("Your current identity is unavailable.");
      return;
    }
    if (sourceSelectionUnavailable) {
      setSubmitError(
        "The selected source Channel is unavailable. Remove it or wait for the Channel directory to reload.",
      );
      return;
    }

    const input: CreateMeetingInput = {
      submissionId: crypto.randomUUID(),
      title,
      description: goal.trim(),
      sourceChannelId: sourceChannel?.id,
      participantPubkeys: selectedCandidates.map((candidate) =>
        normalizePubkey(candidate.pubkey),
      ),
      initialBoard: board,
    };

    if (validationErrors.length > 0) return;

    const capability = await capabilityQuery.refetch();
    if (capability.error) {
      setSubmitError(
        `Could not confirm Meeting creation capability: ${errorMessage(capability.error, "capability unavailable")}`,
      );
      return;
    }
    if (
      capability.data?.status !== "creatable" ||
      !capability.data.supportsDirectActions ||
      !capability.data.canCreateDirectActions
    ) {
      setSubmitError(
        "This Community can read Meetings but cannot create direct-action Meeting V2 sessions.",
      );
      return;
    }

    let verifiedSelected = selectedCandidates;
    if (
      verifiedSelected.some(
        (candidate) =>
          candidate.isAgent && candidate.actionCapability === "unknown",
      )
    ) {
      const refreshedAgents = await relayAgentsQuery.refetch();
      if (refreshedAgents.error) {
        setSubmitError(
          `Could not confirm Agent Meeting capability: ${errorMessage(refreshedAgents.error, "directory unavailable")}`,
        );
        return;
      }
      verifiedSelected = dedupeMeetingRosterCandidates(
        roster.selectedUsers,
        refreshedAgents.data ?? [],
      );
    }
    const unsupportedAgents = verifiedSelected.filter(
      (candidate) =>
        candidate.isAgent && candidate.actionCapability !== "compatible",
    );
    if (unsupportedAgents.length > 0) {
      setSubmitError(
        `Cannot create: ${unsupportedAgents
          .map(
            (candidate) =>
              candidate.displayName ?? truncatePubkey(candidate.pubkey),
          )
          .join(
            ", ",
          )} ${unsupportedAgents.length === 1 ? "does" : "do"} not advertise the required Meeting action capability.`,
      );
      return;
    }

    if (sourceChannel?.visibility === "private") {
      const refreshedMembers = await sourceMembersQuery.refetch();
      if (refreshedMembers.error) {
        setSubmitError(
          "The private source Channel membership could not be verified. Remove the source or try again.",
        );
        return;
      }
      const verifiedAccess = checkMeetingSourceAccess({
        sourceVisibility: "private",
        rosterPubkeys,
        memberPubkeys: refreshedMembers.data?.map((member) => member.pubkey),
      });
      if (verifiedAccess.status !== "ok") {
        setSubmitError(
          "Every participant must be able to read the private source Channel. Remove the source or adjust the roster.",
        );
        return;
      }
    }

    setPendingInput(input);
    await publish(input);
  }, [
    board,
    capabilityQuery.refetch,
    currentPubkey,
    goal,
    isExactRetry,
    pendingInput,
    publish,
    relayAgentsQuery.refetch,
    roster.selectedUsers,
    rosterPubkeys,
    selectedCandidates,
    sourceChannel,
    sourceSelectionUnavailable,
    sourceMembersQuery.refetch,
    title,
    validationErrors.length,
  ]);

  const visibleErrors = submitAttempted ? validationErrors : [];
  const capabilityMessage = capabilityQuery.isLoading
    ? "Checking Meeting capability…"
    : capabilityQuery.error
      ? `Meeting creation capability is unavailable: ${errorMessage(capabilityQuery.error, "could not read Relay capability")}`
      : relayCanCreate
        ? "Relay supports direct-action Meeting V2 creation."
        : "This Community does not currently allow direct-action Meeting creation.";

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent className="max-h-[90vh] max-w-3xl grid-rows-[auto_minmax(0,1fr)_auto] overflow-hidden p-0">
        <DialogHeader className="px-6 pt-6">
          <DialogTitle>Start a Meeting</DialogTitle>
          <DialogDescription>
            You will host a fixed roster. The Board is the shared source of
            truth for the discussion.
          </DialogDescription>
        </DialogHeader>

        <div className="min-h-0 space-y-6 overflow-y-auto px-6 pb-2">
          <section className="space-y-4">
            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="meeting-title">
                Meeting name
              </label>
              <Input
                autoFocus
                data-testid="meeting-create-title"
                disabled={fieldsDisabled}
                id="meeting-title"
                maxLength={255}
                onChange={(event) => {
                  setTitle(event.target.value);
                  markDirty();
                }}
                placeholder="Requirements review"
                value={title}
              />
            </div>

            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="meeting-goal">
                Discussion goal
              </label>
              <Textarea
                data-testid="meeting-create-goal"
                disabled={fieldsDisabled}
                id="meeting-goal"
                onChange={(event) => {
                  setGoal(event.target.value);
                  markDirty();
                }}
                placeholder="What must this Meeting decide or clarify?"
                rows={3}
                value={goal}
              />
            </div>

            <div className="space-y-2">
              <div className="flex items-center justify-between gap-3">
                <span className="text-sm font-medium">Agenda</span>
                <Button
                  data-testid="meeting-add-agenda"
                  disabled={fieldsDisabled}
                  onClick={() => {
                    setAgenda((current) => [...current, newAgendaItem()]);
                    markDirty();
                  }}
                  size="sm"
                  type="button"
                  variant="outline"
                >
                  <Plus className="size-4" />
                  Add item
                </Button>
              </div>
              {agenda.length === 0 ? (
                <p className="text-xs text-muted-foreground">
                  Agenda is optional and remains free Markdown after creation.
                </p>
              ) : (
                <div className="space-y-2">
                  {agenda.map((item, index) => (
                    <div className="flex items-center gap-2" key={item.id}>
                      <span className="w-5 text-right text-xs text-muted-foreground">
                        {index + 1}.
                      </span>
                      <Input
                        aria-label={`Agenda item ${index + 1}`}
                        data-testid={`meeting-agenda-${index}`}
                        disabled={fieldsDisabled}
                        onChange={(event) => {
                          setAgenda((current) =>
                            current.map((candidate) =>
                              candidate.id === item.id
                                ? { ...candidate, value: event.target.value }
                                : candidate,
                            ),
                          );
                          markDirty();
                        }}
                        value={item.value}
                      />
                      <Button
                        aria-label={`Move agenda item ${index + 1} up`}
                        disabled={fieldsDisabled || index === 0}
                        onClick={() => {
                          setAgenda((current) => {
                            const next = [...current];
                            [next[index - 1], next[index]] = [
                              next[index],
                              next[index - 1],
                            ];
                            return next;
                          });
                          markDirty();
                        }}
                        size="icon"
                        type="button"
                        variant="ghost"
                      >
                        <ArrowUp className="size-4" />
                      </Button>
                      <Button
                        aria-label={`Move agenda item ${index + 1} down`}
                        disabled={fieldsDisabled || index === agenda.length - 1}
                        onClick={() => {
                          setAgenda((current) => {
                            const next = [...current];
                            [next[index], next[index + 1]] = [
                              next[index + 1],
                              next[index],
                            ];
                            return next;
                          });
                          markDirty();
                        }}
                        size="icon"
                        type="button"
                        variant="ghost"
                      >
                        <ArrowDown className="size-4" />
                      </Button>
                      <Button
                        aria-label={`Delete agenda item ${index + 1}`}
                        disabled={fieldsDisabled}
                        onClick={() => {
                          setAgenda((current) =>
                            current.filter(
                              (candidate) => candidate.id !== item.id,
                            ),
                          );
                          markDirty();
                        }}
                        size="icon"
                        type="button"
                        variant="ghost"
                      >
                        <Trash2 className="size-4" />
                      </Button>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </section>

          <section className="space-y-2">
            <h3 className="text-sm font-medium">Participants</h3>
            <MeetingRosterPicker
              candidates={directoryCandidates}
              currentHost={{
                displayName:
                  profileQuery.data?.displayName ||
                  identityQuery.data?.displayName ||
                  "You",
                avatarUrl: profileQuery.data?.avatarUrl ?? null,
              }}
              currentPubkey={currentPubkey}
              disabled={fieldsDisabled}
              hasReachedLimit={roster.hasReachedRecipientLimit}
              isLoading={roster.isDirectoryLoading}
              onDirectoryScroll={roster.handleDirectoryScroll}
              onRemove={(pubkey) => {
                roster.removeUser(pubkey);
                markDirty();
              }}
              onSearchChange={roster.setSearchQuery}
              onSelect={(candidate) => {
                roster.selectUser(candidate);
                markDirty();
              }}
              ownerProfiles={roster.ownerProfiles}
              searchError={roster.searchError}
              searchQuery={roster.searchQuery}
              selected={selectedCandidates}
            />
          </section>

          <section className="space-y-4">
            <div className="space-y-1.5">
              <label
                className="text-sm font-medium"
                htmlFor="meeting-background"
              >
                Background and context
              </label>
              <Textarea
                data-testid="meeting-create-background"
                disabled={fieldsDisabled}
                id="meeting-background"
                onChange={(event) => {
                  setBackground(event.target.value);
                  markDirty();
                }}
                placeholder="Optional context participants should know"
                rows={3}
                value={background}
              />
            </div>

            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="meeting-source">
                Source Channel
              </label>
              <select
                className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-xs outline-hidden focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                data-testid="meeting-create-source"
                disabled={fieldsDisabled}
                id="meeting-source"
                onChange={(event) => {
                  setSourceChannelId(event.target.value);
                  markDirty();
                }}
                value={sourceChannelId}
              >
                <option value="">No source Channel</option>
                {ordinarySourceChannels.map((channel) => (
                  <option key={channel.id} value={channel.id}>
                    #{channel.name}
                    {channel.visibility === "private" ? " · private" : ""}
                  </option>
                ))}
              </select>
              {sourceAccess.status === "loading" ? (
                <p className="text-xs text-muted-foreground">
                  Verifying private source access…
                </p>
              ) : sourceSelectionUnavailable ? (
                <p className="text-xs text-destructive">
                  The selected source Channel is unavailable. Remove it or wait
                  for the Channel directory to reload.
                </p>
              ) : sourceAccess.status === "unavailable" ? (
                <p className="text-xs text-destructive">
                  Private source membership could not be verified. Remove the
                  source or choose it again to retry.
                </p>
              ) : sourceAccess.status === "blocked" ? (
                <p className="text-xs text-destructive">
                  Source is unreadable by:{" "}
                  {sourceAccess.missingPubkeys.map(truncatePubkey).join(", ")}
                </p>
              ) : null}
            </div>

            <div className="space-y-1.5">
              <label
                className="text-sm font-medium"
                htmlFor="meeting-references"
              >
                Project View, messages, documents, or URLs
              </label>
              <Textarea
                data-testid="meeting-create-references"
                disabled={fieldsDisabled}
                id="meeting-references"
                onChange={(event) => {
                  setReferences(event.target.value);
                  markDirty();
                }}
                placeholder="Optional references, one per line"
                rows={3}
                value={references}
              />
            </div>
          </section>

          <section className="space-y-2">
            <div className="flex items-center justify-between gap-3">
              <div>
                <h3 className="text-sm font-medium">Initial Board</h3>
                <p className="text-xs text-muted-foreground">
                  This complete Markdown document becomes authoritative at
                  creation.
                </p>
              </div>
              {customBoard !== null ? (
                <Button
                  disabled={fieldsDisabled}
                  onClick={() => {
                    setCustomBoard(null);
                    markDirty();
                  }}
                  size="sm"
                  type="button"
                  variant="outline"
                >
                  <RotateCcw className="size-4" />
                  Regenerate
                </Button>
              ) : null}
            </div>
            <Textarea
              className="min-h-64 font-mono text-sm"
              data-testid="meeting-create-board"
              disabled={fieldsDisabled}
              onChange={(event) => {
                setCustomBoard(event.target.value);
                markDirty();
              }}
              spellCheck={false}
              value={board}
            />
            <p
              className={
                boardBytes > MAX_MEETING_BOARD_BYTES
                  ? "text-xs text-destructive"
                  : "text-xs text-muted-foreground"
              }
            >
              {boardBytes.toLocaleString()} /{" "}
              {MAX_MEETING_BOARD_BYTES.toLocaleString()} UTF-8 bytes
            </p>
          </section>

          <section className="space-y-2" aria-live="polite">
            <p className="text-xs text-muted-foreground">{capabilityMessage}</p>
            {capabilityQuery.error ? (
              <Button
                data-testid="meeting-capability-retry"
                disabled={capabilityQuery.isFetching}
                onClick={() => void capabilityQuery.refetch()}
                size="sm"
                type="button"
                variant="outline"
              >
                {capabilityQuery.isFetching
                  ? "Checking capability…"
                  : "Retry capability check"}
              </Button>
            ) : null}
            {incompatibleAgents.length > 0 ? (
              <p className="flex items-start gap-2 text-sm text-destructive">
                <TriangleAlert className="mt-0.5 size-4 shrink-0" />
                Remove or update Agent{incompatibleAgents.length > 1 ? "s" : ""}{" "}
                missing the required action-finalization capability.
              </p>
            ) : null}
            {visibleErrors.length > 0 ? (
              <ul className="list-disc space-y-1 pl-5 text-sm text-destructive">
                {visibleErrors.map((error) => (
                  <li key={error}>{error}</li>
                ))}
              </ul>
            ) : null}
            {submitError ? (
              <p
                className="text-sm text-destructive"
                data-testid="meeting-create-error"
              >
                {submitError}
              </p>
            ) : null}
            {indeterminateMessage ? (
              <div
                className="rounded-lg border border-amber-500/40 bg-amber-500/10 p-3 text-sm"
                data-testid="meeting-create-indeterminate"
              >
                <p className="font-medium">Creation result is not confirmed</p>
                <p className="mt-1 text-muted-foreground">
                  {indeterminateMessage}
                </p>
                <p className="mt-1 text-muted-foreground">
                  The draft is locked so Retry can publish the exact same signed
                  Create event.
                </p>
              </div>
            ) : null}
          </section>
        </div>

        <DialogFooter className="border-t px-6 py-4">
          <Button
            disabled={createMutation.isPending}
            onClick={() => onOpenChange(false)}
            type="button"
            variant="outline"
          >
            Cancel
          </Button>
          <Button
            data-testid="meeting-create-submit"
            disabled={
              createMutation.isPending ||
              (!isExactRetry &&
                (!relayCanCreate ||
                  incompatibleAgents.length > 0 ||
                  sourceSelectionUnavailable ||
                  sourceAccess.status !== "ok"))
            }
            onClick={() => void handleSubmit()}
            type="button"
          >
            {createMutation.isPending
              ? "Creating…"
              : isExactRetry
                ? "Retry exact Create"
                : "Start Meeting"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
