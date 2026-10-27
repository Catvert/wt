import { useRpc, useWorkspace } from "@getpaseo/plugin/client";
import { useQuery } from "@tanstack/react-query";
import { wtInfo } from "../shared/wt";

export function useWtInfo(workspaceId: string, detail: boolean) {
  const directory = useWorkspace(workspaceId, (workspace) => workspace.directory);
  const fetchInfo = useRpc(wtInfo);
  const query = useQuery({
    queryKey: ["wt-info", directory, detail],
    queryFn: () => fetchInfo({ directory: directory as string, detail }),
    enabled: Boolean(directory),
    refetchInterval: (q) => (q.state.data?.kind === "adopting" ? 3000 : 15000),
  });
  return { directory, ...query };
}
