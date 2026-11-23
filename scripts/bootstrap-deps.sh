#!/usr/bin/env bash
# Resolves the real versions of every external crate Weft uses.
#
# Why a script instead of a hand-written [workspace.dependencies]: guessing
# versions produces unbuildable manifests when the ecosystem moves (wgpu majors
# move fast). `cargo add` asks the registry and writes a compatible range; the
# lockfile is what we commit.
#
# Usage:
#   ./scripts/bootstrap-deps.sh          # add/update deps in existing crates
#   ./scripts/bootstrap-deps.sh --print  # only print the intended manifest
#   CARGO_ADD_FLAGS=--offline ./scripts/bootstrap-deps.sh   # cached index
set -euo pipefail

cd "$(dirname "$0")/.."

PRINT=0
[[ "${1:-}" == "--print" ]] && PRINT=1

have_crate() { [[ -f "crates/$1/Cargo.toml" ]]; }

add() {
  local target="$1"
  shift
  if [[ $PRINT -eq 1 ]]; then
    echo "cargo add -p $target $*"
    return
  fi
  if ! have_crate "$target"; then
    echo "skip: crates/$target no existe todavía (se crea en su hito)"
    return
  fi
  # shellcheck disable=SC2086
  cargo add -p "$target" ${CARGO_ADD_FLAGS:-} "$@"
}

add_dev() {
  local target="$1"
  shift
  [[ $PRINT -eq 1 ]] && { echo "cargo add --dev -p $target $*"; return; }
  have_crate "$target" || return 0
  # shellcheck disable=SC2086
  cargo add --dev -p "$target" ${CARGO_ADD_FLAGS:-} "$@"
}

echo "== weft-term: PTY, VT, bloques (sin UI) =="
add weft-term portable-pty
add weft-term vte
add weft-term alacritty_terminal
add weft-term bytes
add weft-term crossbeam-channel
add weft-term thiserror
add weft-term tracing
add weft-term tokio --features rt,sync,time,macros,io-util

echo "== weft-config: TOML, temas, keybindings =="
add weft-config serde --features derive
add weft-config toml
add weft-config directories
add weft-config thiserror
add weft-config tracing

echo "== weft-store: redb (puro Rust, sin SQLite) =="
add weft-store redb
add weft-store serde --features derive
add weft-store serde_json
add weft-store thiserror
add weft-store tracing
add weft-store uuid --features v4,serde

echo "== weft-agent: proveedores y streaming =="
add weft-agent reqwest --features rustls-tls,http2,stream,json --no-default-features
add weft-agent tokio --features rt-multi-thread,sync,time,macros,io-util
add weft-agent futures-util
add weft-agent eventsource-stream
add weft-agent serde --features derive
add weft-agent serde_json
add weft-agent thiserror
add weft-agent async-trait
add weft-agent tracing
add weft-agent uuid --features v4,serde

echo "== weft: ventana, GPU, texto, layout =="
add weft winit
add weft wgpu
add weft glyphon
add weft cosmic-text
add weft taffy
add weft pollster
add weft bytemuck
add weft arboard
add weft notify
add weft anyhow
add weft tracing
add weft tracing-subscriber --features env-filter,fmt
# Crates internos como path dependencies.
if [[ $PRINT -eq 1 ]]; then
  echo "cargo add -p weft --path crates/weft-term"
  echo "cargo add -p weft --path crates/weft-config"
  echo "cargo add -p weft --path crates/weft-store"
  echo "cargo add -p weft --path crates/weft-agent"
elif have_crate weft; then
  cargo add -p weft --path crates/weft-term
  have_crate weft-config && cargo add -p weft --path crates/weft-config
  have_crate weft-store && cargo add -p weft --path crates/weft-store
  have_crate weft-agent && cargo add -p weft --path crates/weft-agent
fi

echo "== dev-dependencies =="
add_dev weft-term insta
add_dev weft-term proptest
add_dev weft-term expectrl
add_dev weft-term tempfile
add_dev weft-term criterion

echo
echo "Listo. Verifica compatibilidad de GPU con:"
echo "  cargo tree -i wgpu"
echo "  cargo tree -i vte"
echo "glyphon debe compartir la major de wgpu; alacritty_terminal fija su vte."
