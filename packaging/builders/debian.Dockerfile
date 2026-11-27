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
