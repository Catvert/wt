import { existsSync, readFileSync } from "node:fs";
import { rm } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, isAbsolute, join, resolve, sep } from "node:path";
import { adopting, findWorktree, real, run, wt } from "./wt";

const expandTilde = (path: string) => {
  const home = process.env.HOME || homedir();
  return path === "~" ? home : path.startsWith("~/") ? home + path.slice(1) : path;
};

/**
 * Où Paseo range ses worktrees, selon les règles de son démon : `worktrees.root` de
 * `$PASEO_HOME/config.json` (absolu, en `~/`, ou relatif à PASEO_HOME), par défaut
 * `$PASEO_HOME/worktrees`. Lu une fois, comme le démon, qui ne le prend qu'à son démarrage.
 */
function paseoWorktreesRoot() {
  const paseoHome = resolve(expandTilde(process.env.PASEO_HOME ?? "~/.paseo"));
  let configured: unknown;
  try {
    configured = JSON.parse(readFileSync(join(paseoHome, "config.json"), "utf8"))?.worktrees?.root;
  } catch {
    // Pas de config.json lisible : le défaut du démon.
  }
  const root = typeof configured === "string" ? expandTilde(configured.trim()) : "";
  if (!root) return join(paseoHome, "worktrees");
  return isAbsolute(root) ? resolve(root) : resolve(paseoHome, root);
}

// Le plugin ne touche qu'aux worktrees que Paseo a créés lui-même : un dossier local
// ouvert dans Paseo, ou un checkout d'un autre outil, reste à son propriétaire.
const paseoWorktrees = paseoWorktreesRoot() + sep;

const log = (msg: string) => console.log(`[wt] ${msg}`);

function slugFor(directory: string, taken: Set<string>) {
  const base =
    basename(directory)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 30)
      .replace(/-+$/, "") || "paseo";
  let slug = base;
  for (let i = 2; taken.has(slug); i++) slug = `${base}-${i}`;
  return slug;
}

/** Un `paseo.json` qui a son propre `worktree.setup` gère déjà l'adoption (scripts/wt/paseo.sh). */
function projectAdoptsItself(directory: string) {
  try {
    const config = JSON.parse(readFileSync(join(directory, "paseo.json"), "utf8"));
    const setup = config?.worktree?.setup;
    return typeof setup === "string" ? setup.trim() !== "" : Array.isArray(setup) && setup.length > 0;
  } catch {
    return false;
  }
}

/** `wt new --adopt` : port, copies, post_new (npm ci…) comme pour un worktree créé par wt. */
export async function adopt(directory: string, { automatic }: { automatic: boolean }) {
  if (automatic && !directory.startsWith(paseoWorktrees)) return { ok: false, output: "pas un worktree Paseo" };
  const project = await wt.root(directory);
  if (!project) return { ok: false, output: "pas de wt.toml" };
  if ((await real(directory)) === (await real(project.main))) return { ok: false, output: "dépôt principal" };
  if (automatic && projectAdoptsItself(directory)) {
    log(`${directory} : adoption laissée au worktree.setup du paseo.json`);
    return { ok: false, output: "adoption gérée par paseo.json" };
  }
  const worktrees = await wt.ls(project.main);
  if (await findWorktree(worktrees, directory)) return { ok: true, output: "déjà adopté" };
  if (adopting.has(directory)) return { ok: false, output: "adoption déjà en cours" };

  const slug = slugFor(directory, new Set(worktrees.map((w) => w.slug)));
  adopting.add(directory);
  log(`${project.name} : adoption de ${directory} en ${slug}`);
  try {
    // Les hooks post_new peuvent être longs (npm ci, composer install) : 15 minutes.
    const r = await run("wt", ["new", slug, "--adopt", directory, "-C", project.main], 15 * 60_000);
    const output = (r.stdout + r.stderr).trim().split("\n").slice(-20).join("\n");
    log(`${slug} : ${r.ok ? "adopté" : "échec de l'adoption"}\n${output}`);
    return { ok: r.ok, output };
  } finally {
    adopting.delete(directory);
  }
}

/**
 * Après l'archivage, Paseo supprime le checkout (git worktree remove, puis rm). `wt gc`
 * ne nettoie qu'un worktree disparu : on attend donc la suppression, puis il lance les
 * hooks de retrait (pre_rm dans un dossier vide, post_rm) et efface l'état et le port.
 *
 * Les services du worktree tournent encore pendant la suppression : Vite recrée aussitôt
 * public/ et .vite/ dans le dossier, qui ne disparaît alors jamais (et que gc prend pour
 * un dossier étranger). Un checkout sans .git est un worktree que Paseo a retiré : ce qui
 * y reste a été régénéré, `wt down` arrête ce qui l'écrit et le dossier est effacé.
 */
export async function release(directory: string) {
  if (!directory.startsWith(paseoWorktrees)) return;
  const project = await wt.root(directory);
  if (!project) return;
  const worktree = await findWorktree(await wt.ls(project.main), directory);
  if (!worktree) return;

  // Le checkout, pas le lien <root>/<slug> : celui-ci reste, pendant, jusqu'au gc.
  const { slug, checkout } = worktree;
  const deadline = Date.now() + 5 * 60_000;
  for (;;) {
    if (!existsSync(checkout)) {
      const r = await run("wt", ["gc", "--apply", slug, "-C", project.main], 10 * 60_000);
      log(`${slug} : wt gc ${r.ok ? "terminé" : "en échec"}\n${(r.stdout + r.stderr).trim()}`);
      // Recréé entre-temps par un service : on recommence.
      if (r.ok || !existsSync(checkout)) return;
    } else if (!existsSync(join(checkout, ".git"))) {
      const r = await run("wt", ["down", slug, "-C", project.main], 2 * 60_000);
      log(`${slug} : restes régénérés dans ${checkout}, wt down ${r.ok ? "terminé" : "en échec"}, dossier effacé`);
      await rm(checkout, { recursive: true, force: true });
    }
    if (Date.now() >= deadline) {
      // Encore un worktree : un autre workspace l'utilise, ou Paseo n'a pas tout supprimé.
      log(`${slug} : ${checkout} toujours présent, wt gc non lancé`);
      return;
    }
    await new Promise((r) => setTimeout(r, 3000));
  }
}

