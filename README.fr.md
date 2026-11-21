<p align="center"><img src="assets/app_icon.png" alt="git2pkg" width="128"></p>

[English](README.md) | [Deutsch](README.de.md) | [Türkçe](README.tr.md) | [Español](README.es.md) | [Français](README.fr.md)

# git2pkg

Une petite interface graphique Linux qui compile un dépôt Git **dans un
conteneur jetable (Podman/Docker)** et dépose le résultat dans un dossier de
sortie visible (`~/Downloads/git2pkg` par défaut). L'hôte n'a besoin ni de
chaîne de compilation ni de bibliothèques de build.

`git2pkg` fonctionne à 100 % en local, de manière déterministe et basée sur des
règles. **Il ne contient aucune IA, aucun LLM et aucun appel à une API externe.**
Les décisions de compilation proviennent de la détection de fichiers, d'expressions
régulières appliquées à la sortie du compilateur et du gestionnaire de paquets du
conteneur.

> [!WARNING]
> **Statut Alpha :** Ce projet est actuellement en phase de test actif (Alpha). Le clonage et les tests s'effectuent à vos propres risques. Aucun support, assistance ou garantie d'aucune sorte n'est fourni pendant la phase Alpha — le support sera disponible à partir du statut Beta.

## Pourquoi je l'ai créé

J'adore découvrir de nouveaux outils open source sur GitHub, GitLab et ailleurs,
mais je ne voulais pas encombrer mon système principal avec des dizaines de
bibliothèques de compilation et d'en-têtes de développement, ni dépendre
aveuglément de dépôts tiers comme l'AUR. C'est pourquoi j'ai créé `git2pkg` :
tout est compilé dans un conteneur temporaire jetable, le système hôte reste
entièrement propre, et l'on obtient au final un paquet natif (`.pkg.tar.zst`,
`.deb`, `.rpm`) ou un binaire unique — utilisable par tout le monde via une
simple interface graphique.

## Fonctionnalités

- Collez l'URL d'un dépôt Git, choisissez le format cible, cliquez sur
  **Lancer la compilation**.
- **Thème automatique** : reprend au démarrage le réglage clair/sombre du
  bureau (via `dark-light`) ; en cas d'échec de détection, le thème clair est
  utilisé. Un bouton **Auto / Clair / Sombre** en haut à droite permet de
  choisir manuellement.
- **Langue automatique** : allemand (`de*`), turc (`tr*`), espagnol (`es*`) ou
  français (`fr*`) selon `LC_ALL`, `LC_MESSAGES` ou `LANG` ; anglais sinon. Une
  liste **EN / DE / TR / ES / FR** permet de changer à tout moment.
- **Polices Noto Sans / Noto Sans Mono intégrées** : affichage correct des
  caractères turcs (İ, ı, ğ, Ğ, ş, Ş) et de tous les accents espagnols et
  français.
- Détection automatique de la distribution via `/etc/os-release` (Arch/CachyOS,
  Ubuntu/Debian, Fedora, openSUSE) avec les commandes d'installation adaptées
  (`pacman -U`, `apt install`, `dnf install`, `zypper install`).
- Détection automatique du système de compilation (`Cargo.toml`, `CMakeLists.txt`,
  `meson.build`, `go.mod`, `pyproject.toml`, `Makefile`).
- Journal de compilation en direct, défilable, avec bouton de copie.
- Résolution des dépendances manquantes : la sortie du compilateur est analysée
  pour les en-têtes, paquets pkg-config et modules CMake absents ; le paquet
  correspondant est installé **dans le conteneur jetable**, puis la compilation
  est relancée (5 tentatives maximum).
- Paquets natifs : `.pkg.tar.zst` (Arch), `.deb` (Debian/Ubuntu), `.rpm`
  (Fedora/openSUSE).
- **Dossier de sortie** : les artefacts sont écrits dans `~/Downloads/git2pkg`
  par défaut. Modifiable dans l'interface avec le sélecteur natif (`rfd`) ou
  en saisissant un chemin.
- Actions de résultat : ouvrir le dossier de sortie, enregistrer la recette de
  compilation et voir la commande d'installation exacte avant toute exécution.
- Mode headless optionnel : `git2pkg --cli <url> [--format=package|binary]`.

## Prérequis

- Rust 1.80+ (pour compiler depuis les sources)
- Podman (préféré) ou Docker
- Une session graphique (Wayland ou X11) pour l'interface

