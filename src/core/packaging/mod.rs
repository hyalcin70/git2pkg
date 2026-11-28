//! Paket-Erzeugung pro Distribution, getrennt von der Container-Orchestrierung.
//!
//! `container.rs` baut und staged nach `$pkgdir`; dieses Modul schnürt daraus
//! `.pkg.tar.zst`, `.deb` bzw. `.rpm` und prüft anschließend den Inhalt.

pub mod arch;
pub mod debian;
pub mod rpm;
pub mod verify;

use crate::core::container::BuildStrategy;
use crate::core::distro::DistroFamily;

/// Host-/Container-Architektur, dynamisch statt hartcodiert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageArch {
    Amd64,
    Arm64,
    Other(String),
}

impl PackageArch {
    pub fn detect() -> Self {
        match std::env::consts::ARCH {
            "x86_64" | "amd64" => Self::Amd64,
            "aarch64" | "arm64" => Self::Arm64,
            other => Self::Other(other.to_string()),
        }
    }

    /// Nur 64-Bit-Ziele werden unterstuetzt.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Amd64 | Self::Arm64)
    }

    /// Beschreibung der nicht unterstuetzten Architektur (fuer die Meldung).
    pub fn unsupported_label(&self) -> String {
        match self {
            Self::Amd64 => "x86_64".to_string(),
            Self::Arm64 => "aarch64".to_string(),
            Self::Other(value) => value.clone(),
        }
    }

    pub fn deb(&self) -> String {
        match self {
            Self::Amd64 => "amd64".to_string(),
            Self::Arm64 => "arm64".to_string(),
            Self::Other(value) => value.clone(),
        }
    }

    pub fn rpm(&self) -> String {
        match self {
            Self::Amd64 => "x86_64".to_string(),
            Self::Arm64 => "aarch64".to_string(),
            Self::Other(value) => value.clone(),
        }
    }

    pub fn arch(&self) -> String {
        self.rpm()
    }
}

/// Alles, was die Paket-Skripte brauchen - ohne Projekt-Sonderfälle.
pub struct PackageContext<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub description: &'a str,
    pub license: &'a str,
    pub url: &'a str,
    pub family: &'a DistroFamily,
    pub strategy: BuildStrategy,
    pub has_pkgbuild: bool,
    pub arch: PackageArch,
    pub build_step: &'a str,
    pub install_step: &'a str,
    pub asset_step: &'a str,
}

/// Debug-Symbole aus installierten Binaries entfernen, wenn `strip` vorhanden ist.
pub fn strip_binaries_step() -> &'static str {
    "strip --strip-unneeded \"$pkgdir\"/usr/bin/* 2>/dev/null || true"
}

/// Sanitized metadata for package manifests. Removes characters that would be
/// interpreted by shells, RPM macros or heredocs.
pub fn metadata_line(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !matches!(ch, '\n' | '\r' | '\'' | '"' | '`' | '$' | '\\' | '%'))
        .collect::<String>()
        .trim()
        .to_string()
}

pub fn line_echo(text: &str) -> String {
    format!("echo '{}'", text.replace('\'', "'\\''"))
}

/// Führt einen zusammengesetzten Shell-Schritt fail-fast aus. `set -e` allein
/// beendet ein Skript bei einem Fehler innerhalb einer `&&`-Kette nicht;
/// deshalb wird der Status der gesamten Kette explizit geprüft.
pub fn run_step(command: &str) -> String {
    format!("if ! ( {command} ); then echo '[build] step failed - aborting' >&2; exit 1; fi")
}

