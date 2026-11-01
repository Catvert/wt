import type {
  PluginButtonContentProps,
  PluginButtonRegistration,
  PluginClientContext,
  PluginWorkspacePanelProps,
} from "@getpaseo/plugin/client";
import { openExternalUrl } from "@getpaseo/plugin/client";
import { ScrollView } from "@getpaseo/plugin/client/ui";
import { useEffect } from "react";
import { addresses, savedAnswer, startTask, upArgs, wtInfo, type WtInfo } from "./shared/wt";
import { headerIcon, headerLabel, runIcon, WtView, type RunKind } from "./client/wt-view";

// openBrowser de chaque hôte, retenu du dernier panneau wt affiché : Paseo ne le donne pas aux
// popovers des boutons d'en-tête. Sans lui, le popover ouvre le navigateur du système.
type OpenBrowser = (input: { url: string; workspaceId: string }) => void;
const browsers = new Map<string, OpenBrowser>();

function WtPanel({ workspaceId, theme, layout, host, navigation }: PluginWorkspacePanelProps) {
  const openBrowser = navigation?.openBrowser;
  useEffect(() => {
    if (openBrowser) browsers.set(host.id, openBrowser);
  }, [host.id, openBrowser]);
  return (
    <ScrollView style={{ flex: 1, backgroundColor: theme.colors.surface0 }}>
      <WtView
        workspaceId={workspaceId}
        theme={theme}
        compact={layout.compact}
        full
        openBrowser={openBrowser}
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

  // Un bouton « wt » dans l'en-tête de chaque workspace qui relève d'un projet wt, suivi d'un
  // bouton ▶/■ qui démarre ou arrête le worktree d'un clic, comme le bouton Run d'un IDE.
  type Run = { kind: RunKind; title: string; action?: () => Promise<unknown> };
  type Entry = {
    workspaceId: string;
    button: PluginButtonRegistration;
    run: PluginButtonRegistration;
    directory: string;
    settled: boolean;
    shown: boolean;
    running: boolean | null;
    // Le bouton ▶/■ : ce qu'il affiche (pour ne le mettre à jour qu'au changement) et ce qu'il fait.
    runShown: string | null;
    runAction?: () => Promise<unknown>;
    // Après un clic, l'état attendu (true : en marche) ; le sablier reste jusqu'à ce qu'il
    // arrive, 90 s au plus.
    pending: { up: boolean; since: number } | null;
  };
  const entries = new Map<string, Entry>();
  let stopped = false;

  /** Ce que fait le bouton ▶/■, comme les boutons du popover, ses réponses enregistrées comprises. */
  const runState = async (entry: Entry, info: WtInfo): Promise<Run | null> => {
    const project = info.project;
    const w = info.worktree;
    if (info.kind !== "worktree" || !project || !w || info.running === null) return null;
    if (entry.pending) {
      if (info.running !== entry.pending.up && Date.now() - entry.pending.since < 90_000) {
        return entry.pending.up
          ? { kind: "starting", title: "wt : démarrage…" }
          : { kind: "stopping", title: "wt : arrêt…" };
      }
      entry.pending = null;
    }
    const workspace = client.paseo.workspaces.ref(entry.workspaceId);
    // Dans un terminal Paseo du workspace : les [[prompt]] de wt ont un vrai pty, et la sortie
    // reste visible. Le terminal de `@` reste ouvert tant que sa commande tourne.
    const inTerminal = (name: string, args: string[]) =>
      workspace.terminals.create({ name, command: "wt", args: ["-C", project.main, ...args] });
    const canUp = w.main || project.has_up;
    const canDown = w.main || project.has_down;
    const task = startTask(project, w);
    if (!info.running) {
      if (canUp) {
        const name = w.main ? "wt up @" : "wt up";
        const args = upArgs(project, w, (p) => savedAnswer(w, p), info.defaultStopAfter ?? "never");
        return { kind: "start", title: `Démarrer (${name})`, action: () => inTerminal(name, args) };
      }
      if (task) {
        const name = `wt run ${task.name}`;
        return { kind: "start", title: `Démarrer (${name})`, action: () => inTerminal(name, ["run", task.name, w.slug]) };
      }
      return null;
    }
    if (canDown) {
      const name = w.main ? "wt down @" : "wt down";
      return { kind: "stop", title: `Arrêter (${name})`, action: () => inTerminal("wt down", ["down", w.slug]) };
    }
    if (task) {
      // « wt run dev » s'arrête en fermant son terminal, s'il est du plugin (il porte son nom).
      const { entries: terminals } = await workspace.terminals.list();
      const terminal = terminals.find((t) => t.name === `wt run ${task.name}`);
      if (terminal) {
        return { kind: "stop", title: `Arrêter ${task.name}`, action: () => client.paseo.terminals.ref(terminal.id).kill() };
      }
      return { kind: "elsewhere", title: `${task.name} lancé hors du plugin : arrêtez-le là où il tourne` };
    }
    return null;
  };

  const showRun = (entry: Entry, run: Run | null) => {
    entry.runAction = run?.action;
    const key = run ? `${run.kind} ${run.title}` : null;
    if (key === entry.runShown) return;
    entry.runShown = key;
    if (!run) return entry.run.update({ visible: false });
    entry.run.update({ visible: true, title: run.title, icon: runIcon(run.kind), disabled: !run.action });
  };

  const press = async (entry: Entry) => {
    const action = entry.runAction;
    if (!action || entry.pending) return;
    entry.pending = { up: entry.running !== true, since: Date.now() };
    showRun(entry, entry.pending.up ? { kind: "starting", title: "wt : démarrage…" } : { kind: "stopping", title: "wt : arrêt…" });
    try {
      await action();
    } catch (error) {
      entry.pending = null;
      console.error("wt : démarrage ou arrêt impossible", error);
    }
    refresh(entry);
  };

  const refresh = (entry: Entry) => {
    void client
      .rpc(wtInfo, { directory: entry.directory, detail: false })
      .then(async (info) => {
        const visible = info.kind === "worktree" || info.kind === "adopting" || info.kind === "unmanaged";
        entry.settled = info.kind !== "adopting" && info.kind !== "unmanaged";
        if (entry.running !== info.running || !entry.shown) {
          entry.running = info.running;
          entry.button.update({ icon: headerIcon(info.running) });
        }
        entry.shown = true;
        entry.button.update({ visible, label: headerLabel(info) });
        showRun(entry, await runState(entry, info));
      })
      .catch((error) => console.error("wt : état du worktree illisible", error));
  };

  const register = (workspaceId: string, directory: string | undefined) => {
    if (stopped || !directory) return;
    const existing = entries.get(workspaceId);
    if (existing) return refresh(existing);
    const Content = ({ theme, layout, host, close }: PluginButtonContentProps) => (
      <WtView
        workspaceId={workspaceId}
        theme={theme}
        compact={layout.compact}
        full={false}
        openBrowser={browsers.get(host.id)}
        onDone={close}
        onDetails={() => {
          close();
          client.openPanel("wt", { workspaceId });
        }}
      />
    );
    const button = client.addHeaderButton({
      id: "wt",
      workspaceId,
      button: { title: "wt", icon: "Globe", label: "wt", visible: false, behavior: { kind: "popover", Content } },
    });
    const run = client.addHeaderButton({
      id: "wt-run",
      workspaceId,
      button: {
        title: "wt",
        icon: "Play",
        visible: false,
        behavior: { kind: "action", onPress: () => press(entry) },
      },
    });
    const entry: Entry = {
      workspaceId,
      directory,
      settled: false,
      shown: false,
      running: null,
      runShown: null,
      pending: null,
      button,
      run,
    };
    entries.set(workspaceId, entry);
    refresh(entry);
  };
  const unregister = (workspaceId: string) => {
    const entry = entries.get(workspaceId);
    entry?.button.remove();
    entry?.run.remove();
    entries.delete(workspaceId);
  };

  // L'adoption se termine et le serveur de dev démarre sans mise à jour du workspace :
  // on repasse vite sur ceux qui attendent (adoption, clic sur ▶/■), et toutes les 15 s sur
  // les autres.
  let tick = 0;
  const timer = setInterval(() => {
    tick++;
    for (const entry of entries.values()) if (!entry.settled || entry.pending || tick % 3 === 0) refresh(entry);
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
