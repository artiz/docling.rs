#!/bin/sh
# Build the Tesseract OCR command-line binary from source — the engine behind
# `ocr_engine=tesseract` (#460) in the container images, which want a newer
# release than their distro packages (Debian trixie 5.5.0, Ubuntu 24.04
# 5.3.4). On a Debian/Ubuntu host it also cross-compiles for another Debian
# architecture (TARGET_ARCH), so the s390x image builds it on the build
# machine rather than under QEMU, as it does the Rust binaries.
#
# Only the CLI is built, against the distro's Leptonica: libtesseract is
# linked statically, no training tools, no ScrollView, no libarchive/libcurl/
# libtiff (docling.rs hands it PNG crops on stdin), and no OpenMP (upstream's
# CMake default — the OCR lanes are one process each). The result needs only
# the Leptonica runtime package (libleptonica6 on trixie, liblept5 on Ubuntu
# 24.04) and libstdc++. It reads its traineddata from PREFIX/share/tessdata,
# where `cmake --install` also puts the configs (`tsv`, `hocr`, …) the CLI
# needs; TESSDATA_PREFIX / DOCLING_RS_TESSDATA_DIR point it elsewhere. That
# default has to be passed in: unlike autotools, upstream's CMake compiles no
# tessdata path in, and the binary then reads `./` — `--list-langs`, which
# docling.rs runs to validate `ocr_lang`, walks the whole working directory
# tree (from `/`, the entire filesystem).
#
# Host prerequisites: git cmake make pkgconf ca-certificates, plus g++ and
# libleptonica-dev — or, cross-compiling, crossbuild-essential-$TARGET_ARCH
# dpkg-dev and libleptonica-dev:$TARGET_ARCH (`dpkg --add-architecture` first).
#
#   scripts/install/build_tesseract.sh
#   TESSERACT_VERSION=5.5.3 TARGET_ARCH=s390x DESTDIR=/out scripts/install/build_tesseract.sh
#
# Environment:
#   TESSERACT_VERSION  upstream tag to build (default 5.5.3).
#   TARGET_ARCH        Debian architecture to build for (default: the host's).
#   PREFIX             install prefix compiled in as the tessdata default
#                      (default /usr/local).
#   DESTDIR            staging root the install lands under (default
#                      ./tesseract-install; empty = install into PREFIX).
#   WORK_DIR           checkout + build directory (default ./.tesseract-build).
#   JOBS               parallel compile jobs (default: nproc).
set -eu

TESSERACT_VERSION="${TESSERACT_VERSION:-5.5.3}"
PREFIX="${PREFIX:-/usr/local}"
DESTDIR="${DESTDIR-$PWD/tesseract-install}"
WORK="${WORK_DIR:-$PWD/.tesseract-build}"
JOBS="${JOBS:-$(nproc)}"
HOST_ARCH="$(dpkg --print-architecture 2>/dev/null || echo native)"
TARGET_ARCH="${TARGET_ARCH:-$HOST_ARCH}"

rm -rf "$WORK"
git clone --quiet --depth 1 --branch "$TESSERACT_VERSION" \
  https://github.com/tesseract-ocr/tesseract "$WORK/src"

set --
if [ "$TARGET_ARCH" != "$HOST_ARCH" ]; then
  triple="$(dpkg-architecture -a "$TARGET_ARCH" -qDEB_HOST_MULTIARCH)"
  cpu="$(dpkg-architecture -a "$TARGET_ARCH" -qDEB_HOST_GNU_CPU)"
  # The target's multiarch .pc files only, so pkg-config cannot hand the
  # cross compiler the host's Leptonica.
  export PKG_CONFIG_LIBDIR="/usr/lib/$triple/pkgconfig:/usr/share/pkgconfig"
  # LEPT_TIFF_RESULT: the answer to the configure step's run of a Leptonica
  # TIFF probe, which a cross build cannot execute (Debian's Leptonica has
  # TIFF; Tesseract's own TIFF input is off either way).
  set -- -DCMAKE_SYSTEM_NAME=Linux -DCMAKE_SYSTEM_PROCESSOR="$cpu" \
    -DCMAKE_C_COMPILER="$triple-gcc" -DCMAKE_CXX_COMPILER="$triple-g++" \
    -DLEPT_TIFF_RESULT=0
fi

cmake -S "$WORK/src" -B "$WORK/build" "$@" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DTESSDATA_PREFIX="$PREFIX/share" \
  -DBUILD_SHARED_LIBS=OFF \
  -DBUILD_TRAINING_TOOLS=OFF \
  -DBUILD_TESTS=OFF \
  -DGRAPHICS_DISABLED=ON \
  -DOPENMP_BUILD=OFF \
  -DDISABLE_ARCHIVE=ON \
  -DDISABLE_CURL=ON \
  -DDISABLE_TIFF=ON \
  -DENABLE_CCACHE=OFF
cmake --build "$WORK/build" -j "$JOBS"
DESTDIR="$DESTDIR" cmake --install "$WORK/build" --strip
# The CLI and its configs are the product; the static library, headers and
# pkg-config/CMake files of the install are not (named one by one: with an
# empty DESTDIR this is the host's PREFIX).
rm -rf "$DESTDIR$PREFIX/lib/libtesseract.a" "$DESTDIR$PREFIX/lib/pkgconfig/tesseract.pc" \
  "$DESTDIR$PREFIX/lib/cmake/tesseract" "$DESTDIR$PREFIX/include/tesseract"
[ -z "$DESTDIR" ] || find "$DESTDIR" -depth -type d -empty -delete
echo "tesseract $TESSERACT_VERSION ($TARGET_ARCH) → $DESTDIR$PREFIX"
