# Builder-Image fuer Debian/Ubuntu/Mint-Ziele.
FROM debian:stable
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential git pkg-config python3-pip cmake meson ninja-build \
      debhelper devscripts fakeroot dpkg-dev \
      curl ca-certificates xz-utils zlib1g-dev libssl-dev \
      cargo rustc golang \
 && rm -rf /var/lib/apt/lists/*
# Aktuelle stabile Toolchain zusaetzlich bereithalten: Projekte oberhalb der
# Distro-MSRV bauen damit ohne Download im Build (git2pkg nutzt sie nur dann).
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain stable --profile minimal
