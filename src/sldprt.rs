//! Lettura dei file SolidWorks `.sldprt` (versioni dal 2015 in poi), senza SolidWorks.
//!
//! Il formato non e' documentato: questa lettura e' stata ricostruita analizzando file reali, con l'aiuto delle note pubbliche del
//! progetto sldprt-export. Non si legge la geometria esatta (il solido Parasolid richiede il programma di Siemens) ma la **mesh di
//! visualizzazione** che SolidWorks salva nel file, quindi la risoluzione e' quella scelta da SolidWorks.
//!
//! - Il file e' una sequenza di blocchi: firma `14 00 06 00 08 00`, tipo, crc32 (del contenuto decompresso), dimensione compressa,
//!   dimensione decompressa, dimensione del preambolo, preambolo (il nome, con i due mezzi byte di ogni byte scambiati) e il
//!   contenuto compresso in «raw deflate».
//! - Il blocco `Contents/DisplayLists` contiene, per ogni faccia del solido: un array di posizioni (f32 x y z, in metri), un array di
//!   normali, poi due array di interi: la tabella dei descrittori e le lunghezze dei «triangle strip». Le posizioni sono gia' in
//!   ordine di strip: uno strip di L vertici da' L-2 triangoli.

use crate::ffi::Parte;
use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::Path;

const FIRMA: [u8; 6] = [0x14, 0x00, 0x06, 0x00, 0x08, 0x00];
const NOME_DISPLAY_LISTS: &[u8] = b"Contents/DisplayLists";
/// Intestazione di un array di tre f32: dimensione elemento 12, tipo 100, dimensioni 2 (poi viene il numero di elementi).
const ARRAY_F32X3: [u8; 12] = [12, 0, 0, 0, 100, 0, 0, 0, 2, 0, 0, 0];
/// Intestazione di un array di interi a 32 bit.
const ARRAY_U32: [u8; 12] = [4, 0, 0, 0, 8, 0, 0, 0, 2, 0, 0, 0];

fn u32_at(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]])
}

fn f32_at(b: &[u8], p: usize) -> f32 {
    f32::from_bits(u32_at(b, p))
}

