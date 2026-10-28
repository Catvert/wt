import type { PluginTheme } from "@getpaseo/plugin";
import { openExternalUrl, usePaseo, useRpc, type PluginButtonIconProps } from "@getpaseo/plugin/client";
import { useToast } from "@getpaseo/plugin/client/ui";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Pressable, Text, View } from "react-native";
import { addresses, startTask, wtAdopt, type WtInfo } from "../shared/wt";
import { useWtInfo } from "./use-wt-info";

type Props = {
  workspaceId: string;
  theme: PluginTheme;
  compact: boolean;
  // Le panneau en a besoin, le popover non : il reste court.
  full: boolean;
  // Présent dans un panneau sur Electron : ouvre l'adresse dans un onglet navigateur de Paseo.
  openBrowser?: (input: { url: string; workspaceId: string }) => void;
  onDone?: () => void;
  onDetails?: () => void;
};

export function WtView({ workspaceId, theme, compact, full, openBrowser, onDone, onDetails }: Props) {
  // Après « Démarrer », on regarde plus souvent jusqu'à ce que le projet réponde.
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const { directory, data, isLoading, error, refetch } = useWtInfo(workspaceId, true, startedAt !== null);
  const paseo = usePaseo();
  // Les terminaux que le plugin a ouverts dans ce workspace : « wt run dev » se reconnaît à son nom.
  const terminals = useQuery({
    queryKey: ["wt-terminals", workspaceId],
    queryFn: async () => (await paseo.workspaces.ref(workspaceId).terminals.list()).entries,
    refetchInterval: 5000,
  });
  useEffect(() => {
    if (startedAt !== null && (data?.running || Date.now() - startedAt > 90_000)) setStartedAt(null);
  }, [data?.running, startedAt]);
  const toast = useToast();
  const queryClient = useQueryClient();
  const adoptRpc = useRpc(wtAdopt);
  const adopt = useMutation({
    mutationFn: () => adoptRpc({ directory: directory as string }),
    onSuccess: (r) => {
      toast.show(r.ok ? "Adopté dans wt" : `Adoption impossible\n${r.output}`, { variant: r.ok ? "success" : "error" });
      void queryClient.invalidateQueries({ queryKey: ["wt-info", directory] });
    },
  });
  const s = useMemo(() => styles(theme, compact), [theme, compact]);

  if (isLoading) return <Text style={[s.muted, s.pad]}>wt…</Text>;
  if (error) return <Text style={[s.danger, s.pad]}>{String(error)}</Text>;
  if (!data || data.kind === "none") return <Text style={[s.muted, s.pad]}>Pas de wt.toml pour ce workspace.</Text>;
  if (data.kind === "main") return <Text style={[s.muted, s.pad]}>{data.project?.name} · dépôt principal</Text>;
  if (data.kind === "adopting") return <Text style={[s.muted, s.pad]}>Adoption dans wt en cours (dépendances, ports)…</Text>;
  if (data.kind === "unmanaged") {
    return (
      <View style={s.root}>
        <Text style={s.muted}>Ce checkout de {data.project?.name} n'est pas encore un worktree wt.</Text>
        <Button s={s} primary disabled={adopt.isPending} onPress={() => adopt.mutate()}>
          {adopt.isPending ? "Adoption…" : "Adopter dans wt"}
        </Button>
      </View>
    );
  }

  const project = data.project!;
  const w = data.worktree!;
  const detail = data.detail;

  // Dans un terminal Paseo : les [[prompt]] de wt ont un vrai pty, et la sortie reste visible.
  const inTerminal = async (name: string, args: string[], close = true) => {
    await paseo.workspaces.ref(workspaceId).terminals.create({
      name,
      command: "wt",
      args: ["-C", project.main, ...args],
    });
    void terminals.refetch();
    if (close) onDone?.();
  };
  const start = startTask(project);
  const startTerminal = start ? terminals.data?.find((t) => t.name === `wt run ${start.name}`) : undefined;
  const reachable = (url: string) => data.reachable[url];
  const open = (url: string, inPaseo: boolean) => {
    if (inPaseo && openBrowser) openBrowser({ url, workspaceId });
    else void openExternalUrl(url);
    onDone?.();
  };

  return (
    <View style={s.root}>
      <View style={s.row}>
        <Text style={[s.title, { flex: 1 }]} numberOfLines={1}>{w.slug}</Text>
        {startedAt !== null ? <Text style={s.muted}>démarrage…</Text> : <State s={s} theme={theme} running={data.running} />}
      </View>
      <Text style={s.muted} numberOfLines={1}>{w.branch}</Text>

      <View style={[s.row, { flexWrap: "wrap" }]}>
        {project.has_up && !data.running ? (
          <Button s={s} primary onPress={() => void inTerminal("wt up", ["up", w.slug])}>▶ Démarrer (wt up)</Button>
        ) : null}
        {project.has_down && data.running ? (
          <Button s={s} onPress={() => void inTerminal("wt down", ["down", w.slug])}>■ Arrêter (wt down)</Button>
        ) : null}
        {start && startTerminal ? (
          <Button s={s} onPress={() => void paseo.terminals.ref(startTerminal.id).kill().then(() => terminals.refetch())}>
            ■ Arrêter {start.name}
          </Button>
        ) : null}
        {start && !startTerminal && !data.running ? (
          <Button
            s={s}
            primary
            disabled={startedAt !== null}
            onPress={() => {
              setStartedAt(Date.now());
              void inTerminal(`wt run ${start.name}`, ["run", start.name, w.slug], false);
            }}
          >
            {startedAt !== null ? "Démarrage…" : `▶ Démarrer (wt run ${start.name})`}
          </Button>
        ) : null}
      </View>
      {start && !startTerminal && data.running ? (
        <Text style={s.muted}>Lancé hors du plugin (script Paseo ou terminal) : arrêtez-le là-bas.</Text>
      ) : null}
      {!project.has_up && !start && data.running === false ? (
        <Text style={s.muted}>Rien n'écoute sur ses ports, et le wt.toml ne dit pas comment le démarrer.</Text>
      ) : null}

      <Section s={s} title="Adresses">
        {addresses(data).map((a) => (
          <View key={a.url} style={s.address}>
            <Dot theme={theme} on={reachable(a.url)} />
            <Pressable accessibilityRole="link" accessibilityLabel={`Ouvrir ${a.label}`} style={{ flex: 1 }} onPress={() => open(a.url, true)}>
              <Text style={s.value}>
                {a.label}
                {reachable(a.url) === false ? <Text style={s.muted}> · ne répond pas</Text> : null}
              </Text>
              <Text style={[s.url, reachable(a.url) === false && { color: theme.colors.foregroundMuted }]} numberOfLines={1}>
                {a.url}
              </Text>
            </Pressable>
            {openBrowser ? (
              <Pressable accessibilityRole="link" accessibilityLabel="Navigateur externe" onPress={() => open(a.url, false)}>
                <Text style={s.icon}>↗</Text>
              </Pressable>
            ) : null}
          </View>
        ))}
        {addresses(data).length === 0 ? <Text style={s.muted}>Aucune adresse.</Text> : null}
      </Section>

      {Object.keys(w.ports).length > 0 ? (
        <Section s={s} title="Ports">
          <Text style={s.value}>{Object.entries(w.ports).map(([k, v]) => `${k} ${v}`).join(" · ")}</Text>
        </Section>
      ) : null}

      {full ? <Pairs s={s} title="Options" values={w.opts} /> : null}
      {full && detail ? <Pairs s={s} title="Infos" values={detail.info} /> : null}
      {full && detail && detail.changes.length > 0 ? (
        <Section s={s} title={`Modifications (${detail.changes.length})`}>
          {detail.changes.slice(0, 12).map((c) => (
            <Text key={c} style={s.mono} numberOfLines={1}>{c}</Text>
          ))}
        </Section>
      ) : null}

      {full && project.tasks.length > 0 ? (
        <Section s={s} title="Tâches">
          {project.tasks.map((t) => (
            <Pressable key={t.name} style={s.task} onPress={() => void inTerminal(`wt run ${t.name}`, ["run", t.name, w.slug])}>
              <Text style={s.value}>▶ {t.name}</Text>
              <Text style={[s.muted, { flex: 1 }]} numberOfLines={1}>{t.description}</Text>
            </Pressable>
          ))}
        </Section>
      ) : null}

      <View style={[s.row, { marginTop: 8, flexWrap: "wrap" }]}>
        <Button s={s} onPress={() => void refetch()}>Actualiser</Button>
        {onDetails ? <Button s={s} onPress={onDetails}>Panneau</Button> : null}
      </View>
    </View>
  );
}

