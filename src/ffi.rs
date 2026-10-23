//! Collegamento sicuro al ponte C++ di OpenCascade.

use crate::rstp::Voce;
use anyhow::{anyhow, bail, Result};
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;

type RaccogliVoce = extern "C" fn(*mut c_void, i32, u32, *const c_char, *const f32, u64, *const f32, u64, *const u32, *const f32);

extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn rstp_load(
        path: *const c_char,
        format: i32,
        linear: f64,
        angular: f64,
        max_triangles: u64,
        instances: i32,
        cb: RaccogliVoce,
        user: *mut c_void,
        used_linear: *mut f64,
        err: *mut c_char,
        err_len: u32,
    ) -> i32;
    fn rstp_write_test_box(path: *const c_char, format: i32, repeated: i32, err: *mut c_char, err_len: u32) -> i32;
    fn rstp_occt_version() -> *const c_char;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Formato {
    Step,
    Iges,
}

impl Formato {
    /// Riconosce il formato dall'estensione: .stp .step .igs .iges
    pub fn da_percorso(p: &Path) -> Result<Formato> {
        match p.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
            Some("stp") | Some("step") => Ok(Formato::Step),
            Some("igs") | Some("iges") => Ok(Formato::Iges),
            _ => bail!("estensione non riconosciuta: servono .stp, .step, .igs o .iges"),
        }
    }
    fn codice(self) -> i32 {
        match self {
            Formato::Step => 0,
            Formato::Iges => 1,
        }
    }
}

/// L'immagine di un materiale con le sue coordinate di mappatura (file .max).
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    /// Percorso dell'immagine come e' salvato nel file .max
    pub file: String,
    /// u, v di ogni vertice (u, v, u, v...)
    pub uv: Vec<f32>,
    /// Triangoli che toccano un vertice con coordinate diverse da un triangolo all'altro (bordi della mappatura):
    /// (indice del triangolo, u v dei suoi tre angoli)
    pub bordi: Vec<(u32, [f32; 6])>,
}

/// Una parte del modello, in millimetri, nella posizione finale dell'assieme.
#[derive(Clone, Debug, PartialEq)]
pub struct Parte {
    pub nome: String,
    /// Colore RGBA 0..1; `None` se il file non ne dà uno.
    pub colore: Option<[f32; 4]>,
    /// x, y, z, x, y, z…
    pub vertici: Vec<f32>,
    /// tre indici per triangolo
    pub triangoli: Vec<u32>,
    /// L'immagine del materiale e la mappatura, se il file le ha e sono state chieste.
    pub texture: Option<Texture>,
}

#[allow(clippy::too_many_arguments)]
extern "C" fn raccogli(
    user: *mut c_void,
    tipo: i32,
    id: u32,
    nome: *const c_char,
    rgba: *const f32,
    nv: u64,
    v: *const f32,
    nt: u64,
    idx: *const u32,
    matrice: *const f32,
) {
    // SAFETY: il ponte passa puntatori validi per la durata della chiamata; `user` e' il nostro Vec. Un'istanza ha la matrice e
    // nessun triangolo; le altre voci hanno colore e triangoli.
    unsafe {
        let voci = &mut *(user as *mut Vec<Voce>);
        let nome = CStr::from_ptr(nome).to_string_lossy().into_owned();
        if tipo == 2 {
            let m = std::slice::from_raw_parts(matrice, 12);
            let mut matrice = [0.0f32; 12];
            matrice.copy_from_slice(m);
            voci.push(Voce::Istanza { definizione: id, nome, matrice });
            return;
        }
        let c = std::slice::from_raw_parts(rgba, 4);
        let colore = if c[0] < 0.0 { None } else { Some([c[0], c[1], c[2], c[3]]) };
        let parte = Parte {
            nome,
            colore,
            vertici: std::slice::from_raw_parts(v, nv as usize * 3).to_vec(),
            triangoli: std::slice::from_raw_parts(idx, nt as usize * 3).to_vec(),
            texture: None,
        };
        voci.push(if tipo == 1 { Voce::Definizione(id, parte) } else { Voce::Geometria(parte) });
    }
}

fn messaggio(buf: &[u8]) -> String {
    let fine = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..fine]).into_owned()
}

fn c_path(p: &Path) -> Result<CString> {
    let s = p.to_str().ok_or_else(|| anyhow!("percorso non UTF-8: {}", p.display()))?;
    Ok(CString::new(s)?)
}

/// Legge un file STEP o IGES: ogni pezzo nella sua posizione finale. `lineare` e' la tolleranza della triangolazione in mm,
/// `angolare` in radianti.
pub fn carica(percorso: &Path, lineare: f64, angolare: f64) -> Result<Vec<Parte>> {
    let (voci, _) = carica_voci(percorso, lineare, angolare, None, false)?;
    Ok(voci.into_iter().filter_map(|v| if let Voce::Geometria(p) = v { Some(p) } else { None }).collect())
}

/// Legge un file STEP o IGES con le scelte del plugin: `tetto` = al massimo questi triangoli (la tolleranza si allarga finche'
/// ci si sta); `istanze` = i pezzi ripetuti arrivano come una definizione e le sue istanze. Restituisce le voci e la tolleranza
/// usata davvero.
pub fn carica_voci(percorso: &Path, lineare: f64, angolare: f64, tetto: Option<usize>, istanze: bool) -> Result<(Vec<Voce>, f64)> {
    let formato = Formato::da_percorso(percorso)?;
    if !percorso.is_file() {
        bail!("file non trovato: {}", percorso.display());
    }
    let path = c_path(percorso)?;
    let mut voci: Vec<Voce> = Vec::new();
    let mut usata = lineare;
    let mut err = vec![0u8; 512];
    // SAFETY: tutti i puntatori restano validi durante la chiamata.
    let codice = unsafe {
        rstp_load(
            path.as_ptr(),
            formato.codice(),
            lineare,
            angolare,
            tetto.unwrap_or(0) as u64,
            i32::from(istanze),
            raccogli,
            &mut voci as *mut _ as *mut c_void,
            &mut usata,
            err.as_mut_ptr() as *mut c_char,
            err.len() as u32,
        )
    };
    if codice != 0 {
        bail!("{}", messaggio(&err));
    }
    Ok((voci, usata))
}

/// Scrive un file di prova (due cubi colorati in un assieme).
pub fn scrivi_cubi_di_prova(percorso: &Path, formato: Formato) -> Result<()> {
    scrivi_prova(percorso, formato, false)
}

/// Come `scrivi_cubi_di_prova`, con il cubo rosso che compare due volte (il secondo 50 mm piu' in alto).
pub fn scrivi_cubi_ripetuti(percorso: &Path, formato: Formato) -> Result<()> {
    scrivi_prova(percorso, formato, true)
}

fn scrivi_prova(percorso: &Path, formato: Formato, ripetuto: bool) -> Result<()> {
    let path = c_path(percorso)?;
    let mut err = vec![0u8; 512];
    // SAFETY: come sopra.
    let codice = unsafe { rstp_write_test_box(path.as_ptr(), formato.codice(), i32::from(ripetuto), err.as_mut_ptr() as *mut c_char, err.len() as u32) };
    if codice != 0 {
        bail!("{}", messaggio(&err));
    }
    Ok(())
}

pub fn versione_occt() -> String {
    // SAFETY: stringa statica terminata da zero.
    unsafe { CStr::from_ptr(rstp_occt_version()).to_string_lossy().into_owned() }
}
