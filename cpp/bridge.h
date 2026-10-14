// Ponte C tra Rust e OpenCascade: legge un file STEP o IGES e consegna i triangoli, parte per parte.
#pragma once
#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Una parte del modello: un solido (o un guscio) di un colore, già portato nella posizione finale
// dell'assieme. Unità: millimetri. `rgba` è -1 se il file non dà un colore. I triangoli sono orientati
// con la faccia esterna verso l'osservatore (senso antiorario).
typedef void (*rstp_part_cb)(void* user, const char* name_utf8, const float* rgba, uint64_t n_vertices,
                             const float* vertices, uint64_t n_triangles, const uint32_t* indices);

// Legge `path_utf8` (formato: 0 = STEP, 1 = IGES) e chiama `cb` per ogni parte.
// Restituisce 0 se va bene; altrimenti un codice diverso da 0 e il messaggio in `err`.
int rstp_load(const char* path_utf8, int format, double linear_deflection, double angular_deflection, rstp_part_cb cb,
              void* user, char* err, uint32_t err_len);

// Scrive un cubo di prova (due pezzi colorati in un assieme) in STEP o IGES, per provare la lettura.
int rstp_write_test_box(const char* path_utf8, int format, char* err, uint32_t err_len);

// Versione di OpenCascade, es. "7.9.3".
const char* rstp_occt_version(void);

#ifdef __cplusplus
}
#endif