/// Cargo-Build: baut bevorzugt nur die Binaries aus dem Projekt-Packaging-
/// Manifest (`nfpm`). Ohne Manifest bleibt der bisherige Workspace-Build.
pub fn cargo_build_step() -> String {
    r#"mkdir -p /work/dist "${CARGO_TARGET_DIR:-target}/release"
# Upstream-Bootstrap-Projekte (Rust-Compiler & Verwandte) bringen ein eigenes
# Build-System im Wurzelverzeichnis mit (`x.py` + `src/bootstrap`). Sie sind
# kein normales Cargo-Projekt: ein `cargo build` scheitert dort an Nightly-
# Features und fehlender Bootstrap-Konfiguration.
if [ -f x.py ] && [ -d src/bootstrap ]; then
  echo '[build] upstream bootstrap detected (x.py) - using the project build system' >&2
  if [ ! -f bootstrap.toml ] && [ ! -f config.toml ]; then
    ./configure --prefix=/usr --set llvm.download-ci-llvm=true
  fi
  ./x.py build --stage "${GIT2PKG_BOOTSTRAP_STAGE:-1}" || exit 1
  exit 0
fi
bins=""
for manifest in packaging/linux/nfpm.yaml nfpm.yaml packaging/nfpm.yaml; do
  [ -f "$manifest" ] || continue
  bins=$(python3 -c '
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
seen = []
for line in text.splitlines():
    match = re.search(r"src:\s*.*?/usr/bin/([A-Za-z0-9._+-]+)", line)
    if match and match.group(1) not in seen:
        seen.append(match.group(1))
        print(match.group(1))
' "$manifest" 2>/dev/null)
  [ -n "$bins" ] && break
done
build_failed=0
if [ -n "$bins" ]; then
  args=""
  for name in $bins; do args="$args --bin $name"; done
  echo "[build] Cargo bins from packaging manifest: $(echo $bins)"
  if ! cargo build --release $args; then
    echo "[build] selected Cargo bins failed - building the workspace" >&2
    cargo build --release || build_failed=1
  fi
else
  cargo build --release || build_failed=1
fi
# `set -e` ist innerhalb der Fehlerpruefung des Aufrufers abgeschaltet; ein
# fehlgeschlagenes `cargo build` muss deshalb explizit zum Abbruch fuehren,
# sonst laeuft der Installationsschritt ohne Binaries weiter.
if [ "$build_failed" -ne 0 ]; then
  echo '[build] cargo build failed - see the compiler errors above' >&2
  exit 1
fi
for directory in "${CARGO_TARGET_DIR:-target}/release" target/release /work/dist; do
  [ -d "$directory" ] || continue
  find "$directory" -maxdepth 1 -type f -executable ! -name '*.so' ! -name '*.d' ! -name '*.rlib' ! -name 'build-script-*' -exec cp {} /work/dist/ \; 2>/dev/null
done"#
        .to_string()
}

/// Cargo-Staging: wählt zuerst die Binaries des Pakets, dessen Name zum
/// Projekt passt (über `cargo metadata`). Danach greift der Projekt-Präfix,
/// zuletzt der bisherige Fallback auf alle gebauten Binaries.
pub fn cargo_install_step(project_name: &str) -> String {
    let project = project_name.replace('\'', "'\\''");
    format!(
        r#"mkdir -p "$pkgdir/usr/bin"
project='{project}'
selected=0
manifest_bins=""
for manifest in packaging/linux/nfpm.yaml nfpm.yaml packaging/nfpm.yaml; do
  [ -f "$manifest" ] || continue
  manifest_bins=$(python3 -c '
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
seen = []
for line in text.splitlines():
    match = re.search(r"src:\s*.*?/usr/bin/([A-Za-z0-9._+-]+)", line)
    if match and match.group(1) not in seen:
        seen.append(match.group(1))
        print(match.group(1))
' "$manifest" 2>/dev/null)
  [ -n "$manifest_bins" ] && break
done
if [ -n "$manifest_bins" ]; then
  echo "[pkg] packaging manifest binaries: $(echo $manifest_bins)"
  for name in $manifest_bins; do
    for directory in "${{CARGO_TARGET_DIR:-target}}/release" target/release /work/dist; do
      if [ -x "$directory/$name" ]; then
        install -Dm755 "$directory/$name" "$pkgdir/usr/bin/"
        selected=1
        break
      fi
    done
  done
fi
if [ "$selected" -eq 0 ] && command -v python3 >/dev/null 2>&1; then
  bins=$(cargo metadata --no-deps --format-version 1 2>/dev/null | python3 -c '
import json, sys
name = sys.argv[1].lower()
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)
for pkg in data.get("packages", []):
    if pkg.get("name", "").lower() == name:
        for target in pkg.get("targets", []):
            if "bin" in target.get("kind", []):
                print(target.get("name", ""))
' "$project" 2>/dev/null)
  if [ -n "$bins" ]; then
    echo "[pkg] Cargo package '$project' binaries: $(echo $bins)"
    for name in $bins; do
      for directory in "${{CARGO_TARGET_DIR:-target}}/release" target/release /work/dist; do
        if [ -x "$directory/$name" ]; then
          install -Dm755 "$directory/$name" "$pkgdir/usr/bin/"
          selected=1
          break
        fi
      done
    done
  fi
fi
if [ "$selected" -eq 0 ]; then
  matched=0
  for directory in "${{CARGO_TARGET_DIR:-target}}/release" target/release /work/dist; do
    [ -d "$directory" ] || continue
    while IFS= read -r -d '' binary; do
      base=$(basename "$binary")
      case "$base" in
        "$project"|"$project"-*) install -Dm755 "$binary" "$pkgdir/usr/bin/"; matched=1 ;;
      esac
    done < <(find "$directory" -maxdepth 1 -type f -executable ! -name '*.so' ! -name '*.d' ! -name '*.rlib' ! -name 'build-script-*' -print0 2>/dev/null)
  done
  [ "$matched" -eq 1 ] && selected=1
fi
if [ "$selected" -eq 0 ]; then
  for directory in "${{CARGO_TARGET_DIR:-target}}/release" target/release /work/dist; do
    [ -d "$directory" ] || continue
    while IFS= read -r -d '' binary; do
      install -Dm755 "$binary" "$pkgdir/usr/bin/"
    done < <(find "$directory" -maxdepth 1 -type f -executable ! -name '*.so' ! -name '*.d' ! -name '*.rlib' ! -name 'build-script-*' -print0 2>/dev/null)
  done
fi
if [ -z "$(find "$pkgdir/usr/bin" -maxdepth 1 -type f -print -quit 2>/dev/null)" ]; then
  echo "[pkg] no executable Cargo binaries found" >&2
  exit 1
fi"#
    )
}

/// Upstream-Bootstrap-Installation (`x.py install`): nur fuer Projekte mit
/// eigenem Build-System im Wurzelverzeichnis (`x.py` + `src/bootstrap`).
/// Wird dem Cargo-Installationsschritt vorangestellt.
pub fn bootstrap_install_guard() -> &'static str {
    r#"if [ -f x.py ] && [ -d src/bootstrap ]; then
  echo '[pkg] upstream bootstrap detected - installing via x.py' >&2
  DESTDIR="$pkgdir" ./x.py install --stage "${GIT2PKG_BOOTSTRAP_STAGE:-1}" --set install.prefix=/usr
  exit 0
fi
"#
}

