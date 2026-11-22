# AGENTS.md — Working Rules for AI Coding Agents

This file is the authoritative, durable instruction set for any AI coding agent
working on `git2pkg`. Read it before changing anything. Keep it accurate when
the repository structure or the workflow changes.

> User-facing documentation is maintained in five languages: `README.md`
> (EN), `README.de.md` (DE), `README.tr.md` (TR), `README.es.md` (ES) and
> `README.fr.md` (FR). This agent guide is intentionally written in English.

---

## 1. Project Overview & Architecture

`git2pkg` is a **100 % deterministic, local Rust application**. It has an
`egui`/`eframe` desktop GUI and a headless CLI. It builds public Git
repositories inside ephemeral `docker` or `podman` containers and produces
native Linux packages (`.pkg.tar.zst`, `.deb`, `.rpm`) or a standalone binary.
The host system needs no compiler toolchain and no build libraries.

Supported input projects are native Linux desktop and CLI applications using
C/C++, Rust, Go, Python CLI tools, Meson, CMake, Cargo, Make or Autotools.
Build decisions are rule-based: file detection, regular expressions over
compiler output, and the package manager inside the container. There are no AI
APIs, no LLM calls and no external services inside the shipped program.

Dependency resolution is deterministic and self-learning. It lives in
`src/core/resolver.rs` (NOT `src/resolver.rs`). Learned mappings, repository
profiles and build recipes are persisted under
`~/.config/git2pkg/learned_mappings.json`.

The container build orchestration, retry strategies, cache handling and
cleanup live in `src/core/container.rs`. Packaging backends are modular under
`src/core/packaging/`. The GUI lives in `src/gui/app.rs`.

Persistent Git clones live in `~/.cache/git2pkg/repos/<repo_hash>/` and are
updated in place. Temporary build workspaces live in
`~/.cache/git2pkg/builds/<build_id>/` and are removed after success or by the
exit cleanup. Learned mappings are in
`~/.config/git2pkg/learned_mappings.json`; only verified build recipes are reused.

---

## 2. Strict Scope & Non-Goals

`git2pkg` is exclusively for **native Linux desktop and CLI applications**.

Strictly out of scope — never add support or special cases for:

- Embedded / microcontroller toolchains (Arduino, PlatformIO, ESP-IDF)
- IDE plugins (for example VS Code extensions)
- Mobile apps
- Web apps

Do not add repository-specific or product-specific hacks to make such projects
build. Generalize all changes to build systems, file structures and error
classes instead.

---

## 3. Repository Map (as of `v0.2.0-alpha.22`)

### Entry points

- `src/main.rs` — application entry point, GUI/CLI dispatch
- `src/cli.rs` — headless CLI (`--cli <url> --target=...`, `--reset-learning`)

### Core logic

- `src/core/mod.rs` — core module exports
- `src/core/container.rs` — container lifecycle, build loop, retry strategies,
  cleanup, cache handling, progress events
- `src/core/resolver.rs` — deterministic dependency analysis, package mappings,
  learned profiles and build recipes
- `src/core/detector.rs` — build-system detection and project fingerprinting
- `src/core/distro.rs` — host/target distribution detection and package-manager
  metadata

### Packaging

- `src/core/packaging/mod.rs` — shared packaging context and script generation
- `src/core/packaging/arch.rs` — Arch/CachyOS `.pkg.tar.zst`
- `src/core/packaging/debian.rs` — Debian/Ubuntu/Mint `.deb`
- `src/core/packaging/rpm.rs` — Fedora/RHEL and openSUSE `.rpm`
- `src/core/packaging/verify.rs` — package verification
- `packaging/PKGBUILD` — Arch source package template
- `packaging/git2pkg.desktop` — desktop launcher

### GUI

- `src/gui/mod.rs` — GUI module exports
- `src/gui/app.rs` — **the main GUI** (there is NO `src/ui.rs`)
- `src/gui/i18n.rs` — EN/DE/TR/ES/FR string catalog
- `src/gui/settings.rs` — persisted settings
- `src/gui/fonts.rs` — embedded Noto Sans / Noto Sans Mono fonts

### Assets — protected

- `assets/app_icon.png`
- `assets/ui/creator_logo.png`
- `assets/ui/made_for_linux.png`
- `assets/badges/` — third-party badge assets (currently `hermes-agent.svg`)
- `assets/default_mappings.json`
- `assets/NotoSans-Regular.ttf`
- `assets/NotoSansMono-Regular.ttf`

