//! Lettura dei file `.max` di 3ds Max, senza 3ds Max.
//!
//! Il formato non e' documentato da Autodesk: questa lettura e' stata ricostruita analizzando file reali.
//! E' un contenitore OLE (CFB) con uno stream «Scene» fatto di blocchi (id u16, lunghezza u32 o u64, bit alto = contiene altri blocchi).
//! Si leggono: nomi e gerarchia dei nodi, posizione/rotazione/scala (controller fissi), offset dell'oggetto,
//! e la geometria degli oggetti **Editable Mesh** e **Editable Poly** (anche sotto una pila di modificatori: si prende
//! l'oggetto di base, i modificatori non vengono applicati). Spline, forme, luci, camere e texture non si importano; dei materiali si legge solo il colore diffuso dei materiali V-Ray (uno per oggetto).

use crate::ffi::{Parte, Texture};
use crate::rstp::Voce;
use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
struct Blocco {
    id: u16,
    contenitore: bool,
    s: usize,
    e: usize,
}

/// Se lo stream e' compresso con gzip (inizia con 1f 8b) lo decomprime, altrimenti lo lascia com'e'.
fn decomprimi(dati: Vec<u8>) -> Result<Vec<u8>> {
    if dati.len() > 2 && dati[0] == 0x1f && dati[1] == 0x8b {
        let mut fuori = Vec::with_capacity(dati.len() * 4);
        flate2::read::MultiGzDecoder::new(&dati[..]).read_to_end(&mut fuori).context("lo stream compresso del file .max non si decomprime")?;
        return Ok(fuori);
    }
    Ok(dati)
}

fn u32_at(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]])
}

fn f32_at(b: &[u8], p: usize) -> f32 {
    f32::from_bits(u32_at(b, p))
}

/// I blocchi che stanno tra `off` e `fine` (solo il primo livello).
fn blocchi(b: &[u8], mut off: usize, fine: usize) -> Vec<Blocco> {
    let mut v = Vec::new();
    while off + 6 <= fine {
        let id = u16::from_le_bytes([b[off], b[off + 1]]);
        let l32 = u32_at(b, off + 2);
        let (len, cont, intest) = if l32 == 0 {
            if off + 14 > fine {
                break;
            }
            let l = u64::from_le_bytes(b[off + 6..off + 14].try_into().unwrap());
            ((l & 0x7fff_ffff_ffff_ffff) as usize, l >> 63 == 1, 14)
        } else {
            ((l32 & 0x7fff_ffff) as usize, l32 >> 31 == 1, 6)
        };
        if len < intest || off + len > fine {
            break;
        }
        v.push(Blocco { id, contenitore: cont, s: off + intest, e: off + len });
        off += len;
    }
    v
}

fn figli(b: &[u8], c: &Blocco) -> Vec<Blocco> {
    if c.contenitore {
        blocchi(b, c.s, c.e)
    } else {
        Vec::new()
    }
}

fn figlio(b: &[u8], c: &Blocco, id: u16) -> Option<Blocco> {
    figli(b, c).into_iter().find(|x| x.id == id)
}

fn dword_lista(b: &[u8], c: &Blocco) -> Vec<u32> {
    (c.s..c.e - (c.e - c.s) % 4).step_by(4).map(|p| u32_at(b, p)).collect()
}

#[derive(Clone, Debug)]
struct Classe {
    a: u32,
    b: u32,
    superclasse: u32,
}

fn leggi_classi(dati: &[u8]) -> Vec<Classe> {
    blocchi(dati, 0, dati.len())
        .iter()
        .map(|c| {
            let mut k = Classe { a: 0, b: 0, superclasse: 0 };
            for f in figli(dati, c) {
                if f.id == 0x2060 && f.e - f.s >= 16 {
                    k = Classe { a: u32_at(dati, f.s + 4), b: u32_at(dati, f.s + 8), superclasse: u32_at(dati, f.s + 12) };
                }
            }
            k
        })
        .collect()
}

// Identificativi delle classi (a, b) che servono.
const CL_MESH: (u32, u32) = (0xe44f_10b3, 0);
const CL_POLY: (u32, u32) = (0x1bf8_338d, 0x192f_6098);
const CL_NODO: (u32, u32) = (1, 0);
const CL_VRAY: (u32, u32) = (0x37bf_3f2f, 0x7034_695c);
const CL_DERIVATO: u16 = 0x2032; // «oggetto derivato»: pila di modificatori con l'oggetto di base per ultimo

/// Matrice 4x3 con i vettori riga come in 3ds Max: p' = p * M.
#[derive(Clone, Copy, Debug)]
struct Mat([[f64; 3]; 4]);

