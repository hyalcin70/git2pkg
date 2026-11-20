<p align="center"><img src="assets/git2pkg.png" alt="git2pkg" width="128"></p>

[English](README.md) | [Deutsch](README.de.md) | [Türkçe](README.tr.md) | [Español](README.es.md) | [Français](README.fr.md)

# git2pkg

Eine kleine Linux-Desktop-GUI, die ein Git-Repository **in einem temporären
Wegwerf-Container (Podman/Docker)** baut und das Ergebnis in einen sichtbaren
Ausgabeordner legt (standardmäßig `~/Downloads/git2pkg`). Das Host-System
braucht weder Compiler-Toolchain noch Build-Bibliotheken.

`git2pkg` arbeitet zu 100 % lokal, deterministisch und regelbasiert. **Es enthält
keine KI, kein LLM und keine externen API-Aufrufe.** Build-Entscheidungen
erfolgen durch Dateierkennung, reguläre Ausdrücke über Compiler-Ausgaben und
den Paketmanager im Container.

## Warum ich das gebaut habe

Ich entdecke gerne neue Open-Source-Tools auf GitHub, GitLab und anderen
Plattformen – wollte mein Hauptsystem aber nicht mit Dutzenden Build-Bibliotheken
und Header-Dateien zumüllen und mich auch nicht blind auf Drittanbieter-Repos
wie dem AUR verlassen. Deshalb habe ich `git2pkg` gebaut: Alles wird in einem
temporären Wegwerf-Container kompiliert, das Host-System bleibt zu 100 % sauber,
und am Ende entsteht ein natives Paket (`.pkg.tar.zst`, `.deb`, `.rpm`) oder eine
einzelne Binary – per GUI für jeden nutzbar.

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
  `meson.build`, `go.mod`, `pyproject.toml`, `Makefile`).
- Live-Log aus dem Container, scrollbar, mit „Log kopieren"-Button.
- Auflösung fehlender Abhängigkeiten: Die Compiler-Ausgabe wird nach fehlenden
  Headern, pkg-config-Paketen und CMake-Modulen durchsucht; das passende Paket
  wird **im Wegwerf-Container** installiert und der Build erneut gestartet
  (max. 5 Versuche).
- Native Pakete: `.pkg.tar.zst` (Arch via `PKGBUILD` + `makepkg`), `.deb`
  (Debian/Ubuntu via `dpkg-deb`), `.rpm` (Fedora/openSUSE via `rpmbuild`).
- **Ausgabeordner**: Fertige Artefakte landen standardmäßig in
  `~/Downloads/git2pkg`. Die GUI bietet einen nativen Ordnerdialog (`rfd`)
  oder die direkte Eingabe eines Pfads.
- Ergebnis-Aktionen: Ausgabeordner öffnen, Build-Rezept speichern und den
  exakten Installationsbefehl anzeigen, bevor etwas ausgeführt wird.
- Optionaler Headless-Modus: `git2pkg --cli <url> [--format=package|binary]`.

## Voraussetzungen

- Rust 1.80+ (zum Bauen aus dem Quellcode)
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

## Arch-/CachyOS-Paket

`packaging/PKGBUILD` baut `git2pkg` als normales Systempaket:

```sh
cd packaging
makepkg -si
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
  gui/
    app.rs           eframe/egui-Anwendung
    fonts.rs         eingebettete Noto-Schriften
    i18n.rs          EN/DE/TR/ES/FR-Texte, Locale- und Theme-Erkennung
assets/              gebündelte Noto Sans + Noto Sans Mono (OFL 1.1)
.github/workflows/   GitHub-Release-Workflow bei Tags
.gitlab-ci.yml       GitLab-CI-Build und -Paketierung
packaging/PKGBUILD   Arch-Paketrezept für git2pkg selbst
```

## CI / Releases

Ein Tag `v*` löst `.github/workflows/release.yml` aus: Auf `ubuntu-latest` wird
die Linux-`x86_64`-Binary gebaut, `git2pkg-linux-x86_64.tar.gz` samt `.sha256`
erzeugt und beides am GitHub-Release angehängt. `.gitlab-ci.yml` baut dasselbe
Archiv auf GitLab und stellt es als Job-Artefakt bereit.

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

## Danksagung & Transparenz

Ich habe dieses Projekt konzipiert und geleitet und es mit Unterstützung von
KI-Werkzeugen entwickelt. Das sage ich bewusst offen. Das fertige Programm
`git2pkg` selbst arbeitet jedoch zu 100 % lokal und deterministisch, ohne
jegliche KI und ohne externe APIs.

## Lizenz

GPL-3.0-or-later – siehe [LICENSE](LICENSE). Die gebündelten Noto-Sans- und
Noto-Sans-Mono-Schriftdateien in `assets/` stehen unter der SIL Open Font
License 1.1.
