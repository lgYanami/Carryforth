import { decode } from "nostr-tools/nip19";

import {
  canonicalRelayUrl,
  findManagedAgentRuntime,
} from "@/features/agents/managedAgentRuntimeStatus";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type {
  ManagedAgent,
  ManagedAgentRuntimeStatus,
  RelayAgent,
  UserProfileSummary,
} from "@/shared/api/types";
import type {
  ProjectCommunityMemberRole,
  ProjectRoleAssignment,
  ProjectRoleDefinition,
  ProjectRoleProposal,
} from "@/shared/api/tauriProjectView";
import { normalizePubkey, truncatePubkey } from "@/shared/lib/pubkey";

const HEX_PUBKEY = /^[0-9a-f]{64}$/;

export type ProjectRoleCandidateRuntimeStatus =
  | "online"
  | "away"
  | "offline"
  | "running"
  | "stopped";

export type ProjectRoleCandidate = {
  pubkey: string;
  displayName: string;
  avatarUrl: string | null;
  nip05Handle: string | null;
  identityType: "agent" | "person";
  communityRole?: ProjectCommunityMemberRole;
  ownerPubkey?: string;
  ownerDisplayName?: string;
  managedByCurrentUser: boolean;
  runtimeStatus?: ProjectRoleCandidateRuntimeStatus;
  activeAssignment?: {
    assignmentId: string;
    roleId: string;
    roleName: string;
  };
  openProposal?: {
    proposalId: string;
    proposalType: "offer" | "request";
    roleId: string;
  };
  isCurrentAssignee: boolean;
  source: "managed" | "relay_agent" | "member";
};

type ManagedCandidateSource = Pick<
  ManagedAgent,
  "avatarUrl" | "backend" | "name" | "pubkey" | "relayUrl" | "status"
>;

type RelayCandidateSource = Pick<RelayAgent, "name" | "pubkey" | "status">;

export type BuildProjectRoleCandidatesInput = {
  activeRelayUrl?: string;
  archivedPubkeys?: ReadonlySet<string>;
  assignments: readonly ProjectRoleAssignment[];
  currentPubkey?: string;
  managedAgents: readonly ManagedCandidateSource[];
  managedAgentRuntimes: readonly ManagedAgentRuntimeStatus[];
  members: ReadonlyArray<{
    pubkey: string;
    role: ProjectCommunityMemberRole;
  }>;
  now?: number;
  profiles?: UserProfileLookup;
  proposals: readonly ProjectRoleProposal[];
  relayAgents: readonly RelayCandidateSource[];
  roles: readonly ProjectRoleDefinition[];
  targetRoleId: string;
};

type CandidateDraft = {
  pubkey: string;
  memberRole?: ProjectCommunityMemberRole;
  managed?: ManagedCandidateSource;
  managedRuntime?: ManagedAgentRuntimeStatus;
  relayAgent?: RelayCandidateSource;
};

function validDirectoryPubkey(pubkey: string) {
  const normalized = normalizePubkey(pubkey);
  return HEX_PUBKEY.test(normalized) ? normalized : null;
}

function normalizedProfiles(profiles?: UserProfileLookup) {
  const output = new Map<string, UserProfileSummary>();
  for (const [pubkey, profile] of Object.entries(profiles ?? {})) {
    output.set(normalizePubkey(pubkey), profile);
  }
  return output;
}

function managedRuntimeStatus(
  status: ManagedCandidateSource["status"],
): ProjectRoleCandidateRuntimeStatus {
  switch (status) {
    case "running":
      return "running";
    case "deployed":
      return "online";
    case "stopped":
      return "stopped";
    case "not_deployed":
      return "offline";
  }
}

function managedPairRuntimeStatus(
  runtime: ManagedAgentRuntimeStatus,
): ProjectRoleCandidateRuntimeStatus {
  if (!runtime.localSetup) return "stopped";
  switch (runtime.lifecycle) {
    case "starting":
    case "listening":
    case "waking":
    case "ready":
      return "running";
    case "failed":
    case "stopped":
      return "stopped";
  }
}

