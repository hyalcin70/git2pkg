#!/usr/bin/env bash
# Build a native RPM from the current checkout.
# Usage: packaging/build-rpm.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(sed -nE 's/^version = "([^"]+)"/\1/p' "$root/Cargo.toml" | head -n 1)"
[ -n "$version" ] || { echo "cannot read version from Cargo.toml" >&2; exit 1; }

rpm_version="${version//-alpha./~alpha.}"
rpm_version="${rpm_version//-beta./~beta.}"
rpm_version="${rpm_version//-rc./~rc.}"

topdir="${RPM_TOPDIR:-$HOME/rpmbuild}"
mkdir -p "$topdir"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
tarball="$topdir/SOURCES/git2pkg-$rpm_version.tar.gz"

if [ -n "$(git -C "$root" status --porcelain)" ]; then
  echo "warning: the working tree has uncommitted changes; git archive uses HEAD" >&2
fi
git -C "$root" archive --format=tar.gz \
  --prefix="git2pkg-$rpm_version/" \
  -o "$tarball" HEAD

rpmbuild -bb \
  --define "_topdir $topdir" \
  --define "git2pkg_version $rpm_version" \
  "$root/packaging/git2pkg.spec"

echo
echo "RPM written to: $topdir/RPMS"
find "$topdir/RPMS" -type f -name 'git2pkg-*.rpm' -print
