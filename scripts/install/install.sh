#!/usr/bin/env bash
# Local / CI installer for docling.rs: build from source and install a
# self-contained tree under /usr/local/docling.rs with a `docling-rs`
# command on PATH. Designed for one-liner use in dev boxes and pipelines:
#
#   curl -fsSL https://raw.githubusercontent.com/docling-project/docling.rs/master/scripts/install/install.sh | bash
#
# or from a checkout:
#
#   scripts/install/install.sh
#
# What it does:
#   0. Tries the prebuilt CLI binary from the latest GitHub Release first
#      (Linux x64/arm64) — no toolchain, no build. Falls back to source on any
#      mismatch; DOCLING_RS_FROM_SOURCE=1 skips the fast path.
#   1. Checks for a Rust toolchain (>= 1.82 for the PDF crate); installs one
#      via rustup (non-interactive) if `cargo` is missing.
#   2. Builds the CLI in release mode (`cargo build --release -p docling-cli`).
#   3. Installs to $DOCLING_RS_PREFIX (default /usr/local/docling.rs):
#        bin/docling-rs          the CLI (ONNX Runtime statically linked)
#        .models/…, .pdfium/…      fetched by scripts/install/download_dependencies.sh
#      and symlinks it as /usr/local/bin/docling-rs.
#   4. Writes /etc/profile.d/docling-rs.sh exporting the DOCLING_*/PDFIUM_*
#      paths. This is belt-and-braces only: the binary also resolves models
#      and pdfium relative to its own (symlink-resolved) location, so the
#      symlink works in pipelines that never source profile.d.
#
# Options (env vars):
#   DOCLING_RS_PREFIX=/opt/docling.rs   install tree (default /usr/local/docling.rs)
#   DOCLING_RS_BIN_DIR=/usr/local/bin    where the symlink goes
#   DOCLING_RS_REF=master                git ref to build when cloning
#   DOCLING_RS_NO_ASR=1                  skip the Whisper ASR models (~150 MB)
#   DOCLING_RS_SUDO=0                    never invoke sudo (fail instead)
set -euo pipefail

REPO_URL="https://github.com/docling-project/docling.rs"
PREFIX="${DOCLING_RS_PREFIX:-/usr/local/docling.rs}"
BIN_DIR="${DOCLING_RS_BIN_DIR:-/usr/local/bin}"
REF="${DOCLING_RS_REF:-master}"

say() { printf '\033[1m[docling.rs]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[docling.rs]\033[0m %s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || die "curl is required"

# Privilege helper: only used for the install/copy steps, never for the build.
SUDO=""
if [ ! -w "$(dirname "$PREFIX")" ] || { [ -e "$PREFIX" ] && [ ! -w "$PREFIX" ]; }; then
  if [ "${DOCLING_RS_SUDO:-1}" = "0" ]; then
    die "$PREFIX is not writable and DOCLING_RS_SUDO=0"
  fi
  command -v sudo >/dev/null 2>&1 || die "$PREFIX is not writable and sudo is unavailable"
  SUDO="sudo"
fi

# --- 0. Prebuilt binary (fast path) -------------------------------------------
# GitHub Releases carry prebuilt CLI binaries (.github/workflows/cli-binaries.yml)
# for Linux x64/arm64 and Windows x64 — downloading one skips the Rust toolchain
# and the multi-minute source build entirely. Any failure (no matching asset,
# old release, no network to the API) quietly falls back to building from
# source. DOCLING_RS_FROM_SOURCE=1 forces the source build.
PREBUILT_BIN=""
if [ "${DOCLING_RS_FROM_SOURCE:-0}" != "1" ] && [ "$(uname -s)" = "Linux" ]; then
  case "$(uname -m)" in
    x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
    aarch64 | arm64) TARGET="aarch64-unknown-linux-gnu" ;;
    *) TARGET="" ;;
  esac
  if [ -n "$TARGET" ]; then
    TAG="$(curl -fsSL "https://api.github.com/repos/docling-project/docling.rs/releases/latest" 2>/dev/null \
      | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1 || true)"
    if [ -n "${TAG:-}" ]; then
      ASSET="$REPO_URL/releases/download/$TAG/docling-rs-$TAG-$TARGET.tar.gz"
      PB_DIR="$(mktemp -d)"
      if curl -fsSL "$ASSET" 2>/dev/null | tar -xz -C "$PB_DIR" 2>/dev/null \
        && [ -x "$PB_DIR/docling-rs" ]; then
        # Verify the binary actually runs *on this system* before trusting it:
        # the release binaries link ort's static onnxruntime, which needs
        # glibc >= 2.38 — on an older distro the dynamic loader rejects it.
        # No-args is the cheapest full load: the CLI prints usage and exits 2.
        rc=0
        "$PB_DIR/docling-rs" >/dev/null 2>&1 || rc=$?
        if [ "$rc" = "2" ]; then
          PREBUILT_BIN="$PB_DIR/docling-rs"
          say "using the prebuilt $TAG binary for $TARGET (set DOCLING_RS_FROM_SOURCE=1 to build instead)"
        else
          say "the prebuilt $TAG binary does not run here (glibc older than 2.38?) — building from source"
        fi
      else
        say "no prebuilt binary for $TAG/$TARGET — building from source"
      fi
    fi
  fi