function displayNameForCandidate(
  draft: CandidateDraft,
  profile: UserProfileSummary | undefined,
) {
  return (
    draft.managed?.name.trim() ||
    draft.relayAgent?.name.trim() ||
    profile?.displayName?.trim() ||
    profile?.name?.trim() ||
    profile?.nip05Handle?.trim() ||
    truncatePubkey(draft.pubkey)
  );
}

function ownerDisplayName(
  ownerPubkey: string | undefined,
  profiles: ReadonlyMap<string, UserProfileSummary>,
) {
  if (!ownerPubkey) return undefined;
  const profile = profiles.get(ownerPubkey);
  return (
    profile?.displayName?.trim() ||
    profile?.name?.trim() ||
    profile?.nip05Handle?.trim() ||
    truncatePubkey(ownerPubkey)
  );
}

function proposalIsOpen(proposal: ProjectRoleProposal, now: number) {
  if (proposal.status !== "open") return false;
  const expiresAt = Date.parse(proposal.expiresAt);
  return !Number.isFinite(expiresAt) || expiresAt > now;
}

function candidateSort(
  left: ProjectRoleCandidate,
  right: ProjectRoleCandidate,
) {
  if (left.isCurrentAssignee !== right.isCurrentAssignee) {
    return left.isCurrentAssignee ? -1 : 1;
  }
  const leftBusy = Boolean(left.activeAssignment || left.openProposal);
  const rightBusy = Boolean(right.activeAssignment || right.openProposal);
  if (leftBusy !== rightBusy) return leftBusy ? 1 : -1;
  return (
    left.displayName.localeCompare(right.displayName) ||
    left.pubkey.localeCompare(right.pubkey)
  );
}

/**
 * Build the Human-facing Role candidate directory without weakening the
 * Relay's authority. Directory presence is discovery only; the Relay still
 * validates ownership, bans, revision fences, and Assignment conflicts.
 */