type S = ReturnType<typeof styles>;

function Section({ s, title, children }: { s: S; title: string; children: ReactNode }) {
  return (
    <View style={{ gap: 4 }}>
      <Text style={s.section}>{title}</Text>
      {children}
    </View>
  );
}

function Pairs({ s, title, values }: { s: S; title: string; values: Record<string, string> }) {
  const entries = Object.entries(values);
  if (entries.length === 0) return null;
  return (
    <Section s={s} title={title}>
      {entries.map(([k, v]) => (
        <View key={k} style={s.row}>
          <Text style={[s.muted, { width: 90 }]}>{k}</Text>
          <Text style={[s.value, { flex: 1 }]} numberOfLines={1}>{v}</Text>
        </View>
      ))}
    </Section>
  );
}

function State({ s, theme, running }: { s: S; theme: PluginTheme; running: boolean | null }) {
  if (running === null) return null;
  return (
    <View style={s.row}>
      <Dot theme={theme} on={running} />
      <Text style={[s.badge, { color: running ? theme.colors.statusSuccess : theme.colors.foregroundMuted }]}>
        {running ? "en ligne" : "arrêté"}
      </Text>
    </View>
  );
}

/** Pastille d'état : pleine et verte si ça répond, creuse sinon, absente si on ne sait pas. */
function Dot({ theme, on, size = 8 }: { theme: PluginTheme; on: boolean | null | undefined; size?: number }) {
  if (on === undefined || on === null) return null;
  return (
    <View
      style={{
        width: size,
        height: size,
        borderRadius: size / 2,
        borderWidth: 1.5,
        borderColor: on ? theme.colors.statusSuccess : theme.colors.foregroundMuted,
        backgroundColor: on ? theme.colors.statusSuccess : "transparent",
      }}
    />
  );
}

