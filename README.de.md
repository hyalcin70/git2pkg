<p align="center"><img src="assets/app_icon.png" alt="git2pkg" width="128"></p>

[English](README.md) | [Deutsch](README.de.md) | [Türkçe](README.tr.md) | [Español](README.es.md) | [Français](README.fr.md)

# git2pkg

[![CI](https://github.com/hyalcin70/git2pkg/actions/workflows/ci.yml/badge.svg)](https://github.com/hyalcin70/git2pkg/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/hyalcin70/git2pkg?include_prereleases&label=release)](https://github.com/hyalcin70/git2pkg/releases)

Eine kleine Linux-Desktop-GUI, die **unterstützte native Linux-Projekte** aus
Git-Repositories **in einem temporären Wegwerf-Container (Podman/Docker)** baut
und das Ergebnis in einen sichtbaren Ausgabeordner legt (standardmäßig
`~/Downloads/git2pkg`). Das Host-System braucht weder Compiler-Toolchain noch
Build-Bibliotheken.

`git2pkg` arbeitet zu 100 % lokal, deterministisch und regelbasiert. **Es enthält
keine KI, kein LLM und keine externen API-Aufrufe.** Build-Entscheidungen
erfolgen durch Dateierkennung, reguläre Ausdrücke über Compiler-Ausgaben und
den Paketmanager im Container.

> [!WARNING]
> **Alpha-Status:** `git2pkg` befindet sich in aktiver Entwicklung. Mit Änderungen an Schnittstellen und unfertigen Funktionen ist zu rechnen. Bitte in Wegwerf-Containern testen, Backups behalten und Fehler mit dem vollständigen Log melden.

## Unterstützter Anwendungsbereich

`git2pkg` ist für native Linux-Desktop- und CLI-Projekte gebaut. Es ist **kein
universeller Builder**: Die unterstützten Build-Systeme, Zielformate und
Host-Distributionen stehen unten. Was außerhalb dieser Matrix liegt, kann
funktionieren, gehört aber nicht zum getesteten Supportumfang.

| Bereich | Unterstützt |
|---|---|
| Host | x86_64-Linux mit Podman oder Docker |
| Ziele | Arch/CachyOS `.pkg.tar.zst`, Debian/Ubuntu/Mint `.deb`, Fedora/RHEL und openSUSE `.rpm`, einzelne Binary |
| Build-Systeme | CMake, Meson, Autotools, Cargo, Go, Python, Make, qmake |
| Abhängigkeitssuche | Distributionspakete im Ziel-Container, nach der Installation verifiziert |
| Nicht unterstützt | Embedded-/Mikrocontroller-Toolchains, mobile Apps, Web-Apps, IDE-Plugins, Kernelmodule, Cross-Architektur-Builds, proprietäre SDKs, die nicht im Container installierbar sind, garantierte Einhaltung von Distributions-Packaging-Richtlinien |

> **ⓘ Information**
>
> **« Öffentlicher Quellcode allein bedeutet keinen Linux-Support. »**
>
> Aber: git2pkg unterstützt Projekte, die Linux offiziell als Ziel haben.
> Maßgeblich ist die Support-Erklärung des Projekts, nicht die Verfügbarkeit
> des Quellcodes.
>
> <details>
> <summary>Detail: Was ist in der Support-Matrix?</summary>
>
> - ✓ **Offizielle Linux-Target-Projekte** (unterstützt durch git2pkg).
> - ✗ **Reine Windows-/macOS-Projekte** und **reine Community-Ports** liegen
>   außerhalb der Support-Matrix.
>
> </details>

`git2pkg` ist kein Ersatz für Distributions-Packaging, AUR, Flatpak oder ein
Sicherheitsaudit. Es baut, was es zuverlässig bauen kann, meldet den Rest klar,
und das fertige Paket sollte auf der Ziel-Distribution getestet werden.

## Warum ich das gebaut habe

Ich entdecke gerne neue Open-Source-Tools auf GitHub, GitLab und anderen
Plattformen – wollte mein Hauptsystem aber nicht mit Dutzenden Build-Bibliotheken
und Header-Dateien zumüllen und mich auch nicht blind auf Drittanbieter-Repos
wie dem AUR verlassen. Deshalb habe ich `git2pkg` gebaut: Der Build läuft in einem temporären
Wegwerf-Container, das Host-System bleibt zu 100 % sauber, und am Ende entsteht
ein natives Paket (`.pkg.tar.zst`, `.deb`, `.rpm`) oder eine einzelne Binary –
per GUI nutzbar.

## Funktionen

- Git-Repository-URL einfügen, Zielformat wählen, auf **Build starten** klicken.
- **Automatisches Theme**: übernimmt beim Start die Light/Dark-Einstellung des
  Desktops (über die Crate `dark-light`); falls nicht ermittelbar, wird hell
  verwendet. Ein Button oben rechts schaltet **Auto / Hell / Dunkel** um.
- **Automatische Sprache**: Deutsch (`de*`), Türkisch (`tr*`), Spanisch (`es*`)
  oder Französisch (`fr*`) aus `LC_ALL`, `LC_MESSAGES` oder `LANG`, sonst
  Englisch. Ein Dropdown **EN / DE / TR / ES / FR** erlaubt jederzeit den Wechsel.
- **Eingebettete Noto-Sans-/Noto-Sans-Mono-Schriften** stellen türkische
  Sonderzeichen (İ, ı, ğ, Ğ, ş, Ş) und alle spanischen/französischen Akzente
  korrekt dar.
- Automatische Host-Erkennung aus `/etc/os-release` (Arch/CachyOS,
  Ubuntu/Debian, Fedora, openSUSE) mit passenden Installationsbefehlen
  (`pacman -U`, `apt install`, `dnf install`, `zypper install`).
- Automatische Build-System-Erkennung (`Cargo.toml`, `CMakeLists.txt`,
  `meson.build`, Autotools (`configure.ac`), qmake (`.pro`), `go.mod`,
  `pyproject.toml`, `Makefile`).
- Repository-Build-Skripte werden nach Ziel-Distribution ausgewählt; Skripte
  für andere Distributionen oder Betriebssysteme werden ignoriert.
- Live-Log aus dem Container, scrollbar, mit „Log kopieren"-Button.
- Ein Retry mit einer anderen Strategie nutzt den vorhandenen Build-Workspace
  weiter (kein zweiter Clone) und speichert das vorherige fehlgeschlagene Log
  automatisch.
- Auflösung fehlender Abhängigkeiten: Die Compiler-Ausgabe wird nach fehlenden
  Headern, pkg-config-Paketen, CMake-Modulen, Werkzeugen und Linker-Bibliotheken
  durchsucht; das passende Paket wird **im Wegwerf-Container** installiert und
  anschließend geprüft (Header/Modul/Bibliothek muss wirklich vorhanden sein),
  bevor der Build erneut startet (max. 5 Versuche). Nur geprüfte Treffer werden
  dauerhaft gemerkt.
- Fehlerdiagnose: Builds werden klassifiziert (fehlende Abhängigkeit,
  Versions-/ABI-Konflikt, gebündelter Code, Compilerfehler, Paketierungsfehler).
  Fatale ABI- oder Bundled-Finder-Konflikte beenden den Build sofort mit einer
  klaren Versions-/ABI-Diagnose statt weiterer Zufallsversuche.
- Native Pakete: `.pkg.tar.zst` (Arch; direkt erzeugt, vorhandenes `PKGBUILD`
  über `makepkg`), `.deb` (Debian/Ubuntu via `dpkg-deb`), `.rpm`
  (Fedora/openSUSE via `rpmbuild`). Projekte mit eigenem `debian/` oder
  `*.spec` werden mit `dpkg-buildpackage`/`rpmbuild` nach deren Rezept
  paketiert.
- Optionale vorgebaute Builder-Images (`packaging/builders/build.sh`) bringen
  die komplette Toolchain je Distribution und beschleunigen wiederholte Builds
  um ein Mehrfaches; ohne sie funktioniert alles wie bisher.
- **Ausgabeordner**: Fertige Artefakte landen standardmäßig in
  `~/Downloads/git2pkg`. Die GUI bietet einen nativen Ordnerdialog (`rfd`)
  oder die direkte Eingabe eines Pfads.
- Ergebnis-Aktionen: Ausgabeordner öffnen, Build-Rezept speichern und den
  exakten Installationsbefehl anzeigen, bevor etwas ausgeführt wird.
- Optionaler Headless-Modus: `git2pkg --cli <url> [--format=package|binary]`.
- Gelernte Zuordnungen mit `git2pkg --reset-learning` zurücksetzen (der Git-Repository-Cache bleibt erhalten).
- Gelernte Abhängigkeits-Zuordnungen mit `git2pkg --explain-learning` anzeigen.

## Voraussetzungen

- Rust 1.89+ (zum Bauen aus dem Quellcode)
- Podman (bevorzugt) oder Docker
- Eine grafische Sitzung (Wayland oder X11) für die GUI

## Aus dem Quellcode bauen

```sh
git clone https://github.com/hyalcin70/git2pkg.git
cd git2pkg
cargo build --release
```

Die GUI startet mit `cargo run --release`, die CLI mit
`cargo run --release -- --cli https://github.com/user/repo`.

## Desktop-Installation

Im Projektordner `./install.sh` ausführen. Das Skript baut die Release-Binary
und fragt dann **wohin** sie kopiert werden soll: in den Projektordner
(Standard), nach `~/Applications`, auf den Desktop oder in einen eigenen Pfad.
Anschließend wird ein Startmenü-Eintrag angeboten. Mit `./uninstall.sh` werden
Binary und Menüeintrag wieder entfernt – der Pfad wird aus der `.desktop`-Datei
gelesen.

## Native Pakete

`git2pkg` selbst als reguläres Systempaket für die eigene Distribution bauen.
Alle Rezepte bauen den aktuellen Checkout und setzen Rust 1.89 oder neuer
voraus.

### Arch / CachyOS

```sh
cd packaging
makepkg -sc      # baut das Paket und entfernt src/ und pkg/
# makepkg -sic   # wie oben, anschließend installieren
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

Ist das Rust der Distribution älter als 1.89, vorher die aktuelle stabile
Toolchain mit `rustup` installieren.

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

## Projektstruktur

```
src/
  main.rs            Einstiegspunkt, GUI- vs. CLI-Start
  cli.rs             Headless-Modus
  core/              reine Logik, keine UI
    distro.rs        /etc/os-release-Auswertung
    detector.rs      Build-System-Erkennung
    resolver.rs      Regex-Fehleranalyse + Suchbefehle
    container.rs     Container-Steuerung und Paketierung
    packaging/       Paket-Backends für Arch/Debian/RPM
  gui/
    app.rs           eframe/egui-Anwendung
    fonts.rs         eingebettete Noto-Schriften
    i18n.rs          EN/DE/TR/ES/FR-Texte, Locale- und Theme-Erkennung
    settings.rs      GUI-Einstellungen (Ausgabeordner, Theme)
assets/              Schriften, Icon, Abhängigkeits-Wissensbasis
scripts/             manueller Smoke-Test-Korpus (Repo-URLs als Argumente)
packaging/           Arch-PKGBUILD, RPM-Spec/Build-Skript und Desktop-Datei
debian/              Debian/Ubuntu-Paketierung (dpkg-buildpackage)
.github/workflows/   CI- und taggesteuerter Release-Workflow
.gitlab-ci.yml       GitLab-CI-Build und -Paketierung
```

## CI / Releases

Ein Tag `v*` löst `.github/workflows/release.yml` aus: Auf `ubuntu-latest` wird
die Linux-`x86_64`-Binary gebaut, `git2pkg-linux-x86_64.tar.gz` samt `.sha256`
erzeugt und beides am GitHub-Release angehängt. Alpha-, Beta- und
Release-Candidate-Tags werden als GitHub-Pre-Releases veröffentlicht.
`.gitlab-ci.yml` baut dasselbe Archiv auf GitLab und stellt es als Job-Artefakt
bereit.

## Mitwirken & Übersetzungen

Beiträge sind willkommen. [CONTRIBUTING.md](CONTRIBUTING.md) beschreibt den
Pull-Request-Ablauf, `cargo fmt` / `cargo clippy` und neue Build-Systeme.

### Neue Sprache hinzufügen

Alle sichtbaren Texte liegen in `src/gui/i18n.rs`:

1. `Language`-Variante ergänzen und in `Language::ALL` aufnehmen – dieses Array
   steuert das Dropdown in der Toolbar.
2. `Language::detect()` um das Locale-Präfix ergänzen (`de`, `tr`, `es`, `fr`,
   ...) und Kurzcode sowie Label ergänzen.
3. Die Übersetzung in **jeden** `Texts::get(...)`-Aufruf einfügen. Die
   Reihenfolge ist immer `EN, DE, TR, ES, FR, <neue Sprache>`.
4. Prüfen, ob die Schriften in `assets/` die Zeichen der neuen Sprache enthalten.
5. Vor dem PR `cargo fmt`, `cargo clippy` und `cargo test` ausführen.

### Fehlgeschlagenen Build melden

In der GUI auf **Log kopieren** klicken und ein Issue mit der Vorlage
**Build failure** eröffnen. Das **vollständige** Log, die Repository-URL, die
Host-Distro und die verwendete Engine (Podman/Docker) einfügen. Ein Screenshot
nur der letzten Zeile reicht meist nicht aus.

## Unterstützung

Unterstützung ist völlig freiwillig. Wenn dir `git2pkg` Zeit spart, gib dem
Repository einen Stern, teile es, melde einen Fehler mit kopiertem Log oder
trag eine Übersetzung bei. Es gibt keine Telemetrie und keine Verpflichtung –
das Tool funktioniert unabhängig davon genau gleich.

> [!IMPORTANT]
> **Projektumfang & Ein-Personen-Projekt:** `git2pkg` ist ein unabhängiges Ein-Personen-Freizeitprojekt. Ab der Beta-Phase konzentriert sich der freiwillige Support nach verfügbarer Zeit strikt auf die unterstützten Haupt-Distributionsfamilien (**Arch / CachyOS**, **Debian / Ubuntu / Linux Mint**, **Fedora / Red Hat** und **openSUSE**). Andere Distributionen können funktionieren, gehören aber nicht zur getesteten Support-Matrix.

## Danksagung & Transparenz

Ich hatte die Idee zu diesem Projekt, habe die Richtung vorgegeben, getestet und Fehler analysiert — der eigentliche Quellcode wurde jedoch nahezu vollständig mithilfe von KI-Werkzeugen erstellt. Das möchte ich bewusst offen und ehrlich sagen. Das fertige Programm `git2pkg` selbst arbeitet jedoch zu 100 % lokal und deterministisch, ohne jegliche KI und ohne externe APIs.

## Lizenz

GPL-3.0-or-later – siehe [LICENSE](LICENSE). Die gebündelten Noto-Sans- und
Noto-Sans-Mono-Schriftdateien in `assets/` stehen unter der SIL Open Font
License 1.1.

## Danksagung

<a href="https://github.com/NousResearch/hermes-agent"><img src="assets/badges/hermes-agent.svg" alt="Hermes Agent" height="28"></a>

Ein herzliches Dankeschön an [Hermes Agent](https://github.com/NousResearch/hermes-agent) von Nous Research, der als lokales Entwicklungswerkzeug beim Aufbau von `git2pkg` geholfen hat (während `git2pkg` selbst zu 100 % deterministisch und völlig ohne KI-Abhängigkeiten läuft).
