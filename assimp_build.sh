#!/usr/bin/env bash
# Assimp 6.0.2 come libreria statica, con i lettori FBX, glTF/GLB, OBJ, 3MF e PLY (per robo_stp_engine). Il sorgente va in
# C:/assimp/src (git clone --depth 1 --branch v6.0.2 https://github.com/assimp/assimp.git src).
export PATH="/c/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin:$PATH"
cd /c/assimp
cmake -S src -B build -G "Visual Studio 18 2026" -A x64 \
  -DBUILD_SHARED_LIBS=OFF \
  -DASSIMP_BUILD_ALL_IMPORTERS_BY_DEFAULT=OFF -DASSIMP_BUILD_FBX_IMPORTER=ON -DASSIMP_BUILD_GLTF_IMPORTER=ON \
  -DASSIMP_BUILD_OBJ_IMPORTER=ON -DASSIMP_BUILD_3MF_IMPORTER=ON -DASSIMP_BUILD_PLY_IMPORTER=ON \
  -DASSIMP_BUILD_ALL_EXPORTERS_BY_DEFAULT=OFF -DASSIMP_NO_EXPORT=ON \
  -DASSIMP_BUILD_TESTS=OFF -DASSIMP_BUILD_ASSIMP_TOOLS=OFF -DASSIMP_BUILD_SAMPLES=OFF -DASSIMP_INSTALL_PDB=OFF \
  -DASSIMP_BUILD_ZLIB=ON -DASSIMP_WARNINGS_AS_ERRORS=OFF -DASSIMP_BUILD_DRACO=OFF -DASSIMP_DOUBLE_PRECISION=OFF \
  -DCMAKE_INSTALL_PREFIX=C:/assimp/install \
  > cmake_configure.log 2>&1 || { echo CONFIGURE_FAILED; tail -30 cmake_configure.log; exit 1; }
echo CONFIGURED
cmake --build build --config Release --target install --parallel 16 > cmake_build.log 2>&1 && echo BUILD_OK || { echo BUILD_FAILED; tail -40 cmake_build.log; }
