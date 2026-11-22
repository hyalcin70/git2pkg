#!/usr/bin/env bash
# git2pkg-ai - interactive installation script.
# No hidden directories are used without asking the user first.
set -euo pipefail

BOLD=$'\033[1m'
RESET=$'\033[0m'
SOURCE="target/release/git2pkg-ai"
DESKTOP_DIR="$HOME/.local/share/applications"
DESKTOP_FILE="$DESKTOP_DIR/git2pkg-ai.desktop"
ICON_DIR="$HOME/.local/share/icons/hicolor/256x256/apps"
ICON_FILE="$ICON_DIR/git2pkg-ai.png"
ASSET_ICON="assets/app_icon.png"

say() { printf '%s\n' "$*"; }
bold() { printf '%s%s%s\n' "$BOLD" "$1" "$RESET"; }
is_yes() {
  case "${1,,}" in
    j|ja|y|yes) return 0 ;;
    *) return 1 ;;
  esac
}

if [[ ! -f Cargo.toml ]]; then
  echo "Error: please run this script from the git2pkg-ai project directory." >&2
  exit 1
fi

say "Building git2pkg-ai in release mode …"
if ! cargo build --release; then
  echo "The build failed - nothing has been installed." >&2
  exit 1
fi

if [[ ! -f "$SOURCE" ]]; then
  echo "Error: $SOURCE was not found." >&2
  exit 1
fi

say ""
say "Where should the finished git2pkg-ai binary be copied?"
say "  1) Right here in the project folder     -> ./git2pkg-ai               (default)"
say "  2) Visible in your home folder          -> ~/Applications/git2pkg-ai"
say "  3) Directly on the Desktop              -> ~/Desktop/git2pkg-ai"
say "  4) Custom path"
printf "Selection [1-4, default 1]: "
read -r choice
choice=${choice:-1}

case "$choice" in
  1) TARGET="$PWD/git2pkg-ai" ;;
  2) TARGET="$HOME/Applications/git2pkg-ai" ;;
  3) TARGET="$HOME/Desktop/git2pkg-ai" ;;
  4)
    printf "Please enter the destination path (a leading ~/ is allowed): "
    read -r custom
    if [[ -z "$custom" ]]; then
      echo "No path entered - aborted." >&2
      exit 1
    fi
    case "$custom" in
      "~/"*) TARGET="$HOME/${custom#\~/}" ;;
      "~")   TARGET="$HOME" ;;
      *)     TARGET="$custom" ;;
    esac
    if [[ "$TARGET" != /* ]]; then
      TARGET="$PWD/$TARGET"
    fi
    if [[ -d "$TARGET" || "$TARGET" == */ ]]; then
      TARGET="${TARGET%/}/git2pkg-ai"
    fi
    ;;
  *)
    echo "Invalid selection - aborted." >&2
    exit 1
    ;;
esac

mkdir -p -- "$(dirname -- "$TARGET")"

if [[ -e "$TARGET" ]]; then
  printf "A file already exists at the destination. Overwrite? [y/N] "
  read -r overwrite
  if ! is_yes "$overwrite"; then
    say "Aborted - nothing was overwritten."
    exit 0
  fi
fi

cp -f -- "$SOURCE" "$TARGET"
chmod +x -- "$TARGET"

if [[ -f "$ASSET_ICON" ]]; then
  mkdir -p -- "$ICON_DIR"
  cp -f -- "$ASSET_ICON" "$ICON_FILE"
  chmod 644 -- "$ICON_FILE"
  say "Installed application icon: $ICON_FILE"
  if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -f -t "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
  fi
fi

MENU_CREATED="no"
printf "Would you like to add an entry to the applications menu as well? [Y/n] "
read -r menu_answer
menu_answer=${menu_answer:-Y}
if is_yes "$menu_answer"; then
  mkdir -p -- "$DESKTOP_DIR"
  cat > "$DESKTOP_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=git2pkg-ai
GenericName=Package Builder
Comment=Isolated package builds from Git repositories
Exec="$TARGET"
Icon=git2pkg-ai
Terminal=false
Categories=Development;Utility;
EOF
  chmod 644 -- "$DESKTOP_FILE"
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true
  fi
  MENU_CREATED="yes"
fi

say ""
bold "Finished! The executable is here:"
printf '  %s%s%s\n' "$BOLD" "$TARGET" "$RESET"
say ""
say "In your file manager: press Ctrl+L, paste the path and press Enter."
if [[ "$MENU_CREATED" == "yes" ]]; then
  say "Start-menu entry created: $DESKTOP_FILE"
fi

# Container engine check - git2pkg-ai needs either Podman or Docker.
say ""
if command -v podman >/dev/null 2>&1 || command -v docker >/dev/null 2>&1; then
  say "Container engine: found (Podman or Docker)."
else
  distro_id=""
  distro_like=""
  if [[ -r /etc/os-release ]]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    distro_id="${ID:-}"
    distro_like="${ID_LIKE:-}"
  fi

  case "$distro_id" in
    arch|cachyos|manjaro)           engine_cmd="sudo pacman -S podman" ;;
    ubuntu|debian|linuxmint|pop)    engine_cmd="sudo apt install podman" ;;
    fedora)                         engine_cmd="sudo dnf install podman" ;;
    opensuse*|sles*)                engine_cmd="sudo zypper install podman" ;;
    *)
      if [[ " $distro_like " == *" arch "* ]]; then
        engine_cmd="sudo pacman -S podman"
      elif [[ " $distro_like " == *" debian "* ]]; then
        engine_cmd="sudo apt install podman"
      elif [[ " $distro_like " == *" suse "* ]]; then
        engine_cmd="sudo zypper install podman"
      elif [[ " $distro_like " == *" fedora "* ]]; then
        engine_cmd="sudo dnf install podman"
      else
        engine_cmd="sudo pacman -S podman"
      fi
      ;;
  esac

  echo "Warning: neither Podman nor Docker was found on this system."
  echo "git2pkg-ai needs one of them to build packages. Install it with:"
  printf '  %s\n' "$engine_cmd"
fi
