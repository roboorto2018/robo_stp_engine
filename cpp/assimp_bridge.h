// Ponte C tra Rust e Assimp (Open Asset Import Library): legge un file FBX, glTF/GLB, OBJ, 3MF o PLY e consegna i triangoli,
// mesh per mesh.
#pragma once
#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Una voce del modello, come in bridge.h. Unità: millimetri. `rgba` è -1 se il materiale non dà un colore (e la mesh non ha
// colori nei vertici: allora è la loro media).
//   kind 0: una mesh già portata nella posizione finale; kind 1: la definizione `id` di una mesh usata da più nodi, nelle sue
//   coordinate; kind 2: un'istanza della definizione `id`, senza triangoli, `matrix` = immagini degli assi x, y, z e traslazione (mm).
// `uv` (se non è nullo): u, v di ogni vertice; `texture` (se non è nullo): il percorso dell'immagine del materiale, già cercato
// accanto al file o estratto dal file (immagini incorporate).
typedef void (*rasm_item_cb)(void* user, int kind, uint32_t id, const char* name_utf8, const float* rgba, uint64_t n_vertices,
                             const float* vertices, uint64_t n_triangles, const uint32_t* indices, const float* uv,
                             const char* texture_utf8, const float* matrix);

// Legge `path_utf8` (il formato lo riconosce Assimp). `unit_mm` > 0: millimetri per unità scelti dall'utente; <= 0: quelli scritti
// nel file (FBX: UnitScaleFactor) o, se il file non li dice, `default_unit_mm` (quelli del formato: glTF metri, 3MF quella del file
// letta da Rust...). `file_axes` != 0: gli assi del file diventano quelli di SketchUp (Z in alto): quelli scritti nel file (FBX:
// UpAxis, FrontAxis, CoordAxis) o, se il file non li dice, Y in alto (glTF, OBJ); 0: le coordinate come sono. `instances` != 0:
// una mesh usata da più nodi arriva come una definizione e le sue istanze. `with_uv` != 0: coordinate di mappatura e immagini
// dei materiali; le immagini incorporate nel file si scrivono in `texture_dir_utf8`.
// `info` (sette numeri): millimetri per unità usati, asse in alto scritto nel file (0 x, 1 y, 2 z; -1 se non lo dice), mesh, nodi
// con mesh, triangoli del file (ogni uso di una mesh conta), immagini incorporate scritte, 1 se l'unità è scritta nel file.
// Restituisce 0 se va bene; altrimenti un codice diverso da 0 e il messaggio in `err`.
int rasm_load(const char* path_utf8, double unit_mm, double default_unit_mm, int file_axes, int instances, int with_uv,
              const char* texture_dir_utf8, rasm_item_cb cb, void* user, double* info, char* err, uint32_t err_len);

// Versione di Assimp, es. "6.0.2".
const char* rasm_assimp_version(void);

#ifdef __cplusplus
}
#endif
