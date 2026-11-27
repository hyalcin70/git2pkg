# AGENTS.md — Working Rules for AI Coding Agents

This file is the authoritative, durable instruction set for any AI coding agent
working on `git2pkg`. Read it before changing anything. Keep it accurate when
the repository structure or the workflow changes.

> User-facing documentation is maintained in five languages: `README.md`
> (EN), `README.de.md` (DE), `README.tr.md` (TR), `README.es.md` (ES) and
> `README.fr.md` (FR). This agent guide is intentionally written in English.

> **Repository.** `git2pkg` is maintained at
> [`hyalcin70/git2pkg`](https://github.com/hyalcin70/git2pkg); `origin` is the
> canonical remote with push access. Never push without an explicit request
> from the maintainer. Commits stay local until then.

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

The learning store is safe against concurrent writers (GUI + CLI): every save
merges the on-disk state under a short-lived lock file and replaces the file
atomically. All sections are bounded by deterministic LRU eviction
(`MAX_LEARNED_PACKAGES`, `MAX_DEP_ENTRIES`, `MAX_NEGATIVE_ENTRIES`,
`MAX_BUILD_FIXES`, `MAX_REPO_PROFILES`, `MAX_BUILD_RECIPES` in `resolver.rs`).

Dependency learnings are stored per kind and distro family
(`family|kind|name`) with `package`, `verified`, `evidence`, `score` and
`last_seen`. A package is learned only after it really provides the missing
artifact (header, pkg-config module, CMake module, tool, linker library),
verified inside the container - CMake modules via a minimal `find_package`
probe, so built-in modules such as `ZLIB` are covered without a config file.
Mappings that fail verification are scored down and blocked after two strikes;
a mapping whose package is still missing in the next round is scored down as
well ("install success is not a fix"). Package names that cannot be installed
are cached per distro family too, never globally. Defaults and legacy flat
mappings remain as fallbacks.
`git2pkg --explain-learning` prints the learned table.

Failures are classified (dependency, version/ABI, bundled dependency,
source/compiler, packaging, no artifact, empty package, toolchain). If no
dependency pattern remains and the failure is a source or packaging error, the
build stops with a clear diagnosis instead of trying more packages. The
generated scripts set the markers `__GIT2PKG_NO_ARTIFACT__` and
`__GIT2PKG_EMPTY_PACKAGE__`, which take precedence over text patterns, and a
failed `cargo build` aborts explicitly instead of leaving the install step to
report "no binaries". When the distro `rustc` is older than the project's MSRV
(`rustc X is not supported`, `requires rustc`), the build is retried once in
the same run with a current stable toolchain via rustup.

CMake dependency analysis is context-aware. `find_package`, header and library
errors inside a CMake error block become CMakeModule/Header/Library entries,
while `find_program`, explicit `command not found` and shell failures stay
tools. A broken or empty version probe (`FinderBroken`) never penalizes a
verified mapping: it switches to CMake config mode and sets `<Module>_DIR` when
an installed config is found, without guessing package names. `CMAKE_PREFIX_PATH`
is assembled at configure time from existing superbuild prefixes only; a
nonexistent path must never be passed to CMake.

Strategy profiles marked "incomplete" are skipped, but if every strategy is
marked for a repository the markers are cleared and one real attempt runs
instead of failing without a build. CMake and Meson projects without install
rules (or with empty install rules) stage their built ELF programs to
`/usr/bin` and shared libraries to `/usr/lib` as a generic fallback; internal
CMake probe binaries in `CMakeFiles/` are never treated as programs.

Autotools projects without a pre-generated `configure` run their own
`./bootstrap` or `./autogen.sh` first (GNU projects fetch gnulib there), then
`autoreconf -fi`. The rustup healing only applies to Cargo projects - learned
build fixes must never install a Rust toolchain for a C/C++ build.

Repositories that ship their own upstream bootstrap (`x.py` plus
`src/bootstrap` in the repository root, for example the Rust compiler) are not
normal Cargo projects: a plain `cargo build` fails on nightly features and a
missing bootstrap configuration. Such trees are configured with
`./configure --prefix=/usr --set llvm.download-ci-llvm=true` and built /
installed through `./x.py build|install --stage <n>`; the stage defaults to 1
and can be overridden with `GIT2PKG_BOOTSTRAP_STAGE`. The extra packages they
need (curl, xz, zlib, OpenSSL) are installed only when those marker files
actually exist.

**Log language policy:** static UI text is localized in five languages
(`src/gui/i18n.rs`), but live build logs are English technical diagnostics
(`[learn]`, `[warn]`, `[strategy]`, `[retry]`, `[cmake]`, ...). Do not add new
German log strings; the strategy display names in the GUI come from
`Texts::strategy_label`, while `BuildStrategy::label()` is the English
canonical name used in logs and stored profiles.

**Generic include fallback:** when a compiler reports a missing header, the
dependency resolution looks for the header below `/usr/include` /
`/usr/local/include` and adds that directory via `BuildConfig::include_dirs`
(exported as `CFLAGS`/`CXXFLAGS`). For pkg-config dependencies the `cflags`
from `pkg-config` are added the same way. This is intentionally generic: never
add project-specific include paths or macro definitions by name.

The container build orchestration, retry strategies, cache handling and
cleanup live in `src/core/container.rs`. Packaging backends are modular under
`src/core/packaging/`. The GUI lives in `src/gui/app.rs`. GUI strategy retries reuse the
existing build workspace (`BuildConfig::workspace`) instead of cloning again and
auto-save the previous failed log before clearing the live log.

For local Arch builds use `makepkg -sc`: `-c` removes the `src/` and `pkg/`
work directories after a successful build, so only the finished package
remains. `makepkg -sic` additionally installs it.

Generated Arch packages (`.pkg.tar.zst`) are assembled directly with
`bsdtar` from `.PKGINFO`/`.MTREE`, because `makepkg`'s `fakeroot` step is
unreliable inside root containers. In-tree `PKGBUILD` files still use
`makepkg`. Debian packages compute `Depends` via `dpkg-shlibdeps` (the
temporary `debian/control` needs a `Source:` stanza or the parser refuses to
run); RPM
packages use `AutoReqProv`, ignore standard RPATHs (`QA_RPATHS=0x0001`) and
compress man pages before the `%files` list is generated. The target libdir and
`CMAKE_INSTALL_PREFIX=/usr` are passed explicitly (`/usr/lib64` on
Fedora/openSUSE) so installed files land where packages expect them and
binaries can find their libraries. Packages that contain executables under
`usr/bin` drop development-only artifacts afterwards (`usr/include`, `*.a`,
`*.pc`); such files almost always come from vendored libraries (for example an
embedded test framework) and belong in a separate -dev package. Projects that ship their own `debian/`
directory are packaged with `dpkg-buildpackage` using that recipe - package
name and version come from `debian/control` and `debian/changelog`, and
`dh` handles layout, `Depends` and documentation. If that fails (for example
missing build dependencies), the generated path runs unchanged. The same
applies to a project-provided `*.spec`: RPM packaging then runs `rpmbuild`
with that spec (name and version are read from it), and falls back to the
generated spec when the native run fails. Project versions are read from Cargo, CMake,
`pyproject.toml`, `AC_INIT`/`PACKAGE_VERSION`, then Git tags. The clone is
shallow, so tags are fetched separately (`git fetch --tags --depth=1`); a tag
directly at `HEAD` wins, otherwise the newest version tag is used (stable
releases before alpha/beta/rc). Untagged checkouts fall back to `0.1.0`.

Optional prebuilt builder images (`git2pkg-builder:arch`, `:debian`,
`:fedora`, `:suse`) carry the complete toolchain and are built with
`packaging/builders/build.sh`. When one exists locally, git2pkg uses it
instead of the bare base image and skips the package installation at the
start of the build; project-specific extra packages are still installed. The
images are optional - without them everything works as before.

Persistent Git clones live in `~/.cache/git2pkg/repos/repo-<repo_hash>/` and
are refreshed to the remote default branch (`origin/HEAD`) and updated in place;
submodules are initialized shallow and in parallel (`git submodule update
--init --recursive --depth=1 --jobs=4`) because only the working tree is
needed - the Rust compiler clone drops from roughly 15 GB to 5 GB and the
clone time from about 15 to 8 minutes. Tags are fetched separately for the
shallow clone. Build workspaces use the separate prefix
`~/.cache/git2pkg/builds/build-<unix_millis>-<pid>/src/` and are complete
independent copies (no symlink or shared clone). The exit dialog option
"Delete everything" first cancels the active build and stops build containers,
then wipes the complete `~/.cache/git2pkg/` tree (including `repos/`), using a
short-lived container if root-owned artifacts remain. Learned mappings in
`~/.config/git2pkg/learned_mappings.json` are never touched by cache cleanup;
only verified build recipes are reused. Recipes and their saved package names
are scoped to the target distro family, so an Arch recipe never leaks into a
Debian build. Unresolvable (often optional) CMake modules do not abort a build
when other dependencies were resolved in the same round; the build is retried. The GUI has no learning-reset button;
the CLI flag `--reset-learning` remains available. After package installs
`CMakeCache.txt`/`CMakeFiles` are reset; missing CLI tools are resolved through
the package manager file database; `option(...)` flags in `CMakeLists.txt` are
applied to system-lib and shared-lib strategies. Generated CMake PKGBUILDs use
`set -eo pipefail` in `build()` so configure/compile errors abort immediately.

---

## 2. Strict Scope & Non-Goals

`git2pkg` is exclusively for **native Linux desktop and CLI applications**.

Strictly out of scope — never add support or special cases for:

- Embedded / microcontroller toolchains
- IDE plugins
- Mobile apps
- Web apps

Do not add repository-specific or product-specific hacks to make such projects
build. Generalize all changes to build systems, file structures and error
classes instead.

Concretely, the code must never branch on a repository URL or project name, and
must not contain fixed lists of one project's files. Detect patterns (script
names containing `build`/`compile`, error classes, file types) instead. Repo build
scripts are selected by target distribution family (`arch`, `debian`, `fedora`,
`suse`); scripts naming another distribution or a foreign OS are ignored. Tests,
scripts and documentation must not use real application names as fixtures or
examples. The dependency table in `assets/default_mappings.json` and
`distro_package_name` is generic library metadata (CMake module names and
pkg-config identifiers mapped to distro packages) and is explicitly allowed;
it applies to every project and never special-cases one.

