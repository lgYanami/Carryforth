import { Bot, UserRound } from "lucide-react";

import {
  resolveUserLabel,
  type UserProfileLookup,
} from "@/features/profile/lib/identity";
import { Badge } from "@/shared/ui/badge";
import { PubKey } from "@/shared/ui/PubKey";

type ProjectViewActorProps = {
  pubkey: string;
  currentPubkey?: string;
  profiles?: UserProfileLookup;
  compact?: boolean;
  pubkeyTestId?: string;
};

export function ProjectViewActor({
  compact = false,
  currentPubkey,
  profiles,
  pubkey,
  pubkeyTestId,
}: ProjectViewActorProps) {
  const normalizedPubkey = pubkey.trim().toLowerCase();
  const profile = profiles?.[normalizedPubkey];
  const label = resolveUserLabel({
    currentPubkey,
    profiles,
    pubkey: normalizedPubkey,
  });
  const isAgent = profile?.isAgent === true;

  return (
    <span className="inline-flex min-w-0 items-center gap-1">
      {isAgent ? (
        <Bot className="h-3 w-3 shrink-0" aria-hidden />
      ) : (
        <UserRound className="h-3 w-3 shrink-0" aria-hidden />
      )}
      <span className="truncate">{label}</span>
      {!compact && isAgent ? <Badge variant="info">Agent</Badge> : null}
      {!compact ? (
        <span className="text-muted-foreground">
          (
          <PubKey
            className="text-xs"
            pubkey={normalizedPubkey}
            testId={pubkeyTestId}
          />
          )
        </span>
      ) : null}
    </span>
  );
}
