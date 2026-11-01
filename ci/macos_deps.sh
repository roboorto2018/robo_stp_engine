#!/usr/bin/env bash
# OpenCascade 7.9.3 and Assimp 6.0.2 as universal (arm64 + x86_64) static libraries for macOS, the same options as the
# Windows builds (C:/occt/build.sh, assimp_build.sh). Runs on the macOS runner of GitHub Actions (.github/workflows/macos.yml).
# Result: $DEPS/occt (include/opencascade, lib/libTK*.a) and $DEPS/assimp (include, lib/libassimp.a). zlib: the one of macOS
# (the copy inside Assimp does not compile with the SDK of Xcode 16).
set -euo pipefail
DEPS="${DEPS:-$HOME/deps}"
ARCHS="arm64;x86_64"
JOBS="$(sysctl -n hw.ncpu)"
mkdir -p "$DEPS/src"
cd "$DEPS/src"

if [ ! -d OCCT-7_9_3 ]; then
  curl -fsSL https://github.com/Open-Cascade-SAS/OCCT/archive/refs/tags/V7_9_3.tar.gz | tar xz
  [ -d OCCT-7_9_3 ] || mv OCCT-* OCCT-7_9_3
fi
cmake -S OCCT-7_9_3 -B occt-build -DCMAKE_BUILD_TYPE=Release -DCMAKE_OSX_ARCHITECTURES="$ARCHS" \
  -DBUILD_LIBRARY_TYPE=Static \
  -DBUILD_MODULE_Draw=OFF \
  -DBUILD_DOC_Overview=OFF \
  -DUSE_FREETYPE=OFF -DUSE_TK=OFF -DUSE_TBB=OFF -DUSE_FREEIMAGE=OFF -DUSE_RAPIDJSON=OFF -DUSE_DRACO=OFF -DUSE_VTK=OFF \
  -DUSE_OPENVR=OFF -DUSE_FFMPEG=OFF -DUSE_XLIB=OFF \
  -DINSTALL_DIR="$DEPS/occt"
cmake --build occt-build --target install --parallel "$JOBS"

if [ ! -d assimp ]; then
  git clone --depth 1 --branch v6.0.2 https://github.com/assimp/assimp.git assimp
fi
cmake -S assimp -B assimp-build -DCMAKE_BUILD_TYPE=Release -DCMAKE_OSX_ARCHITECTURES="$ARCHS" \
  -DBUILD_SHARED_LIBS=OFF \
  -DASSIMP_BUILD_ALL_IMPORTERS_BY_DEFAULT=OFF -DASSIMP_BUILD_FBX_IMPORTER=ON -DASSIMP_BUILD_GLTF_IMPORTER=ON \
  -DASSIMP_BUILD_OBJ_IMPORTER=ON -DASSIMP_BUILD_3MF_IMPORTER=ON -DASSIMP_BUILD_PLY_IMPORTER=ON \
  -DASSIMP_BUILD_ALL_EXPORTERS_BY_DEFAULT=OFF -DASSIMP_NO_EXPORT=ON \
  -DASSIMP_BUILD_TESTS=OFF -DASSIMP_BUILD_ASSIMP_TOOLS=OFF -DASSIMP_BUILD_SAMPLES=OFF \
  -DASSIMP_BUILD_ZLIB=OFF -DASSIMP_WARNINGS_AS_ERRORS=OFF -DASSIMP_BUILD_DRACO=OFF -DASSIMP_DOUBLE_PRECISION=OFF \
  -DCMAKE_INSTALL_PREFIX="$DEPS/assimp"
cmake --build assimp-build --target install --parallel "$JOBS"

ls "$DEPS/occt/lib" | head -60
ls "$DEPS/assimp/lib"