Positioning is deliberately narrow: never claim that `git2pkg` builds "any" or
"all" projects. Keep the support matrix in the five READMEs accurate and speak
of "supported projects", not universal support. Do not add feature promises to
release notes or the repository description. A public repository does not imply
Linux support: projects that officially target only Windows/macOS, or whose
Linux build is explicitly a community or unsupported port, are outside the
support matrix. Never add URL-based warnings, blocklists or project-name lists
for them.

---

## 3. Repository Map (as of `v0.2.0-alpha.34`)

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
- `packaging/git2pkg.spec` — RPM spec for Fedora/RHEL/openSUSE
- `packaging/build-rpm.sh` — local RPM build helper
- `packaging/git2pkg.desktop` — desktop launcher
- `debian/` — Debian/Ubuntu packaging (`dpkg-buildpackage`)

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
- `.github/workflows/ci.yml` — QA workflow (`fmt`, `clippy`, `test`, release build)
- `.github/ISSUE_TEMPLATE/build_failure.md`
- `.gitlab-ci.yml`
- `scripts/test-corpus.sh` — manual smoke corpus; pass repository URLs as arguments
- `debian/` — Debian/Ubuntu native packaging
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
6. **Feature-freeze guard.** During the `v0.2.0-alpha.*`/`beta` stabilization
   phase, stop the maintainer before implementing a request that adds a new
   feature, a new option, a new integration or an expanded support scope. State
   clearly that it is a new feature, point to the stability/feature-freeze goal,
   and ask for explicit confirmation. **Even an explicit request must be refused
   if the agent judges that it would endanger the stability or reliability of
   `git2pkg`; explain the risk and offer a safer alternative or defer the change
   until after `v1.0.0`.** Bug fixes, robustness, tests, documentation
   corrections and packaging fixes are not new features. Only proceed after an
   explicit "yes" and only when the change is stability-neutral.

