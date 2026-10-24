// Ponte C tra Rust e OpenCascade: legge un file STEP o IGES e consegna i triangoli, parte per parte.
#pragma once
#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Una voce del modello. Unità: millimetri. `rgba` è -1 se il file non dà un colore. I triangoli sono orientati con la
// faccia esterna verso l'osservatore (senso antiorario).
//   kind 0: una parte (un solido o un guscio di un colore) già portata nella posizione finale dell'assieme;
//   kind 1: la definizione `id` di un pezzo ripetuto, nelle sue coordinate (non va nel modello: si riusa);
//   kind 2: un'istanza della definizione `id`, senza triangoli: `matrix` = 12 valori, le immagini degli assi x, y, z e la
//           traslazione (mm).
typedef void (*rstp_item_cb)(void* user, int kind, uint32_t id, const char* name_utf8, const float* rgba, uint64_t n_vertices,
                             const float* vertices, uint64_t n_triangles, const uint32_t* indices, const float* matrix);

// Legge `path_utf8` (formato: 0 = STEP, 1 = IGES) e chiama `cb` per ogni voce.
// `max_triangles` > 0: se la triangolazione supera questo totale, la tolleranza si allarga e si triangola di nuovo; in
// `used_linear` (se non è nullo) la tolleranza usata davvero. `instances` != 0: i pezzi che compaiono più volte arrivano
// come una definizione e le sue istanze.
// `n_estimates` > 0 (anteprima): per ognuna delle coppie (tolleranza, angolo) in `estimate_params`, dalla piu' grossolana alla
// piu' fine, i triangoli che darebbe: in `estimate_out` tre numeri per coppia (nel modello, da costruire, 1 se misurati davvero,
// 0 se stimati dalla curva dei punti misurati).
// `source_counts` (se non è nullo, due numeri): le facce (superfici) e i bordi (curve) del file, prima della triangolazione;
// ogni copia di un pezzo ripetuto conta.
// Restituisce 0 se va bene; altrimenti un codice diverso da 0 e il messaggio in `err`.
int rstp_load(const char* path_utf8, int format, double linear_deflection, double angular_deflection, uint64_t max_triangles,
              int instances, int n_estimates, const double* estimate_params, uint64_t* estimate_out, rstp_item_cb cb, void* user,
              double* used_linear, uint64_t* source_counts, char* err, uint32_t err_len);

// Scrive un cubo di prova (due pezzi colorati in un assieme) in STEP o IGES, per provare la lettura. `repeated` != 0: il pezzo
// rosso compare due volte (per provare i pezzi ripetuti).
int rstp_write_test_box(const char* path_utf8, int format, int repeated, char* err, uint32_t err_len);

// Versione di OpenCascade, es. "7.9.3".
const char* rstp_occt_version(void);

#ifdef __cplusplus
}
#endif