## Compilation depuis les sources

```sh
git clone https://github.com/hyalcin70/git2pkg.git
cd git2pkg
cargo build --release
```

Lancez l'interface avec `cargo run --release`, ou la CLI avec
`cargo run --release -- --cli https://github.com/user/repo`.

## Installation sur le bureau

À la racine du dépôt, lancez `./install.sh`. Le script compile le binaire
release puis demande **où** le placer : le dossier du projet (par défaut),
`~/Applications`, le bureau ou un chemin personnalisé. Une entrée dans le menu
d'applications est ensuite proposée. `./uninstall.sh` retire le binaire et
l'entrée en lisant le chemin dans le fichier `.desktop`.

## Paquet Arch / CachyOS

`packaging/PKGBUILD` compile `git2pkg` comme un paquet système classique :

```sh
cd packaging
makepkg -si
```

## Structure du projet

```
src/
  main.rs            point d'entrée, GUI ou CLI
  cli.rs             mode headless
  core/              logique pure, aucun code d'interface
    distro.rs        analyse de /etc/os-release
    detector.rs      détection du système de compilation
    resolver.rs      analyse regex des erreurs + commandes de recherche
    container.rs     orchestration des conteneurs et empaquetage
  gui/
    app.rs           application eframe/egui
    fonts.rs         polices Noto intégrées
    i18n.rs          textes EN/DE/TR/ES/FR, détection langue et thème
assets/              Noto Sans + Noto Sans Mono intégrées (OFL 1.1)
.github/workflows/   workflow de release GitHub déclenché par les tags
.gitlab-ci.yml       compilation et paquetage GitLab CI
packaging/PKGBUILD   recette Arch pour git2pkg
```

## CI / Releases

Un tag `v*` déclenche `.github/workflows/release.yml` : le binaire Linux
`x86_64` est compilé sur `ubuntu-latest`, `git2pkg-linux-x86_64.tar.gz` et son
`.sha256` sont générés, puis les deux sont joints à la release GitHub.
`.gitlab-ci.yml` construit la même archive sur GitLab et l'expose comme artefact
de tâche.

## Contribuer et traductions

Les contributions sont bienvenues. Consultez [CONTRIBUTING.md](CONTRIBUTING.md)
pour le déroulement des pull requests, `cargo fmt` / `cargo clippy` et l'ajout
de nouveaux systèmes de compilation.

### Ajouter une langue

Toutes les chaînes visibles sont dans `src/gui/i18n.rs` :

1. Ajoutez une variante `Language` et incluez-la dans `Language::ALL` — ce
   tableau pilote la liste déroulante de la barre d'outils.
2. Étendez `Language::detect()` avec le préfixe de locale (`de`, `tr`, `es`,
   `fr`, ...) ainsi que le code court et le libellé.
3. Ajoutez la traduction à **chaque** appel `Texts::get(...)`. L'ordre est
   toujours `EN, DE, TR, ES, FR, <nouvelle langue>`.
4. Vérifiez que les polices de `assets/` contiennent bien les caractères de la
   nouvelle langue.
5. Lancez `cargo fmt`, `cargo clippy` et `cargo test` avant le PR.

### Signaler un échec de compilation

Cliquez sur **Copier le journal** dans l'interface et ouvrez un issue avec le
modèle **Build failure**. Collez le journal **complet**, l'URL du dépôt, votre
distribution et le moteur utilisé (Podman ou Docker). Une capture de la seule
dernière ligne ne suffit généralement pas.

## Soutien

Le soutien est entièrement volontaire. Si `git2pkg` vous fait gagner du temps,
mettez une étoile au dépôt, partagez-le, signalez un bug avec le journal copié
ou contribuez à une traduction. Aucune télémétrie, aucune obligation : l'outil
fonctionne exactement de la même manière avec ou sans tout cela.

## Remerciements et transparence

J'ai conçu et dirigé ce projet, et je l'ai développé avec l'aide d'outils
d'IA. Je préfère le dire ouvertement. Le programme `git2pkg` terminé fonctionne
toutefois à 100 % en local et de manière déterministe, sans aucune IA ni API
externe.

## Licence

GPL-3.0-or-later — voir [LICENSE](LICENSE). Les fichiers de polices Noto Sans
et Noto Sans Mono dans `assets/` sont sous SIL Open Font License 1.1.
