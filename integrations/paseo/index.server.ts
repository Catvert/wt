import type { PluginServerContext } from "@getpaseo/plugin/server";
import { adopt, release } from "./server/lifecycle";
import { getInfo } from "./server/wt";
import { wtAdopt, wtInfo } from "./shared/wt";

export default function contribute(server: PluginServerContext) {
  server.handle(wtInfo, getInfo);
  server.handle(wtAdopt, ({ directory }) => adopt(directory, { automatic: false }));

  // Les hooks expirent après 30 s : l'adoption (npm ci…) et l'attente de la suppression
  // tournent en tâche de fond, sans les attendre.
  const removeCreated = server.on("workspace.created", ({ workspace }) => {
    void adopt(workspace.cwd, { automatic: true }).catch((e) => console.error("[wt] adoption", e));
  });
  const removeArchived = server.on("workspace.archived", ({ workspace }) => {
    void release(workspace.cwd).catch((e) => console.error("[wt] nettoyage", e));
  });
  return () => {
    removeCreated();
    removeArchived();
  };
}