/// La posizione della prossima occorrenza di `ago` in `dati` a partire da `da`.
fn cerca(dati: &[u8], ago: &[u8], da: usize) -> Option<usize> {
    if da >= dati.len() {
        return None;
    }
    let primo = ago[0];
    let mut i = da;
    while i + ago.len() <= dati.len() {
        i += dati[i..dati.len() - ago.len() + 1].iter().position(|&b| b == primo)?;
        if &dati[i..i + ago.len()] == ago {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Un blocco del file: il nome e dove sta il contenuto compresso.
struct Blocco {
    nome: Vec<u8>,
    crc: u32,
    compresso: std::ops::Range<usize>,
    dimensione: usize,
}

fn blocchi(d: &[u8]) -> Vec<Blocco> {
    let mut out = Vec::new();
    let mut p = 0;
    while let Some(i) = cerca(d, &FIRMA, p) {
        p = i + 1;
        if i + 26 > d.len() {
            break;
        }
        let crc = u32_at(d, i + 10);
        let (cs, us, ps) = (u32_at(d, i + 14) as usize, u32_at(d, i + 18) as usize, u32_at(d, i + 22) as usize);
        let inizio = i + 26 + ps;
        if ps > 1024 || inizio + cs > d.len() {
            continue;
        }
        // il nome ha i due mezzi byte di ogni byte scambiati
        let nome: Vec<u8> = d[i + 26..inizio].iter().map(|b| b.rotate_left(4)).collect();
        out.push(Blocco { nome, crc, compresso: inizio..inizio + cs, dimensione: us });
    }
    out
}

/// Il contenuto decompresso di un blocco, controllato con il suo crc32.
fn decomprimi(d: &[u8], b: &Blocco) -> Option<Vec<u8>> {
    let mut fuori = Vec::with_capacity(b.dimensione);
    flate2::read::DeflateDecoder::new(&d[b.compresso.clone()]).read_to_end(&mut fuori).ok()?;
    let mut c = flate2::Crc::new();
    c.update(&fuori);
    (fuori.len() == b.dimensione && c.sum() == b.crc).then_some(fuori)
}

/// Una faccia letta dalle liste di visualizzazione: (inizio delle posizioni, inizio delle normali, lunghezze degli strip, fine del record).
struct Faccia {
    posizioni: usize,
    normali: usize,
    n: usize,
    strip: Vec<usize>,
    fine: usize,
}

/// Prova a leggere una faccia con l'intestazione delle posizioni in `s`.
fn leggi_faccia(d: &[u8], s: usize) -> Option<Faccia> {
    let l = d.len();
    if s + 16 > l {
        return None;
    }
    let n = u32_at(d, s + 12) as usize;
    if !(3..=50_000_000).contains(&n) {
        return None;
    }
    let normali = s + 16 + 12 * n; // intestazione delle normali
    if normali + 16 > l || d[normali..normali + 12] != ARRAY_F32X3 || u32_at(d, normali + 12) as usize != n {
        return None;
    }
    let dati_normali = normali + 16;
    if dati_normali + 12 * n > l {
        return None;
    }
    // le normali hanno lunghezza uno
    for k in 0..n.min(8) {
        let (x, y, z) = (f32_at(d, dati_normali + 12 * k), f32_at(d, dati_normali + 12 * k + 4), f32_at(d, dati_normali + 12 * k + 8));
        if ((x * x + y * y + z * z).sqrt() - 1.0).abs() > 0.01 {
            return None;
        }
    }
    // tabella dei descrittori (si salta) e poi le lunghezze degli strip
    let desc = dati_normali + 12 * n;
    if desc + 16 > l || d[desc..desc + 12] != ARRAY_U32 {
        return None;
    }
    let c = u32_at(d, desc + 12) as usize;
    let strip = desc + 16 + 4 * c;
    if strip + 16 > l || d[strip..strip + 12] != ARRAY_U32 {
        return None;
    }
    let k = u32_at(d, strip + 12) as usize;
    if k == 0 || strip + 16 + 4 * k > l {
        return None;
    }
    // ogni valore e' 2L - 2: uno strip di L vertici
    let mut lunghezze = Vec::with_capacity(k);
    let mut somma = 0usize;
    for j in 0..k {
        let v = u32_at(d, strip + 16 + 4 * j) as usize;
        if v < 4 || !v.is_multiple_of(2) {
            return None;
        }
        lunghezze.push(v / 2 + 1);
        somma += v / 2 + 1;
    }
    (somma == n).then_some(Faccia { posizioni: s + 16, normali: dati_normali, n, strip: lunghezze, fine: strip + 16 + 4 * k })
}

/// Tutte le facce delle liste di visualizzazione, come un'unica mesh in millimetri.
fn mesh_da_liste(d: &[u8]) -> (Vec<f32>, Vec<u32>) {
    let mut vertici: Vec<f32> = Vec::new();
    let mut triangoli: Vec<u32> = Vec::new();
    let mut i = 0;
    while let Some(s) = cerca(d, &ARRAY_F32X3, i) {
        i = s + 1;
        let Some(f) = leggi_faccia(d, s) else { continue };
        i = f.fine;
        let base = (vertici.len() / 3) as u32;
        for k in 0..f.n {
            for c in 0..3 {
                vertici.push(f32_at(d, f.posizioni + 12 * k + 4 * c) * 1000.0); // da metri a millimetri
            }
        }
        let punto = |k: usize| -> [f32; 3] { [f32_at(d, f.posizioni + 12 * k), f32_at(d, f.posizioni + 12 * k + 4), f32_at(d, f.posizioni + 12 * k + 8)] };
        let normale = |k: usize| -> [f32; 3] { [f32_at(d, f.normali + 12 * k), f32_at(d, f.normali + 12 * k + 4), f32_at(d, f.normali + 12 * k + 8)] };
        let mut inizio = 0usize;
        for &l in &f.strip {
            for j in 0..l - 2 {
                // gli strip alternano il verso dei triangoli
                let (a, mut b, mut c) = if j % 2 == 0 { (inizio + j, inizio + j + 1, inizio + j + 2) } else { (inizio + j + 1, inizio + j, inizio + j + 2) };
                // il verso giusto e' quello che concorda con le normali dei vertici
                let (pa, pb, pc) = (punto(a), punto(b), punto(c));
                let (u, v) = ([pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]], [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]]);
                let g = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                let (na, nb, nc) = (normale(a), normale(b), normale(c));
                let n = [na[0] + nb[0] + nc[0], na[1] + nb[1] + nc[1], na[2] + nb[2] + nc[2]];
                if g[0] * n[0] + g[1] * n[1] + g[2] * n[2] < 0.0 {
                    std::mem::swap(&mut b, &mut c);
                }
                // un triangolo schiacciato (due vertici uguali) non serve
                if pa != pb && pb != pc && pa != pc {
                    triangoli.extend_from_slice(&[base + a as u32, base + b as u32, base + c as u32]);
                }
            }
            inizio += l;
        }
    }
    (vertici, triangoli)
}

/// Legge un file `.sldprt` e restituisce una parte con la mesh di visualizzazione (in millimetri).
pub fn carica(percorso: &Path) -> Result<Vec<Parte>> {
    let d = std::fs::read(percorso).with_context(|| format!("impossibile aprire {}", percorso.display()))?;
    if cerca(&d, &FIRMA, 0).is_none() {
        bail!("file SolidWorks non riconosciuto: questo lettore funziona con i file .sldprt salvati da SolidWorks 2015 o successivo");
    }
    let tutti = blocchi(&d);
    let Some(liste) = tutti.iter().filter(|b| b.nome == NOME_DISPLAY_LISTS).find_map(|b| decomprimi(&d, b)) else {
        bail!("nel file SolidWorks non ci sono le liste di visualizzazione (Contents/DisplayLists): serve un file salvato da SolidWorks con la grafica");
    };
    let (vertici, triangoli) = mesh_da_liste(&liste);
    if triangoli.is_empty() {
        bail!("le liste di visualizzazione del file SolidWorks non contengono superfici leggibili (solidi importati da STEP o formato non riconosciuto)");
    }
    let nome = percorso.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "SolidWorks".into());
    Ok(vec![Parte { nome, colore: None, vertici, triangoli }])
}