fi

if [ -z "$PREBUILT_BIN" ]; then
  # --- 1. Rust toolchain -----------------------------------------------------
  if ! command -v cargo >/dev/null 2>&1; then
    # Respect an existing rustup installation that just isn't on PATH yet.
    if [ -f "$HOME/.cargo/env" ]; then
      # shellcheck disable=SC1091
      . "$HOME/.cargo/env"
    fi
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    say "Rust toolchain not found — installing via rustup (stable, non-interactive)"
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi
  command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 || command -v clang >/dev/null 2>&1 \
    || die "no C compiler (cc/gcc/clang) — install build-essential / clang first"
  say "using $(cargo --version)"
fi

# --- 2. Sources: use the checkout we're in, else clone ------------------------
# Needed even with a prebuilt binary: download_dependencies.sh (models + pdfium)
# runs from the source tree.
if [ -f Cargo.toml ] && [ -d crates/docling-cli ]; then
  SRC_DIR="$(pwd)"
  say "using existing checkout: $SRC_DIR"
else
  SRC_DIR="$(mktemp -d)/docling.rs"
  say "cloning $REPO_URL ($REF) to $SRC_DIR"
  if command -v git >/dev/null 2>&1; then
    git clone --depth 1 --branch "$REF" "$REPO_URL" "$SRC_DIR"
  else
    mkdir -p "$SRC_DIR"
    curl -fsSL "$REPO_URL/archive/refs/heads/$REF.tar.gz" | tar -xz -C "$SRC_DIR" --strip-components=1
  fi
  cd "$SRC_DIR"
fi

# --- 3. Build (skipped when a prebuilt binary was downloaded) -------------------
if [ -z "$PREBUILT_BIN" ]; then
  say "building the CLI (release)"
  cargo build --release -p docling-cli
fi

# --- 4. Install tree -----------------------------------------------------------
say "installing to $PREFIX"
$SUDO mkdir -p "$PREFIX/bin"
$SUDO cp "${PREBUILT_BIN:-target/release/docling-rs}" "$PREFIX/bin/docling-rs"

# Models + pdfium land inside the prefix; download_dependencies.sh fetches into
# the *current* directory, so run it from there. It is idempotent (skips files
# already present), so re-running the installer only fetches what's missing.
DL_ARGS=""
[ "${DOCLING_RS_NO_ASR:-0}" = "1" ] && DL_ARGS="--no-asr"
say "fetching models + pdfium into $PREFIX (idempotent)"
# shellcheck disable=SC2086
(cd "$PREFIX" && $SUDO sh "$SRC_DIR/scripts/install/download_dependencies.sh" $DL_ARGS)

say "linking $BIN_DIR/docling-rs -> $PREFIX/bin/docling-rs"
$SUDO mkdir -p "$BIN_DIR"
$SUDO ln -sfn "$PREFIX/bin/docling-rs" "$BIN_DIR/docling-rs"
# Older installers linked the command as `docling.rs` (dot instead of dash);
# drop that stale symlink so only the documented name remains.
if [ -L "$BIN_DIR/docling.rs" ] && [ "$(readlink "$BIN_DIR/docling.rs")" = "$PREFIX/bin/docling-rs" ]; then
  $SUDO rm -f "$BIN_DIR/docling.rs"
fi

# --- 5. Environment (optional convenience) --------------------------------------
# The binary resolves .models/.pdfium next to its own location through the
# symlink, so these exports are not required for `docling-rs` itself — they
# help other tools (scripts, the Node bindings) find the same assets.
if [ -d /etc/profile.d ] && [ -n "$SUDO" -o -w /etc/profile.d ]; then
  say "writing /etc/profile.d/docling-rs.sh"
  $SUDO tee /etc/profile.d/docling-rs.sh >/dev/null <<EOF
# docling.rs (installed by scripts/install/install.sh) — not required by the CLI
# itself (it resolves these relative to its own binary), provided for other
# consumers of the same model tree.
export PDFIUM_DYNAMIC_LIB_PATH="$PREFIX/.pdfium/lib"
export DOCLING_OCR_REC_ONNX="$PREFIX/.models/ocr_rec.onnx"
export DOCLING_OCR_DICT="$PREFIX/.models/ppocr_keys_v1.txt"
export DOCLING_TABLEFORMER_ENCODER="$PREFIX/.models/tableformer/encoder.onnx"
export DOCLING_TABLEFORMER_BBOX="$PREFIX/.models/tableformer/bbox.onnx"
EOF
fi

# --- 6. Smoke test ---------------------------------------------------------------
say "smoke test: converting a trivial Markdown document"
TMP_MD="$(mktemp --suffix=.md 2>/dev/null || mktemp -t docling.rs.XXXXXX.md)"
printf '# docling.rs\n\ninstalled.\n' > "$TMP_MD"
"$BIN_DIR/docling-rs" "$TMP_MD" >/dev/null || die "smoke test failed"
rm -f "$TMP_MD"

say "done. Try:  docling-rs your.pdf > out.md"
say "layout: $PREFIX  (binary, .models/, .pdfium/); uninstall: rm -rf $PREFIX $BIN_DIR/docling-rs /etc/profile.d/docling-rs.sh"
