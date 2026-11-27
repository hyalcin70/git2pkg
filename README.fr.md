<p align="center"><img src="assets/app_icon.png" alt="git2pkg" width="128"></p>

[English](README.md) | [Deutsch](README.de.md) | [Türkçe](README.tr.md) | [Español](README.es.md) | [Français](README.fr.md)

# git2pkg

[![CI](https://github.com/hyalcin70/git2pkg/actions/workflows/ci.yml/badge.svg)](https://github.com/hyalcin70/git2pkg/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/hyalcin70/git2pkg?include_prereleases&label=release)](https://github.com/hyalcin70/git2pkg/releases)

Une petite interface graphique Linux qui compile **des projets natifs Linux
pris en charge** depuis des dépôts Git **dans un conteneur jetable
(Podman/Docker)** et dépose le résultat dans un dossier de sortie visible
(`~/Downloads/git2pkg` par défaut). L'hôte n'a besoin ni de chaîne de
compilation ni de bibliothèques de compilation.

`git2pkg` fonctionne à 100 % en local, de manière déterministe et basée sur des
règles. **Il ne contient aucune IA, aucun LLM et aucun appel à une API externe.**
Les décisions de compilation proviennent de la détection de fichiers, d'expressions
régulières appliquées à la sortie du compilateur et du gestionnaire de paquets du
conteneur.

> [!WARNING]
> **Statut alpha :** `git2pkg` est en cours de développement actif. Des changements incompatibles et des fonctionnalités incomplètes sont à prévoir. Testez dans des conteneurs jetables, conservez des sauvegardes et signalez les échecs avec le journal complet.

## Périmètre pris en charge

`git2pkg` est conçu pour les projets natifs Linux de bureau et en ligne de
commande. Ce n'est **pas un constructeur universel** : les systèmes de
compilation, formats cibles et distributions hôtes pris en charge sont listés
ci-dessous. Ce qui sort de cette matrice peut fonctionner, mais ne fait pas
partie du périmètre de support testé.

| Domaine | Pris en charge |
|---|---|
| Hôte | Linux x86_64 avec Podman ou Docker |
| Cibles | Arch/CachyOS `.pkg.tar.zst`, Debian/Ubuntu/Mint `.deb`, Fedora/RHEL et openSUSE `.rpm`, binaire autonome |
| Systèmes de compilation | CMake, Meson, Autotools, Cargo, Go, Python, Make, qmake |
| Recherche des dépendances | Paquets de la distribution dans le conteneur cible, vérifiés après installation |
| Non pris en charge | Chaînes d'outils embarquées/microcontrôleurs, applications mobiles, applications web, plugins d'IDE, modules du noyau, compilations pour une autre architecture, SDK propriétaires non installables dans le conteneur, conformité garantie aux politiques d'empaquetage de la distribution |

> **ⓘ Information**
>
> **« La disponibilité du code source ne suffit pas à garantir la prise en charge de Linux. »**
>
> Mais : git2pkg prend en charge les projets qui ciblent officiellement Linux.
> Ce qui compte est la déclaration de support du projet lui-même, et non la
> disponibilité de son code source.
>
> <details>
> <summary>Détail : Que contient la matrice de prise en charge ?</summary>
>
> - ✓ **Projets officiellement ciblés Linux** (pris en charge par git2pkg).
> - ✗ **Les projets exclusivement Windows/macOS** et **les ports uniquement
>   communautaires** sont hors de la matrice de prise en charge.
>
> </details>

`git2pkg` ne remplace ni l'empaquetage de la distribution, ni AUR, ni Flatpak, ni
un audit de sécurité. Il compile ce qu'il peut compiler de manière fiable,
signale clairement le reste, et le paquet produit doit être testé sur la
distribution cible.

## Pourquoi je l'ai créé

J'adore découvrir de nouveaux outils open source sur GitHub, GitLab et ailleurs,
mais je ne voulais pas encombrer mon système principal avec des dizaines de
bibliothèques de compilation et d'en-têtes de développement, ni dépendre
aveuglément de dépôts tiers comme l'AUR. C'est pourquoi j'ai créé `git2pkg` :
la compilation s'exécute dans un conteneur temporaire jetable, le système hôte
reste entièrement propre, et l'on obtient au final un paquet natif
(`.pkg.tar.zst`, `.deb`, `.rpm`) ou un binaire unique — utilisable via une
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
  `meson.build`, Autotools (`configure.ac`), qmake (`.pro`), `go.mod`,
  `pyproject.toml`, `Makefile`).
- Les scripts de compilation du dépôt sont choisis selon la distribution cible ;
  ceux des autres distributions ou systèmes d'exploitation sont ignorés.
- Journal de compilation en direct, défilable, avec bouton de copie.
- Une nouvelle tentative avec une autre stratégie réutilise l'espace de travail
  existant (pas de second clone) et enregistre automatiquement le journal
  d'échec précédent.
- Résolution des dépendances manquantes : la sortie du compilateur est analysée
  pour les en-têtes, paquets pkg-config, modules CMake, outils et bibliothèques
  de liaison absents ; le paquet
  correspondant est installé **dans le conteneur jetable** et vérifié (l'en-tête,
  le module ou la bibliothèque doit réellement exister), puis la compilation est
  relancée (5 tentatives maximum). Seules les correspondances vérifiées sont
  mémorisées.
- Diagnostic des échecs : les compilations sont classées (dépendance manquante,
  incompatibilité de version/ABI, code embarqué, erreur de compilation, erreur
  d'empaquetage). Les conflits fatals d'ABI ou de finders intégrés arrêtent
  immédiatement la compilation avec un diagnostic clair de version/ABI au lieu
  d'essayer des paquets au hasard.
- Paquets natifs : `.pkg.tar.zst` (Arch ; généré directement, `PKGBUILD` du
  dépôt via `makepkg` s'il existe), `.deb` (Debian/Ubuntu), `.rpm`
  (Fedora/openSUSE). Les projets qui fournissent leur propre `debian/` ou
  `*.spec` sont empaquetés avec `dpkg-buildpackage`/`rpmbuild` selon cette
  recette.
- Images de compilation facultatives (`packaging/builders/build.sh`) : elles
  fournissent la chaîne d'outils complète par distribution et accélèrent
  nettement les compilations répétées ; sans elles, tout fonctionne comme
  avant.
- **Dossier de sortie** : les artefacts sont écrits dans `~/Downloads/git2pkg`
  par défaut. Modifiable dans l'interface avec le sélecteur natif (`rfd`) ou
  en saisissant un chemin.
- Actions de résultat : ouvrir le dossier de sortie, enregistrer la recette de
  compilation et voir la commande d'installation exacte avant toute exécution.
- Mode headless optionnel : `git2pkg --cli <url> [--format=package|binary]`.
- Réinitialisez les correspondances apprises avec `git2pkg --reset-learning` (le cache du dépôt Git est conservé).
- Affichez les correspondances de dépendances apprises avec `git2pkg --explain-learning`.

## Prérequis

- Rust 1.89+ (pour compiler depuis les sources)
- Podman (préféré) ou Docker
- Une session graphique (Wayland ou X11) pour l'interface

## Compilation depuis les sources

```sh
# une fois : chaîne Rust à jour (celle de la distribution est souvent ancienne)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

git clone https://github.com/hyalcin70/git2pkg.git
cd git2pkg
rustup default stable
cargo build --release
```

`git2pkg` nécessite Rust 1.89 ou plus récent. Linux Mint 22.3 (Ubuntu 24.04)
fournit 1.75 et Debian 13 fournit 1.85 ; `rustup default stable` apporte une
chaîne d'outils assez récente.

Lancez l'interface avec `cargo run --release`, ou la CLI avec
`cargo run --release -- --cli https://github.com/user/repo`.

## Installation sur le bureau

À la racine du dépôt, lancez `./install.sh`. Le script compile le binaire
release puis demande **où** le placer : le dossier du projet (par défaut),
`~/Applications`, le bureau ou un chemin personnalisé. Une entrée dans le menu
d'applications est ensuite proposée. `./uninstall.sh` retire le binaire et
l'entrée en lisant le chemin dans le fichier `.desktop`.

## Paquets natifs

Compilez `git2pkg` comme paquet système pour votre distribution. Toutes les
recettes compilent le checkout courant et nécessitent Rust 1.89 ou plus récent.

### Arch / CachyOS

```sh
cd packaging
makepkg -sc      # compile le paquet et supprime src/ et pkg/
# makepkg -sic   # idem, puis installation
```

### Debian / Ubuntu

```sh
sudo apt install debhelper dpkg-dev build-essential cargo rustc pkg-config \
  libxkbcommon-dev libwayland-dev libxcb1-dev libxrandr-dev libxi-dev \
  libxcursor-dev libx11-dev libgl1-mesa-dev libatk1.0-dev libgtk-3-dev \
  libasound2-dev
dpkg-buildpackage -us -uc -b
sudo apt install ../git2pkg_*.deb
```

Si le Rust de la distribution est antérieur à 1.89, installez d'abord la
chaîne stable actuelle avec `rustup`.

### Fedora / RHEL

```sh
sudo dnf install rpm-build rpmdevtools cargo rust pkgconf-pkg-config dnf-plugins-core
sudo dnf builddep packaging/git2pkg.spec
./packaging/build-rpm.sh
sudo dnf install ~/rpmbuild/RPMS/*/git2pkg-*.rpm
```

### openSUSE

```sh
sudo zypper install rpm-build cargo rust pkgconf-pkg-config \
  libxkbcommon-devel wayland-devel libxcb-devel libXrandr-devel \
  libXi-devel libXcursor-devel Mesa-libGL-devel atk-devel gtk3-devel alsa-devel
./packaging/build-rpm.sh
sudo zypper install ~/rpmbuild/RPMS/*/git2pkg-*.rpm
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
    packaging/       backends d'empaquetage Arch/Debian/RPM
  gui/
    app.rs           application eframe/egui
    fonts.rs         polices Noto intégrées
    i18n.rs          textes EN/DE/TR/ES/FR, détection langue et thème
    settings.rs      réglages de l'interface (dossier de sortie, thème)
assets/              polices, icône, base de connaissances des dépendances
scripts/             corpus de tests manuel (URLs en arguments)
packaging/           PKGBUILD Arch, spec/script RPM et fichier .desktop
debian/              empaquetage Debian/Ubuntu (dpkg-buildpackage)
.github/workflows/   workflows CI et release déclenchés par les tags
.gitlab-ci.yml       compilation et paquetage GitLab CI
```

## CI / Releases

Un tag `v*` déclenche `.github/workflows/release.yml` : le binaire Linux
`x86_64` est compilé sur `ubuntu-latest`, `git2pkg-linux-x86_64.tar.gz` et son
`.sha256` sont générés, puis les deux sont joints à la release GitHub.
`.gitlab-ci.yml` construit la même archive sur GitLab et l'expose comme artefact
de tâche. Les étiquettes alpha, bêta et release candidate sont publiées comme
pré-releases GitHub.

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

> [!IMPORTANT]
> **Périmètre du projet et développement en solo :** `git2pkg` est un projet personnel indépendant développé sur mon temps libre. À partir de la phase Beta, le support (selon disponibilité) se concentrera strictement sur les grandes familles de distributions prises en charge (**Arch / CachyOS**, **Debian / Ubuntu / Linux Mint**, **Fedora / Red Hat** et **openSUSE**). D'autres distributions peuvent fonctionner, mais ne font pas partie de la matrice de support testée.

## Remerciements et transparence

J’ai eu l’idée de ce projet, j’en ai orienté le développement, effectué les tests et analysé les bugs — tandis que la quasi-totalité du code source a été générée à l’aide d’outils d’IA. Je tiens à le dire ouvertement et honnêtement. Le programme `git2pkg` final fonctionne toutefois de manière 100 % locale et déterministe, sans aucune IA ni API externe.

## Licence

GPL-3.0-or-later — voir [LICENSE](LICENSE). Les fichiers de polices Noto Sans
et Noto Sans Mono dans `assets/` sont sous SIL Open Font License 1.1.

## Remerciements

<a href="https://github.com/NousResearch/hermes-agent"><img src="assets/badges/hermes-agent.svg" alt="Hermes Agent" height="28"></a>

Un grand merci à [Hermes Agent](https://github.com/NousResearch/hermes-agent) de Nous Research, qui a servi d'outil de développement local lors de la création de `git2pkg` (tandis que `git2pkg` lui-même fonctionne de manière 100 % déterministe et sans aucune dépendance à l'IA).