#[cfg(test)]
mod test {
    use super::*;
    use std::io::Write;

    fn dword(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn file_di_prova(percorso: &Path, lista: &[u8]) {
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(lista).unwrap();
        let compresso = enc.finish().unwrap();
        let mut crc = flate2::Crc::new();
        crc.update(lista);
        let nome: Vec<u8> = NOME_DISPLAY_LISTS.iter().map(|b| b.rotate_left(4)).collect();
        let mut f = FIRMA.to_vec();
        f.extend(dword(&[0x7fe0_0000, crc.sum(), compresso.len() as u32, lista.len() as u32, nome.len() as u32]));
        f.extend(nome);
        f.extend(compresso);
        std::fs::write(percorso, f).unwrap();
    }

    #[test]
    fn legge_una_faccia_a_strip() {
        // un quadrato di 100 x 50 mm in uno strip di 4 vertici, con le normali verso +z
        let file = std::env::temp_dir().join("robo_stp_prova.sldprt");
        let mut lista = Vec::new();
        lista.extend_from_slice(&[9, 9, 9]);
        let pos = [[0.0f32, 0.0, 0.0], [0.1, 0.0, 0.0], [0.0, 0.05, 0.0], [0.1, 0.05, 0.0]];
        for tipo in 0..2 {
            lista.extend_from_slice(&ARRAY_F32X3);
            lista.extend_from_slice(&4u32.to_le_bytes());
            for p in pos {
                for c in if tipo == 0 { p } else { [0.0, 0.0, 1.0] } {
                    lista.extend_from_slice(&c.to_le_bytes());
                }
            }
        }
        lista.extend_from_slice(&ARRAY_U32);
        lista.extend(dword(&[6, 1, 9, 193, 0, 194, 142]));
        lista.extend_from_slice(&ARRAY_U32);
        lista.extend(dword(&[1, 6]));
        file_di_prova(&file, &lista);
        let parti = carica(&file).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!(parti.len(), 1);
        let p = &parti[0];
        assert_eq!(p.vertici.len(), 12);
        assert_eq!(p.triangoli.len(), 6, "uno strip di 4 vertici = 2 triangoli");
        assert!((p.vertici[3] - 100.0).abs() < 1e-3, "da metri a millimetri");
        // il verso concorda con le normali (+z): il prodotto vettoriale del primo triangolo punta in su
        let v = |i: u32| &p.vertici[3 * i as usize..3 * i as usize + 3];
        let (a, b, c) = (v(p.triangoli[0]), v(p.triangoli[1]), v(p.triangoli[2]));
        let z = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        assert!(z > 0.0);
    }

    #[test]
    fn file_non_solidworks_da_errore() {
        let file = std::env::temp_dir().join("robo_stp_prova_non_sw.sldprt");
        std::fs::write(&file, b"questo non e' un file SolidWorks").unwrap();
        let e = carica(&file).expect_err("deve fallire").to_string();
        std::fs::remove_file(&file).ok();
        assert!(e.contains("non riconosciuto"), "{e}");
    }

    #[test]
    fn senza_liste_di_visualizzazione_da_errore() {
        let file = std::env::temp_dir().join("robo_stp_prova_senza_liste.sldprt");
        let mut f = FIRMA.to_vec();
        f.extend(dword(&[0, 0, 0, 0, 0]));
        std::fs::write(&file, f).unwrap();
        let e = carica(&file).expect_err("deve fallire").to_string();
        std::fs::remove_file(&file).ok();
        assert!(e.contains("liste di visualizzazione"), "{e}");
    }
}
