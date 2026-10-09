#!/usr/bin/env bash
# Build in release mode, install to ~/.local, enable autostart and bind a hotkey.
set -euo pipefail

HOTKEY="${PARSEC_HOTKEY:-<Control>space}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

cargo build --release --manifest-path "$ROOT/Cargo.toml"

install -Dm755 "$ROOT/target/release/parsec" "$HOME/.local/bin/parsec"
install -Dm644 "$ROOT/data/org.abidibo.Parsec.desktop" "$HOME/.local/share/applications/org.abidibo.Parsec.desktop"
install -Dm644 "$ROOT/data/org.abidibo.Parsec.desktop" "$HOME/.config/autostart/org.abidibo.Parsec.desktop"
install -Dm644 "$ROOT/data/org.abidibo.Parsec.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/org.abidibo.Parsec.svg"
gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

# Custom keybinding: GNOME stores these as an array of relocatable paths.
SCHEMA="org.gnome.settings-daemon.plugins.media-keys"
KEY_PATH="/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/parsec/"
CURRENT="$(gsettings get "$SCHEMA" custom-keybindings)"
if [[ "$CURRENT" != *"$KEY_PATH"* ]]; then
  if [[ "$CURRENT" == "@as []" || "$CURRENT" == "[]" ]]; then
    NEW="['$KEY_PATH']"
  else
    NEW="${CURRENT%]}, '$KEY_PATH']"
  fi
  gsettings set "$SCHEMA" custom-keybindings "$NEW"
fi
gsettings set "$SCHEMA.custom-keybinding:$KEY_PATH" name "Parsec"
gsettings set "$SCHEMA.custom-keybinding:$KEY_PATH" command "$HOME/.local/bin/parsec toggle"
gsettings set "$SCHEMA.custom-keybinding:$KEY_PATH" binding "$HOTKEY"

# (Re)start the daemon.
pkill -x parsec 2>/dev/null || true
nohup "$HOME/.local/bin/parsec" --background >/dev/null 2>&1 &

echo "installed. hotkey: $HOTKEY  (override with PARSEC_HOTKEY=... )"
