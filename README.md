<p align="center"><img src="assets/app_icon.png" alt="git2pkg" width="128"></p>

[English](README.md) | [Deutsch](README.de.md) | [Türkçe](README.tr.md) | [Español](README.es.md) | [Français](README.fr.md)

# git2pkg

[![CI](https://github.com/hyalcin70/git2pkg/actions/workflows/ci.yml/badge.svg)](https://github.com/hyalcin70/git2pkg/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/hyalcin70/git2pkg?include_prereleases&label=release)](https://github.com/hyalcin70/git2pkg/releases)

A small Linux desktop GUI that builds **supported native Linux projects** from
Git repositories **inside a disposable Podman/Docker container** and drops the
finished artifact into a visible output folder (`~/Downloads/git2pkg` by
default). The host never needs a compiler toolchain or build libraries.

`git2pkg` is 100 % local, deterministic and rule-based. **It contains no AI,
no LLM and no external API calls.** Build decisions come from file detection,
regular expressions over compiler output, and the container's package manager.

> [!WARNING]
> **Alpha status:** `git2pkg` is under active development. Expect breaking changes and incomplete features. Please test with disposable containers, keep backups, and report failures with the complete log.

## Supported scope

`git2pkg` is built for native Linux desktop and CLI projects. It is **not a
universal builder**: the supported build systems, target formats and host
distributions are listed below. Anything outside this matrix may work, but it is
not part of the tested support scope.

| Area | Supported |
|---|---|
| Host | x86_64 Linux with Podman or Docker |
| Targets | Arch/CachyOS `.pkg.tar.zst`, Debian/Ubuntu/Mint `.deb`, Fedora/RHEL and openSUSE `.rpm`, standalone binary |
| Build systems | CMake, Meson, Autotools, Cargo, Go, Python, Make, qmake |
| Dependency lookup | Distribution packages inside the target container, verified after installation |
| Not supported | Embedded/microcontroller toolchains, mobile apps, web apps, IDE plugins, kernel modules, cross-architecture builds, proprietary SDKs that cannot be installed in the container, guaranteed compliance with distribution packaging policies |

> **ⓘ Information**
>
> **« Public source code alone does not imply Linux support. »**
>
> But: git2pkg supports projects that officially target Linux. What matters is
> the project's own support statement, not the availability of its source code.
>
> <details>
> <summary>Detail: What is in the support matrix?</summary>
>
> - ✓ **Official Linux-target projects** (supported by git2pkg).
> - ✗ **Windows/macOS-only projects** and **community-only ports** are outside
>   the support matrix.
>
> </details>

`git2pkg` is not a replacement for distribution packaging, AUR, Flatpak or a
security audit. It builds what it can build reliably, reports the rest clearly,
and the resulting package should be tested on the target distribution.

## Why I Built This

I love discovering new open-source tools on GitHub, GitLab and elsewhere, but I
did not want to clutter my main system with dozens of build libraries and
development headers — or depend blindly on third-party repositories such as the
AUR. So I built `git2pkg`: the build runs in a temporary throwaway container,
the host stays completely clean, and what comes out at the end is a native
package (`.pkg.tar.zst`, `.deb`, `.rpm`) or a single binary — usable through a
simple GUI.

## Features

- Paste a Git repository URL, pick a target format, click **Start build**.
- **Automatic theme**: follows the desktop light/dark setting at startup (via
  the `dark-light` crate); falls back to light when it cannot be detected. An
  **Auto / Light / Dark** button in the top-right corner overrides it.
- **Automatic language**: German (`de*`), Turkish (`tr*`), Spanish (`es*`) or
  French (`fr*`) from `LC_ALL`, `LC_MESSAGES` or `LANG`; English everywhere
  else. An **EN / DE / TR / ES / FR** dropdown switches languages at any time.
- **Embedded Noto Sans / Noto Sans Mono fonts** guarantee correct rendering of
  Turkish characters (İ, ı, ğ, Ğ, ş, Ş) and all Spanish/French accents.
- Automatic host detection from `/etc/os-release` (Arch/CachyOS, Ubuntu/Debian,
  Fedora, openSUSE) with matching install commands (`pacman -U`, `apt install`,
  `dnf install`, `zypper install`).
- Automatic build-system detection (`Cargo.toml`, `CMakeLists.txt`, `meson.build`,
  Autotools (`configure.ac`), qmake (`.pro`), `go.mod`, `pyproject.toml`,
  `Makefile`).
- Repository build scripts are selected by target distribution; scripts for
  other distributions or operating systems are ignored.
- Live, scrollable build log streamed from the container, with a copy button.
- Retrying with another strategy reuses the existing build workspace (no second
  clone) and automatically saves the previous failed log.
- Missing-dependency resolution: the compiler output is scanned for missing
  headers, pkg-config packages, CMake modules, tools and linker libraries; the
  matching package is installed **inside the throwaway container** and then
  verified (the header/module/library must really exist) before the build is
  retried (max. 5 attempts). Only verified matches are remembered.
- Failure diagnosis: builds are classified (missing dependency, version/ABI
  mismatch, bundled code, compiler error, packaging error). Fatal ABI or
  bundled-finder conflicts stop the build immediately with a clear version/ABI
  diagnosis instead of trying random packages.
- Native packages: `.pkg.tar.zst` (Arch; assembled directly, in-tree `PKGBUILD`
  via `makepkg` when present), `.deb` (Debian/Ubuntu, via `dpkg-deb`), `.rpm`
  (Fedora/openSUSE, via `rpmbuild`). Projects that ship their own `debian/` or
  `*.spec` are packaged with `dpkg-buildpackage`/`rpmbuild` using that recipe.
- Optional prebuilt builder images (`packaging/builders/build.sh`) carry the
  complete toolchain for each distribution and make repeated builds several
  times faster; without them everything works as before.
- **Output folder**: finished artifacts land in `~/Downloads/git2pkg` by default.
  Change it in the GUI with the native folder picker (`rfd`) or type a path.
- Result actions: open the output folder, save the build recipe, and see the
  exact install command before anything is executed.
- Optional headless mode: `git2pkg --cli <url> [--format=package|binary]`.
- Reset learned mappings with `git2pkg --reset-learning` (the Git repository cache is kept).
- Inspect learned dependency mappings with `git2pkg --explain-learning`.

## Requirements

- Rust 1.89+ (for building from source)
- Podman (preferred) or Docker
- A graphical session (Wayland or X11) for the GUI

## Build from source

```sh
# once: current Rust toolchain (the distro version is often too old)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

git clone https://github.com/hyalcin70/git2pkg.git
cd git2pkg
rustup default stable
cargo build --release
```

`git2pkg` needs Rust 1.89 or newer. Linux Mint 22.3 (Ubuntu 24.04) ships 1.75,
Debian 13 ships 1.85 - `rustup default stable` provides a toolchain that is new
enough.

Run the GUI with `cargo run --release`, or the CLI with
`cargo run --release -- --cli https://github.com/user/repo`.

## Desktop installation

From the repository root, run `./install.sh`. It builds the release binary and
then asks **where** to put it: the project folder (default), `~/Applications`,
the Desktop, or a custom path. A start-menu entry is offered afterwards.
Use `./uninstall.sh` to remove the binary and the menu entry again — it reads
the installed path from the `.desktop` file.

## Native packages

Build `git2pkg` itself as a regular system package for your distribution. All
recipes build the current checkout and require Rust 1.89 or newer.

### Arch / CachyOS

```sh
cd packaging
makepkg -sc      # build the package and remove src/ and pkg/
# makepkg -sic   # same, then install it
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

If the distribution's Rust is older than 1.89, install the current stable
toolchain with `rustup` first.

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

## Project layout

```
src/
  main.rs            entry point, GUI vs. CLI dispatch
  cli.rs             headless mode
  core/              pure logic, no UI code
    distro.rs        /etc/os-release parsing
    detector.rs      build-system detection
    resolver.rs      regex error analysis + search commands
    container.rs     container orchestration and packaging
    packaging/       Arch/Debian/RPM packaging backends
  gui/
    app.rs           eframe/egui application
    fonts.rs         embedded Noto fonts
    i18n.rs          EN/DE/TR/ES/FR texts, locale and theme detection
    settings.rs      GUI settings (output folder, theme)
assets/              fonts, icon, dependency knowledge base
scripts/             manual smoke corpus (repository URLs as arguments)
packaging/           Arch PKGBUILD, RPM spec/build script and desktop entry
debian/              Debian/Ubuntu packaging (dpkg-buildpackage)
.github/workflows/   CI and tag-driven release workflows
.gitlab-ci.yml       GitLab CI build and package job
```

## CI / Releases

Pushing a tag matching `v*` triggers `.github/workflows/release.yml`, which
builds the Linux `x86_64` binary on `ubuntu-latest`, produces
`git2pkg-linux-x86_64.tar.gz` plus a `.sha256` file, and attaches both to the
GitHub release. Alpha, beta and release-candidate tags are published as GitHub
pre-releases. `.gitlab-ci.yml` builds the same tarball on GitLab and exposes it
as a job artifact.

## Contributing & Translations

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for the
pull-request workflow, the `cargo fmt` / `cargo clippy` requirements, and for
adding new build systems.

### Adding a language

Every user-facing string lives in `src/gui/i18n.rs`:

1. Add a `Language` variant and include it in `Language::ALL` — this array
   drives the dropdown in the toolbar.
2. Extend `Language::detect()` with the locale prefix (`de`, `tr`, `es`, `fr`,
   ...) and add its short code and label.
3. Add the new translation to **every** `Texts::get(...)` call. The argument
   order is `EN, DE, TR, ES, FR, <new language>` and must stay identical
   everywhere.
4. Verify that the bundled fonts in `assets/` really contain the glyphs of the
   new language.
5. Run `cargo fmt`, `cargo clippy` and `cargo test` before opening the PR.

### Reporting a failed build

Click **Copy log** in the GUI and open a new issue using the **Build failure**
template. Paste the **complete** log, the Git repository URL, your host
distribution and whether you use Podman or Docker. A screenshot of only the
last line is usually not enough to identify a missing dependency.

## Support

Support is entirely voluntary. If `git2pkg` saves you time, star the repository,
share it with someone who builds from source, report a bug with the copied log,
or contribute a translation. There is no telemetry and no obligation — the tool
works exactly the same whether you do any of this or not.

> [!IMPORTANT]
> **Project Scope & Solo Maintenance:** `git2pkg` is an independent solo hobby project maintained in my spare time. Starting from the Beta stage, best-effort support will focus strictly on the supported mainstream distribution families (**Arch / CachyOS**, **Debian / Ubuntu / Linux Mint**, **Fedora / Red Hat**, and **openSUSE**). Other distributions may work, but they are outside the tested support matrix.

## Credits & Transparency

I came up with the idea for this project, steered its direction, tested it, and analyzed bugs — while almost the entire source code was written with the help of AI tools. I want to state that openly and honestly. The finished `git2pkg` program itself, however, works 100 % locally and deterministically, with no AI and no external APIs at all.

## License

GPL-3.0-or-later — see [LICENSE](LICENSE). The bundled Noto Sans and Noto Sans
Mono font files in `assets/` are licensed under the SIL Open Font License 1.1.

## Acknowledgments

<a href="https://github.com/NousResearch/hermes-agent"><img src="assets/badges/hermes-agent.svg" alt="Hermes Agent" height="28"></a>

Special thanks to [Hermes Agent](https://github.com/NousResearch/hermes-agent) by Nous Research, which assisted as a local development tool during the creation of `git2pkg` (while `git2pkg` itself runs 100% deterministically with zero AI dependencies).
