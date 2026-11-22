//! Debian/Ubuntu/Mint: `.deb` mit echter `Depends`-Ermittlung.

use super::{line_echo, PackageContext};

pub fn script(ctx: &PackageContext<'_>) -> Vec<String> {
    let mut lines = vec![line_echo("[pkg] building Debian package")];
    lines.push(super::run_step(&format!(
        "cd /work/src && {}",
        ctx.build_step
    )));
    lines.push(super::run_step(&format!(
        "cd /work/src && pkgdir=/work/out && {} && {}",
        ctx.install_step, ctx.asset_step
    )));
    lines.push("mkdir -p /work/deb/DEBIAN /work/debian".to_string());
    lines.push(format!(
        "cat > /work/debian/control <<CONTROL_EOF\nSource: {name}\nPackage: {name}\nVersion: {version}\nSection: utils\nPriority: optional\nArchitecture: {arch}\nMaintainer: git2pkg <noreply@example.invalid>\nDescription: Package built by git2pkg\nCONTROL_EOF",
        name = ctx.name,
        version = ctx.version,
        arch = ctx.arch.deb(),
    ));
    lines.push(
        r#"depends=""
for binary in $(find /work/out/usr/bin /work/out/usr/lib /work/out/usr/lib/* -type f -perm -u+x 2>/dev/null); do
  value=$(cd /work && dpkg-shlibdeps -O -e "$binary" 2>/dev/null | sed 's/^shlibs:Depends=//' || true)
  if [ -n "$value" ]; then
    if [ -n "$depends" ]; then depends="$depends, "; fi
    depends="$depends$value"
  fi
done"#.to_string(),
    );
    lines.push(format!(
        "{{ echo 'Package: {name}'; echo 'Version: {version}'; echo 'Section: utils'; echo 'Priority: optional'; echo 'Architecture: {arch}'; if [ -n \"$depends\" ]; then echo \"Depends: $depends\"; fi; echo 'Maintainer: git2pkg <noreply@example.invalid>'; echo 'Description: Package built by git2pkg'; }} > /work/deb/DEBIAN/control",
        name = ctx.name,
        version = ctx.version,
        arch = ctx.arch.deb(),
    ));
    lines.push("mkdir -p /work/deb/usr/bin /work/deb/usr/lib /work/deb/usr/share".to_string());
    lines.push("cp -a /work/out/. /work/deb/".to_string());
    lines.push(
        "pkgdir=/work/deb && strip --strip-unneeded \"$pkgdir\"/usr/bin/* 2>/dev/null || true"
            .to_string(),
    );
    lines.push(format!(
        "fakeroot dpkg-deb --build /work/deb /work/dist/{}_{}_{}.deb",
        ctx.name,
        ctx.version,
        ctx.arch.deb()
    ));
    lines.push("rm -f /work/dist/*-dbgsym*.deb".to_string());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::container::BuildStrategy;
    use crate::core::distro::DistroFamily;
    use crate::core::packaging::PackageArch;

    #[test]
    fn control_enthaelt_dynamic_depends() {
        let family = DistroFamily::Debian;
        let ctx = PackageContext {
            name: "repo",
            version: "1.0.0",
            family: &family,
            strategy: BuildStrategy::Default,
            has_pkgbuild: false,
            arch: PackageArch::Amd64,
            build_step: "true",
            install_step: "true",
            asset_step: "true",
        };
        let script = script(&ctx).join("\n");
        assert!(script.contains("Source: repo"));
        assert!(script.contains("dpkg-shlibdeps -O"));
        assert!(script.contains("if [ -n \"$depends\" ]"));
        assert!(script.contains("Depends: $depends"));
        assert!(script.contains("Architecture: amd64"));
        assert!(script.contains("--strip-unneeded"));
    }

    #[test]
    fn cargo_install_step_beruecksichtigt_cargo_target_dir() {
        let family = DistroFamily::Debian;
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
    }
}