Additional practical rules:

- The main GUI is `src/gui/app.rs`; never look for or create `src/ui.rs` as the
  primary GUI module.
- After each completed code change: run QA, restart the GUI with the current
  release binary and commit locally. Do not push unless the maintainer asks
  explicitly.
- Create physical backups under `/home/hakan/Privat/projects/` when a backup
  folder is requested; never delete old backups.
- Cargo staging prefers the binary list from a project packaging manifest
  (`packaging/linux/nfpm.yaml`, `nfpm.yaml`), then the Cargo package matching
  the project name, then a project-name prefix; only when none match does it
  fall back to all built binaries. Never package unrelated workspace tools when
  a project-named or manifest-listed binary exists.
- Cargo builds use the same manifest/Cargo package selection as staging; only
  the selected binaries are compiled. If that targeted build fails, fall back
  to the full workspace build.
- Package metadata (description, license, URL) is read from the project
  manifests (`Cargo.toml`, `pyproject.toml`, CMake, nfpm) and desktop files;
  `README*`, `LICENSE*`, `NOTICE*` and `THIRD-PARTY*` are staged. Never use the
  builder's own GPL license as a project fallback.
- Arch packages use zstd level 19 to stay close to native package sizes.
- Package content verification: a package that contains
  `usr/share/applications/*.desktop` must also contain the executable named by
  the desktop file's `Exec=` key. A package that fails this check is rejected,
  its strategy is marked incomplete, and it must never be recorded as a
  verified recipe.

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
- Commit locally with a clear message. **Do not push.**

