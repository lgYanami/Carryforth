import { Crown, MoreHorizontal, Search, Shield } from "lucide-react";
import { nip19 } from "nostr-tools";
import * as React from "react";
import { toast } from "sonner";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
  useChangeRelayMemberRoleMutation,
  useMyRelayMembershipLookupQuery,
  useRelayMembersQuery,
  useRemoveRelayMemberMutation,
} from "@/features/community-members/hooks";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { useProjectViewQuery } from "@/features/project-view/hooks";
import { SettingsSectionHeader } from "@/features/settings/ui/SettingsSectionHeader";
import type {
  RelayMember,
  RelayMemberRole,
  UserProfileSummary,
} from "@/shared/api/types";
import { normalizePubkey, truncatePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { VirtualizedList } from "@/shared/ui/VirtualizedList";
import { CommunityInviteDialog } from "./CommunityInviteDialog";

function formatDisplayName(member: RelayMember, displayName?: string | null) {
  const trimmedDisplayName = displayName?.trim();
  if (
    trimmedDisplayName &&
    !trimmedDisplayName.toLowerCase().startsWith("npub1")
  ) {
    return trimmedDisplayName;
  }
  return member.role === "owner" ? "Community owner" : "Unnamed member";
}

function npubFromPubkey(pubkey: string): string | null {
  try {
    return nip19.npubEncode(pubkey);
  } catch {
    return null;
  }
}

function formatDate(dateString: string): string {
  return new Date(dateString).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

function HoverMemberIdentity({
  displayName,
  pubkey,
}: {
  displayName: string;
  pubkey: string;
}) {
  const npub = npubFromPubkey(pubkey) ?? pubkey;
  return (
    <span className="inline-grid min-w-0 max-w-full grid-cols-1" title={npub}>
      <span
        className="col-start-1 row-start-1 max-w-40 truncate opacity-100 blur-0 transition-[max-width,opacity,filter] duration-[250ms] ease-in-out group-hover/member:max-w-0 group-hover/member:opacity-0 group-hover/member:blur-[2px] group-focus-within/member:max-w-0 group-focus-within/member:opacity-0 group-focus-within/member:blur-[2px] motion-reduce:transition-none"
        data-testid={`relay-member-name-${pubkey}`}
      >
        {displayName}
      </span>
      <span
        className="col-start-1 row-start-1 max-w-0 truncate font-mono text-2xs opacity-0 blur-0 transition-[max-width,opacity,filter] duration-[250ms] ease-in-out group-hover/member:max-w-40 group-hover/member:opacity-100 group-hover/member:blur-0 group-focus-within/member:max-w-40 group-focus-within/member:opacity-100 group-focus-within/member:blur-0 motion-reduce:transition-none"
        data-testid={`relay-member-npub-${pubkey}`}
      >
        {truncatePubkey(npub)}
      </span>
    </span>
  );
}

function RelayMemberRow({
  currentRole,
  currentPubkey,
  directMembershipAllowed,
  onManageInView,
  profile,
  member,
  roleGoverned,
  roleName,
}: {
  currentRole: RelayMemberRole;
  currentPubkey?: string;
  directMembershipAllowed: boolean;
  onManageInView: () => void;
  profile?: UserProfileSummary;
  member: RelayMember;
  roleGoverned: boolean;
  roleName?: string;
}) {
  const removeMutation = useRemoveRelayMemberMutation();
  const changeRoleMutation = useChangeRelayMemberRoleMutation();
  const isSelf = currentPubkey
    ? normalizePubkey(currentPubkey) === normalizePubkey(member.pubkey)
    : false;
  const isBusy = removeMutation.isPending || changeRoleMutation.isPending;
  const canRemove =
    directMembershipAllowed &&
    !isSelf &&
    member.role !== "owner" &&
    (currentRole === "owner" || member.role === "member");
  const canPromote =
    directMembershipAllowed &&
    currentRole === "owner" &&
    member.role === "member";
  const canDemote =
    directMembershipAllowed &&
    currentRole === "owner" &&
    member.role === "admin";
  const canManageInView = roleGoverned && member.role !== "owner";
  const hasActions = canRemove || canPromote || canDemote || canManageInView;
  const displayName = formatDisplayName(member, profile?.displayName);

  async function mutateWithToast(
    action: () => Promise<unknown>,
    success: string,
  ) {
    try {
      await action();
      toast.success(success);
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : "Couldn’t update this community member.",
      );
    }
  }

  return (
    <div
      className="group/member flex min-h-14 items-center gap-3 px-1 py-2.5"
      data-testid={`relay-member-row-${member.pubkey}`}
    >
      <ProfileAvatar
        avatarUrl={profile?.avatarUrl ?? null}
        className="h-9 w-9 text-xs shadow-none"
        label={displayName}
      />
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-1.5 text-sm font-medium">
          <HoverMemberIdentity
            displayName={displayName}
            pubkey={member.pubkey}
          />
          {member.role === "owner" ? (
            <Crown className="h-4 w-4 text-amber-500" />
          ) : null}
          {member.role === "admin" ? (
            <Shield className="h-4 w-4 text-blue-500" />
          ) : null}
        </div>
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span className="shrink-0 capitalize">{member.role}</span>
          <span aria-hidden="true" className="shrink-0">
            ·
          </span>
          <span className="shrink-0">Added {formatDate(member.createdAt)}</span>
          {roleName ? (
            <>
              <span aria-hidden="true" className="shrink-0">
                ·
              </span>
              <span className="min-w-0 truncate">{roleName}</span>
            </>
          ) : null}
          {isSelf ? (
            <>
              <span aria-hidden="true" className="shrink-0">
                ·
              </span>
              <span className="shrink-0">You</span>
            </>
          ) : null}
        </div>
      </div>

      {hasActions ? (
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              aria-label={`Actions for ${displayName}`}
              data-testid={`relay-member-actions-${member.pubkey}`}
              disabled={isBusy}
              size="icon"
              variant="ghost"
            >
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {canManageInView ? (
              <DropdownMenuItem onClick={onManageInView}>
                Manage Role in View
              </DropdownMenuItem>
            ) : null}
            {canPromote ? (
              <DropdownMenuItem
                onClick={() =>
                  void mutateWithToast(
                    () =>
                      changeRoleMutation.mutateAsync({
                        pubkey: member.pubkey,
                        role: "admin",
                      }),
                    "Made community admin",
                  )
                }
              >
                Make admin
              </DropdownMenuItem>
            ) : null}
            {canDemote ? (
              <DropdownMenuItem
                onClick={() =>
                  void mutateWithToast(
                    () =>
                      changeRoleMutation.mutateAsync({
                        pubkey: member.pubkey,
                        role: "member",
                      }),
                    "Made community member",
                  )
                }
              >
                Make member
              </DropdownMenuItem>
            ) : null}
            {canRemove && (canPromote || canDemote) ? (
              <DropdownMenuSeparator />
            ) : null}
            {canRemove ? (
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onClick={() =>
                  void mutateWithToast(
                    () => removeMutation.mutateAsync(member.pubkey),
                    "Removed community member",
                  )
                }
              >
                Remove from community
              </DropdownMenuItem>
            ) : null}
          </DropdownMenuContent>
        </DropdownMenu>
      ) : null}
    </div>
  );
}

export function CommunityMembersSettingsCard({
  currentPubkey,
}: {
  currentPubkey?: string;
}) {
  const { goView } = useAppNavigation();
  const myMembershipQuery = useMyRelayMembershipLookupQuery();
  const projectViewQuery = useProjectViewQuery();
  const roleContinuity =
    projectViewQuery.data?.status === "ready"
      ? projectViewQuery.data.roleContinuity
      : undefined;
  const roleGoverned = Boolean(roleContinuity);
  const directMembershipAllowed =
    projectViewQuery.data?.status === "unsupported" ||
    projectViewQuery.data?.status === "uninitialized";
  const roleNamesByMember = React.useMemo(() => {
    const names = new Map<string, string>();
    if (!roleContinuity) return names;
    const namesByRole = new Map(
      roleContinuity.roles.map((role) => [role.roleId, role.name]),
    );
    for (const assignment of roleContinuity.assignments) {
      if (assignment.endedAt) continue;
      const roleName = namesByRole.get(assignment.roleId);
      if (roleName)
        names.set(normalizePubkey(assignment.memberPubkey), roleName);
    }
    return names;
  }, [roleContinuity]);
  const currentRole = myMembershipQuery.data?.membership?.role ?? null;
  const canManageRelay = currentRole === "owner" || currentRole === "admin";
  const membersQuery = useRelayMembersQuery(canManageRelay);
  const members = React.useMemo(
    () => membersQuery.data ?? [],
    [membersQuery.data],
  );
  const profilesQuery = useUsersBatchQuery(
    members.map((member) => member.pubkey),
    {
      enabled: canManageRelay && members.length > 0,
    },
  );
  const profiles = profilesQuery.data?.profiles;
  const [inviteDialogOpen, setInviteDialogOpen] = React.useState(false);
  const [search, setSearch] = React.useState("");

  const filteredMembers = React.useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return members;
    return members.filter((member) => {
      const normalizedPubkey = normalizePubkey(member.pubkey);
      const profile = profiles?.[normalizedPubkey];
      const displayName = profile?.displayName?.toLowerCase() ?? "";
      const nip05 = profile?.nip05Handle?.toLowerCase() ?? "";
      const npub = npubFromPubkey(member.pubkey)?.toLowerCase() ?? "";
      return (
        displayName.includes(q) ||
        nip05.includes(q) ||
        npub.includes(q) ||
        member.pubkey.toLowerCase().includes(q) ||
        member.role.includes(q)
      );
    });
  }, [members, profiles, search]);

  if (myMembershipQuery.isLoading) {
    return (
      <section className="min-w-0" data-testid="settings-community-members">
        <p className="text-sm text-muted-foreground">
          Checking invite permissions…
        </p>
      </section>
    );
  }

  if (!canManageRelay || !currentRole) {
    return null;
  }

  return (
    <section className="min-w-0" data-testid="settings-community-members">
      <SettingsSectionHeader
        action={
          roleGoverned ? (
            <Button
              data-testid="community-members-manage-in-view"
              onClick={() => void goView()}
              variant="outline"
            >
              Manage Roles in View
            </Button>
          ) : directMembershipAllowed ? (
            <Button
              data-testid="community-invite-dialog-trigger"
              onClick={() => setInviteDialogOpen(true)}
            >
              Invite to community
            </Button>
          ) : (
            <Button disabled variant="outline">
              Checking View governance…
            </Button>
          )
        }
        title="Invites"
        description={
          roleGoverned
            ? "Role levels and active tenures are governed in View; direct admin changes and member removal are disabled here."
            : directMembershipAllowed
              ? "Manage members and community access."
              : "Membership changes are disabled until View governance can be verified."
        }
      />

      <div className="overflow-hidden rounded-2xl border border-border/70 bg-background/70 shadow-xs">
        <div className="space-y-3 p-4 sm:p-5">
          <div className="flex items-center justify-between gap-3">
            <h2 className="text-sm font-medium">
              Members
              {members.length > 0 ? (
                <span className="ml-1.5 text-xs font-normal text-muted-foreground">
                  {members.length}
                </span>
              ) : null}
            </h2>
          </div>

          <div className="relative">
            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <input
              autoCapitalize="none"
              autoCorrect="off"
              className="w-full rounded-lg border border-border/70 bg-background py-2 pl-9 pr-3 text-sm placeholder:text-muted-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
              data-testid="community-members-search"
              onChange={(event) => setSearch(event.target.value)}
              placeholder="Search members"
              spellCheck={false}
              type="text"
              value={search}
            />
          </div>

          {membersQuery.error instanceof Error ? (
            <p className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              {membersQuery.error.message}
            </p>
          ) : null}

          {membersQuery.isLoading ? (
            <p className="py-3 text-sm text-muted-foreground">
              Loading community members…
            </p>
          ) : members.length === 0 ? (
            <p className="rounded-lg border border-dashed border-border/70 px-3 py-6 text-center text-sm text-muted-foreground">
              No community members yet.
            </p>
          ) : filteredMembers.length === 0 ? (
            <p className="rounded-lg border border-dashed border-border/70 px-3 py-6 text-center text-sm text-muted-foreground">
              No members match your search.
            </p>
          ) : (
            <VirtualizedList
              className="max-h-[28rem] divide-y divide-border/60"
              estimateSize={60}
              getItemKey={(member) => member.pubkey}
              items={filteredMembers}
              renderItem={(member) => (
                <RelayMemberRow
                  currentPubkey={currentPubkey}
                  currentRole={currentRole}
                  directMembershipAllowed={directMembershipAllowed}
                  member={member}
                  onManageInView={() => void goView()}
                  profile={profiles?.[normalizePubkey(member.pubkey)]}
                  roleGoverned={roleGoverned}
                  roleName={roleNamesByMember.get(
                    normalizePubkey(member.pubkey),
                  )}
                />
              )}
            />
          )}
        </div>
      </div>

      {directMembershipAllowed ? (
        <CommunityInviteDialog
          onOpenChange={setInviteDialogOpen}
          open={inviteDialogOpen}
        />
      ) : null}
    </section>
  );
}
