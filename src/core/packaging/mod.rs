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
    pub family: &'a DistroFamily,
    pub strategy: BuildStrategy,
    pub has_pkgbuild: bool,
    pub arch: PackageArch,
    pub build_step: &'a str,
    pub install_step: &'a str,
    /// Cargo-Projekte brauchen die robuste Binary-Übernahme im package()-Schritt.
    pub is_cargo: bool,
    pub asset_step: &'a str,
}

pub fn line_echo(text: &str) -> String {
    format!("echo '{}'", text.replace('\'', "'\\''"))
}

/// Cargo-Build: nur echte ausführbare Artefakte nach `/work/dist` kopieren.
pub fn cargo_build_step() -> &'static str {
    "mkdir -p /work/dist /work/cargo-target && cargo build --release && find /work/cargo-target/release target/release -maxdepth 1 -type f -executable ! -name '*.so' ! -name '*.d' ! -name '*.rlib' ! -name 'build-script-*' -exec cp {} /work/dist/ \\;"
}

/// Cargo-Staging: Binaries aus `target/release/` UND `/work/dist/` übernehmen.
pub fn cargo_install_step() -> &'static str {
    r#"mkdir -p "$pkgdir/usr/bin"
for directory in /work/cargo-target/release target/release /work/dist; do
  [ -d "$directory" ] || continue
  while IFS= read -r -d '' binary; do
    install -Dm755 "$binary" "$pkgdir/usr/bin/"
  done < <(find "$directory" -maxdepth 1 -type f -executable ! -name '*.so' ! -name '*.d' ! -name '*.rlib' ! -name 'build-script-*' -print0 2>/dev/null)
done
if [ -z "$(find "$pkgdir/usr/bin" -maxdepth 1 -type f -print -quit 2>/dev/null)" ]; then
  echo "[pkg] Keine ausführbaren Cargo-Binaries gefunden" >&2
  exit 1
fi"#
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
    fn cargo_staging_sucht_beide_verzeichnisse() {
        let build = cargo_build_step();
        let install = cargo_install_step();
        for script in [build, install] {
            assert!(script.contains("target/release"));
            assert!(script.contains("install -Dm755") || build.contains(script));
        }
        assert!(install.contains("/work/dist"));
        assert!(install.contains("/work/cargo-target/release"));
        assert!(install.contains("mkdir -p \"$pkgdir/usr/bin\""));
        assert!(install.contains("install -Dm755"));
        assert!(install.contains("build-script-*"));
        assert!(install.contains("exit 1"));
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