export function buildProjectRoleCandidates({
  activeRelayUrl,
  archivedPubkeys = new Set(),
  assignments,
  currentPubkey,
  managedAgents,
  managedAgentRuntimes,
  members,
  now = Date.now(),
  profiles,
  proposals,
  relayAgents,
  roles,
  targetRoleId,
}: BuildProjectRoleCandidatesInput): ProjectRoleCandidate[] {
  const profileLookup = normalizedProfiles(profiles);
  const normalizedCurrentPubkey = currentPubkey
    ? validDirectoryPubkey(currentPubkey)
    : null;
  const normalizedArchived = new Set(
    [...archivedPubkeys].map((pubkey) => normalizePubkey(pubkey)),
  );
  const memberRoles = new Map<string, ProjectCommunityMemberRole>();
  const drafts = new Map<string, CandidateDraft>();

  const isArchived = (pubkey: string) =>
    pubkey !== normalizedCurrentPubkey && normalizedArchived.has(pubkey);
  const ensureDraft = (pubkey: string) => {
    const current = drafts.get(pubkey);
    if (current) return current;
    const created: CandidateDraft = { pubkey };
    drafts.set(pubkey, created);
    return created;
  };

  for (const member of members) {
    const pubkey = validDirectoryPubkey(member.pubkey);
    if (!pubkey || isArchived(pubkey)) continue;
    memberRoles.set(pubkey, member.role);
    ensureDraft(pubkey).memberRole = member.role;
  }

  const activeCanonicalRelay = activeRelayUrl
    ? canonicalRelayUrl(activeRelayUrl)
    : null;
  for (const agent of managedAgents) {
    const pubkey = validDirectoryPubkey(agent.pubkey);
    if (!pubkey || isArchived(pubkey) || !activeCanonicalRelay) {
      continue;
    }

    const managedRuntime = findManagedAgentRuntime(
      managedAgentRuntimes,
      pubkey,
      activeRelayUrl ?? activeCanonicalRelay,
    );
    const legacyCanonicalRelay = canonicalRelayUrl(agent.relayUrl);
    const legacyRelayMatches = legacyCanonicalRelay === activeCanonicalRelay;
    // New local managed Agents are global definitions whose Community
    // attachment lives in per-relay runtime rows. Keep the blank-relay
    // fallback so a just-created or explicitly stopped local Agent remains
    // assignable while its runtime cache has no row. Provider-backed Agents
    // need an explicit pair (or a legacy scalar relay) to avoid guessing.
    const isGlobalLocalAgent =
      agent.backend.type === "local" && agent.relayUrl.trim() === "";
    if (!managedRuntime && !legacyRelayMatches && !isGlobalLocalAgent) {
      continue;
    }

    const profileOwner = profileLookup.get(pubkey)?.ownerPubkey;
    const ownerPubkey = profileOwner
      ? validDirectoryPubkey(profileOwner)
      : normalizedCurrentPubkey;
    if (
      !memberRoles.has(pubkey) &&
      (!ownerPubkey || !memberRoles.has(ownerPubkey))
    ) {
      continue;
    }
    const draft = ensureDraft(pubkey);
    draft.managed = agent;
    draft.managedRuntime = managedRuntime;
  }

  for (const agent of relayAgents) {
    const pubkey = validDirectoryPubkey(agent.pubkey);
    if (!pubkey || isArchived(pubkey)) continue;
    const ownerPubkey = profileLookup.get(pubkey)?.ownerPubkey;
    const normalizedOwnerPubkey = ownerPubkey
      ? validDirectoryPubkey(ownerPubkey)
      : null;
    if (
      !memberRoles.has(pubkey) &&
      (!normalizedOwnerPubkey || !memberRoles.has(normalizedOwnerPubkey)) &&
      !drafts.get(pubkey)?.managed
    ) {
      continue;
    }
    ensureDraft(pubkey).relayAgent = agent;
  }

  const rolesById = new Map(roles.map((role) => [role.roleId, role]));
  const activeAssignmentsByMember = new Map<string, ProjectRoleAssignment>();
  for (const assignment of assignments) {
    if (assignment.endedAt) continue;
    const pubkey = validDirectoryPubkey(assignment.memberPubkey);
    if (pubkey && !activeAssignmentsByMember.has(pubkey)) {
      activeAssignmentsByMember.set(pubkey, assignment);
    }
  }

  const openProposalsByCandidate = new Map<string, ProjectRoleProposal>();
  const sortedOpenProposals = proposals
    .filter((proposal) => proposalIsOpen(proposal, now))
    .sort(
      (left, right) =>
        Number(right.roleId === targetRoleId) -
          Number(left.roleId === targetRoleId) ||
        right.createdAt.localeCompare(left.createdAt),
    );
  for (const proposal of sortedOpenProposals) {
    const pubkey = validDirectoryPubkey(proposal.candidatePubkey);
    if (pubkey && !openProposalsByCandidate.has(pubkey)) {
      openProposalsByCandidate.set(pubkey, proposal);
    }
  }

  return [...drafts.values()]
    .map((draft): ProjectRoleCandidate => {
      const profile = profileLookup.get(draft.pubkey);
      const profileOwner = profile?.ownerPubkey
        ? validDirectoryPubkey(profile.ownerPubkey)
        : null;
      const ownerPubkey =
        profileOwner ?? (draft.managed ? normalizedCurrentPubkey : null);
      const activeAssignment = activeAssignmentsByMember.get(draft.pubkey);
      const openProposal = openProposalsByCandidate.get(draft.pubkey);
      const isAgent = Boolean(
        draft.managed || draft.relayAgent || profile?.isAgent,
      );

      return {
        pubkey: draft.pubkey,
        displayName: displayNameForCandidate(draft, profile),
        avatarUrl: draft.managed?.avatarUrl ?? profile?.avatarUrl ?? null,
        nip05Handle: profile?.nip05Handle ?? null,
        identityType: isAgent ? "agent" : "person",
        communityRole: draft.memberRole,
        ownerPubkey: ownerPubkey ?? undefined,
        ownerDisplayName: ownerDisplayName(
          ownerPubkey ?? undefined,
          profileLookup,
        ),
        managedByCurrentUser: Boolean(
          ownerPubkey && ownerPubkey === normalizedCurrentPubkey,
        ),
        runtimeStatus: draft.managedRuntime
          ? managedPairRuntimeStatus(draft.managedRuntime)
          : draft.managed
            ? managedRuntimeStatus(draft.managed.status)
            : draft.relayAgent?.status,
        activeAssignment: activeAssignment
          ? {
              assignmentId: activeAssignment.assignmentId,
              roleId: activeAssignment.roleId,
              roleName:
                rolesById.get(activeAssignment.roleId)?.name ??
                activeAssignment.roleId,
            }
          : undefined,
        openProposal: openProposal
          ? {
              proposalId: openProposal.proposalId,
              proposalType: openProposal.proposalType,
              roleId: openProposal.roleId,
            }
          : undefined,
        isCurrentAssignee: activeAssignment?.roleId === targetRoleId,
        source: draft.managed
          ? "managed"
          : draft.relayAgent
            ? "relay_agent"
            : "member",
      };
    })
    .sort(candidateSort);
}

