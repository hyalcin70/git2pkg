//! Fedora/RHEL/openSUSE: `.rpm` mit AutoReqProv und sauberer Dateiliste.

use super::{line_echo, PackageContext};

pub fn script(ctx: &PackageContext<'_>) -> Vec<String> {
    let mut lines = vec![line_echo("[pkg] building RPM package")];
    lines.push(super::run_step(&format!(
        "cd /work/src && {}",
        ctx.build_step
    )));
    lines.push(super::run_step(&format!(
        "cd /work/src && pkgdir=/work/out && {} && {}",
        ctx.install_step, ctx.asset_step
    )));
    lines.push("mkdir -p /root/rpmbuild/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}".to_string());
    lines.push(format!(
        "cat > /root/rpmbuild/SPECS/{name}.spec <<'SPEC_EOF'\n%define debug_package %{{nil}}\nName: {name}\nVersion: {version}\nRelease: 1\nSummary: Built by git2pkg-ai\nLicense: GPL-3.0-or-later\nBuildArch: {arch}\nAutoReqProv: yes\n%description\nPackage built by git2pkg-ai.\n%install\nmkdir -p %{{buildroot}}/usr/bin %{{buildroot}}/usr/lib %{{buildroot}}/usr/share\ncp -a /work/out/. %{{buildroot}}/\nstrip --strip-unneeded %{{buildroot}}/usr/bin/* 2>/dev/null || true\nfind %{{buildroot}} -name '*.la' -delete\n[ -x /usr/lib/rpm/brp-compress ] && /usr/lib/rpm/brp-compress || true\n(cd %{{buildroot}} && find . \\( -type f -o -type l \\) | sed 's|^\\.||') > /tmp/package-files.list\n%files -f /tmp/package-files.list\n%defattr(-,root,root,-)\nSPEC_EOF",
        name = ctx.name,
        version = ctx.version,
        arch = ctx.arch.rpm(),
    ));
    lines.push(format!(
        "QA_RPATHS=0x0001 rpmbuild -bb --clean --define \"_topdir /root/rpmbuild\" /root/rpmbuild/SPECS/{}.spec",
        ctx.name
    ));
    lines.push(
        [
            "rpm_file=$(find /root/rpmbuild/RPMS /usr/src/packages/RPMS -type f -name \"*.rpm\" 2>/dev/null | head -n 1 || true)",
            "if [ -z \"$rpm_file\" ]; then",
            "  echo \"[pkg] no RPM file found\" >&2",
            "  find /root/rpmbuild/RPMS /usr/src/packages/RPMS -maxdepth 3 -print 2>/dev/null || true",
            "  exit 1",
            "fi",
            "cp \"$rpm_file\" /work/dist/",
        ]
        .join("\n"),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::container::BuildStrategy;
    use crate::core::distro::DistroFamily;
    use crate::core::packaging::PackageArch;

    #[test]
    fn spec_enthaelt_autoreqprov_und_defattr() {
        for family in [DistroFamily::Fedora, DistroFamily::Suse] {
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
            assert!(script.contains("AutoReqProv: yes"));
            assert!(script.contains("QA_RPATHS=0x0001"));
            assert!(script.contains("brp-compress"));
            assert!(script.contains("%defattr(-,root,root,-)"));
            assert!(script.contains("%files -f /tmp/package-files.list"));
            assert!(script.contains("--strip-unneeded"));
            assert!(script.contains("> /tmp/package-files.list"));
        }
    }

    #[test]
    fn cargo_install_step_beruecksichtigt_cargo_target_dir() {
        let family = DistroFamily::Fedora;
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

    #[test]
    fn rpm_script_setzt_topdir_und_finde_rpm() {
        for family in [DistroFamily::Fedora, DistroFamily::Suse] {
            let ctx = PackageContext {
                name: "sample-tool",
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
            assert!(script
                .contains("mkdir -p /root/rpmbuild/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}"));
            assert!(script.contains("--define \"_topdir /root/rpmbuild\""));
            assert!(script.contains("find /root/rpmbuild/RPMS /usr/src/packages/RPMS -type f -name \"*.rpm\" 2>/dev/null | head -n 1"));
            assert!(script.contains("cp \"$rpm_file\" /work/dist/"));
            assert!(!script.contains("cp \"$HOME\"/rpmbuild/RPMS/*/*.rpm"));
        }
    }
}
