import type { PluginTheme } from "@getpaseo/plugin";
import { openExternalUrl, usePaseo, useRpc } from "@getpaseo/plugin/client";
import { useToast } from "@getpaseo/plugin/client/ui";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useMemo, type ReactNode } from "react";
import { Pressable, Text, View } from "react-native";
import { addresses, wtAdopt, type WtInfo } from "../shared/wt";
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
  const { directory, data, isLoading, error, refetch } = useWtInfo(workspaceId, true);
  const paseo = usePaseo();
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
  const inTerminal = async (name: string, args: string[]) => {
    await paseo.workspaces.ref(workspaceId).terminals.create({
      name,
      command: "wt",
      args: ["-C", project.main, ...args],
    });
    onDone?.();
  };
  const open = (url: string, inPaseo: boolean) => {
    if (inPaseo && openBrowser) openBrowser({ url, workspaceId });
    else void openExternalUrl(url);
    onDone?.();
  };

  return (
    <View style={s.root}>
      <View style={s.row}>
        <Text style={s.title}>{w.slug}</Text>
        {w.up !== null ? <State s={s} theme={theme} up={w.up} /> : null}
      </View>
      <Text style={s.muted} numberOfLines={1}>{w.branch}</Text>

      <Section s={s} title="Adresses">
        {addresses(data).map((a) => (
          <View key={a.url} style={s.address}>
            <Pressable accessibilityRole="link" accessibilityLabel={`Ouvrir ${a.label}`} style={{ flex: 1 }} onPress={() => open(a.url, true)}>
              <Text style={s.value}>{a.label}</Text>
              <Text style={s.url} numberOfLines={1}>{a.url}</Text>
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
        {project.has_up && !w.up ? (
          <Button s={s} primary onPress={() => void inTerminal("wt up", ["up", w.slug])}>wt up</Button>
        ) : null}
        {project.has_down && w.up !== false ? (
          <Button s={s} onPress={() => void inTerminal("wt down", ["down", w.slug])}>wt down</Button>
        ) : null}
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

function State({ s, theme, up }: { s: S; theme: PluginTheme; up: boolean }) {
  return <Text style={[s.badge, { color: up ? theme.colors.statusSuccess : theme.colors.foregroundMuted }]}>{up ? "démarré" : "arrêté"}</Text>;
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
