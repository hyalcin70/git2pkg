# Builder-Image fuer openSUSE-Ziele.
FROM opensuse/tumbleweed:latest
RUN zypper --non-interactive install \
      git pkgconf-pkg-config python3 python3-pip gcc gcc-c++ make cmake meson ninja \
      rpm-build curl xz zlib-devel libopenssl-devel \
      cargo rust go \
 && zypper clean -a
