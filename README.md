# robo_stp_engine

Il motore di **robo importer** (plugin di SketchUp): legge file **STEP** e **IGES** con
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

Opzioni per i file grandi (usate da robo importer 0.7):

- `--instances 1` (anche STEP e IGES): un pezzo che compare piu' volte nell'assieme arriva come **una definizione** (nelle sue
  coordinate) e le sue **istanze** (posizione = matrice), cosi' SketchUp lo costruisce una volta sola. Provato su un assieme reale:
  796.568 triangoli da costruire diventano 159.578 (35 definizioni, 230 istanze).
- `--max-triangles N` (anche STEP e IGES): se la triangolazione supera N triangoli da costruire, `cpp/bridge.cpp` allarga la
  tolleranza in proporzione e triangola di nuovo (al massimo 4 volte). Ogni pezzo si triangola una volta sola.
- `--soften GRADI`: scrive anche le **linee morbide** (`src/spigoli.rs`): per ogni triangolo, quali lati stanno fra due facce
  che piegano meno di GRADI. I punti uguali (entro 0,001 mm) si uniscono con una griglia che guarda anche le 26 celle vicine.
  Il file esce nella versione 4 del formato (`src/rstp.rs`). 343.000 triangoli in 0,05 s.
- `--estimate L,A,L,A,...` (con `--preview`, STEP e IGES; robo importer 0.8): quanti triangoli darebbe ogni dettaglio, nel
  resoconto `estimate` = [nel modello, da costruire, 1 = triangolato davvero / 0 = stimato]. Bozza e (se Bozza costruisce meno
  di 300.000 triangoli) Normale si triangolano per intero; Alta si stima da un campione di pezzi presi dal piu' piccolo (al
  massimo 100.000 triangoli o 10 s), con una retta logaritmica del rapporto in funzione della grandezza del pezzo. Su un assieme
  reale di 18,6 MB: Bozza e Normale esatti, Alta 3,6 milioni stimati contro 14,7 reali (i pezzi grandi e curvi crescono di piu').
- `--info-file FILE` (robo importer 0.9): il resoconto JSON si scrive anche in questo file. Il programma senza finestra
  lanciato da SketchUp non porta la sua uscita standard nel file che il plugin gli da', quindi il plugin legge questo.
- Il motore lavora a **priorita' sotto la normale** e lascia libero un processore (i suoi fili di lavoro e quelli di
  OpenCascade), cosi' il computer resta usabile mentre legge.

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
4. Copia `target/release/robo_stp_engine_silent.exe` in `installer/engine/` del repository dei plugin: l'installatore di robo importer lo mette in `%LOCALAPPDATA%\RoboTools\robo_importer\bin`. Chi vuole puo' sostituire quel file con il motore ricompilato da se'.

## File 3ds Max (.max)

`robo_stp_engine --input FILE.max --output FILE.rstp [--unit mm|cm|m|in|ft]` legge un file di 3ds Max **senza 3ds Max** (`src/maxfile.rs`,
contenitore OLE con il crate `cfb`). Il formato non e' documentato da Autodesk: la lettura e' stata ricostruita analizzando file
reali, quindi va sempre controllato il risultato.

- Si leggono i nodi (nome, gruppo, posizione/rotazione/scala fisse, offset dell'oggetto) e la geometria degli oggetti **Editable Mesh** e
  **Editable Poly** (sotto una pila di modificatori si prende l'oggetto di base: i modificatori non si applicano).
- Non si importano spline e forme, luci, camere, materiali, texture, animazioni; le parti non hanno colore.
- Nel blocco delle facce dell'Editable Poly la lunghezza della coda di ogni faccia non e' nota con certezza: si usa una formula e, se la faccia
  successiva non torna, una ricerca con ritorno indietro. Una faccia che non si legge si segnala (`unreadable_faces`).
- L'unita' del file non si legge dal file: predefinita `mm`, da indicare con `--unit`.
- Provato su un file reale (Corona, 596 MB, 51 oggetti, 5,2 milioni di triangoli, letto in circa 1 secondo).

## File SolidWorks (.sldprt)

`robo_stp_engine --input FILE.sldprt --output FILE.rstp` legge un file di SolidWorks 2015 o successivo **senza SolidWorks**
(`src/sldprt.rs`). Non si legge il solido esatto (il formato Parasolid richiede il programma di Siemens) ma la **mesh di visualizzazione**
salvata nel file: il blocco `Contents/DisplayLists` (raw deflate, nome con i mezzi byte scambiati) contiene, per ogni faccia, posizioni e
normali in f32 (metri) e le lunghezze dei triangle strip. Risultato: una sola parte, in millimetri.

- Non si leggono colori, assiemi (`.sldasm`), solidi importati da STEP dentro SolidWorks, ne' i file anteriori al 2015 (contenitore OLE).
- Provato su un solo file reale (`SS CHAIR.SLDPRT`, 174 facce, 66.828 triangoli, letto in 60 ms).

## Limiti

- Provato con file generati da OpenCascade stesso e con alcuni assiemi STEP reali (fino a 18,6 MB, 234 parti); ogni nuovo
  programma CAD puo' avere assiemi grandi, superfici complesse o unita' diverse.
- IGES conserva meno dei STEP: nomi e assiemi spesso si perdono.

## Licenza

Il motore e' distribuito con la **GNU Lesser General Public License 2.1 con l'eccezione Open CASCADE 1.0** (file `LICENSE` e
`OCCT-EXCEPTION.txt`), la stessa di OpenCascade, che e' collegato staticamente: chiunque puo' modificare il motore,
ricompilarlo (anche con una versione modificata di OpenCascade, vedi *Compilare*) e usarlo al posto di quello installato.

Open Cascade Technology 7.9.3 (c) Open CASCADE SAS, sorgente non modificato: https://github.com/Open-Cascade-SAS/OCCT/releases/tag/V7_9_3

Le librerie Rust incluse (tutte con licenza MIT) sono elencate, con le righe di copyright, in `THIRD-PARTY.txt`.
