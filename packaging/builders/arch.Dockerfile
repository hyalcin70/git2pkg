# Builder-Image fuer Arch-basierte Ziele.
#
# Enthaelt die komplette Toolchain, damit git2pkg im Build keine Pakete mehr
# nachinstallieren muss: deterministisch, offline-faehig und deutlich schneller.
FROM archlinux:latest
RUN pacman -Syu --noconfirm --needed \
      base-devel git pkgconf python python-pip cmake meson ninja \
      rust go \
 && pacman -Scc --noconfirm