The same gates run in CI (`.github/workflows/ci.yml`) on every push to `main`
and on every pull request.

Version bumps happen **only when cutting a release**:

- Update `Cargo.toml` and `Cargo.lock` (via `cargo check`).
- Update `packaging/PKGBUILD` with an Arch-compliant `pkgver` without hyphens
  (for example `0.2.0alpha29`). This PKGBUILD builds the local checkout and
  deliberately has no `_tag` or source archive. It sets `options=('!lto')`
  because makepkg's default `-flto=auto` produces GCC LTO objects for the
  `ring` C code that `rust-lld` cannot link.
- Update `debian/changelog` and the fallback version in
  `packaging/git2pkg.spec`; both native recipes build the local checkout, and
  `packaging/build-rpm.sh` derives the RPM version from `Cargo.toml`.
- Create an annotated tag: `git tag -a v<VERSION> -m "Release v<VERSION>"`.
- Add curated release notes under `.github/release-notes/<tag>.md` with
  `Highlights` and `Changes`; the release workflow uses that file and falls
  back to a generated commit list when it is missing.
- Do not push tags unless the maintainer asks explicitly.

`--reset-learning` deletes `learned_mappings.json` without touching the Git
repository cache and prints a confirmation.

Documentation-only commits do **not** require a version bump or release tag.

---

## 6. Current Milestone & Roadmap (as of `v0.2.0-alpha.34`)

Current release: `v0.2.0-alpha.34`.

- **`v0.2.0-alpha.21` → `v0.2.0-alpha.34`**: hardening tests on CachyOS —
  C++/CMake superbuilds, Meson/C projects and Go projects. Fix real failures
  generically; never hardcode application or project names into the code.
- **`v0.2.0-beta.1`**: feature freeze and a fresh VM test round —
  Debian/Ubuntu, Fedora, openSUSE, plus a Podman check.
- **Deferred until `v0.2.0-beta.1` (do not implement during alpha):** support
  source archives (`.tar.gz`, `.tar.xz`, `.zip`) and local source directories in
  addition to Git URLs. Read version and metadata from project manifests; do not
  assume Git tags, submodules or LFS. CLI first, GUI path picker later. No
  project names or special cases in code.
- **`v0.2.0-rc.1` → `v1.0.0`**: release candidate, then official support start.

When updating this roadmap, keep the current version at the top accurate.
