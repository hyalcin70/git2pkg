//! Arch/CachyOS: `.pkg.tar.zst` über `makepkg`.

use super::{line_echo, PackageContext};

pub fn script(ctx: &PackageContext<'_>) -> Vec<String> {
    let mut lines = vec![
        line_echo("[pkg] building Arch package"),
        "id builder >/dev/null 2>&1 || useradd -m -s /bin/bash builder".to_string(),
        "echo 'builder ALL=(ALL) NOPASSWD: /usr/bin/pacman' > /etc/sudoers.d/git2pkg-builder && chmod 440 /etc/sudoers.d/git2pkg-builder".to_string(),
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
    lines.push(super::run_step(&format!(
        "cd /work/src && {}",
        ctx.build_step
    )));
    lines.push(super::run_step(&format!(
        "cd /work/src && pkgdir=/work/out && {} && {}",
        ctx.install_step, ctx.asset_step
    )));
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
    // Direkte Paketierung: Quellen sind bereits gebaut und nach /work/out
    // gestagt. Ein .pkg.tar.zst mit .PKGINFO/.MTREE ist deterministisch und
    // braucht weder makepkg noch fakeroot.
    let version = ctx.version.replace('-', "_");
    lines.push("mkdir -p /work/pkg /work/dist".to_string());
    lines.push(format!(
        r#"installed_size=$(du -sb /work/out 2>/dev/null | cut -f1)
cat > /work/out/.PKGINFO <<PKGINFO_EOF
pkgname = {name}
pkgbase = {name}
pkgver = {version}-1
pkgdesc = Built by git2pkg
url = https://example.invalid
builddate = $(date +%s)
packager = git2pkg <noreply@example.invalid>
size = $installed_size
arch = {arch}
license = GPL-3.0-or-later
PKGINFO_EOF
for dep in $dep_line; do echo "depend = $dep" >> /work/out/.PKGINFO; done
(cd /work/out && bsdtar -czf .MTREE --format=mtree --options="!all,use-set,type,uid,gid,mode,time,size,md5,sha256,link" .PKGINFO usr)
(cd /work/out && bsdtar --zstd -cf "/work/dist/{name}-{version}-1-{arch}.pkg.tar.zst" .PKGINFO .MTREE usr)
rm -f /work/out/.PKGINFO /work/out/.MTREE"#,
        name = ctx.name,
        version = version,
        arch = ctx.arch.arch(),
    ));
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
    fn generiertes_paket_enthaelt_pkginfo_und_depends() {
        let family = DistroFamily::Arch;
        let ctx = PackageContext {
            name: "repo",
            version: "1.0.0-beta.1",
            family: &family,
            strategy: BuildStrategy::Default,
            has_pkgbuild: false,
            arch: PackageArch::Amd64,
            build_step: "true",
            install_step: "true",
            asset_step: "true",
        };
        let script = script(&ctx).join("\n");
        assert!(script.contains("pacman -Qoq"));
        assert!(script.contains("ldd"));
        assert!(script.contains(".PKGINFO"));
        assert!(script.contains("depend = $dep"));
        assert!(script.contains("bsdtar --zstd"));
        assert!(script.contains("pkgver = 1.0.0_beta.1-1"));
        assert!(!script.contains("makepkg -s --noconfirm"));
    }

    #[test]
    fn cmake_buildschritt_ist_fail_fast() {
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
            asset_step: "true",
        };
        let script = script(&ctx).join("\n");
        assert!(script.contains("if ! ( cd /work/src && cmake -S . -B build"));
        assert!(script.contains("step failed - aborting"));
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
            asset_step: "true",
        };
        let script = script(&ctx).join("\n");
        assert!(script.contains("${CARGO_TARGET_DIR:-target}/release"));
        assert!(script.contains("[ -d \"$directory\" ] || continue"));
        assert!(script.contains("/work/dist"));
        assert!(script.contains("install -Dm755"));
        assert!(script.contains("exit 1"));
    }
}
