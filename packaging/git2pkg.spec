%global debug_package %{nil}

Name:           git2pkg
Version:        %{?git2pkg_version}%{!?git2pkg_version:0.2.0~alpha.35}
Release:        1%{?dist}
Summary:        Isolated package builder from Git repositories
License:        GPL-3.0-or-later
URL:            https://github.com/hyalcin70/git2pkg
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  pkgconfig(alsa)
BuildRequires:  pkgconfig(atk)
BuildRequires:  pkgconfig(gl)
BuildRequires:  pkgconfig(gtk+-3.0)
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(x11)
BuildRequires:  pkgconfig(xcb)
BuildRequires:  pkgconfig(xcursor)
BuildRequires:  pkgconfig(xi)
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(xrandr)

%description
git2pkg builds a Git repository inside a disposable Podman or Docker
container and produces a native package or a standalone binary. The host
system does not need a compiler toolchain or build libraries.

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --release --locked

%install
install -Dm755 target/release/git2pkg %{buildroot}%{_bindir}/git2pkg
install -Dm644 packaging/git2pkg.desktop %{buildroot}%{_datadir}/applications/git2pkg.desktop
install -Dm644 assets/app_icon.png %{buildroot}%{_datadir}/icons/hicolor/256x256/apps/git2pkg.png
install -Dm644 assets/app_icon.png %{buildroot}%{_datadir}/pixmaps/git2pkg.png

%files
%{_bindir}/git2pkg
%{_datadir}/applications/git2pkg.desktop
%{_datadir}/icons/hicolor/256x256/apps/git2pkg.png
%{_datadir}/pixmaps/git2pkg.png
%doc README.md
%license LICENSE

%changelog
* Fri Nov 27 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.35-1
- Optional prebuilt builder images per distribution (git2pkg-builder:*).
- Native packaging through the project's own debian/ or *.spec.
- CMake installs under /usr; versions from git tags; license from LICENSE files.
- Clear failure diagnoses, in-run toolchain heal, shallow submodules.
- Only direct DT_NEEDED libraries as Arch dependencies.

* Tue Nov 24 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.34-1
- Build only the Cargo binaries selected by the project packaging manifest.
- Document that source-archive input is deferred to beta.1.

* Mon Nov 23 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.33-1
- Select Cargo application binaries from project packaging manifests and Cargo metadata.
- Read description, license and URL from project manifests and stage documentation/license files.
- Compress Arch packages with zstd level 19.

* Mon Nov 23 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.32-1
- Prefer project-named Cargo binaries when staging a package.
- Add curated release notes with a generated fallback.

* Mon Nov 23 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.31-1
- Validate the selected container engine and show concise installation hints.
- Localize the support-scope information box in all five READMEs.

* Mon Nov 23 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.30-1
- Reject packages whose desktop entry references a missing executable.
- Refresh cached repositories to the remote default branch before reuse.

* Mon Nov 23 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.29-1
- Workspace/log retry fixes, distro-aware build scripts, pre-install verification and early ABI diagnosis.

* Mon Nov 23 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.28-1
- CMake finder, context-aware dependency and prefix fixes; document the supported scope.

* Mon Nov 23 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.27-1
- Remove the misleading build progress percentage from the GUI.

* Sun Nov 22 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.26-1
- Native RPM package for 0.2.0-alpha.26.
