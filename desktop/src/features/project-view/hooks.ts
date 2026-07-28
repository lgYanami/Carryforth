import { useQuery } from "@tanstack/react-query";

import { useCommunities } from "@/features/communities/useCommunities";
import { getProjectView } from "@/shared/api/tauriProjectView";

export const projectViewQueryKey = (communityId: string | undefined) =>
  ["project-view", communityId ?? "no-community"] as const;

export function useProjectViewQuery() {
  const { activeCommunity } = useCommunities();
  return useQuery({
    queryKey: projectViewQueryKey(activeCommunity?.id),
    queryFn: getProjectView,
    enabled: Boolean(activeCommunity),
    staleTime: 15_000,
    refetchOnWindowFocus: true,
  });
}
