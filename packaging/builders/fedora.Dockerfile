# Builder-Image fuer Fedora/RHEL-Ziele.
FROM fedora:latest
RUN dnf install -y @development-tools gcc gcc-c++ make git pkgconf-pkg-config \
      python3-pip cmake meson ninja-build \
      rpm-build rpmdevtools dnf-plugins-core \
      curl xz zlib-devel openssl-devel \
      cargo rust golang \
 && dnf clean all