/// Erzeugt die Shell-Zeilen zum Schnüren des passenden Systempakets.
pub fn generate(ctx: &PackageContext<'_>) -> Vec<String> {
    match ctx.family {
        DistroFamily::Arch => arch::script(ctx),
        DistroFamily::Debian => debian::script(ctx),
        DistroFamily::Fedora | DistroFamily::Suse => rpm::script(ctx),
        DistroFamily::Unknown => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_step_bricht_bei_fehler_in_und_kette_ab() {
        let step = run_step("cd /work/src && cmake . && cmake --build .");
        assert!(step.starts_with("if ! ( cd /work/src && cmake . && cmake --build . ); then"));
        assert!(step.contains("echo '[build] step failed - aborting' >&2; exit 1; fi"));
    }

    #[test]
    fn cargo_staging_sucht_beide_verzeichnisse() {
        let build = cargo_build_step();
        let install = cargo_install_step("repo");
        assert!(build.contains("packaging/linux/nfpm.yaml"));
        assert!(build.contains("--bin"));
        assert!(build.contains("[build] Cargo bins from packaging manifest"));
        for script in [build.as_str(), install.as_str()] {
            assert!(script.contains("${CARGO_TARGET_DIR:-target}/release"));
            assert!(script.contains("install -Dm755") || build.contains(script));
        }
        assert!(install.contains("/work/dist"));
        assert!(install.contains("${CARGO_TARGET_DIR:-target}/release"));
        assert!(install.contains("[ -d \"$directory\" ] || continue"));
        assert!(install.contains("mkdir -p \"$pkgdir/usr/bin\""));
        assert!(install.contains("install -Dm755"));
        assert!(install.contains("build-script-*"));
        assert!(install.contains("exit 1"));
        assert!(install.contains("project='repo'"));
        assert!(install.contains("packaging/linux/nfpm.yaml"));
        assert!(install.contains("packaging manifest binaries"));
        assert!(install.contains("cargo metadata --no-deps"));
        assert!(install.contains("python3 -c"));
        assert!(install.contains(r#""$project"|"$project"-*"#));
        assert!(install.contains("selected=0"));
        assert!(install.contains("if [ \"$selected\" -eq 0 ]"));
        assert!(install.contains("[ \"$matched\" -eq 1 ]"));
        assert!(strip_binaries_step().contains("--strip-unneeded"));
    }

    #[test]
    fn cargo_build_fehler_bricht_ab() {
        let build = cargo_build_step();
        assert!(build.contains("build_failed=0"));
        assert!(build.contains("cargo build --release || build_failed=1"));
        assert!(build.contains("if [ \"$build_failed\" -ne 0 ]"));
        assert!(build.contains("cargo build failed - see the compiler errors above"));
        assert!(build.contains("./x.py build --stage"));
        assert!(build.contains("|| exit 1"));
    }

    #[test]
    fn bootstrap_projekte_bauen_ueber_x_py() {
        let build = cargo_build_step();
        assert!(build.contains("[ -f x.py ] && [ -d src/bootstrap ]"));
        assert!(build.contains("./x.py build --stage"));
        assert!(build.contains("llvm.download-ci-llvm=true"));
        let guard = bootstrap_install_guard();
        assert!(guard.contains("[ -f x.py ] && [ -d src/bootstrap ]"));
        assert!(guard.contains("./x.py install"));
        assert!(guard.contains("DESTDIR=\"$pkgdir\""));
        assert!(guard.contains("install.prefix=/usr"));
    }

    #[test]
    fn alle_zielformate_erzeugen_syntaktisch_gueltige_paketskripte() {
        let install = "mkdir -p \"$pkgdir/usr/bin\"";
        let asset = "true";
        for family in [
            DistroFamily::Arch,
            DistroFamily::Debian,
            DistroFamily::Fedora,
            DistroFamily::Suse,
        ] {
            let ctx = PackageContext {
                name: "repo",
                version: "1.0.0",
                family: &family,
                strategy: BuildStrategy::Default,
                has_pkgbuild: false,
                arch: PackageArch::Amd64,
                build_step: "true",
                install_step: install,
                asset_step: asset,
                description: "Test package",
                license: "MIT",
                url: "https://example.invalid",
            };
            let script = generate(&ctx).join("\n");
            assert!(!script.trim().is_empty());
            match family {
                DistroFamily::Arch => assert!(script.contains("bsdtar --zstd")),
                DistroFamily::Debian => assert!(script.contains("dpkg-deb --build")),
                DistroFamily::Fedora | DistroFamily::Suse => {
                    assert!(script.contains("rpmbuild"))
                }
                DistroFamily::Unknown => unreachable!(),
            }
            let path = std::env::temp_dir().join(format!(
                "git2pkg-format-{:?}-{}.sh",
                family,
                std::process::id()
            ));
            assert!(
                script.contains("step failed - aborting"),
                "Build-Schritt ist nicht fail-fast fuer {:?}",
                family
            );
            std::fs::write(&path, format!("{}\n", script)).unwrap();
            let ok = std::process::Command::new("bash")
                .arg("-n")
                .arg(&path)
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            let _ = std::fs::remove_file(&path);
            assert!(ok, "ungueltiges Paketskript fuer {:?}", family);
        }
    }

    #[test]
    fn nur_64_bit_architekturen_sind_erlaubt() {
        assert!(PackageArch::Amd64.is_supported());
        assert!(PackageArch::Arm64.is_supported());
        for unsupported in ["i386", "i686", "x86", "armv7l"] {
            let arch = PackageArch::Other(unsupported.to_string());
            assert!(!arch.is_supported(), "{unsupported}");
            assert_eq!(arch.unsupported_label(), unsupported);
        }
    }

    #[test]
    fn architektur_wird_gemapped() {
        assert_eq!(PackageArch::Amd64.deb(), "amd64");
        assert_eq!(PackageArch::Amd64.rpm(), "x86_64");
        assert_eq!(PackageArch::Arm64.deb(), "arm64");
        assert_eq!(PackageArch::Arm64.rpm(), "aarch64");
        assert!(!PackageArch::detect().deb().is_empty());
    }
}
