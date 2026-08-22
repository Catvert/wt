# wt

Gestionnaire de worktrees git, configuré **par projet** dans un fichier `wt.toml` — comme
`just` avec son `justfile`.

*[English version](README.md)*

Le binaire ne connaît ni Docker, ni Laravel, ni npm. Il sait faire cinq choses :

- créer et supprimer des worktrees git ;
- recopier ce que le worktree doit hériter du repo principal (hardlinks pour `vendor/`,
  `node_modules/`… — quasi gratuit sur le disque) ;
- exécuter les commandes shell déclarées par le projet aux moments qui comptent
  (création, démarrage, arrêt, suppression) et à la demande (`[tasks]`) ;
- suivre un état par worktree (branche, options du dernier démarrage, ports si le projet
  en demande) ;
- présenter tout ça dans une interface interactive.

Tout ce qui est spécifique à une stack passe par des hooks shell : un projet Laravel, un
CLI Rust et un traitement de données Python utilisent le même binaire avec trois
`wt.toml` différents.

**Rien n'est présumé web.** Ports, URL et sondes d'état sont facultatifs : un projet qui
n'en déclare pas ne voit ni colonne « PORTS », ni état, ni action « navigateur ». Pour
beaucoup de dépôts, un `wt.toml` de trois lignes suffit :

```toml
branch = "wt/{{slug}}"

[tasks]
test = { description = "tests", run = "cargo test {{args}}" }
```

## Installation

### Nix / NixOS

```bash
nix run github:Catvert/wt                 # essayer sans installer
nix profile install github:Catvert/wt     # installer pour l'utilisateur courant
```

Dans une configuration NixOS ou avec home-manager :

```nix
{
  inputs.wt.url = "github:Catvert/wt";

  # environment.systemPackages = [ inputs.wt.packages.${system}.default ];
  # ou, via l'overlay :
  # nixpkgs.overlays = [ inputs.wt.overlays.default ];
  # environment.systemPackages = [ pkgs.wt ];
}
```

Les complétions shell sont installées par le paquet, complétion des slugs comprise.

### Cache binaire (aucune compilation locale)