impl Mat {
    const ID: Mat = Mat([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]]);
    /// `self` poi `o` (come in 3ds Max: p * self * o).
    fn per(&self, o: &Mat) -> Mat {
        let mut r = [[0.0; 3]; 4];
        for (i, riga) in r.iter_mut().enumerate() {
            for (j, cella) in riga.iter_mut().enumerate() {
                let mut s = 0.0;
                for k in 0..3 {
                    s += self.0[i][k] * o.0[k][j];
                }
                if i == 3 {
                    s += o.0[3][j];
                }
                *cella = s;
            }
        }
        Mat(r)
    }
    fn applica(&self, p: [f64; 3]) -> [f64; 3] {
        let m = &self.0;
        [
            p[0] * m[0][0] + p[1] * m[1][0] + p[2] * m[2][0] + m[3][0],
            p[0] * m[0][1] + p[1] * m[1][1] + p[2] * m[2][1] + m[3][1],
            p[0] * m[0][2] + p[1] * m[1][2] + p[2] * m[2][2] + m[3][2],
        ]
    }
    fn scala(s: [f64; 3]) -> Mat {
        Mat([[s[0], 0.0, 0.0], [0.0, s[1], 0.0], [0.0, 0.0, s[2]], [0.0, 0.0, 0.0]])
    }
    fn trasla(t: [f64; 3]) -> Mat {
        Mat([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], t])
    }
    /// Rotazione antioraria di `a` radianti attorno a un asse (0 = X, 1 = Y, 2 = Z), con i vettori riga.
    fn ruota(asse: usize, a: f64) -> Mat {
        let (s, c) = a.sin_cos();
        let mut m = Mat::ID;
        let (i, j) = match asse {
            0 => (1, 2),
            1 => (2, 0),
            _ => (0, 1),
        };
        m.0[i][i] = c;
        m.0[i][j] = s;
        m.0[j][i] = -s;
        m.0[j][j] = c;
        m
    }
    fn da_quaternione(q: [f64; 4]) -> Mat {
        let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if n < 1e-9 {
            return Mat::ID;
        }
        let (x, y, z, w) = (q[0] / n, q[1] / n, q[2] / n, q[3] / n);
        // il quaternione di 3ds Max descrive la rotazione con il segno opposto: si usa il coniugato
        let (x, y, z) = (-x, -y, -z);
        Mat([
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
            [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
            [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
            [0.0, 0.0, 0.0],
        ])
    }
}

/// Un nodo della scena.
struct Nodo {
    nome: String,
    padre: usize,
    riferimenti: HashMap<u32, usize>,
    offset: Mat,
}

/// Geometria di un oggetto, nel suo sistema locale.
#[derive(PartialEq)]
struct Geometria {
    vertici: Vec<[f32; 3]>,
    triangoli: Vec<[u32; 3]>,
    /// Coordinate di mappatura (u, v) dei tre angoli di ogni triangolo, se sono state chieste e il file le ha.
    uv: Option<Vec<[[f32; 2]; 3]>>,
}

/// Cosa e' successo nella lettura.
#[derive(Debug, Default, Clone)]
pub struct Resoconto {
    pub nodi: usize,
    pub con_geometria: usize,
    pub non_supportati: usize,
    pub facce_illeggibili: usize,
    /// Oggetti ripetuti messi come istanze di una definizione.
    pub istanze: usize,
    pub scartati_piccoli: usize,
    pub scartati_pesanti: usize,
    /// Oggetti la cui geometria e' stata semplificata.
    pub semplificati: usize,
    /// Triangoli che avrebbe il modello senza la semplificazione.
    pub triangoli_origine: usize,
}

/// Come leggere un file `.max`.
#[derive(Clone, Debug)]
pub struct OpzioniMax {
    pub mm_per_unita: f64,
    /// Gli oggetti ripetuti (stesso oggetto base in piu' nodi) diventano una definizione e delle istanze.
    pub istanze: bool,
    /// Semplifica gli oggetti piu' pesanti finche' il totale e' al massimo questo numero di triangoli.
    pub max_triangoli: Option<usize>,
    /// Ignora gli oggetti la cui dimensione maggiore e' sotto questi millimetri.
    pub dimensione_minima_mm: Option<f64>,
    /// Ignora gli oggetti con piu' di questi triangoli.
    pub oggetto_max_triangoli: Option<usize>,
    /// Ignora gli oggetti piu' piccoli di questa frazione della scena (per l'anteprima).
    pub piccoli_relativi: Option<f64>,
    /// Legge anche l'immagine dei materiali e la mappatura (UV).
    pub uv: bool,
}

impl OpzioniMax {
    pub fn semplici(mm_per_unita: f64) -> OpzioniMax {
        OpzioniMax { mm_per_unita, istanze: false, max_triangoli: None, dimensione_minima_mm: None, oggetto_max_triangoli: None, piccoli_relativi: None, uv: false }
    }
}

pub struct FileMax {
    scena: Vec<u8>,
    blocchi: Vec<Blocco>,
    classi: Vec<Classe>,
    /// Le immagini del file: identificativo (16 byte) -> percorso, dallo stream «FileAssetMetaData3».
    immagini: Vec<([u8; 16], String)>,
}

impl FileMax {
    pub fn apri(percorso: &Path) -> Result<FileMax> {
        let file = std::fs::File::open(percorso).with_context(|| format!("impossibile aprire {}", percorso.display()))?;
        let mut cfb = cfb::CompoundFile::open(file).context("non e' un file .max valido (contenitore OLE non leggibile)")?;
        let mut dir = Vec::new();
        cfb.open_stream("/ClassDirectory3").context("manca ClassDirectory3: file .max non riconosciuto")?.read_to_end(&mut dir)?;
        let dir = decomprimi(dir)?;
        let classi = leggi_classi(&dir);
        let mut scena = Vec::new();
        {
            let mut s = cfb.open_stream("/Scene").context("manca lo stream Scene: file .max non riconosciuto")?;
            scena.reserve(s.len() as usize);
            s.read_to_end(&mut scena)?;
        }
        // le versioni piu' recenti di 3ds Max comprimono gli stream con gzip
        let scena = decomprimi(scena)?;
        let alto = blocchi(&scena, 0, scena.len());
        let radice = alto.first().filter(|b| b.contenitore).context("la scena non ha la struttura attesa (file di una versione di 3ds Max non riconosciuta?)")?;
        let blocchi = blocchi_di(&scena, radice);
        let immagini = {
            let mut d = Vec::new();
            match cfb.open_stream("/FileAssetMetaData3") {
                Ok(mut st) => {
                    let _ = st.read_to_end(&mut d);
                    leggi_immagini(&decomprimi(d).unwrap_or_default())
                }
                Err(_) => Vec::new(),
            }
        };
        Ok(FileMax { scena, blocchi, classi, immagini })
    }

    fn classe(&self, pos: usize) -> Option<(u32, u32)> {
        let id = self.blocchi.get(pos)?.id as usize;
        self.classi.get(id).map(|c| (c.a, c.b))
    }

    fn riferimenti(&self, pos: usize) -> Vec<u32> {
        figlio(&self.scena, &self.blocchi[pos], 0x2034).map(|c| dword_lista(&self.scena, &c)).unwrap_or_default()
    }

    /// Valore fisso di un controller di numeri decimali (non animato).
    fn valore_float(&self, pos: usize) -> f64 {
        let b = &self.scena;
        let Some(c) = self.blocchi.get(pos) else { return 0.0 };
        // il valore sta direttamente nel controller (0x2501) oppure dentro un blocco 0x7127, secondo la versione di 3ds Max
        let valore = |p: &Blocco| match figlio(b, p, 0x2501) {
            Some(v) if v.e - v.s >= 4 => Some(f64::from(f32_at(b, v.s))),
            _ => None,
        };
        valore(c).or_else(|| figlio(b, c, 0x7127).and_then(|k| valore(&k))).unwrap_or(0.0)
    }

    /// Posizione/rotazione/scala locali di un nodo, dal suo controller (solo valori fissi).
    fn matrice_locale(&self, ctrl: usize) -> Mat {
        let b = &self.scena;
        let refs = self.riferimenti(ctrl);
        let mut t = [0.0; 3];
        let mut r = [0.0; 3];
        let mut s = [1.0; 3];
        for rif in refs {
            let rif = rif as usize;
            match self.classe(rif) {
                Some((0x118f_7e02, _)) => {
                    let f = self.riferimenti(rif);
                    if f.len() == 3 {
                        t = [self.valore_float(f[0] as usize), self.valore_float(f[1] as usize), self.valore_float(f[2] as usize)];
                    }
                }
                Some((0x2012, _)) => {
                    let f = self.riferimenti(rif);
                    if f.len() == 3 {
                        r = [self.valore_float(f[0] as usize), self.valore_float(f[1] as usize), self.valore_float(f[2] as usize)];
                    }
                }
                Some((0x2010, _)) => {
                    if let Some(v) = figlio(b, &self.blocchi[rif], 0x2505) {
                        if v.e - v.s >= 12 {
                            s = [f64::from(f32_at(b, v.s)), f64::from(f32_at(b, v.s + 4)), f64::from(f32_at(b, v.s + 8))];
                        }
                    }
                }
                _ => {}
            }
        }
        Mat::scala(s).per(&Mat::ruota(0, r[0])).per(&Mat::ruota(1, r[1])).per(&Mat::ruota(2, r[2])).per(&Mat::trasla(t))
    }

    fn nodi(&self) -> HashMap<usize, Nodo> {
        let b = &self.scena;
        let mut m = HashMap::new();
        for (pos, c) in self.blocchi.iter().enumerate() {
            if self.classe(pos) != Some(CL_NODO) || self.classi[c.id as usize].superclasse != 1 {
                continue;
            }
            let mut n = Nodo { nome: String::new(), padre: usize::MAX, riferimenti: HashMap::new(), offset: Mat::ID };
            let (mut po, mut ro, mut so) = ([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
            for f in figli(b, c) {
                let len = f.e - f.s;
                match f.id {
                    0x962 => {
                        let u: Vec<u16> = (f.s..f.e - len % 2).step_by(2).map(|p| u16::from_le_bytes([b[p], b[p + 1]])).collect();
                        n.nome = String::from_utf16_lossy(&u);
                    }
                    0x960 if len >= 4 => n.padre = u32_at(b, f.s) as usize,
                    0x2035 => {
                        let w = dword_lista(b, &f);
                        for p in w.get(1..).unwrap_or(&[]).chunks(2) {
                            if p.len() == 2 {
                                n.riferimenti.insert(p[0], p[1] as usize);
                            }
                        }
                    }
                    0x96a if len >= 12 => po = [f64::from(f32_at(b, f.s)), f64::from(f32_at(b, f.s + 4)), f64::from(f32_at(b, f.s + 8))],
                    0x96b if len >= 16 => ro = [f64::from(f32_at(b, f.s)), f64::from(f32_at(b, f.s + 4)), f64::from(f32_at(b, f.s + 8)), f64::from(f32_at(b, f.s + 12))],
                    0x96c if len >= 12 => so = [f64::from(f32_at(b, f.s)), f64::from(f32_at(b, f.s + 4)), f64::from(f32_at(b, f.s + 8))],
                    _ => {}
                }
            }
            n.offset = Mat::scala(so).per(&Mat::da_quaternione(ro)).per(&Mat::trasla(po));
            m.insert(pos, n);
        }
        m
    }

    /// L'oggetto di base (Editable Mesh o Editable Poly) di un nodo, scendendo la pila dei modificatori.
    fn oggetto_base(&self, nodo: &Nodo) -> Option<usize> {
        let mut pos = *nodo.riferimenti.get(&1)?;
        for _ in 0..32 {
            let c = self.blocchi.get(pos)?;
            if c.id == CL_DERIVATO {
                pos = *self.riferimenti(pos).last()? as usize;
                continue;
            }
            return match self.classe(pos) {
                Some(CL_MESH) | Some(CL_POLY) => Some(pos),
                _ => None,
            };
        }
        None
    }

    /// Il colore del materiale assegnato a un nodo (nel riferimento 3): quello «diffuso» del primo materiale V-Ray che si trova,
    /// anche dentro Multi/Sub-Object, Double Sided e simili (di un Multi/Sub-Object si usa il primo sotto-materiale).
    /// I materiali Standard e le texture non si leggono: in quel caso None.
    fn colore_nodo(&self, nodo: &Nodo) -> Option<[f32; 4]> {
        let mut coda = vec![(*nodo.riferimenti.get(&3)?, 0usize)];
        let mut visti = HashSet::new();
        while !coda.is_empty() {
            let (pos, livello) = coda.remove(0);
            if pos >= self.blocchi.len() || !visti.insert(pos) {
                continue;
            }
            if self.classe(pos) == Some(CL_VRAY) {
                return self.diffuso_vray(pos);
            }
            if livello < 4 {
                for r in self.riferimenti_slot(pos) {
                    coda.push((r, livello + 1));
                }
            }
        }
        None
    }

    /// I riferimenti di un blocco: sia nella forma «coppie (posizione, riferimento)» (0x2035) sia in quella a elenco (0x2034).
    fn riferimenti_slot(&self, pos: usize) -> Vec<usize> {
        let b = &self.scena;
        let c = &self.blocchi[pos];
        if let Some(f) = figlio(b, c, 0x2035) {
            let w = dword_lista(b, &f);
            return w.get(1..).unwrap_or(&[]).chunks(2).filter(|p| p.len() == 2).map(|p| p[1] as usize).collect();
        }
        self.riferimenti(pos).into_iter().map(|r| r as usize).collect()
    }

    /// Il colore «diffuso» di un materiale V-Ray: il parametro 1 (tipo colore) del suo primo ParamBlock2.
    fn diffuso_vray(&self, pos: usize) -> Option<[f32; 4]> {
        let b = &self.scena;
        let f = figlio(b, &self.blocchi[pos], 0x2035)?;
        let w = dword_lista(b, &f);
        let blocco = w.get(1..)?.chunks(2).find(|p| p.len() == 2 && p[0] == 1).map(|p| p[1] as usize)?;
        let pb = self.blocchi.get(blocco)?;
        for e in figli(b, pb) {
            // id (u16), tipo (u16), 11 byte di intestazione, poi i valori; il tipo 2 e' un colore (3 numeri decimali)
            if (e.id == 0x100e || e.id == 0x000e) && e.e - e.s >= 27 && u16::from_le_bytes([b[e.s], b[e.s + 1]]) == 1 && u16::from_le_bytes([b[e.s + 2], b[e.s + 3]]) == 2 {
                let c = |i: usize| f32_at(b, e.s + 15 + i * 4).clamp(0.0, 1.0);
                return Some([c(0), c(1), c(2), 1.0]);
            }
        }
        None
    }

    /// L'immagine (texture) del materiale di un nodo: la prima immagine che si trova scendendo nel materiale,
    /// anche dentro Multi/Sub-Object e simili. Il percorso e' quello salvato nel file .max.
    fn immagine_nodo(&self, nodo: &Nodo) -> Option<String> {
        if self.immagini.is_empty() {
            return None;
        }
        let mut coda = vec![(*nodo.riferimenti.get(&3)?, 0usize)];
        let mut visti = HashSet::new();
        while !coda.is_empty() {
            let (pos, livello) = coda.remove(0);
            if pos >= self.blocchi.len() || !visti.insert(pos) {
                continue;
            }
            let c = &self.blocchi[pos];
            let dati = &self.scena[c.s..c.e];
            if let Some((_, nome)) = self.immagini.iter().find(|(g, _)| dati.windows(16).any(|w| w == g)) {
                return Some(nome.clone());
            }
            if livello < 5 {
                for r in self.riferimenti_slot(pos) {
                    coda.push((r, livello + 1));
                }
            }
        }
        None
    }

    /// Il canale di mappatura 1 di un oggetto Editable Mesh: (coordinate, facce di coordinate).
    fn uv_mesh(&self, m: &Blocco) -> Option<(Vec<[f32; 2]>, Vec<[u32; 3]>)> {
        let b = &self.scena;
        let tutti = figli(b, m);
        let da = tutti.iter().position(|f| f.id == 0x959 && f.e - f.s >= 4 && u32_at(b, f.s) == 1)?;
        let (mut punti, mut facce) = (None, None);
        for f in tutti[da + 1..].iter().take_while(|f| f.id != 0x959) {
            match f.id {
                0x2394 if f.e - f.s >= 4 => {
                    let n = u32_at(b, f.s) as usize;
                    if f.e - f.s >= 4 + n * 12 {
                        punti = Some((0..n).map(|i| [f32_at(b, f.s + 4 + i * 12), f32_at(b, f.s + 8 + i * 12)]).collect::<Vec<_>>());
                    }
                }
                0x2396 if f.e - f.s >= 4 => {
                    let n = u32_at(b, f.s) as usize;
                    if f.e - f.s >= 4 + n * 12 {
                        facce = Some((0..n).map(|i| [u32_at(b, f.s + 4 + i * 12), u32_at(b, f.s + 8 + i * 12), u32_at(b, f.s + 12 + i * 12)]).collect::<Vec<_>>());
                    }
                }
                _ => {}
            }
        }
        Some((punti?, facce?))
    }

    /// Il canale di mappatura 1 di un oggetto Editable Poly: (coordinate, per ogni faccia gli indici delle coordinate dei suoi angoli).
    fn uv_poly(&self, m: &Blocco) -> Option<(Vec<[f32; 2]>, Vec<Vec<u32>>)> {
        let b = &self.scena;
        let tutti = figli(b, m);
        let da = tutti.iter().position(|f| f.id == 0x124 && f.e - f.s >= 4 && u32_at(b, f.s) == 1)?;
        let (mut punti, mut facce) = (None, None);
        for f in tutti[da + 1..].iter().take_while(|f| f.id != 0x124) {
            match f.id {
                0x128 if f.e - f.s >= 4 => {
                    let n = u32_at(b, f.s) as usize;
                    if f.e - f.s >= 4 + n * 12 {
                        punti = Some((0..n).map(|i| [f32_at(b, f.s + 4 + i * 12), f32_at(b, f.s + 8 + i * 12)]).collect::<Vec<_>>());
                    }
                }
                0x12b => {
                    let mut v = Vec::new();
                    let mut p = f.s;
                    while p + 4 <= f.e {
                        let g = u32_at(b, p) as usize;
                        if g > 4096 || p + 4 + 4 * g > f.e {
                            return None;
                        }
                        v.push((0..g).map(|k| u32_at(b, p + 4 + 4 * k)).collect::<Vec<u32>>());
                        p += 4 + 4 * g;
                    }
                    facce = Some(v);
                }
                _ => {}
            }
        }
        Some((punti?, facce?))
    }

    fn geometria(&self, pos: usize, uv: bool) -> Option<(Geometria, bool)> {
        match self.classe(pos)? {
            CL_MESH => self.mesh(pos, uv).map(|g| (g, true)),
            _ => self.poligoni(pos, uv),
        }
    }

    fn mesh(&self, pos: usize, con_uv: bool) -> Option<Geometria> {
        let b = &self.scena;
        let m = figlio(b, &self.blocchi[pos], 0x8fe)?;
        let v = figlio(b, &m, 0x914)?;
        let f = figlio(b, &m, 0x912)?;
        let nv = u32_at(b, v.s) as usize;
        let nf = u32_at(b, f.s) as usize;
        if v.e - v.s < 4 + nv * 12 || f.e - f.s < 4 + nf * 20 {
            return None;
        }
        let vertici = (0..nv).map(|i| [f32_at(b, v.s + 4 + i * 12), f32_at(b, v.s + 8 + i * 12), f32_at(b, v.s + 12 + i * 12)]).collect();
        let mappa = if con_uv { self.uv_mesh(&m) } else { None };
        let mut triangoli = Vec::with_capacity(nf);
        let mut uv: Option<Vec<[[f32; 2]; 3]>> = mappa.as_ref().map(|_| Vec::with_capacity(nf));
        for i in 0..nf {
            let p = f.s + 4 + i * 20;
            let t = [u32_at(b, p), u32_at(b, p + 4), u32_at(b, p + 8)];
            if t.iter().all(|&x| (x as usize) < nv) {
                triangoli.push(t);
                if let (Some((punti, facce)), Some(u)) = (&mappa, uv.as_mut()) {
                    let angolo = |k: usize| facce.get(i).and_then(|tf| punti.get(tf[k] as usize)).copied().unwrap_or([0.0, 0.0]);
                    u.push([angolo(0), angolo(1), angolo(2)]);
                }
            }
        }
        Some(Geometria { vertici, triangoli, uv })
    }

    /// Editable Poly: (geometria, true se le facce sono state lette tutte).
    fn poligoni(&self, pos: usize, con_uv: bool) -> Option<(Geometria, bool)> {
        let b = &self.scena;
        let m = figlio(b, &self.blocchi[pos], 0x8fe)?;
        let v = figlio(b, &m, 0x100)?;
        let f = figlio(b, &m, 0x11a)?;
        let nv = u32_at(b, v.s) as usize;
        // vertici: flags u32 e, se il vertice non e' cancellato (bit 1), x y z
        let mut vertici = Vec::with_capacity(nv);
        let mut p = v.s + 4;
        for _ in 0..nv {
            if p + 4 > v.e {
                return None;
            }
            let fl = u32_at(b, p);
            p += 4;
            if fl & 2 != 0 {
                vertici.push([0.0; 3]);
            } else {
                if p + 12 > v.e {
                    return None;
                }
                vertici.push([f32_at(b, p), f32_at(b, p + 4), f32_at(b, p + 8)]);
                p += 12;
            }
        }
        let (facce, ok) = leggi_facce(&b[f.s..f.e], nv)?;
        let mappa = if con_uv { self.uv_poly(&m).filter(|(_, tf)| tf.len() == facce.len()) } else { None };
        let mut triangoli = Vec::new();
        let mut uv: Option<Vec<[[f32; 2]; 3]>> = mappa.as_ref().map(|_| Vec::new());
        for (i, fc) in facce.iter().enumerate() {
            let prima = triangoli.len();
            triangola(fc, &vertici, &mut triangoli);
            if let (Some((punti, tf)), Some(u)) = (&mappa, uv.as_mut()) {
                // l'angolo di un triangolo e' un vertice della faccia: la sua coordinata e' quella allo stesso posto nella faccia delle coordinate
                let angolo = |v: u32| fc.iter().position(|&x| x == v).and_then(|k| tf[i].get(k)).and_then(|&c| punti.get(c as usize)).copied().unwrap_or([0.0, 0.0]);
                for t in &triangoli[prima..] {
                    u.push([angolo(t[0]), angolo(t[1]), angolo(t[2])]);
                }
            }
        }
        Some((Geometria { vertici, triangoli, uv }, ok))
    }

    /// Legge il file e restituisce le voci (in millimetri) e il resoconto.
    pub fn parti(&self, o: &OpzioniMax) -> (Vec<Voce>, Resoconto) {
        let nodi = self.nodi();
        let mut res = Resoconto { nodi: nodi.len(), ..Resoconto::default() };
        let mut mondo: HashMap<usize, Mat> = HashMap::new();
        let mut cache: HashMap<usize, Option<(Geometria, bool)>> = HashMap::new();
        let mut posizioni: Vec<usize> = nodi.keys().copied().collect();
        posizioni.sort_unstable();
        // le geometrie di tutti gli oggetti si leggono insieme, su piu' processori
        let mut basi: Vec<usize> = posizioni.iter().filter_map(|p| self.oggetto_base(&nodi[p])).collect();
        basi.sort_unstable();
        basi.dedup();
        for (b, g) in basi.iter().copied().zip(in_parallelo(&basi, |&b| self.geometria(b, o.uv))) {
            cache.insert(b, g);
        }
        // 1. i nodi con una geometria, e i filtri
        struct Uso {
            pos: usize,
            base: usize,
            m: Mat,
        }
        let mut usi: Vec<Uso> = Vec::new();
        for pos in posizioni {
            let n = &nodi[&pos];
            let Some(base) = self.oggetto_base(n) else {
                if n.riferimenti.contains_key(&1) {
                    res.non_supportati += 1;
                }
                continue;
            };
            let g = cache.entry(base).or_insert_with(|| self.geometria(base, o.uv));
            let Some((g, ok)) = g else {
                res.non_supportati += 1;
                continue;
            };
            if !*ok {
                res.facce_illeggibili += 1;
            }
            if g.triangoli.is_empty() {
                continue;
            }
            if o.oggetto_max_triangoli.is_some_and(|max| g.triangoli.len() > max) {
                res.scartati_pesanti += 1;
                continue;
            }
            let m = n.offset.per(&self.matrice_mondo(pos, &nodi, &mut mondo));
            if let Some(minimo) = o.dimensione_minima_mm {
                if dimensione_massima(g, &m) * o.mm_per_unita < minimo {
                    res.scartati_piccoli += 1;
                    continue;
                }
            }
            usi.push(Uso { pos, base, m });
        }
        // 1a. anteprima: gli oggetti molto piccoli rispetto alla scena non si vedono e si saltano
        if let Some(frazione) = o.piccoli_relativi {
            let riquadri: Vec<([f64; 3], [f64; 3])> = usi.iter().map(|u| cache.get(&u.base).and_then(|x| x.as_ref()).map_or(([0.0; 3], [0.0; 3]), |x| riquadro(&x.0, &u.m))).collect();
            let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
            for (a, b) in &riquadri {
                for k in 0..3 {
                    lo[k] = lo[k].min(a[k]);
                    hi[k] = hi[k].max(b[k]);
                }
            }
            let diagonale = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
            let grandi: Vec<bool> = riquadri.iter().map(|(a, b)| (0..3).map(|k| b[k] - a[k]).fold(0.0, f64::max) >= frazione * diagonale).collect();
            if grandi.iter().any(|&g| g) {
                let prima = usi.len();
                let mut i = 0;
                usi.retain(|_| {
                    i += 1;
                    grandi[i - 1]
                });
                res.scartati_piccoli += prima - usi.len();
            }
        }
        // 1b. oggetti diversi con la stessa identica geometria (copie fatte con «Copy» e non con «Instance») valgono come uno solo
        if o.istanze {
            let mut visti: HashMap<u64, Vec<usize>> = HashMap::new();
            let mut canonico: HashMap<usize, usize> = HashMap::new();
            let mut basi: Vec<usize> = usi.iter().map(|u| u.base).collect();
            basi.sort_unstable();
            basi.dedup();
            for b in basi {
                let Some(Some((g, _))) = cache.get(&b) else { continue };
                if g.triangoli.len() < 20 {
                    continue;
                }
                let gia = visti.entry(impronta(g)).or_default();
                let uguale = gia.iter().copied().find(|&c| cache.get(&c).and_then(|x| x.as_ref()).is_some_and(|x| x.0 == *g));
                match uguale {
                    Some(c) => {
                        canonico.insert(b, c);
                    }
                    None => gia.push(b),
                }
            }
            for u in &mut usi {
                if let Some(&c) = canonico.get(&u.base) {
                    u.base = c;
                }
            }
        }
        // 2. quante volte e' usato ogni oggetto
        let mut volte: HashMap<usize, usize> = HashMap::new();
        for u in &usi {
            *volte.entry(u.base).or_insert(0) += 1;
        }
        res.triangoli_origine = usi.iter().map(|u| cache.get(&u.base).and_then(|x| x.as_ref()).map_or(0, |g| g.0.triangoli.len())).sum();
        // 3. semplificazione: ogni oggetto ha al massimo `tetto` triangoli, scelto in modo che il totale torni
        if let Some(totale_voluto) = o.max_triangoli {
            let peso = |base: usize| if o.istanze { 1 } else { volte[&base] };
            let mut elenco: Vec<(usize, usize, usize)> = volte.keys().map(|&b| (b, cache[&b].as_ref().map_or(0, |g| g.0.triangoli.len()), peso(b))).collect();
            let somma = |tetto: usize| -> usize { elenco.iter().map(|&(_, t, w)| w * t.min(tetto)).sum() };
            let massimo = elenco.iter().map(|e| e.1).max().unwrap_or(0);
            if somma(massimo) > totale_voluto {
                let (mut lo, mut hi) = (1usize, massimo);
                while lo < hi {
                    let mid = (lo + hi).div_ceil(2);
                    if somma(mid) <= totale_voluto {
                        lo = mid;
                    } else {
                        hi = mid - 1;
                    }
                }
                let tetto = lo.max(100);
                elenco.sort_unstable_by_key(|e| std::cmp::Reverse(e.1));
                let da_fare: Vec<usize> = elenco.iter().filter(|e| e.1 > tetto).map(|e| e.0).collect();
                let nuove = in_parallelo(&da_fare, |b| cache.get(b).and_then(|x| x.as_ref()).map(|x| semplifica(&x.0, tetto)));
                for (b, n) in da_fare.iter().zip(nuove) {
                    if let (Some(n), Some(Some((g, _)))) = (n, cache.get_mut(b)) {
                        *g = n;
                        res.semplificati += 1;
                    }
                }
            }
        }
        // 4. le voci
        let mm = o.mm_per_unita;
        let in_mm = |g: &Geometria, m: Option<&Mat>| -> (Vec<f32>, Vec<u32>) {
            let mut vertici = Vec::with_capacity(g.vertici.len() * 3);
            for v in &g.vertici {
                let p = [f64::from(v[0]), f64::from(v[1]), f64::from(v[2])];
                let w = m.map_or(p, |m| m.applica(p));
                for c in w {
                    vertici.push((c * mm) as f32);
                }
            }
            (vertici, g.triangoli.iter().flatten().copied().collect())
        };
        // l'immagine e la mappatura di un oggetto: solo se la geometria ha le coordinate e il materiale ha un'immagine
        let texture_di = |pos: usize, g: &Geometria| -> Option<Texture> {
            let uv = g.uv.as_ref()?;
            if uv.len() != g.triangoli.len() {
                return None;
            }
            let file = nodi.get(&pos).and_then(|n| self.immagine_nodo(n))?;
            Some(texture_da_angoli(file, g, uv))
        };
        let mut voci = Vec::new();
        let mut definite: HashMap<usize, u32> = HashMap::new();
        let mut colori: HashMap<usize, Option<[f32; 4]>> = HashMap::new();
        let mut colore_di = |pos: usize| *colori.entry(pos).or_insert_with(|| nodi.get(&pos).and_then(|n| self.colore_nodo(n)));
        for u in &usi {
            let Some(Some((g, _))) = cache.get(&u.base) else { continue };
            let nome = percorso_nome(u.pos, &nodi);
            if o.istanze && volte[&u.base] > 1 {
                let id = match definite.get(&u.base) {
                    Some(&id) => id,
                    None => {
                        let id = definite.len() as u32;
                        let (vertici, triangoli) = in_mm(g, None);
                        let colore = colore_di(u.pos);
                        voci.push(Voce::Definizione(id, Parte { nome: nome.clone(), colore, vertici, triangoli, texture: texture_di(u.pos, g) }));
                        definite.insert(u.base, id);
                        id
                    }
                };
                let r = &u.m.0;
                let matrice = [
                    r[0][0] as f32,
                    r[0][1] as f32,
                    r[0][2] as f32,
                    r[1][0] as f32,
                    r[1][1] as f32,
                    r[1][2] as f32,
                    r[2][0] as f32,
                    r[2][1] as f32,
                    r[2][2] as f32,
                    (r[3][0] * mm) as f32,
                    (r[3][1] * mm) as f32,
                    (r[3][2] * mm) as f32,
                ];
                voci.push(Voce::Istanza { definizione: id, nome, matrice });
                res.istanze += 1;
            } else {
                let (vertici, triangoli) = in_mm(g, Some(&u.m));
                let colore = colore_di(u.pos);
                voci.push(Voce::Geometria(Parte { nome, colore, vertici, triangoli, texture: texture_di(u.pos, g) }));
            }
            res.con_geometria += 1;
        }
        (voci, res)
    }

    fn matrice_mondo(&self, pos: usize, nodi: &HashMap<usize, Nodo>, memo: &mut HashMap<usize, Mat>) -> Mat {
        if let Some(m) = memo.get(&pos) {
            return *m;
        }
        let mut catena = vec![pos];
        let mut visti = HashSet::from([pos]);
        let mut cur = pos;
        while let Some(p) = nodi.get(&cur).map(|n| n.padre).filter(|p| nodi.contains_key(p) && visti.insert(*p)) {
            catena.push(p);
            cur = p;
        }
        let mut m = Mat::ID;
        for &p in catena.iter().rev() {
            let ctrl = nodi[&p].riferimenti.get(&0).copied();
            let locale = ctrl.map(|c| self.matrice_locale(c)).unwrap_or(Mat::ID);
            m = locale.per(&m);
            memo.insert(p, m);
        }
        m
    }
}

/// Le immagini salvate nel file: per ognuna un identificativo di 16 byte, il nome della classe, il nome del file e il percorso.
/// Ogni voce: 16 byte, poi tre testi (u32 lunghezza in caratteri, UTF-16, uno zero finale). Si tengono le immagini «Bitmap».
fn leggi_immagini(d: &[u8]) -> Vec<([u8; 16], String)> {
    let mut v = Vec::new();
    let mut p = 0usize;
    'voci: while p + 16 <= d.len() {
        let id: [u8; 16] = d[p..p + 16].try_into().unwrap();
        p += 16;
        let mut testi = Vec::with_capacity(3);
        for _ in 0..3 {
            if p + 4 > d.len() {
                break 'voci;
            }
            let n = u32_at(d, p) as usize;
            p += 4;
            if n > 4096 || p + n * 2 + 2 > d.len() {
                break 'voci;
            }
            let unita: Vec<u16> = (0..n).map(|k| u16::from_le_bytes([d[p + 2 * k], d[p + 2 * k + 1]])).collect();
            testi.push(String::from_utf16_lossy(&unita));
            p += n * 2 + 2;
        }
        if testi[0] == "Bitmap" {
            v.push((id, testi[2].clone()));
        }
    }
    v
}

/// Dalle coordinate dei tre angoli di ogni triangolo: una coordinata per vertice, e i triangoli che toccano un vertice
/// con coordinate diverse da un triangolo all'altro (bordi della mappatura), con le loro coordinate.
fn texture_da_angoli(file: String, g: &Geometria, uv: &[[[f32; 2]; 3]]) -> Texture {
    let mut per_vertice: Vec<Option<[f32; 2]>> = vec![None; g.vertici.len()];
    let mut diverso = vec![false; g.vertici.len()];
    for (t, a) in g.triangoli.iter().zip(uv) {
        for k in 0..3 {
            let v = t[k] as usize;
            match per_vertice[v] {
                None => per_vertice[v] = Some(a[k]),
                Some(u) if (u[0] - a[k][0]).abs() > 1e-5 || (u[1] - a[k][1]).abs() > 1e-5 => diverso[v] = true,
                _ => {}
            }
        }
    }
    let bordi = g
        .triangoli
        .iter()
        .zip(uv)
        .enumerate()
        .filter(|(_, (t, _))| t.iter().any(|&v| diverso[v as usize]))
        .map(|(i, (_, a))| (i as u32, [a[0][0], a[0][1], a[1][0], a[1][1], a[2][0], a[2][1]]))
        .collect();
    Texture { file, uv: per_vertice.iter().flat_map(|u| u.unwrap_or([0.0, 0.0])).collect(), bordi }
}

fn blocchi_di(scena: &[u8], radice: &Blocco) -> Vec<Blocco> {
    blocchi(scena, radice.s, radice.e)
}

/// Nome con il percorso dei gruppi: «Gruppo/Oggetto».
fn percorso_nome(pos: usize, nodi: &HashMap<usize, Nodo>) -> String {
    let mut nomi = Vec::new();
    let mut visti = HashSet::new();
    let mut cur = pos;
    while let Some(n) = nodi.get(&cur) {
        if !visti.insert(cur) {
            break;
        }
        nomi.push(if n.nome.is_empty() { "senza nome".to_string() } else { n.nome.replace('/', "_") });
        cur = n.padre;
    }
    nomi.reverse();
    nomi.join("/")
}

/// Le facce di un Editable Poly: ognuna e' «gradi, indici, flags» seguita da una coda la cui lunghezza dipende dai flags e dai gradi.
/// La coda e' cercata cosi': prima la lunghezza prevista, poi le vicine; vale la prima per cui anche la faccia dopo ha senso
/// (indici dentro l'elenco dei vertici) e il conteggio finale torna.
fn leggi_facce(d: &[u8], nv: usize) -> Option<(Vec<Vec<u32>>, bool)> {
    if d.len() < 4 {
        return None;
    }
    let conta = u32_at(d, 0) as usize;
    let l = d.len();
    let inizio_ok = |p: usize| -> bool {
        if p + 12 > l {
            return false;
        }
        let g = u32_at(d, p) as usize;
        if !(3..=4096).contains(&g) || p + 8 + 4 * g > l {
            return false;
        }
        (0..g).all(|j| (u32_at(d, p + 4 + 4 * j) as usize) < nv)
    };
    struct Faccia {
        p: usize,
        g: usize,
        pref: usize,
        cand: usize,
    }
    // lunghezze da provare per la coda: la prevista, poi pref+2, pref-2, pref+4...
    let candidato = |pref: usize, i: usize| -> Option<usize> {
        if i == 0 {
            return Some(pref);
        }
        let k = i.div_ceil(2) * 2;
        if i % 2 == 1 {
            Some(pref + k)
        } else {
            pref.checked_sub(k)
        }
    };
    const TENTATIVI: usize = 60;
    enum Stato {
        Nuova,
        Prova,
        Indietro,
    }
    let mut pila: Vec<Faccia> = Vec::with_capacity(conta);
    let mut p = 4usize;
    let mut stato = Stato::Nuova;
    let mut passi = 0u64;
    loop {
        passi += 1;
        if passi > 60_000_000 {
            return None;
        }
        match stato {
            Stato::Nuova => {
                if pila.len() == conta {
                    if l - p <= 8 {
                        break;
                    }
                    stato = Stato::Indietro;
                } else if inizio_ok(p) {
                    let g = u32_at(d, p) as usize;
                    let fl = u32_at(d, p + 4 + 4 * g);
                    pila.push(Faccia { p, g, pref: prevista(fl, g), cand: 0 });
                    stato = Stato::Prova;
                } else {
                    stato = Stato::Indietro;
                }
            }
            Stato::Prova => {
                let ultima = pila.len() == conta;
                let top = pila.last_mut().expect("pila non vuota");
                let mut trovato = None;
                while top.cand < TENTATIVI {
                    let c = candidato(top.pref, top.cand);
                    top.cand += 1;
                    if let Some(t) = c {
                        let q = top.p + 8 + 4 * top.g + t;
                        if q <= l && (ultima || inizio_ok(q)) {
                            trovato = Some(q);
                            break;
                        }
                    }
                }
                match trovato {
                    Some(q) => {
                        p = q;
                        stato = Stato::Nuova;
                    }
                    None => {
                        pila.pop();
                        stato = Stato::Indietro;
                    }
                }
            }
            Stato::Indietro => {
                if pila.is_empty() {
                    return None;
                }
                stato = Stato::Prova;
            }
        }
    }
    let facce = pila.iter().map(|f| (0..f.g).map(|j| u32_at(d, f.p + 4 + 4 * j)).collect()).collect();
    Some((facce, true))
}

/// Lunghezza prevista (byte) della coda di una faccia, dai flags e dai gradi.
fn prevista(flags: u32, g: usize) -> usize {
    let mut u16s = 0usize;
    if flags & 0x1 != 0 {
        u16s += 2;
    }
    if flags & 0x8 != 0 {
        u16s += 1;
    }
    if flags & 0x10 != 0 {
        u16s += g - 2;
    }
    if flags & 0x20 != 0 {
        u16s += 3 * (g - 3);
    }
    u16s * 2
}

/// Divide una faccia in triangoli: i triangoli restano com'e', i quadrilateri sulla diagonale piu' corta, gli altri a «ear clipping».
fn triangola(f: &[u32], v: &[[f32; 3]], out: &mut Vec<[u32; 3]>) {
    let n = f.len();
    if n < 3 {
        return;
    }
    if n == 3 {
        out.push([f[0], f[1], f[2]]);
        return;
    }
    let pt = |i: u32| -> [f64; 3] {
        let a = v[i as usize];
        [f64::from(a[0]), f64::from(a[1]), f64::from(a[2])]
    };
    if n == 4 {
        let d = |a: u32, b: u32| {
            let (p, q) = (pt(a), pt(b));
            (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)
        };
        if d(f[0], f[2]) <= d(f[1], f[3]) {
            out.push([f[0], f[1], f[2]]);
            out.push([f[0], f[2], f[3]]);
        } else {
            out.push([f[0], f[1], f[3]]);
            out.push([f[1], f[2], f[3]]);
        }
        return;
    }
    // normale del poligono (Newell) e proiezione sul piano migliore
    let mut nr = [0.0f64; 3];
    for i in 0..n {
        let (a, b) = (pt(f[i]), pt(f[(i + 1) % n]));
        nr[0] += (a[1] - b[1]) * (a[2] + b[2]);
        nr[1] += (a[2] - b[2]) * (a[0] + b[0]);
        nr[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let asse = if nr[0].abs() >= nr[1].abs() && nr[0].abs() >= nr[2].abs() {
        0
    } else if nr[1].abs() >= nr[2].abs() {
        1
    } else {
        2
    };
    let (iu, iv) = match asse {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    let segno = if nr[asse] >= 0.0 { 1.0 } else { -1.0 };
    let p2 = |i: u32| -> (f64, f64) {
        let a = pt(i);
        (a[iu] * segno, a[iv] * segno)
    };
    let mut idx: Vec<u32> = f.to_vec();
    let area = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    let mut guardia = 0;
    while idx.len() > 3 && guardia < n * n + 8 {
        guardia += 1;
        let m = idx.len();
        let mut fatto = false;
        for i in 0..m {
            let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (pa, pb, pc) = (p2(a), p2(b), p2(c));
            if area(pa, pb, pc) <= 1e-12 {
                continue;
            }
            let dentro = idx.iter().any(|&x| {
                if x == a || x == b || x == c {
                    return false;
                }
                let q = p2(x);
                area(pa, pb, q) >= 0.0 && area(pb, pc, q) >= 0.0 && area(pc, pa, q) >= 0.0
            });
            if !dentro {
                out.push([a, b, c]);
                idx.remove(i);
                fatto = true;
                break;
            }
        }
        if !fatto {
            // poligono degenere: si chiude a ventaglio
            break;
        }
    }
    for i in 1..idx.len().saturating_sub(1) {
        out.push([idx[0], idx[i], idx[i + 1]]);
    }
}

/// Millimetri per unita' del file: «mm», «cm», «m», «in», «ft».
pub fn mm_per_unita(nome: &str) -> Result<f64> {
    Ok(match nome.trim().to_lowercase().as_str() {
        "mm" => 1.0,
        "cm" => 10.0,
        "m" => 1000.0,
        "in" | "inch" | "pollici" => 25.4,
        "ft" | "feet" | "piedi" => 304.8,
        altro => bail!("unita' sconosciuta: «{altro}» (usa mm, cm, m, in o ft)"),
    })
}

/// Semplifica delle parti gia' lette (per l'anteprima): al massimo `totale` triangoli in tutto, tagliando le parti piu' pesanti.
pub fn semplifica_parti(parti: &mut [Parte], totale: usize) {
    let tri: Vec<usize> = parti.iter().map(|p| p.triangoli.len() / 3).collect();
    let somma = |tetto: usize| -> usize { tri.iter().map(|&t| t.min(tetto)).sum() };
    let massimo = tri.iter().copied().max().unwrap_or(0);
    if somma(massimo) <= totale {
        return;
    }
    let (mut lo, mut hi) = (1usize, massimo);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if somma(mid) <= totale {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let tetto = lo.max(100);
    let lette: &[Parte] = parti;
    let pesanti: Vec<usize> = (0..lette.len()).filter(|&i| lette[i].triangoli.len() / 3 > tetto).collect();
    let nuove = in_parallelo(&pesanti, |&i| {
        let p = &lette[i];
        let g = Geometria {
            uv: None,
            vertici: (0..p.vertici.len() / 3).map(|k| [p.vertici[3 * k], p.vertici[3 * k + 1], p.vertici[3 * k + 2]]).collect(),
            triangoli: (0..p.triangoli.len() / 3).map(|k| [p.triangoli[3 * k], p.triangoli[3 * k + 1], p.triangoli[3 * k + 2]]).collect(),
        };
        semplifica(&g, tetto)
    });
    for (&i, s) in pesanti.iter().zip(nuove) {
        parti[i].vertici = s.vertici.iter().flatten().copied().collect();
        parti[i].triangoli = s.triangoli.iter().flatten().copied().collect();
    }
}

/// Legge un file `.max` e restituisce le voci (geometrie, definizioni e istanze).
pub fn carica(percorso: &Path, opzioni: &OpzioniMax) -> Result<(Vec<Voce>, Resoconto)> {
    let f = FileMax::apri(percorso)?;
    Ok(f.parti(opzioni))
}

/// Un numero che dipende dal contenuto della geometria (per trovare le copie).
fn impronta(g: &Geometria) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    g.vertici.len().hash(&mut h);
    g.triangoli.len().hash(&mut h);
    for v in &g.vertici {
        for c in v {
            c.to_bits().hash(&mut h);
        }
    }
    for tr in &g.triangoli {
        tr.hash(&mut h);
    }
    h.finish()
}

/// Il riquadro (minimo, massimo) che contiene l'oggetto dopo la trasformazione.
fn riquadro(g: &Geometria, m: &Mat) -> ([f64; 3], [f64; 3]) {
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in &g.vertici {
        for i in 0..3 {
            mn[i] = mn[i].min(v[i]);
            mx[i] = mx[i].max(v[i]);
        }
    }
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for k in 0..8 {
        let p = [
            f64::from(if k & 1 == 0 { mn[0] } else { mx[0] }),
            f64::from(if k & 2 == 0 { mn[1] } else { mx[1] }),
            f64::from(if k & 4 == 0 { mn[2] } else { mx[2] }),
        ];
        let w = m.applica(p);
        for i in 0..3 {
            lo[i] = lo[i].min(w[i]);
            hi[i] = hi[i].max(w[i]);
        }
    }
    (lo, hi)
}

/// Il lato maggiore del riquadro dell'oggetto dopo la trasformazione (nelle unita' del file).
fn dimensione_massima(g: &Geometria, m: &Mat) -> f64 {
    let (lo, hi) = riquadro(g, m);
    (0..3).map(|i| hi[i] - lo[i]).fold(0.0, f64::max)
}

/// Esegue `f` su ogni voce usando tutti i processori meno uno (il computer resta usabile); i risultati sono nello stesso ordine
/// delle voci.
pub(crate) fn in_parallelo<T: Sync, R: Send>(voci: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let processori = std::thread::available_parallelism().map_or(2, |n| n.get());
    let n = processori.saturating_sub(1).max(1).min(voci.len()).max(1);
    if n == 1 {
        return voci.iter().map(&f).collect();
    }
    let prossimo = AtomicUsize::new(0);
    let mut fatti: Vec<(usize, R)> = Vec::with_capacity(voci.len());
    std::thread::scope(|s| {
        let lavoratori: Vec<_> = (0..n)
            .map(|_| {
                s.spawn(|| {
                    let mut mio = Vec::new();
                    loop {
                        let i = prossimo.fetch_add(1, Ordering::Relaxed);
                        if i >= voci.len() {
                            break;
                        }
                        mio.push((i, f(&voci[i])));
                    }
                    mio
                })
            })
            .collect();
        for l in lavoratori {
            fatti.extend(l.join().expect("un lavoratore si e' fermato"));
        }
    });
    fatti.sort_unstable_by_key(|x| x.0);
    fatti.into_iter().map(|x| x.1).collect()
}

/// Un hash semplice e veloce (come FxHash) per le mappe delle celle.
#[derive(Default)]
struct HashVeloce(u64);

impl std::hash::Hasher for HashVeloce {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write_i64(&mut self, i: i64) {
        self.write_u64(i as u64);
    }
    fn write_u32(&mut self, i: u32) {
        self.write_u64(u64::from(i));
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

type MappaVeloce<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<HashVeloce>>;

/// Semplifica una geometria per «raggruppamento dei vertici»: i vertici nella stessa cella di una griglia diventano uno solo
/// (la media); la griglia si infittisce o si allarga fino a stare sotto `obiettivo` triangoli. Veloce, ma perde i dettagli piccoli.
fn semplifica(g: &Geometria, obiettivo: usize) -> Geometria {
    if g.triangoli.len() <= obiettivo {
        return Geometria { vertici: g.vertici.clone(), triangoli: g.triangoli.clone(), uv: g.uv.clone() };
    }
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for t in &g.triangoli {
        for &i in t {
            let v = g.vertici[i as usize];
            for k in 0..3 {
                mn[k] = mn[k].min(v[k]);
                mx[k] = mx[k].max(v[k]);
            }
        }
    }
    let diagonale = f64::from(((mx[0] - mn[0]).powi(2) + (mx[1] - mn[1]).powi(2) + (mx[2] - mn[2]).powi(2)).sqrt()).max(1e-9);
    // la cella di ogni vertice usato, per una data misura della griglia
    let celle = |s: f64| -> (Vec<u32>, usize) {
        let mut mappa: MappaVeloce<[i64; 3], u32> = MappaVeloce::default();
        let mut di_vertice = vec![u32::MAX; g.vertici.len()];
        for t in &g.triangoli {
            for &i in t {
                if di_vertice[i as usize] == u32::MAX {
                    let v = g.vertici[i as usize];
                    let chiave = [(f64::from(v[0]) / s).floor() as i64, (f64::from(v[1]) / s).floor() as i64, (f64::from(v[2]) / s).floor() as i64];
                    let n = mappa.len() as u32;
                    di_vertice[i as usize] = *mappa.entry(chiave).or_insert(n);
                }
            }
        }
        let n = mappa.len();
        (di_vertice, n)
    };
    let conta = |s: f64| -> usize {
        let (c, _) = celle(s);
        g.triangoli
            .iter()
            .filter(|t| {
                let (a, b, k) = (c[t[0] as usize], c[t[1] as usize], c[t[2] as usize]);
                a != b && b != k && a != k
            })
            .count()
    };
    let (mut lo, mut hi) = (diagonale / 6000.0, diagonale);
    for _ in 0..14 {
        let mid = (lo * hi).sqrt();
        if conta(mid) <= obiettivo {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let (di_vertice, n) = celle(hi);
    let mut somme = vec![[0.0f64; 3]; n];
    let mut quanti = vec![0u32; n];
    for (i, &c) in di_vertice.iter().enumerate() {
        if c != u32::MAX {
            let v = g.vertici[i];
            for k in 0..3 {
                somme[c as usize][k] += f64::from(v[k]);
            }
            quanti[c as usize] += 1;
        }
    }
    let vertici: Vec<[f32; 3]> = somme
        .iter()
        .zip(&quanti)
        .map(|(s, &q)| [(s[0] / f64::from(q)) as f32, (s[1] / f64::from(q)) as f32, (s[2] / f64::from(q)) as f32])
        .collect();
    let mut visti: std::collections::HashSet<[u32; 3], std::hash::BuildHasherDefault<HashVeloce>> = Default::default();
    let mut triangoli = Vec::new();
    for t in &g.triangoli {
        let (a, b, k) = (di_vertice[t[0] as usize], di_vertice[t[1] as usize], di_vertice[t[2] as usize]);
        if a == b || b == k || a == k {
            continue;
        }
        let mut chiave = [a, b, k];
        chiave.sort_unstable();
        if visti.insert(chiave) {
            triangoli.push([a, b, k]);
        }
    }
    Geometria { vertici, triangoli, uv: None }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::io::Write;

    fn solo_parti(v: Vec<Voce>) -> Vec<Parte> {
        v.into_iter().filter_map(|x| if let Voce::Geometria(p) = x { Some(p) } else { None }).collect()
    }

    fn blocco(id: u16, contenitore: bool, dati: &[u8]) -> Vec<u8> {
        let mut o = id.to_le_bytes().to_vec();
        let len = (dati.len() + 6) as u32 | if contenitore { 0x8000_0000 } else { 0 };
        o.extend_from_slice(&len.to_le_bytes());
        o.extend_from_slice(dati);
        o
    }

    fn dword(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn classe(nome: &str, a: u32, b: u32, sc: u32) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&dword(&[0xffff_ffff, a, b, sc]));
        let u: Vec<u8> = nome.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut interno = blocco(0x2060, false, &c);
        interno.extend(blocco(0x2042, false, &u));
        blocco(0x2040, true, &interno)
    }

    fn controller_float(v: f32, diretto: bool) -> Vec<u8> {
        let valore = blocco(0x2501, false, &v.to_le_bytes());
        if diretto {
            // 3ds Max piu' recente: il valore sta direttamente nel controller
            blocco(3, true, &valore)
        } else {
            blocco(3, true, &blocco(0x7127, true, &valore))
        }
    }

    /// Un file .max minimo: un nodo «Cubo» dentro il gruppo «G», con un Editable Poly di un quadrato, spostato di (10, 20, 30).
    fn max_di_prova(percorso: &Path, facce: &[u8]) {
        max_di_prova_n(percorso, facce, 1, false);
    }

    fn gzip(dati: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(dati).unwrap();
        e.finish().unwrap()
    }

    /// Come `max_di_prova` ma con `n_nodi` nodi che usano lo stesso oggetto.
    fn max_di_prova_n(percorso: &Path, facce: &[u8], n_nodi: usize, comprimi: bool) {
        max_di_prova_v(percorso, facce, n_nodi, comprimi, false);
    }

    fn max_di_prova_v(percorso: &Path, facce: &[u8], n_nodi: usize, comprimi: bool, diretti: bool) {
        // classi: 0 nodo, 1 Editable Poly, 2 Position XYZ, 3 Bezier float, 4 PRS, 5 Editable Mesh
        let mut dir = classe("Node", 1, 0, 1);
        dir.extend(classe("Editable Poly", 0x1bf8_338d, 0x192f_6098, 0x10));
        dir.extend(classe("Position XYZ", 0x118f_7e02, 0xffee_238a, 0x900b));
        dir.extend(classe("Bezier Float", 0x2007, 0, 0x9003));
        dir.extend(classe("PRS", 0x2005, 0, 0x9008));
        dir.extend(classe("Editable Mesh", 0xe44f_10b3, 0, 0x10));
        let mut figli = Vec::new();
        for v in [10.0f32, 20.0, 30.0] {
            figli.extend(controller_float(v, diretti)); // pos 0, 1, 2
        }
        figli.extend(blocco(2, true, &blocco(0x2034, false, &dword(&[0, 1, 2])))); // pos 3: Position XYZ
        figli.extend(blocco(4, true, &blocco(0x2034, false, &dword(&[3])))); // pos 4: PRS
        // pos 5: Editable Poly con 4 vertici (flags 0 + x y z) e le facce date
        let mut vert = dword(&[4]);
        for p in [[0.0f32, 0.0, 0.0], [100.0, 0.0, 0.0], [100.0, 50.0, 0.0], [0.0, 50.0, 0.0]] {
            vert.extend_from_slice(&0u32.to_le_bytes());
            for c in p {
                vert.extend_from_slice(&c.to_le_bytes());
            }
        }
        let mut geom = blocco(0x100, false, &vert);
        geom.extend(blocco(0x11a, false, facce));
        figli.extend(blocco(1, true, &blocco(0x8fe, true, &geom)));
        // pos 6: nodo «G» (gruppo senza oggetto), pos 7: nodo «Cubo» figlio di G
        let nome = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() };
        let mut g = blocco(0x962, false, &nome("G"));
        g.extend(blocco(0x960, false, &dword(&[99, 0])));
        g.extend(blocco(0x2035, false, &dword(&[16, 0, 4])));
        figli.extend(blocco(0, true, &g));
        for k in 0..n_nodi {
            let mut n = blocco(0x962, false, &nome(&if k == 0 { "Cubo".to_string() } else { format!("Cubo{k}") }));
            n.extend(blocco(0x960, false, &dword(&[6, 0])));
            n.extend(blocco(0x2035, false, &dword(&[16, 1, 5])));
            figli.extend(blocco(0, true, &n));
        }
        let scena = blocco(0x2026, true, &figli);
        // con la variabile d'ambiente impostata si scrive lo stream Scene compresso con gzip, come fanno le versioni recenti di 3ds Max
        let scena = if comprimi { gzip(&scena) } else { scena };
        let mut cfb = cfb::CompoundFile::create(std::fs::File::create(percorso).unwrap()).unwrap();
        cfb.create_stream("/ClassDirectory3").unwrap().write_all(&dir).unwrap();
        cfb.create_stream("/Scene").unwrap().write_all(&scena).unwrap();
        cfb.flush().unwrap();
    }

    fn facce_quadrato(flags: u32, coda: &[u32]) -> Vec<u8> {
        let mut f = dword(&[1, 4, 0, 1, 2, 3, flags]);
        f.extend(dword(coda));
        f
    }

    #[test]
    fn legge_un_poligono_con_nome_e_posizione() {
        let file = std::env::temp_dir().join("robo_stp_prova_max_1.max");
        max_di_prova(&file, &facce_quadrato(0x20038, &[1, 2, 0]));
        let (parti, r) = carica(&file, &OpzioniMax::semplici(1.0)).map(|(v, r)| (solo_parti(v), r)).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!(r.facce_illeggibili, 0);
        assert_eq!(parti.len(), 1);
        let p = &parti[0];
        assert_eq!(p.nome, "G/Cubo");
        assert_eq!(p.triangoli.len(), 6, "un quadrilatero = 2 triangoli");
        // il primo vertice (0,0,0) va in (10,20,30); il terzo (100,50,0) in (110,70,30)
        assert_eq!(&p.vertici[0..3], &[10.0, 20.0, 30.0]);
        assert_eq!(&p.vertici[6..9], &[110.0, 70.0, 30.0]);
    }

    #[test]
    fn le_unita_cambiano_le_misure() {
        let file = std::env::temp_dir().join("robo_stp_prova_max_2.max");
        max_di_prova(&file, &facce_quadrato(0x20038, &[1, 2, 0]));
        let (parti, _) = carica(&file, &OpzioniMax::semplici(mm_per_unita("cm").unwrap())).map(|(v, r)| (solo_parti(v), r)).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!(&parti[0].vertici[0..3], &[100.0, 200.0, 300.0]);
    }

    #[test]
    fn la_coda_della_faccia_si_trova_anche_se_la_formula_sbaglia() {
        // flags 0x30 prevede una coda di 5 u16 (10 byte), ma qui ce ne sono 12: la ricerca deve trovarla lo stesso
        let file = std::env::temp_dir().join("robo_stp_prova_max_3.max");
        max_di_prova(&file, &facce_quadrato(0x30, &[7, 7, 7]));
        let (parti, r) = carica(&file, &OpzioniMax::semplici(1.0)).map(|(v, r)| (solo_parti(v), r)).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!(parti.len(), 1);
        assert_eq!(r.facce_illeggibili, 0);
    }

    #[test]
    fn file_non_max_da_errore() {
        let file = std::env::temp_dir().join("robo_stp_prova_non_max.max");
        std::fs::write(&file, b"questo non e' un file max").unwrap();
        let e = carica(&file, &OpzioniMax::semplici(1.0)).expect_err("deve fallire").to_string();
        std::fs::remove_file(&file).ok();
        assert!(e.contains("non e' un file .max valido"), "{e}");
    }

    #[test]
    fn lunghezza_coda_prevista() {
        assert_eq!(prevista(0x20038, 4), 12); // 3 parole da 4 byte
        assert_eq!(prevista(0x39, 4), 16);
        assert_eq!(prevista(0x11, 3), 6);
        assert_eq!(prevista(0x0, 4), 0);
    }

    #[test]
    fn triangolazione() {
        let v: Vec<[f32; 3]> = vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 4.0, 0.0], [2.0, 1.0, 0.0], [0.0, 4.0, 0.0]];
        // pentagono concavo: 3 triangoli, area totale 10 (formula dell'area di Gauss)
        let mut t = Vec::new();
        triangola(&[0, 1, 2, 3, 4], &v, &mut t);
        assert_eq!(t.len(), 3);
        let area: f32 = t
            .iter()
            .map(|t| {
                let (a, b, c) = (v[t[0] as usize], v[t[1] as usize], v[t[2] as usize]);
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2.0
            })
            .sum();
        assert!((area - 10.0).abs() < 1e-4, "area {area}");
    }

    #[test]
    fn unita_sconosciuta() {
        assert!(mm_per_unita("km").is_err());
        assert_eq!(mm_per_unita("m").unwrap(), 1000.0);
    }

    #[test]
    fn gli_oggetti_ripetuti_diventano_istanze() {
        let file = std::env::temp_dir().join("robo_stp_prova_max_4.max");
        max_di_prova_n(&file, &facce_quadrato(0x20038, &[1, 2, 0]), 3, false);
        let mut o = OpzioniMax::semplici(1.0);
        let (voci, r) = carica(&file, &o).unwrap();
        assert_eq!(voci.len(), 3, "senza istanze: 3 parti");
        o.istanze = true;
        let (voci, r2) = carica(&file, &o).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!(r.istanze, 0);
        assert_eq!(r2.istanze, 3);
        assert_eq!(voci.len(), 4, "una definizione e 3 istanze");
        assert!(matches!(voci[0], Voce::Definizione(0, _)));
        let Voce::Istanza { matrice, .. } = &voci[1] else { panic!("manca l'istanza") };
        assert_eq!(&matrice[9..12], &[10.0, 20.0, 30.0], "la traslazione dell'istanza");
    }

    #[test]
    fn i_filtri_scartano_gli_oggetti() {
        let file = std::env::temp_dir().join("robo_stp_prova_max_5.max");
        max_di_prova(&file, &facce_quadrato(0x20038, &[1, 2, 0])); // un rettangolo 100 x 50
        let mut o = OpzioniMax::semplici(1.0);
        o.dimensione_minima_mm = Some(101.0);
        let (voci, r) = carica(&file, &o).unwrap();
        assert_eq!((voci.len(), r.scartati_piccoli), (0, 1));
        o.dimensione_minima_mm = Some(99.0);
        assert_eq!(carica(&file, &o).unwrap().0.len(), 1);
        o.dimensione_minima_mm = None;
        o.oggetto_max_triangoli = Some(1); // l'oggetto ha 2 triangoli
        let (voci, r) = carica(&file, &o).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!((voci.len(), r.scartati_pesanti), (0, 1));
    }

    #[test]
    fn la_semplificazione_riduce_i_triangoli() {
        // una griglia di 40 x 40 quadrati = 3200 triangoli
        let n = 41usize;
        let mut vertici = Vec::new();
        for y in 0..n {
            for x in 0..n {
                vertici.push([x as f32, y as f32, ((x * y) % 3) as f32 * 0.01]);
            }
        }
        let mut triangoli = Vec::new();
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let a = (y * n + x) as u32;
                let (b, c, d) = (a + 1, a + n as u32, a + n as u32 + 1);
                triangoli.push([a, b, d]);
                triangoli.push([a, d, c]);
            }
        }
        let g = Geometria { vertici, triangoli, uv: None };
        let s = semplifica(&g, 800);
        assert!(s.triangoli.len() <= 800 && s.triangoli.len() > 100, "{} triangoli", s.triangoli.len());
        assert!(s.triangoli.iter().flatten().all(|&i| (i as usize) < s.vertici.len()));
        // sotto l'obiettivo: resta com'e'
        assert_eq!(semplifica(&g, 10_000).triangoli.len(), 3200);
    }

    #[test]
    fn lo_stream_compresso_con_gzip_si_legge() {
        let file = std::env::temp_dir().join("robo_stp_prova_max_6.max");
        max_di_prova_n(&file, &facce_quadrato(0x20038, &[1, 2, 0]), 1, true);
        let (voci, r) = carica(&file, &OpzioniMax::semplici(1.0)).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!(voci.len(), 1);
        assert_eq!(r.facce_illeggibili, 0);
    }

    #[test]
    fn il_valore_del_controller_puo_stare_direttamente_nel_blocco() {
        let file = std::env::temp_dir().join("robo_stp_prova_max_7.max");
        max_di_prova_v(&file, &facce_quadrato(0x20038, &[1, 2, 0]), 1, false, true);
        let (voci, _) = carica(&file, &OpzioniMax::semplici(1.0)).unwrap();
        std::fs::remove_file(&file).ok();
        let Voce::Geometria(p) = &voci[0] else { panic!("manca la parte") };
        assert_eq!(&p.vertici[0..3], &[10.0, 20.0, 30.0], "la posizione deve essere letta anche in questo formato");
    }
}
