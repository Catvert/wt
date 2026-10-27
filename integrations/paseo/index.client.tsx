import type {
  PluginButtonContentProps,
  PluginButtonRegistration,
  PluginClientContext,
  PluginWorkspacePanelProps,
} from "@getpaseo/plugin/client";
import { openExternalUrl } from "@getpaseo/plugin/client";
import { ScrollView } from "@getpaseo/plugin/client/ui";
import { addresses, wtInfo } from "./shared/wt";
import { headerLabel, WtView } from "./client/wt-view";

function WtPanel({ workspaceId, theme, layout, navigation }: PluginWorkspacePanelProps) {
  return (
    <ScrollView style={{ flex: 1, backgroundColor: theme.colors.surface0 }}>
      <WtView
        workspaceId={workspaceId}
        theme={theme}
        compact={layout.compact}
        full
        openBrowser={navigation?.openBrowser}
      />
    </ScrollView>
  );
}

export default function contribute(client: PluginClientContext) {
  client.addWorkspacePanel({
    id: "wt",
    title: "wt",
    icon: "GitFork",
    locations: ["workspace", "explorer"],
    Component: WtPanel,
  });

  client.addCommandCenterItem({
    id: "open-app",
    title: "wt : ouvrir l'application du worktree",
    icon: "Globe",
    keywords: ["wt", "url", "navigateur"],
    context: "workspace",
    async onSelect({ rpc, workspace }) {
      const first = addresses(await rpc(wtInfo, { directory: workspace.directory, detail: true }))[0];
      if (!first) throw new Error("wt : aucune adresse pour ce workspace");
      await openExternalUrl(first.url);
    },
  });
  client.addCommandCenterItem({
    id: "open-panel",
    title: "wt : panneau du worktree",
    icon: "GitFork",
    keywords: ["wt", "worktree", "ports", "tâches"],
    context: "workspace",
    onSelect({ openPanel }) {
      openPanel("wt");
    },
  });

  // Un bouton « wt » dans l'en-tête de chaque workspace qui relève d'un projet wt.
  type Entry = { button: PluginButtonRegistration; directory: string; settled: boolean };
  const entries = new Map<string, Entry>();
  let stopped = false;

  const refresh = (entry: Entry) => {
    void client.rpc(wtInfo, { directory: entry.directory, detail: false }).then((info) => {
      const visible = info.kind === "worktree" || info.kind === "adopting" || info.kind === "unmanaged";
      entry.settled = info.kind !== "adopting" && info.kind !== "unmanaged";
      entry.button.update({ visible, label: headerLabel(info) });
    });
  };

  const register = (workspaceId: string, directory: string | undefined) => {
    if (stopped || !directory) return;
    const existing = entries.get(workspaceId);
    if (existing) return refresh(existing);
    const Content = ({ theme, layout, close }: PluginButtonContentProps) => (
      <WtView
        workspaceId={workspaceId}
        theme={theme}
        compact={layout.compact}
        full={false}
        onDone={close}
        onDetails={() => {
          close();
          client.openPanel("wt", { workspaceId });
        }}
      />
    );
    const entry: Entry = {
      directory,
      settled: false,
      button: client.addHeaderButton({
        id: "wt",
        workspaceId,
        button: { title: "wt", icon: "Globe", label: "wt", visible: false, behavior: { kind: "popover", Content } },
      }),
    };
    entries.set(workspaceId, entry);
    refresh(entry);
  };
  const unregister = (workspaceId: string) => {
    entries.get(workspaceId)?.button.remove();
    entries.delete(workspaceId);
  };

  // L'adoption se termine sans mise à jour du workspace : on repasse sur ceux qui attendent.
  const timer = setInterval(() => {
    for (const entry of entries.values()) if (!entry.settled) refresh(entry);
  }, 5000);

  let release: (() => Promise<void>) | null = null;
  void client.paseo.workspaces
    .list({ subscribe: {} })
    .then(({ subscription }) => {
      if (stopped) return void subscription.release();
      release = () => subscription.release();
      subscription.subscribe({
        snapshot: ({ entries: list }) => {
          for (const id of [...entries.keys()]) unregister(id);
          for (const workspace of list) register(workspace.id, workspace.workspaceDirectory);
        },
        update: (message) => {
          if (message.type !== "workspace_update") return;
          const update = message.payload;
          if (update.kind === "upsert") return register(update.workspace.id, update.workspace.workspaceDirectory);
          unregister(update.id);
        },
      });
      return undefined;
    })
    .catch((error) => {
      if (!stopped) console.error("wt : suivi des workspaces impossible", error);
    });

  return () => {
    stopped = true;
    clearInterval(timer);
    void release?.();
    for (const id of [...entries.keys()]) unregister(id);
  };
}
