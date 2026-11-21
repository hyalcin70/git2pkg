<p align="center"><img src="assets/app_icon.png" alt="git2pkg" width="128"></p>

[English](README.md) | [Deutsch](README.de.md) | [Türkçe](README.tr.md) | [Español](README.es.md) | [Français](README.fr.md)

# git2pkg

A small Linux desktop GUI that builds a Git repository **inside a disposable
Podman/Docker container** and drops the finished artifact into a visible output
folder (`~/Downloads/git2pkg` by default). The host never needs a compiler
toolchain or build libraries.

`git2pkg` is 100 % local, deterministic and rule-based. **It contains no AI,
no LLM and no external API calls.** Build decisions come from file detection,
regular expressions over compiler output, and the container's package manager.

> **Alpha Status:** This project is currently in active testing (Alpha). No support, assistance, or warranty of any kind is provided.

## Why I Built This

I love discovering new open-source tools on GitHub, GitLab and elsewhere, but I
did not want to clutter my main system with dozens of build libraries and
development headers — or depend blindly on third-party repositories such as the
AUR. So I built `git2pkg`: everything is compiled in a temporary throwaway
container, the host stays completely clean, and what comes out at the end is a
native package (`.pkg.tar.zst`, `.deb`, `.rpm`) or a single binary — usable by
anyone through a simple GUI.

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
  `go.mod`, `pyproject.toml`, `Makefile`).
- Live, scrollable build log streamed from the container, with a copy button.
- Missing-dependency resolution: the compiler output is scanned for missing
  headers, pkg-config packages and CMake modules; the matching package is
  installed **inside the throwaway container**, then the build is retried
  (max. 5 attempts).
- Native packages: `.pkg.tar.zst` (Arch, via `PKGBUILD` + `makepkg`), `.deb`
  (Debian/Ubuntu, via `dpkg-deb`), `.rpm` (Fedora/openSUSE, via `rpmbuild`).
- **Output folder**: finished artifacts land in `~/Downloads/git2pkg` by default.
  Change it in the GUI with the native folder picker (`rfd`) or type a path.
- Result actions: open the output folder, save the build recipe, and see the
  exact install command before anything is executed.
- Optional headless mode: `git2pkg --cli <url> [--format=package|binary]`.

## Requirements

- Rust 1.80+ (for building from source)
- Podman (preferred) or Docker
- A graphical session (Wayland or X11) for the GUI

## Build from source

```sh
git clone https://github.com/hyalcin70/git2pkg.git
cd git2pkg
cargo build --release
```

Run the GUI with `cargo run --release`, or the CLI with
`cargo run --release -- --cli https://github.com/user/repo`.

## Desktop installation

From the repository root, run `./install.sh`. It builds the release binary and
then asks **where** to put it: the project folder (default), `~/Applications`,
the Desktop, or a custom path. A start-menu entry is offered afterwards.
Use `./uninstall.sh` to remove the binary and the menu entry again — it reads
the installed path from the `.desktop` file.

## Arch / CachyOS package

`packaging/PKGBUILD` builds `git2pkg` as a regular system package:

```sh
cd packaging
makepkg -si
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
  gui/
    app.rs           eframe/egui application
    fonts.rs         embedded Noto fonts
    i18n.rs          EN/DE/TR/ES/FR texts, locale and theme detection
assets/              bundled Noto Sans + Noto Sans Mono (OFL 1.1)
.github/workflows/   tag-driven GitHub release workflow
.gitlab-ci.yml       GitLab CI build and package job
packaging/PKGBUILD   Arch package recipe for git2pkg itself
```

## CI / Releases

Pushing a tag matching `v*` triggers `.github/workflows/release.yml`, which
builds the Linux `x86_64` binary on `ubuntu-latest`, produces
`git2pkg-linux-x86_64.tar.gz` plus a `.sha256` file, and attaches both to the
GitHub release. `.gitlab-ci.yml` builds the same tarball on GitLab and exposes
it as a job artifact.

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

## Credits & Transparency

I designed and directed this project and developed it with the help of AI
tools. I want to say that openly. The finished `git2pkg` program itself,
however, works 100 % locally and deterministically, with no AI and no external
APIs at all.

## License

GPL-3.0-or-later — see [LICENSE](LICENSE). The bundled Noto Sans and Noto Sans
Mono font files in `assets/` are licensed under the SIL Open Font License 1.1.
