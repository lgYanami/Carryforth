import { MessageSquareText } from "lucide-react";

import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import type { UserProfileSummary } from "@/shared/api/types";
import type { MeetingSpeech } from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import { Markdown } from "@/shared/ui/markdown";
import { Spinner } from "@/shared/ui/spinner";

function speechAuthorName(
  pubkey: string,
  profiles: Record<string, UserProfileSummary>,
): string {
  return (
    profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey)
  );
}

export function MeetingSpeechTimeline({
  hasOlder,
  isFetchingOlder,
  onFetchOlder,
  profiles,
  speeches,
}: {
  hasOlder: boolean;
  isFetchingOlder: boolean;
  onFetchOlder: () => void;
  profiles: Record<string, UserProfileSummary>;
  speeches: MeetingSpeech[];
}) {
  return (
    <div
      className="mx-auto flex w-full max-w-3xl flex-col gap-1 px-4 py-5 sm:px-7"
      data-testid="meeting-speech-timeline"
    >
      {hasOlder ? (
        <div className="mb-4 flex justify-center">
          <Button
            disabled={isFetchingOlder}
            onClick={onFetchOlder}
            size="sm"
            variant="outline"
          >
            {isFetchingOlder ? <Spinner size={14} /> : null}
            Load earlier speech
          </Button>
        </div>
      ) : null}
      {speeches.length === 0 ? (
        <div className="flex min-h-72 flex-col items-center justify-center gap-3 text-center text-muted-foreground">
          <span className="flex size-12 items-center justify-center rounded-full bg-muted">
            <MessageSquareText className="size-5" />
          </span>
          <div>
            <p className="text-sm font-medium text-foreground">
              No formal speech yet
            </p>
            <p className="mt-1 max-w-sm text-xs">
              Only Speech accepted against a Meeting Grant appears here.
            </p>
          </div>
        </div>
      ) : (
        speeches.map((speech) => {
          const profile = profiles[speech.authorPubkey.toLowerCase()];
          const authorName = speechAuthorName(speech.authorPubkey, profiles);
          return (
            <article
              className="group flex gap-3 rounded-xl px-2 py-3 hover:bg-muted/35"
              data-speech-revision={speech.speechRevision}
              data-testid={`meeting-speech-${speech.eventId}`}
              key={speech.eventId}
            >
              <ProfileAvatar
                avatarUrl={profile?.avatarUrl ?? null}
                className="mt-0.5 size-9"
                label={authorName}
              />
              <div className="min-w-0 flex-1">
                <div className="mb-1 flex min-w-0 items-baseline gap-2">
                  <span className="truncate text-sm font-semibold">
                    {authorName}
                  </span>
                  <time
                    className="shrink-0 text-2xs text-muted-foreground"
                    dateTime={new Date(speech.createdAt * 1_000).toISOString()}
                  >
                    {new Date(speech.createdAt * 1_000).toLocaleTimeString([], {
                      hour: "2-digit",
                      minute: "2-digit",
                    })}
                  </time>
                </div>
                <Markdown
                  className="max-w-full text-base"
                  content={speech.content}
                  interactive
                />
              </div>
            </article>
          );
        })
      )}
    </div>
  );
}
