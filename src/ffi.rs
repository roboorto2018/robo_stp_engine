//! Collegamento sicuro al ponte C++ di OpenCascade.

use anyhow::{anyhow, bail, Result};
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;

extern "C" {
    fn rstp_load(
        path: *const c_char,
        format: i32,
        linear: f64,
        angular: f64,
        cb: extern "C" fn(*mut c_void, *const c_char, *const f32, u64, *const f32, u64, *const u32),
        user: *mut c_void,
        err: *mut c_char,
        err_len: u32,
    ) -> i32;
    fn rstp_write_test_box(path: *const c_char, format: i32, err: *mut c_char, err_len: u32) -> i32;
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
}

extern "C" fn raccogli(user: *mut c_void, nome: *const c_char, rgba: *const f32, nv: u64, v: *const f32, nt: u64, idx: *const u32) {
    // SAFETY: il ponte passa puntatori validi per la durata della chiamata; `user` e' il nostro Vec.
    unsafe {
        let parti = &mut *(user as *mut Vec<Parte>);
        let nome = CStr::from_ptr(nome).to_string_lossy().into_owned();
        let c = std::slice::from_raw_parts(rgba, 4);
        let colore = if c[0] < 0.0 { None } else { Some([c[0], c[1], c[2], c[3]]) };
        parti.push(Parte {
            nome,
            colore,
            vertici: std::slice::from_raw_parts(v, nv as usize * 3).to_vec(),
            triangoli: std::slice::from_raw_parts(idx, nt as usize * 3).to_vec(),
        });
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

/// Legge un file STEP o IGES. `lineare` e' la tolleranza della triangolazione in mm, `angolare` in radianti.
pub fn carica(percorso: &Path, lineare: f64, angolare: f64) -> Result<Vec<Parte>> {
    let formato = Formato::da_percorso(percorso)?;
    if !percorso.is_file() {
        bail!("file non trovato: {}", percorso.display());
    }
    let path = c_path(percorso)?;
    let mut parti: Vec<Parte> = Vec::new();
    let mut err = vec![0u8; 512];
    // SAFETY: tutti i puntatori restano validi durante la chiamata.
    let codice = unsafe {
        rstp_load(path.as_ptr(), formato.codice(), lineare, angolare, raccogli, &mut parti as *mut _ as *mut c_void, err.as_mut_ptr() as *mut c_char, err.len() as u32)
    };
    if codice != 0 {
        bail!("{}", messaggio(&err));
    }
    Ok(parti)
}

/// Scrive un file di prova (due cubi colorati in un assieme).
pub fn scrivi_cubi_di_prova(percorso: &Path, formato: Formato) -> Result<()> {
    let path = c_path(percorso)?;
    let mut err = vec![0u8; 512];
    // SAFETY: come sopra.
    let codice = unsafe { rstp_write_test_box(path.as_ptr(), formato.codice(), err.as_mut_ptr() as *mut c_char, err.len() as u32) };
    if codice != 0 {
        bail!("{}", messaggio(&err));
    }
    Ok(())
}

pub fn versione_occt() -> String {
    // SAFETY: stringa statica terminata da zero.
    unsafe { CStr::from_ptr(rstp_occt_version()).to_string_lossy().into_owned() }
}