function searchScore(candidate: ProjectRoleCandidate, query: string) {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return 0;
  const labels = [
    candidate.displayName,
    candidate.nip05Handle ?? "",
    candidate.identityType,
    candidate.ownerDisplayName ?? "",
    candidate.communityRole ?? "",
  ].map((label) => label.toLowerCase());
  if (labels.some((label) => label === normalizedQuery)) return 0;
  if (labels.some((label) => label.startsWith(normalizedQuery))) return 1;
  if (
    labels.some((label) =>
      label.split(/[\s\-_]+/).some((word) => word.startsWith(normalizedQuery)),
    )
  ) {
    return 2;
  }
  if (labels.some((label) => label.includes(normalizedQuery))) return 3;
  if (candidate.pubkey.startsWith(normalizedQuery)) return 4;
  if (candidate.pubkey.includes(normalizedQuery)) return 5;
  return null;
}

/** Filter and rank Role candidates while preserving the Agents/People split. */
export function filterProjectRoleCandidates(
  candidates: readonly ProjectRoleCandidate[],
  query: string,
) {
  const ranked = candidates
    .map((candidate) => ({
      candidate,
      score: searchScore(candidate, query),
    }))
    .filter(
      (entry): entry is { candidate: ProjectRoleCandidate; score: number } =>
        entry.score !== null,
    )
    .sort(
      (left, right) =>
        left.score - right.score ||
        candidateSort(left.candidate, right.candidate),
    )
    .map(({ candidate }) => candidate);

  return {
    agents: ranked.filter((candidate) => candidate.identityType === "agent"),
    people: ranked.filter((candidate) => candidate.identityType === "person"),
  };
}

/** Normalize a manually entered hex pubkey or npub to the command's hex ID. */
export function normalizeRoleCandidateInput(value: string): string | null {
  const normalized = normalizePubkey(value);
  if (HEX_PUBKEY.test(normalized)) return normalized;
  if (!normalized.startsWith("npub1")) return null;

  try {
    const decoded = decode(normalized);
    if (decoded.type !== "npub" || typeof decoded.data !== "string") {
      return null;
    }
    const pubkey = normalizePubkey(decoded.data);
    return HEX_PUBKEY.test(pubkey) ? pubkey : null;
  } catch {
    return null;
  }
}
