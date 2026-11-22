# Contributing to git2pkg-ai

Thanks for helping out. `git2pkg-ai` is a small, dependency-light Rust project:
keep changes focused, deterministic and free of any network/AI calls at runtime.

## Pull requests

1. Fork the repository and create a topic branch (`fix/...`, `feat/...`).
2. Make your change and keep the commit history readable.
3. Before opening a PR, run all of these from the repository root:

   ```sh
   cargo fmt -- --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   cargo build --release
   ```

4. Describe **what** changed and **why**. If you touched the UI, add a
   screenshot. If you changed generated build scripts, add a test in
   `src/core/container.rs`.

## Code style

- Rust formatting via `cargo fmt`; no hand-formatted code.
- `cargo clippy` must pass without warnings.
- Keep `src/core/` free of GUI code and keep `src/gui/` free of build logic.
- All user-facing strings belong in `src/gui/i18n.rs` — never hard-code them
  in widgets or build messages.
- New behaviour needs a unit test next to the module it touches.

## Adding a build system

1. Add a marker file to `detect_build_system()` in
   `src/core/detector.rs` (e.g. `justfile`, `build.gradle`) and a variant to
   `BuildSystem` with a stable `label()`.
2. Add its build command to `build_command()`, `binary_build_step()` and
   `package_install_step()` in `src/core/container.rs`.
3. Add a detector test and a script-generation test.

## Adding a language

1. Add the variant to `Language` in `src/gui/i18n.rs`.
2. Add it to `Language::ALL` (this drives the toolbar dropdown) and to
   `code()` / `label()`.
3. Extend `Language::detect()` with the locale prefix (`de`, `tr`, `es`, ...).
4. Add the new string to **every** `Texts::get(...)` call. The argument order
   is `EN, DE, TR, ES, FR, <new>` — keep it consistent everywhere.
5. Make sure the bundled fonts in `assets/` actually contain the glyphs of
   the new language (check with `fontTools`, not by guessing).
6. Run `cargo test`; the i18n tests catch missing translations.

## Reporting a failed build

Use the **build failure** issue template. In the app, click **Log kopieren**
(**Copy log**) and paste the full log into the issue — a screenshot of the
last line is not enough to diagnose a missing dependency.

## License

By contributing you agree that your changes are licensed under
GPL-3.0-or-later.