/** Icône du bouton d'en-tête selon l'état du worktree. */
export function headerIcon(running: boolean | null) {
  return function WtStateIcon({ size, color, theme }: PluginButtonIconProps) {
    if (running === null) return <Text style={{ color, fontSize: size * 0.8 }}>⑂</Text>;
    return (
      <View style={{ width: size, height: size, alignItems: "center", justifyContent: "center" }}>
        <Dot theme={theme} on={running} size={Math.round(size * 0.55)} />
      </View>
    );
  };
}

function Button({ s, primary, disabled, onPress, children }: { s: S; primary?: boolean; disabled?: boolean; onPress(): void; children: ReactNode }) {
  return (
    <Pressable accessibilityRole="button" disabled={disabled} style={primary ? s.primary : s.ghost} onPress={onPress}>
      <Text style={primary ? s.primaryText : s.value}>{children}</Text>
    </Pressable>
  );
}

/** Libellé du bouton d'en-tête : `wt · <premier port>`, ou un état. */
export function headerLabel(info: WtInfo) {
  if (info.kind === "adopting") return "wt…";
  if (info.kind === "unmanaged") return "wt ?";
  const port = info.worktree ? Object.values(info.worktree.ports)[0] : undefined;
  return port ? `wt · ${port}` : "wt";
}

function styles(theme: PluginTheme, compact: boolean) {
  const c = theme.colors;
  return {
    root: { gap: 8, padding: compact ? 12 : 16 },
    pad: { padding: compact ? 12 : 16 },
    row: { flexDirection: "row" as const, alignItems: "center" as const, gap: 8 },
    title: { color: c.foreground, fontSize: 16, fontWeight: "600" as const },
    badge: { fontSize: 12 },
    section: { color: c.foregroundMuted, fontSize: 11, textTransform: "uppercase" as const, marginTop: 4 },
    address: {
      flexDirection: "row" as const, alignItems: "center" as const, gap: 8,
      borderWidth: 1, borderColor: c.border, borderRadius: 8, padding: 8, backgroundColor: c.surface1,
    },
    task: { flexDirection: "row" as const, alignItems: "center" as const, gap: 8, paddingVertical: 4 },
    url: { color: c.accent, fontSize: 12 },
    icon: { color: c.foregroundMuted, fontSize: 16, paddingHorizontal: 4 },
    value: { color: c.foreground, fontSize: 13 },
    mono: { color: c.foreground, fontSize: 12, fontFamily: "monospace" },
    muted: { color: c.foregroundMuted, fontSize: 13 },
    danger: { color: c.statusDanger, fontSize: 13 },
    primary: { paddingVertical: 6, paddingHorizontal: 12, borderRadius: 6, backgroundColor: c.accent },
    primaryText: { color: c.accentForeground },
    ghost: { paddingVertical: 6, paddingHorizontal: 12, borderRadius: 6, borderWidth: 1, borderColor: c.border },
  };
}
