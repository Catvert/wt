import { execFile } from "node:child_process";
import { realpath } from "node:fs/promises";
import { connect } from "node:net";
import type { ZodType, output } from "zod";
import {
  addresses,
  detailSchema,
  tools,
  projectSchema,
  worktreeSchema,
  type WtInfo,
  type WtProject,
  type WtWorktree,
} from "../shared/wt";

const env = { ...process.env, WT_LANG: "en", NO_COLOR: "1" };
const strip = (s: string) => s.replace(/\x1b\[[0-9;]*m/g, "");

export function run(cmd: string, args: string[], timeout = 20000) {
  return new Promise<{ ok: boolean; stdout: string; stderr: string }>((resolve) => {
    const child = execFile(cmd, args, { env, timeout, maxBuffer: 16 << 20 }, (err, stdout, stderr) => {
      resolve({ ok: !err, stdout: strip(String(stdout)), stderr: strip(String(stderr)) });
    });
    child.stdin?.end();
  });
}

async function json<S extends ZodType>(schema: S, args: string[]): Promise<output<S> | null> {
  const r = await run("wt", [...args, "--json"]);
  if (!r.ok && /unexpected argument '--json'/.test(r.stderr)) {
    throw new Error("le wt du PATH est trop ancien : --json demande wt ≥ 0.10");
  }
  if (!r.ok) return null;
  const parsed = schema.safeParse(JSON.parse(r.stdout));
  return parsed.success ? parsed.data : null;
}

export async function real(path: string) {
  return realpath(path).catch(() => path);
}

export const wt = {
  root: (dir: string) => json(projectSchema, ["root", "-C", dir]),
  ls: async (main: string) => (await json(worktreeSchema.array(), ["ls", "-C", main])) ?? [],
  show: (main: string, slug: string) => json(detailSchema, ["show", slug, "-C", main]),
};

/** Le worktree wt dont `directory` est le checkout (ou le lien `<root>/<slug>`). */
export async function findWorktree(worktrees: WtWorktree[], directory: string) {
  const here = await real(directory);
  for (const w of worktrees) {
    if (w.checkout === directory || w.dir === directory) return w;
    if ((await real(w.checkout)) === here || (await real(w.dir)) === here) return w;
  }
  return null;
}

// Adoptions lancées par les hooks et pas encore terminées, par checkout.
export const adopting = new Set<string>();

export async function getInfo({ directory, detail }: { directory: string; detail: boolean }): Promise<WtInfo> {
  const base: WtInfo = {
    kind: "none", project: null, worktree: null, detail: null, proxyUrl: null,
    running: null, reachable: {}, error: null,
  };
  const project: WtProject | null = await wt.root(directory);
  if (!project) return base;
  if ((await real(directory)) === (await real(project.main))) return { ...base, kind: "main", project };
  const worktree = await findWorktree(await wt.ls(project.main), directory);
  if (!worktree) return { ...base, kind: adopting.has(directory) ? "adopting" : "unmanaged", project };
  const [show, proxy] = await Promise.all([
    detail ? wt.show(project.main, worktree.slug) : Promise.resolve(null),
    run("wt-proxy-url", [directory], 5000),
  ]);
  const proxyUrl = proxy.ok && proxy.stdout.trim().startsWith("http") ? proxy.stdout.trim() : null;
  const info: WtInfo = { ...base, kind: "worktree", project, worktree, detail: show, proxyUrl };
  const ports = Object.values(worktree.ports);
  info.running =
    worktree.up ?? (ports.length > 0 ? (await Promise.all(ports.map(listening))).some(Boolean) : null);
  if (detail) {
    const urls = [...addresses(info), ...tools(info)].map((a) => a.url);
    const answers = await Promise.all(urls.map(responds));
    info.reachable = Object.fromEntries(urls.map((u, i) => [u, answers[i]]));
  }
  return info;
}

/** Un port écoute-t-il en local ? */
function listening(port: number) {
  return new Promise<boolean>((resolve) => {
    const socket = connect({ host: "127.0.0.1", port, timeout: 500 });
    const done = (ok: boolean) => {
      socket.destroy();
      resolve(ok);
    };
    socket.once("connect", () => done(true));
    socket.once("error", () => done(false));
    socket.once("timeout", () => done(false));
  });
}

/** L'adresse répond-elle ? Un proxy dont l'application ne tourne pas répond 502 à 504. */
async function responds(url: string) {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), 1500);
  try {
    const r = await fetch(url, { method: "HEAD", redirect: "manual", signal: abort.signal });
    return r.status < 502 || r.status > 504;
  } catch {
    return false;
  } finally {
    clearTimeout(timer);
  }
}
