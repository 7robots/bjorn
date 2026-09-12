#!/usr/bin/env bash
# Builds the release binaries and installs a `bjorn` launcher into ~/bin (or
# --dir DIR). `git pull && ./install.sh` is the update path. The archived Python
# Bjorn installed a launcher of the same name; run its `install.sh --uninstall`
# first if it is still there.
set -euo pipefail

APP="bjorn"
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_DIR="$HOME/bin"
SHARE_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/bjorn/bin"

usage() {
    cat <<USAGE
Usage: ./install.sh [--dir DIR] [--uninstall]

  --dir DIR     install the launcher into DIR instead of $DEFAULT_DIR
  --uninstall   remove the launcher and the installed binaries

Config lives in \${XDG_CONFIG_HOME:-\$HOME/.config}/bjorn/config.toml and is
left alone by both install and uninstall.
USAGE
}

die() { echo "install.sh: $*" >&2; exit 1; }

TARGET_DIR="$DEFAULT_DIR"
UNINSTALL=0
while [ $# -gt 0 ]; do
    case "$1" in
        --dir) [ $# -ge 2 ] || die "--dir needs an argument"; TARGET_DIR="$2"; shift 2 ;;
        --uninstall) UNINSTALL=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
done

LAUNCHER="$TARGET_DIR/$APP"

if [ "$UNINSTALL" -eq 1 ]; then
    rm -f "$LAUNCHER"
    rm -rf "$SHARE_DIR"
    echo "Removed $LAUNCHER and $SHARE_DIR"
    exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
    if [ -x /opt/homebrew/opt/rustup/bin/cargo ]; then
        export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
    else
        die "cargo is required — brew install rustup && rustup default stable"
    fi
fi

echo "Building release binaries in $PROJECT_DIR …"
(cd "$PROJECT_DIR" && cargo build --release --quiet)

mkdir -p "$SHARE_DIR" "$TARGET_DIR"
for bin in bjorn fake-bearcli fake-remctl bjorn-gate; do
    install -m 755 "$PROJECT_DIR/target/release/$bin" "$SHARE_DIR/$bin"
done
ln -sfn "$SHARE_DIR/bjorn" "$LAUNCHER"

echo "Installed $LAUNCHER -> $SHARE_DIR/bjorn"
case ":$PATH:" in
    *":$TARGET_DIR:"*) ;;
    *) echo "note: $TARGET_DIR is not on this shell's PATH — add it to use \`$APP\`" ;;
esac