Les builds sont poussés sur [Cachix](https://cachix.org) par la CI : `nix run` et
`nix profile install` téléchargent le binaire au lieu de le compiler.

Le flake annonce le substituteur, mais Nix n'honore le `nixConfig` d'un flake qu'après
une confirmation interactive — il est **ignoré silencieusement** dans un script ou en CI.
Mieux vaut le configurer sur la machine. Sur NixOS :

```nix
{
  nix.settings = {
    substituters = [ "https://catvert.cachix.org" ];
    trusted-public-keys = [
      "catvert.cachix.org-1:R5plivdLnx2WtmZkBryZwUF51Uvl6TJldhFGYOcyPXg="
    ];
  };
}
```

Hors NixOS — ou pour un essai ponctuel — `cachix use catvert` écrit la même chose dans
`~/.config/nix/nix.conf`, et un build isolé l'accepte en ligne :

```bash
nix build github:Catvert/wt \
  --option extra-substituters https://catvert.cachix.org \
  --option extra-trusted-public-keys "catvert.cachix.org-1:R5plivdLnx2WtmZkBryZwUF51Uvl6TJldhFGYOcyPXg="
```

### Depuis les sources

```bash
cargo install --git https://github.com/Catvert/wt
```

### Binaire précompilé

Chaque release publie des archives `x86_64-unknown-linux-gnu` et
`x86_64-unknown-linux-musl` (lié statiquement), avec leurs sommes de contrôle.
Complétions :

```bash
wt completions zsh > ~/.zfunc/_wt      # bash | zsh | fish | elvish | powershell
```

## Démarrage

```bash
cd mon-projet
wt init                 # un wt.toml sans service ; --preset web pour port + URL
$EDITOR wt.toml
wt new demo             # crée ../mon-projet-wt/demo sur la branche wt/demo
wt new fix --from dev   # même chose, mais la branche part de dev
wt shell demo           # un shell dans le worktree — claude, un build, un rebase…
wt                      # sélecteur fuzzy : worktree, puis action
wt tui                  # tableau de bord Ratatui persistant
```

## Commandes

| Commande | Effet |
|---|---|
| `wt` | interface Skim : choix fuzzy d'un worktree, puis d'une action |
| `wt tui` | tableau de bord Ratatui persistant (liste, aperçu, actions) |
| `wt init [--preset plain\|web] [--force]` | écrit un `wt.toml` d'exemple |
| `wt new <slug> [branche] [--from ref] [--set k=v]` | checkout + dossiers + copies + hooks `post_new` |
| `wt up [slug] [--set k=v]` | hooks `up` (et allocation des ports, s'il y en a) |
| `wt down [slug]` | hooks `down` — le checkout et l'état sont conservés |
| `wt ls` / `wt show [slug]` | état des worktrees |
| `wt rm [slug] [-y]` | hooks `pre_rm`, retrait du worktree, hooks `post_rm` |
| `wt shell [slug]` | ouvre un shell à la racine du worktree |
| `wt cd [slug]` | se déplace dans le worktree (nécessite `wt shell-init`) |
| `wt shell-init <bash\|zsh\|fish>` | la fonction shell dont `wt cd` a besoin |
| `wt ide [slug] [éditeur]` | ouvre le worktree dans un éditeur |
| `wt open [slug] [cible] [--list]` | ouvre une adresse dans le navigateur (WSL compris) |
| `wt run [tâche] [slug] [args…]` | lance une tâche du `wt.toml` |
| `wt tasks` / `wt root` / `wt path [slug]` | introspection |
| `wt completions <shell>` | script de complétion (slugs, tâches et branches compris) |

`wt` fonctionne depuis le repo principal **comme depuis un worktree** : la configuration
est toujours celle du repo principal, pas celle de la branche en cours de checkout.

Quand une sous-commande omet une valeur que wt sait énumérer, Skim la demande :
`wt open`, par exemple, ouvre le sélecteur de worktree ; `wt run` demande la tâche puis
le worktree. Avec des arguments explicites, le chemin reste direct et adapté aux scripts.

### Interface par défaut : Skim

`wt` ouvre un sélecteur fuzzy léger : choisis un worktree, puis l'action à exécuter.
La création est toujours proposée en tête de liste, y compris quand aucun worktree
n'existe encore. Les sélecteurs suivants — branche, tâche, éditeur, adresse et choix
du `wt.toml` — utilisent eux aussi Skim. `ENTRÉE` choisit, `ÉCHAP` annule et, pour un
choix multiple, `TAB` coche.

L'action choisie récupère ensuite le terminal. C'est notamment naturel pour un shell,
un éditeur terminal ou une tâche interactive. Skim est embarqué dans `wt` : aucun
binaire `fzf` ou `sk` n'est requis.

### Tableau de bord Ratatui : `wt tui`

`wt tui` conserve l'interface persistante avec liste, aperçu, actions en arrière-plan
et panneau de sortie. Ses raccourcis sont :

`↑↓`/`jk` naviguer · `ENTRÉE` menu d'actions · `n` créer · `s` démarrer ·
`S` démarrer avec options · `d` arrêter · `c` shell · `e` éditeur · `o` navigateur ·
`t` tâche · `r` supprimer · `g` rafraîchir · `m` souris · `?` aide · `q` quitter.

**Souris** (active par défaut) : clic sélectionne une ligne, double-clic ouvre le menu
d'actions, la molette fait défiler la liste, un sélecteur ou le panneau de sortie. Dans
une liste à choix multiples, un clic coche. `m` coupe la capture souris — le terminal
retrouve sa sélection de texte native, le temps de copier une ligne.

Le pied de page, le menu d'actions et l'aide n'affichent que ce que le `wt.toml`
déclare : sans `[hooks] up`, pas de « démarrer » ; sans `[open] url`, pas de
« navigateur ».

### Créer un worktree

Dans `wt tui`, `n` — ou « créer un worktree » dans le menu d'actions — enchaîne trois
questions : **quelle branche** (une existante, ou `＋ nouvelle branche`) ; pour une nouvelle, **d'où elle
part** — `dev`, `master`, la branche d'une collègue… — celle en checkout dans le dépôt
principal étant présélectionnée et marquée `●` ; puis le **slug** du worktree. Les
questions du `wt.toml` viennent ensuite, s'il y en a.

`⌫` revient **d'une question** — la branche, le slug, une réponse déjà donnée — avec ce
qui y avait été choisi de nouveau sous le curseur, et les questions suivantes reposées.
Il ne le fait que lorsqu'il n'a plus rien à effacer (recherche vide, champ vide — `^U` le
vide), et la fenêtre affiche alors `⌫  retour`. `ÉCHAP` abandonne toujours l'ensemble.

En ligne de commande, c'est `wt new <slug> [branche] --from <ref>`. `--from` accepte tout
ce que git accepte comme point de départ (branche locale, `origin/dev`, un tag, un
commit) et ne s'applique qu'à une branche **qui n'existe pas encore** : une branche déjà
écrite a son histoire, et wt refuse plutôt que de créer autre chose que ce qui est
demandé.

### Entrer dans un worktree

L'essentiel de ce qu'on fait dans un worktree n'est pas un hook : un `claude`, un
`git rebase -i`, un build qu'on veut regarder passer. Deux chemins, selon qu'on compte
revenir ou non.

`wt shell <slug>` ouvre un shell à la racine du worktree et ne demande rien à installer.
`exit` ramène d'où l'on vient :

```bash
wt shell demo
claude              # dans ../mon-projet-wt/demo
exit
```

`wt cd <slug>` déplace le shell **courant** — pas d'imbrication, pas d'`exit`. Un
processus ne peut pas changer le répertoire de son parent : celui-là a donc besoin d'une
fonction shell, que `wt shell-init` écrit :

```bash
eval "$(wt shell-init bash)"   # dans ~/.bashrc — ou `zsh` dans ~/.zshrc
wt shell-init fish > ~/.config/fish/functions/wt.fish
```

La fonction n'intercepte que `wt cd` ; toute autre commande part au binaire telle quelle.
Sans elle, `wt cd demo` affiche quand même le chemin et rappelle la ligne qui manque.

Dans `wt tui` c'est `c`, ou « shell dans le worktree » dans le menu d'actions : une
fenêtre de terminal s'ouvre sur le worktree et la liste reste où elle est (voir plus bas
quand la machine n'a aucun émulateur pour en ouvrir une).

Les deux acceptent **un fragment de slug** — `wt cd auth` trouve `fix-auth` — et les deux
posent la question quand ce qui a été tapé en laisse plusieurs, ou quand rien n'a été
tapé :

```
$ wt cd fix
  1) fix-auth             wt/fix-auth
  2) hotfix               wt/hotfix
quel worktree ? [1-2, ENTRÉE pour 1]
```

wt ne devine jamais entre deux candidats : un mauvais répertoire se découvre trois
commandes plus tard. Sans terminal pour poser la question — un script, un pipe — la
commande échoue en les nommant.

Un shell ouvert par `wt shell` (ou par `c`) hérite des variables du worktree, les mêmes
que les hooks : `$WT_SLUG`, `$WT_PATH`, `$WT_PORT_VITE`… Lequel c'est vient de
`WT_TERMINAL`, puis `[editor] terminal`, puis `$SHELL`. `wt cd`, qui est ton propre shell,
n'exporte rien.

Depuis `wt tui`, `c` ouvre ce shell dans une **fenêtre de terminal à lui** : la liste
reste où elle est, et la session lui survit. L'émulateur vient de `WT_TERMINAL_WINDOW`,
puis `[editor] terminal_window`, puis de celui qui est installé — Windows Terminal sous
WSL, sinon ghostty, WezTerm, kitty, Alacritty, foot, GNOME Terminal, Konsole,
xfce4-terminal, xterm, et Terminal.app sur macOS. Une machine qui n'en a aucun — un TTY
nu, une session ssh — garde l'ancien comportement : le shell prend le terminal où tourne
l'interface, et `exit` y ramène. `WT_TERMINAL_WINDOW=""` demande ce comportement
explicitement. `wt shell` en ligne de commande reste toujours dans le terminal courant :
il n'a nulle part où revenir.

Pour une commande lancée assez souvent pour mériter un nom, une tâche vaut mieux que les
deux — un `[tasks.claude]` de trois lignes avec `interactive = true`, puis
`wt run claude demo`.

### Complétion

`wt cd <TAB>` propose les worktrees du projet, `wt run <TAB>` les tâches du `wt.toml`, et
`wt new demo <TAB>` les branches du dépôt — avec la branche, la description de la tâche ou
le sujet du commit à côté, là où le shell les affiche.

Le script ne transporte pas la liste : il interroge le binaire à chaque TAB, seule façon
d'être juste après un `wt new`. Les deux installations se valent :

```bash
wt completions zsh > ~/.zfunc/_wt         # un fichier, comme avant
echo 'source <(COMPLETE=bash wt)' >> ~/.bashrc   # ou au démarrage du shell
```

La seconde est régénérée à chaque ouverture de shell : elle ne peut pas être en retard
d'une version sur le binaire. La première est ce qu'installe un gestionnaire de paquets —
c'est exactement ce que fait le paquet Nix, où le script et le binaire sortent du même
build et ne peuvent pas se désynchroniser.

Hors d'un projet — ou si le `wt.toml` est cassé — la complétion ne propose rien plutôt
qu'une erreur : un TAB n'est pas l'endroit où apprendre que quelque chose ne va pas.

### Recherche dans les sélecteurs

Tout sélecteur — dans le mode Skim par défaut comme dans `wt tui` —
branches, tâches, éditeurs, adresses, et questions du `wt.toml` —
**se filtre à la frappe**, à la manière de `fzf` : tape `acme` et les trois cents
tenants deviennent trois. Les lettres n'ont pas à se suivre (`fab` trouve
`feature/acme-billing`), la casse est ignorée, et **l'espace affine** au lieu de
chercher : `acme prod` ne garde que ce qui contient les deux. La recherche porte sur les
deux colonnes affichées, libellé et détail.

Les résultats sont classés par pertinence — un mot entier avant des lettres éparpillées,
un début de mot avant un milieu de mot — et les caractères trouvés sont surlignés. Le
compteur en bas à droite indique ce qui reste.

Comme la frappe alimente la recherche, la navigation se fait aux flèches ou avec
`^N`/`^P` (`^J`/`^K`), `TAB` coche dans un choix multiple et `^U` efface la recherche.
Dans le mode Skim, `ÉCHAP` annule. Dans `wt tui`, il efface d'abord la recherche, puis
ferme le sélecteur ; sur une recherche vide, `⌫` revient d'un pas quand le sélecteur
suit une autre question.

### Sortie des actions dans `wt tui`

Créations, démarrages, arrêts, suppressions et tâches s'exécutent **sans quitter
l'interface** : leur sortie (stdout et stderr) défile au fil de l'eau dans un panneau,
avec `↑↓` / `PgUp` / `PgDn` pour remonter, et `ENTRÉE` pour fermer une fois l'action
terminée. La liste et l'aperçu se rafraîchissent tout seuls.

Les couleurs des commandes sont **interprétées**, pas affichées telles quelles : un hook
qui écrit `\033[36m…\033[0m`, ou un `docker`/`cargo` qui colore sa sortie, s'affiche
comme dans un terminal (palettes 256 et RGB comprises). Les barres de progression qui se
réécrivent avec `\r` n'affichent que leur dernier état.

Une tâche qui a besoin du terminal — un shell, un `logs -f`, un watcher plein écran — se
déclare `interactive = true` : l'interface s'efface le temps de son exécution, puis
reprend la main. C'est aussi le cas de l'éditeur.

### Enchaînements proposés par `wt tui`

- **après une création**, si le projet a des `[hooks] up`, le panneau demande « démarrer
  les services maintenant ? » — `o` enchaîne (questions du `wt.toml` comprises), toute
  autre touche ferme ;
- **après l'ouverture d'un éditeur graphique**, l'interface propose un terminal à la
  racine du worktree — ce qu'une fenêtre d'IDE ne donne pas. Il s'ouvre exactement comme
  avec `c`. Le shell est `WT_TERMINAL`, sinon `[editor] terminal` du `wt.toml`, sinon
  `$SHELL`. (Un éditeur qui vit dans le terminal, comme `nvim`, remplace le process :
  rien ne peut être enchaîné derrière, la question n'est donc pas posée.)

## Le fichier `wt.toml`

Toutes les sections sont facultatives. Celle-ci montre tout à la fois, à titre de
référence — un projet sans serveur laisse simplement `[ports]`, `[status]` et `[open]`
de côté.

```toml
root   = "{{main}}/../{{repo}}-wt"   # défaut
branch = "wt/{{slug}}"               # défaut

[vars]
host = "{{slug}}.wt.localhost"       # les vars peuvent se référencer entre elles

dirs = ["storage/framework/views"]   # créés après le checkout

[[copy]]
from = "node_modules"                # to = from par défaut
mode = "hardlink"                    # hardlink | copy | symlink

[ports.vite]
base = 5200                          # premier port testé
allocate = "up"                      # "up" (défaut) | "new"

[hooks]
post_new = ["npm install"]
up       = ["docker compose -p {{repo}}-{{slug}} up -d"]
down     = ["docker compose -p {{repo}}-{{slug}} down"]
pre_rm   = ["docker compose -p {{repo}}-{{slug}} down -v"]
post_rm  = []

[status]
up = "docker compose -p {{repo}}-{{slug}} ps -q app | grep -q ."   # code 0 = démarré

[status.info]                        # lignes en plus dans l'aperçu
taille = "du -sh . | cut -f1"

[open]
url = "http://{{host}}"              # adresse principale
label = "application"                # son libellé dans le sélecteur
source = "./scripts/urls.sh"         # adresses supplémentaires : url<TAB>libellé

[editor]
command = "phpstorm"                 # WT_IDE de l'environnement reste prioritaire
terminal = "zsh"                     # shell proposé après l'ouverture de l'éditeur
terminal_window = "kitty"            # émulateur qui l'ouvre dans une fenêtre à lui

[lsp.php]
command = "phpantom_lsp"             # un serveur de langage pour le code du projet
extensions = ["php", "blade.php"]    # les fichiers qu'il sert

[tasks.shell]
description = "shell dans le conteneur"
interactive = true
run = "docker compose -p {{repo}}-{{slug}} exec app bash"
```

### Serveurs de langage (`[lsp]`)

Un projet peut nommer les serveurs de langage que son code réclame. `wt` n'en fait
rien : il n'en lance ni n'en surveille aucun, et la ligne de commande n'en a pas
l'usage. S'ils se déclarent ici, c'est que c'est le fichier où un projet dit déjà ce
qu'il lui faut — une interface graphique qui embarque la bibliothèque les lit à côté des
tâches et des ports, et aucun projet n'a un second fichier à apprendre.

```toml
[lsp.php]
command = "phpantom_lsp"             # templaté : {{main}}/vendor/bin/… marche
args = []
extensions = ["php", "blade.php"]    # sans le point
env = { PHPANTOM_LOG = "info" }
language = "php"                     # le languageId LSP ; par défaut, la clé de la table
```

Les réglages propres au serveur n'ont rien à faire ici : un serveur de langage a presque
toujours son fichier de configuration à lui, dans le projet, qu'il lit et surveille
lui-même.

À quelle entrée appartient un fichier donné regarde l'interface et non nous :
`page.blade.php` en désigne deux à la fois, et la règle qui les départage — la plus
longue extension l'emporte — appartient là où les fichiers s'ouvrent.

### Exemples

| Fichier | Profil |
|---|---|
| `examples/rust-cli.toml` | binaire/bibliothèque — **aucun service, aucun port** |
| `examples/python-cli.toml` | script ou traitement de données — venv par worktree, données en symlink |
| `examples/node-vite.toml` | serveur de dev par worktree |
| `examples/laravel-sail.toml` | multi-tenant, Caddy partagé, bases isolées |

### Variables

| Variable | Valeur |
|---|---|
| `{{slug}}` `{{branch}}` `{{path}}` | le worktree |
| `{{main}}` `{{root}}` `{{repo}}` `{{project}}` | le projet |
| `{{port.<nom>}}` | port alloué pour `[ports.<nom>]` (si le projet en déclare) |
| `{{opt.<nom>}}` | option passée en `--set <nom>=<valeur>` |
| `{{args}}` | arguments de `wt run` (tâches uniquement) |

Les mêmes valeurs sont exportées à l'environnement des hooks : `WT_SLUG`, `WT_PATH`,
`WT_PORT_VITE`, `WT_OPT_TENANTS`… Pratique dès qu'un hook dépasse une ligne.

Une clé inconnue est laissée telle quelle (`{{port.web}}` visible dans la commande qui
échoue vaut mieux qu'un argument silencieusement disparu), et `awk '{print $1}'` traverse
le moteur sans dommage.

### Options `--set`

`wt up demo --set tenants=acme,globex --set services=queue,reverb` rend
`{{opt.tenants}}` et `{{opt.services}}` disponibles dans les hooks. Elles sont
**mémorisées** : un `wt up demo` ultérieur reconduit le même démarrage, et un nouveau
`--set` ne remplace que les clés citées.

### Questions posées avant l'action (`[[prompt]]`)

Un projet peut déclarer les questions que l'interface doit poser avant `new` ou `up`.
Les réponses deviennent des options — exactement comme un `--set`, mémorisation comprise.

```toml
[[prompt]]
name = "db"                  # → {{opt.db}} et $WT_OPT_DB
ask = "new"                  # "up" (défaut) | "new" | "both"
question = "bases de données"
type = "choice"              # "choice" | "multi" | "confirm" | "text"
default = "shared"
options = [
    { value = "shared",   label = "partagées", detail = "aucune migration possible" },
    { value = "isolated", label = "isolées",   detail = "obligatoire si la branche migre" },
]

[[prompt]]
name = "tenants"
type = "multi"                            # TAB coche, ENTRÉE valide
separator = ","                           # jointure des valeurs cochées (défaut)
when = "test \"$WT_OPT_DB\" = isolated"   # posée seulement si la commande renvoie 0
source = "mon-script --liste"             # une ligne par choix : valeur<TAB>libellé<TAB>détail
```

- **`source`** rend la liste dynamique : le binaire ne sait pas ce qu'est un tenant, une
  base ou un device — il exécute la commande du projet et affiche ce qu'elle énumère.
- **`when`** voit les réponses déjà données (`$WT_OPT_*`) et la phase en cours
  (`$WT_PHASE` = `new` ou `up`), ce qui permet d'enchaîner les questions.
- Une option **déjà connue** n'est pas redemandée — c'est ce qui fait qu'un `wt up`
  répété reconduit le même montage sans rien demander. `always = true` force la question.
- `default` présélectionne (ou pré-coche en `multi`) : le cas courant se valide d'un
  ENTRÉE.
- `ÉCHAP` pendant une question **annule l'action entière** : la lancer avec des réponses
  à moitié collectées serait pire que ne rien faire.
- Une `source` qui ne renvoie rien n'immobilise pas l'interface : la question est ignorée
  et l'action continue.

En ligne de commande, rien n'est demandé — `wt new demo --set db=isolated` reste
entièrement scriptable.

### Plusieurs adresses (`[open] source`)

Un worktree n'a pas toujours une seule adresse : une application multi-tenant en a une
par tenant monté, un projet à plusieurs services une par service. `source` est une
commande shell qui les énumère, une ligne par lien :

```
http://acme.demo.wt.localhost	tenant acme
http://globex.demo.wt.localhost	tenant globex
```

Elle voit les options du worktree (`$WT_OPT_TENANTS`…), ce qui lui permet de ne proposer
que ce qui est réellement monté. Elle n'est lancée qu'au moment d'ouvrir — jamais pour
afficher la liste des worktrees — car elle peut interroger une base ou un conteneur.

```bash
wt open demo                 # la première adresse (l'application)
wt open demo globex          # celle dont le libellé ou l'URL contient « globex »
wt open demo --list          # les afficher toutes
```

Dans le mode Skim, l'action « ouvrir dans le navigateur » ouvre directement s'il n'y a
qu'une adresse et propose un sélecteur dès qu'il y en a plusieurs. Dans `wt tui`, son
raccourci est `o`.

## Ce que `wt` garantit

- **Rien n'est imposé.** Ports, état, URL, hooks : tout est facultatif, et l'interface se
  réduit à ce que le projet déclare. Un `wt.toml` vide est viable.
- **Ports stables** — pour les projets qui en demandent. Alloués une seule fois,
  conservés à l'arrêt, jamais réutilisés par un autre worktree du projet : signets et
  configs d'IDE ne bougent pas.
- **L'état vit hors du checkout**, dans `<root>/.wt/<slug>.toml` : rien à ajouter au
  `.gitignore` du projet, et l'état survit à un changement de branche.
- **La branche survit à `wt rm`.** Retirer un worktree ne doit jamais faire disparaître
  du travail non fusionné ; `wt` rappelle la commande `git branch -d` à exécuter
  soi-même.
- **Un shell POSIX** (`sh`) exécute les hooks, quel que soit le shell de l'utilisateur
  (surchargeable par `WT_SHELL`).

## Variables d'environnement

| Variable | Effet |
|---|---|
| `WT_LANG` | langue de l'interface (`en`, `fr`) ; sinon `LC_ALL`/`LC_MESSAGES`/`LANG`, puis l'anglais |
| `WT_CONFIG` | chemin d'un `wt.toml` alternatif |
| `WT_IDE` | éditeur prioritaire (commande ou chemin absolu, `.exe` Windows compris) |
| `WT_TERMINAL` | shell ouvert dans le worktree (défaut `$SHELL`) |
| `WT_SHELL` | shell des hooks (défaut `sh`) |

## Projets voisins

`wt.exe` est Windows Terminal, et crates.io héberge `wt-core` et `wt-cli`, deux
gestionnaires de worktrees sans rapport. Ce projet n'est pas publié sur crates.io.

## Licence

MIT.