### User documentation

- `README.md`, `README.de.md`, `README.tr.md`, `README.es.md`, `README.fr.md`
- `CONTRIBUTING.md`
- `LICENSE` (GPL-3.0-or-later)

### Agent documentation

- `AGENTS.md` — durable rules for all AI coding agents (this file)
- `CLAUDE.md` — relative symlink to `AGENTS.md`

### Automation / installation

- `.github/workflows/release.yml` — release workflow (tag push → GitHub Release)
- `.github/ISSUE_TEMPLATE/build_failure.md`
- `.gitlab-ci.yml`
- `install.sh`, `uninstall.sh`

---

## 4. Mandatory Engineering Rules

1. **100 % deterministic.** No AI APIs, no LLM calls, no external API calls in
   the program. No overengineering. Extend existing modules cleanly.
2. **No hardcoded project names or repository-specific special cases in Rust
   code.** No product names, repository URLs or concrete test project names in
   `src/` or tests. Use neutral placeholders such as `example-app`,
   `sample-tool`, `example.invalid`. Every fix must generalize to build systems,
   file structures or error classes.
3. **Strict asset protection.** Never modify, overwrite or regenerate existing
   PNG assets (`assets/ui/made_for_linux.png`, `assets/ui/creator_logo.png`,
   `assets/app_icon.png`) unless the user explicitly orders that exact asset
   change. Do not rename or swap assets.
4. **Multilingual documentation.** Whenever a user-facing feature, scope or
   support statement changes, update all five READMEs (`EN`, `DE`, `TR`, `ES`,
   `FR`) consistently.
5. **Mandatory `AGENTS.md` maintenance.** Every AI coding agent working on this
   repository MUST keep `AGENTS.md` (and therefore the `CLAUDE.md` symlink)
   permanently up to date whenever new files or modules are added under `src/`,
   engineering rules or workflows change, or a major milestone is reached
   (for example `beta.1`, `rc.1`, `v1.0.0`). Do NOT bloat `AGENTS.md` with minor
   bugfix history; that belongs in Git commits.

Additional practical rules:

- The main GUI is `src/gui/app.rs`; never look for or create `src/ui.rs` as the
  primary GUI module.
- After each completed code change: run QA, restart the GUI with the current
  release binary, commit cleanly and push to `origin main`.
- Create physical backups under `/home/hakan/projects/` when a backup folder is
  requested; never delete old backups.

---

## 5. Mandatory QA & Release Workflow

Before changes:

1. Clean unnecessary temporary files without touching Git data.
2. Create a safety backup tag, e.g.
   `git tag backup-before-<short-description>`.

After changes, all four gates must pass with zero warnings and zero errors:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

Then:

- Review `git diff` and confirm no hardcoded project names or repository URLs
  were introduced in Rust code.
- Restart the GUI with the freshly built `target/release/git2pkg`.
- Commit with a clear message and push to `origin main`.

Version bumps happen **only when cutting a release**:

- Update `Cargo.toml` and `Cargo.lock` (via `cargo check`).
- Update `packaging/PKGBUILD` with an Arch-compliant `pkgver` without hyphens
  (for example `0.2.0alpha21`) and `_tag=v0.2.0-alpha.21`.
- Create an annotated tag: `git tag -a v<VERSION> -m "Release v<VERSION>"`.
- Push both: `git push origin main && git push origin v<VERSION>`.
- Verify that the GitHub Actions release workflow completed successfully and
  that the release is visible on GitHub before reporting success.

`--reset-learning` deletes `learned_mappings.json` without touching the Git
repository cache and prints a confirmation.

Documentation-only commits do **not** require a version bump or release tag.

---

## 6. Current Milestone & Roadmap (as of `v0.2.0-alpha.22`)

Current release: `v0.2.0-alpha.22`.

- **`v0.2.0-alpha.21` → `v0.2.0-alpha.30`**: hardening tests on CachyOS —
  C++/CMake superbuilds (for example Bambu Studio), Meson/C projects (for
  example mpv) and Go projects (for example lazygit). Fix real failures
  generically, never by hardcoding these project names into the code.
- **`v0.2.0-beta.1`**: feature freeze and a fresh VM test round —
  Debian/Ubuntu, Fedora, openSUSE, plus a Podman check.
- **`v0.2.0-rc.1` → `v1.0.0`**: release candidate, then official support start.

When updating this roadmap, keep the current version at the top accurate.
