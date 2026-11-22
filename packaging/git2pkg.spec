%global debug_package %{nil}

Name:           git2pkg
Version:        %{?git2pkg_version}%{!?git2pkg_version:0.2.0~alpha.27}
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
* Mon Nov 23 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.27-1
- Remove the misleading build progress percentage from the GUI.

* Sun Nov 22 2026 Hakan Yalçın <hyalcin70@github.com> - 0.2.0~alpha.26-1
- Native RPM package for 0.2.0-alpha.26.
