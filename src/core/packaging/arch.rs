//! Arch/CachyOS: `.pkg.tar.zst` über `makepkg`.

use super::{line_echo, PackageContext};

pub fn script(ctx: &PackageContext<'_>) -> Vec<String> {
    let mut lines = vec![
        line_echo("[pkg] building Arch package"),
        "id builder >/dev/null 2>&1 || useradd -m -s /bin/bash builder".to_string(),
        "echo 'builder ALL=(ALL) NOPASSWD: /usr/bin/pacman' > /etc/sudoers.d/git2pkg-ai-builder && chmod 440 /etc/sudoers.d/git2pkg-ai-builder".to_string(),
    ];

    if ctx.has_pkgbuild
        && ctx.strategy != crate::core::container::BuildStrategy::DirectStagingFallback
    {
        lines.extend([
            line_echo("[pkgbuild] using existing PKGBUILD directly from the repository (makepkg in /work/src)"),
            "rm -f /work/src/*.pkg.tar.zst".to_string(),
            "mkdir -p /work/cargo-target /work/cargo-home /work/ccache".to_string(),
            "chown -R builder:builder /work/src /work/dist /work/cargo-target /work/cargo-home /work/ccache".to_string(),
            "su builder -c 'export CARGO_TARGET_DIR=/work/cargo-target; export CARGO_HOME=/work/cargo-home; export CCACHE_DIR=/work/ccache; cd /work/src && makepkg -s --noconfirm'".to_string(),
            "rm -f /work/src/*-debug-*.pkg.tar.zst".to_string(),
            "cp /work/src/*.pkg.tar.zst /work/dist/".to_string(),
        ]);
        return lines;
    }

    // Generischer Weg: erst bauen und nach /work/out stagen, dann Laufzeit-
    // abhängigkeiten aus den erzeugten Binaries ermitteln und makepkg aufrufen.
    lines.push(format!("cd /work/src && {}", ctx.build_step));
    lines.push(format!(
        "cd /work/src && pkgdir=/work/out && {} && {}",
        ctx.install_step, ctx.asset_step
    ));
    lines.push(
        r#"depends=()
for binary in $(find /work/out/usr/bin /work/out/usr/lib /work/out/usr/lib64 -type f -perm -u+x 2>/dev/null); do
  for lib in $(ldd "$binary" 2>/dev/null | awk '/=> \// {print $3}'); do
    package=$(pacman -Qoq "$lib" 2>/dev/null || true)
    [ -n "$package" ] && depends+=("$package")
  done
done
mapfile -t depends < <(printf '%s\n' "${depends[@]}" | sort -u)
dep_line=""
for package in "${depends[@]}"; do
  dep_line="$dep_line $package"
done"#.to_string(),
    );
    lines.push("mkdir -p /work/pkg".to_string());
    let install_body = if ctx.is_cargo {
        format!(
            "mkdir -p \"$pkgdir\"\n  cp -a /work/out/. \"$pkgdir\"/ 2>/dev/null || true\n  cd /work/src\n  {}",
            super::cargo_install_step()
        )
    } else {
        "mkdir -p \"$pkgdir\" \"$pkgdir/usr/bin\" \"$pkgdir/usr/lib\" \"$pkgdir/usr/share\"\n  cp -a /work/out/. \"$pkgdir\"/".to_string()
    };
    let install_body = format!("{}\n  {}", install_body, super::strip_binaries_step());
    let build_body = if ctx.build_step.contains("cmake") {
        "set -eo pipefail\n  true || return 1"
    } else {
        "true"
    };
    lines.push(format!(
        "cat > /work/pkg/PKGBUILD <<'PKGBUILD_EOF'\npkgname={name}\npkgver={version}\npkgrel=1\npkgdesc='Built by git2pkg-ai'\narch=('{arch}')\nlicense=('GPL-3.0-or-later')\noptions=('!strip' '!debug')\ndepends=(${{dep_line}})\nbuild() {{\n  {build_body}\n}}\npackage() {{\n  {install_body}\n}}\nPKGBUILD_EOF",
        name = ctx.name,
        version = ctx.version,
        arch = ctx.arch.arch(),
        build_body = build_body,
        install_body = install_body,
    ));
    lines.push("chown -R builder:builder /work/pkg /work/src /work/out /work/dist".to_string());
    lines.push("su builder -c 'cd /work/pkg && makepkg -s --noconfirm'".to_string());
    lines.push("rm -f /work/pkg/*-debug-*.pkg.tar.zst".to_string());
    lines.push("cp /work/pkg/*.pkg.tar.zst /work/dist/".to_string());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::container::BuildStrategy;
    use crate::core::distro::DistroFamily;
    use crate::core::packaging::PackageArch;

    #[test]
    fn in_tree_pkgbuild_setzt_cargo_target_dir() {
        let family = DistroFamily::Arch;
        let ctx = PackageContext {
            name: "repo",
            version: "1.0.0",
            family: &family,
            strategy: BuildStrategy::Default,
            has_pkgbuild: true,
            arch: PackageArch::Amd64,
            build_step: "true",
            install_step: "true",
            is_cargo: true,
            asset_step: "true",
        };
        let script = script(&ctx).join("\n");
        assert!(script.contains("export CARGO_TARGET_DIR=/work/cargo-target"));
        assert!(script.contains("makepkg -s --noconfirm"));
    }

    #[test]
    fn eigenes_pkgbuild_nutzt_cargo_target_dir() {
        let pkgbuild = include_str!("../../../packaging/PKGBUILD");
        assert!(pkgbuild.contains("${CARGO_TARGET_DIR:-target}/release"));
        assert!(!pkgbuild.contains("install -Dm755 \"target/release"));
    }

    #[test]
    fn generierte_pkgbuild_enthaelt_depends() {
        let family = DistroFamily::Arch;
        let ctx = PackageContext {
            name: "repo",
            version: "1.0.0",
            family: &family,
            strategy: BuildStrategy::Default,
            has_pkgbuild: false,
            arch: PackageArch::Amd64,
            build_step: "true",
            install_step: "true",
            is_cargo: false,
            asset_step: "true",
        };
        let script = script(&ctx).join("\n");
        assert!(script.contains("pacman -Qoq"));
        assert!(script.contains("depends=(${dep_line})"));
        assert!(script.contains("ldd"));
    }

    #[test]
    fn cmake_pkgbuild_bricht_build_block_schnell_ab() {
        let family = DistroFamily::Arch;
        let ctx = PackageContext {
            name: "repo",
            version: "1.0.0",
            family: &family,
            strategy: BuildStrategy::Default,
            has_pkgbuild: false,
            arch: PackageArch::Amd64,
            build_step: "cmake -S . -B build && cmake --build build",
            install_step: "true",
            is_cargo: false,
            asset_step: "true",
        };
        let script = script(&ctx).join("\n");
        assert!(script.contains("set -eo pipefail\n  true || return 1"));
        assert!(script.contains("build() {\n  set -eo pipefail"));
    }

    #[test]
    fn cargo_package_schritt_installiert_binaries_aus_beiden_ordnern() {
        let family = DistroFamily::Arch;
        let ctx = PackageContext {
            name: "repo",
            version: "1.0.0",
            family: &family,
            strategy: BuildStrategy::Default,
            has_pkgbuild: false,
            arch: PackageArch::Amd64,
            build_step: "true",
            install_step: crate::core::packaging::cargo_install_step(),
            is_cargo: true,
            asset_step: "true",
        };
        let script = script(&ctx).join("\n");
        assert!(script.contains("${CARGO_TARGET_DIR:-target}/release"));
        assert!(script.contains("[ -d \"$directory\" ] || continue"));
        assert!(script.contains("/work/dist"));
        assert!(script.contains("install -Dm755"));
        assert!(script.contains("exit 1"));
        assert!(script.contains("--strip-unneeded"));
    }
}
