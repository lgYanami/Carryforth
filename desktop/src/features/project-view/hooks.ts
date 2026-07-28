import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { useCommunities } from "@/features/communities/useCommunities";
import {
  getProjectView,
  mutateProjectView,
} from "@/shared/api/tauriProjectView";

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

export function useProjectViewMutation() {
  const { activeCommunity } = useCommunities();
  const queryClient = useQueryClient();
  const communityId = activeCommunity?.id;

  return useMutation({
    mutationFn: mutateProjectView,
    onSuccess: (result) => {
      if (result.status !== "applied") return undefined;
      return queryClient.invalidateQueries({
        queryKey: projectViewQueryKey(communityId),
      });
    },
  });
}
