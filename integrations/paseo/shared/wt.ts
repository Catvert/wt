import { defineRpc } from "@getpaseo/plugin";
import { z } from "zod";

// Les formes de `wt … --json` (src/json.rs dans wt) : des champs ajoutés, jamais renommés.
// icon : un outil du worktree (base de données, mails…), pas une adresse de l'application ;
// wt ≥ 0.11, absent avant.
export const linkSchema = z.object({ url: z.string(), label: z.string(), icon: z.string().nullable().default(null) });

export const taskSchema = z.object({
  name: z.string(),
  description: z.string(),
  interactive: z.boolean(),
});

export const promptSchema = z.object({
  name: z.string(),
  question: z.string(),
  type: z.enum(["choice", "multi", "confirm", "text"]),
  ask: z.enum(["up", "new", "both", "task"]),
  options: z.array(z.object({ value: z.string(), label: z.string(), detail: z.string() })),
  has_source: z.boolean(),
  default: z.string().nullable(),
  separator: z.string(),
  when: z.string().nullable(),
  always: z.boolean(),
});

export const projectSchema = z.object({
  name: z.string(),
  config: z.string(),
  main: z.string(),
  root: z.string(),
  has_up: z.boolean(),
  has_down: z.boolean(),
  has_status: z.boolean(),
  has_ports: z.boolean(),
  has_open: z.boolean(),
  tasks: z.array(taskSchema),
  // wt ≥ 0.11 ; absent avant.
  prompts: z.array(promptSchema).default([]),
});

export const worktreeSchema = z.object({
  slug: z.string(),
  dir: z.string(),
  checkout: z.string(),
  adopted: z.boolean(),
  branch: z.string(),
  up: z.boolean().nullable(),
  ports: z.record(z.string(), z.number()),
  opts: z.record(z.string(), z.string()),
  url: z.string().nullable(),
});

export const detailSchema = worktreeSchema.extend({
  commit: z.string(),
  info: z.record(z.string(), z.string()),
  changes: z.array(z.string()),
  links: z.array(linkSchema),
});

export type WtProject = z.infer<typeof projectSchema>;
export type WtPrompt = z.infer<typeof promptSchema>;
export type WtWorktree = z.infer<typeof worktreeSchema>;
export type WtDetail = z.infer<typeof detailSchema>;
export type WtLink = z.infer<typeof linkSchema>;

export const wtInfoSchema = z.object({
  // none : pas de wt.toml · main : le dépôt principal · worktree : géré par wt
  // adopting : adoption en cours · unmanaged : checkout du projet que wt ne connaît pas
  kind: z.enum(["none", "main", "worktree", "adopting", "unmanaged"]),
  project: projectSchema.nullable(),
  worktree: worktreeSchema.nullable(),
  detail: detailSchema.nullable(),
  // Adresse du proxy des worktrees quand la machine en a un (wt-proxy-url, sur le VPS).
  proxyUrl: z.string().nullable(),
  // Le worktree tourne-t-il ? `[status] up` quand le projet en a un, sinon un de ses ports
  // écoute. null : ni l'un ni l'autre, rien à en dire.
  running: z.boolean().nullable(),
  // Avec detail : chaque adresse répond-elle (par URL) ?
  reachable: z.record(z.string(), z.boolean()),
  error: z.string().nullable(),
});
export type WtInfo = z.infer<typeof wtInfoSchema>;

export const wtInfo = defineRpc({
  name: "wt.info",
  // detail : lance aussi `wt show` (sondes [status.info] et [open] source), pour les vues ouvertes.
  input: z.object({ directory: z.string(), detail: z.boolean().default(false) }),
  output: wtInfoSchema,
});

export const wtAdopt = defineRpc({
  name: "wt.adopt",
  input: z.object({ directory: z.string() }),
  output: z.object({ ok: z.boolean(), output: z.string() }),
});

/**
 * La tâche qui démarre le projet quand le wt.toml n'a pas de hook `up` : un serveur de dev
 * lancé par `wt run dev`, par exemple.
 */
export function startTask(project: WtProject) {
  if (project.has_up) return null;
  return project.tasks.find((t) => ["dev", "start", "serve"].includes(t.name)) ?? null;
}

/** Les adresses à proposer : le proxy, puis celles de wt, sinon une par port. */
export function addresses(info: WtInfo): WtLink[] {
  const out: WtLink[] = [];
  if (info.proxyUrl) out.push({ url: info.proxyUrl, label: "proxy", icon: null });
  const links = info.detail?.links.filter((l) => !l.icon) ??
    (info.worktree?.url ? [{ url: info.worktree.url, label: "application", icon: null }] : []);
  out.push(...links.filter((l) => !out.some((o) => o.url === l.url)));
  if (out.length === 0 && info.worktree) {
    for (const [name, port] of Object.entries(info.worktree.ports)) {
      out.push({ url: `http://localhost:${port}`, label: name, icon: null });
    }
  }
  return out;
}

/** Les outils du worktree : les liens de `[open] source` qui portent une icône. */
export function tools(info: WtInfo): WtLink[] {
  return info.detail?.links.filter((l) => l.icon) ?? [];
}

/**
 * Les questions de `wt up` que le panneau peut poser lui-même : choix fixes, sans condition
 * (`when` dépend de réponses qu'il ne peut pas évaluer). Les autres restent au dialogue de wt,
 * dans le terminal.
 */
export function upPrompts(project: WtProject): WtPrompt[] {
  return project.prompts.filter(
    (p) =>
      (p.ask === "up" || p.ask === "both") &&
      !p.has_source &&
      p.when === null &&
      (p.type === "confirm" || (p.type !== "text" && p.options.length > 0)),
  );
}

/** Un choix oui/non (`yes`/`no`, `1`/`0`…) : une case à cocher plutôt que deux pastilles. */
export function toggleValues(p: WtPrompt): { on: string; off: string } | null {
  if (p.type === "confirm") return { on: "1", off: "0" };
  if (p.type !== "choice" || p.options.length !== 2) return null;
  const values = p.options.map((o) => o.value);
  for (const [on, off] of [["yes", "no"], ["oui", "non"], ["1", "0"], ["true", "false"], ["on", "off"]]) {
    if (values.includes(on) && values.includes(off)) return { on, off };
  }
  return null;
}
