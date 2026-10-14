# robo_stp_engine

Il motore di **robo STP/IGES importer** (plugin di SketchUp): legge file **STEP** e **IGES** con
[OpenCascade](https://dev.opencascade.org/) (OCCT 7.9.3) e scrive i triangoli, con nomi, colori e posizioni
dell'assieme, in un piccolo formato binario (`.rstp`, vedi `src/rstp.rs`).

Rust fa da regista: argomenti, formato di uscita, errori. OpenCascade è chiamato tramite un ponte C
(`cpp/bridge.cpp`) che usa i lettori XDE (`STEPCAFControl_Reader`, `IGESCAFControl_Reader`), percorre l'albero
dell'assieme, triangola con `BRepMesh_IncrementalMesh` e consegna le parti a Rust con una funzione di ritorno.

## Uso

```
robo_stp_engine --input FILE.stp|.step|.igs|.iges --output FILE.rstp [--deflection MM] [--angle RADIANTI] [--error-file FILE]
robo_stp_engine --selftest     # scrive due cubi colorati in STEP e IGES, li rilegge e controlla
robo_stp_engine --version
```

`robo_stp_engine_silent.exe` è lo stesso programma senza finestra nera: è quello che usa il plugin.

## Compilare

Servono Rust (target `x86_64-pc-windows-msvc`), gli strumenti C++ di Visual Studio e CMake.

1. Compila OpenCascade come librerie statiche (una volta sola, circa 20 minuti):

   ```
   cmake -S OCCT-7_9_3 -B build -G "Visual Studio 18 2026" -A x64 -DBUILD_LIBRARY_TYPE=Static ^
     -DBUILD_MODULE_Draw=OFF -DBUILD_DOC_Overview=OFF -DUSE_FREETYPE=OFF -DUSE_TK=OFF -DUSE_TBB=OFF ^
     -DUSE_FREEIMAGE=OFF -DUSE_RAPIDJSON=OFF -DUSE_DRACO=OFF -DUSE_VTK=OFF -DINSTALL_DIR=C:/occt/install
   cmake --build build --config Release --target install --parallel 16
   ```

2. `cargo build --release` (se OpenCascade è altrove: variabile `OCCT_ROOT`).
3. `cargo test --release`: prova STEP e IGES con due cubi colorati (nomi, colori, posizioni, volumi, orientamento).
4. Copia `target/release/robo_stp_engine_silent.exe` in `robo_stp_importer/bin/` del repository dei plugin.

## Licenza

OpenCascade è distribuito con licenza LGPL 2.1 con l'eccezione OCCT. Il collegamento è statico: chi distribuisce il
programma deve rispettare le condizioni della LGPL (indicare l'uso di OpenCascade e permettere di sostituirlo).
Va verificato con attenzione prima di una distribuzione pubblica.

## Limiti

- Provato con file generati da OpenCascade stesso; **non** ancora con file reali di programmi CAD (SolidWorks,
  Fusion, FreeCAD…), che possono avere assiemi grandi, superfici complesse o unità diverse.
- IGES conserva meno dei STEP: nomi e assiemi spesso si perdono.
