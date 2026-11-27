//! Debian/Ubuntu/Mint: `.deb` mit echter `Depends`-Ermittlung.

use super::{line_echo, metadata_line, PackageContext};

pub fn script(ctx: &PackageContext<'_>) -> Vec<String> {
    let mut lines = vec![line_echo("[pkg] building Debian package")];
    // Bringt das Projekt seine eigene Debian-Paketierung mit, uebernimmt
    // `dpkg-buildpackage` mit *seinem* Rezept: Dateilayout, Abhaengigkeiten und
    // Doku-Regeln kommen dann von dh statt von unseren generierten Schritten.
    // Schlaegt das fehl (z. B. fehlende Build-Depends), laeuft der generische
    // Weg unveraendert weiter.
    let name = ctx.name.replace('\'', "'\\''");
    let ok_marker = crate::core::container::OK_MARKER;
    lines.push(format!(
        r#"if [ -f /work/src/debian/control ] && [ -f /work/src/debian/rules ]; then
  echo '[pkg] in-tree Debian packaging detected - using dpkg-buildpackage' >&2
  package='{name}'
  if ( cd /work/src && dpkg-buildpackage -b -us -uc -tc >/dev/null 2>&1 ); then
    for candidate in /work/*.deb; do
      case "$candidate" in *-dbgsym_*|*-dbg_*|*.ddeb) continue;; esac
      case "$(basename "$candidate")" in
        "$package"_*)
          if dpkg-deb -c "$candidate" 2>/dev/null | grep -qE 'usr/(bin|lib|lib64)/'; then
            cp "$candidate" /work/dist/
            echo "[pkg] native package: $(basename "$candidate")" >&2
            echo '{ok_marker}'
            exit 0
          fi
          echo '__GIT2PKG_EMPTY_PACKAGE__' >&2
          exit 1
          ;;
      esac
    done
    for candidate in /work/*.deb; do
      case "$candidate" in *-dbgsym_*|*-dbg_*|*.ddeb) continue;; esac
      if dpkg-deb -c "$candidate" 2>/dev/null | grep -qE 'usr/(bin|lib|lib64)/'; then
        cp "$candidate" /work/dist/
        echo "[pkg] native package: $(basename "$candidate")" >&2
        echo '{ok_marker}'
        exit 0
      fi
    done
  fi
  echo '[pkg] in-tree packaging failed - using the generated build' >&2
fi"#
    ));
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
        "cat > /work/debian/control <<CONTROL_EOF\nSource: {name}\nPackage: {name}\nVersion: {version}\nSection: utils\nPriority: optional\nArchitecture: {arch}\nMaintainer: git2pkg <noreply@example.invalid>\nHomepage: {url}\nDescription: {description}\nCONTROL_EOF",
        name = ctx.name,
        version = ctx.version,
        arch = ctx.arch.deb(),
        url = metadata_line(ctx.url),
        description = metadata_line(ctx.description),
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
        "{{ echo 'Package: {name}'; echo 'Version: {version}'; echo 'Section: utils'; echo 'Priority: optional'; echo 'Architecture: {arch}'; if [ -n \"$depends\" ]; then echo \"Depends: $depends\"; fi; echo 'Maintainer: git2pkg <noreply@example.invalid>'; echo 'Homepage: {url}'; echo 'Description: {description}'; }} > /work/deb/DEBIAN/control",
        name = ctx.name,
        version = ctx.version,
        arch = ctx.arch.deb(),
        url = metadata_line(ctx.url),
        description = metadata_line(ctx.description),
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
    fn eigenes_debian_verzeichnis_nutzt_dpkg_buildpackage() {
        let ctx = PackageContext {
            name: "example-app",
            version: "1.0.0",
            family: &DistroFamily::Debian,
            strategy: BuildStrategy::Default,
            has_pkgbuild: false,
            arch: PackageArch::Amd64,
            description: "Example application",
            license: "MIT",
            url: "https://example.invalid/app",
            build_step: "true",
            install_step: "true",
            asset_step: "true",
        };
        let script = script(&ctx).join("\n");
        assert!(script.contains("[ -f /work/src/debian/control ]"));
        assert!(script.contains("dpkg-buildpackage -b -us -uc -tc"));
        assert!(script.contains("in-tree packaging failed - using the generated build"));
        assert!(script.contains("cp \"$candidate\" /work/dist/"));
        assert!(script.contains("*-dbgsym_*|*-dbg_*|*.ddeb"));
    }

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
            description: "Test package",
            license: "MIT",
            url: "https://example.invalid",
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
        let install = crate::core::packaging::cargo_install_step("repo");
        let ctx = PackageContext {
            name: "repo",
            version: "1.0.0",
            family: &family,
            strategy: BuildStrategy::Default,
            has_pkgbuild: false,
            arch: PackageArch::Amd64,
            build_step: "true",
            install_step: &install,
            asset_step: "true",
            description: "Test package",
            license: "MIT",
            url: "https://example.invalid",
        };
        let script = script(&ctx).join("\n");
        assert!(script.contains("${CARGO_TARGET_DIR:-target}/release"));
        assert!(script.contains("[ -d \"$directory\" ] || continue"));
    }
}
