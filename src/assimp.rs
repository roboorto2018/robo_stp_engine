//! File FBX, glTF/GLB, OBJ, 3MF e PLY con Assimp (Open Asset Import Library, ponte C++ in `cpp/assimp_bridge.cpp`): le mesh dei nodi,
//! con nomi, posizioni, colori dei materiali (o la media dei colori dei vertici) e, se chiesto, immagini e mappatura. Unita' e assi
//! dal file (FBX: UnitScaleFactor, UpAxis...; 3MF: l'attributo unit del modello), da quelli del formato (glTF: metri, Y in alto) o
//! scelti.

use crate::ffi::{Parte, Texture};
use crate::rstp::Voce;
use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};

type RaccogliAssimp =
    extern "C" fn(*mut c_void, i32, u32, *const c_char, *const f32, u64, *const f32, u64, *const u32, *const f32, *const c_char, *const f32);

extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn rasm_load(
        path: *const c_char,
        unit_mm: f64,
        default_unit_mm: f64,
        file_axes: i32,
        instances: i32,
        with_uv: i32,
        texture_dir: *const c_char,
        cb: RaccogliAssimp,
        user: *mut c_void,
        info: *mut f64,
        err: *mut c_char,
        err_len: u32,
    ) -> i32;
    fn rasm_assimp_version() -> *const c_char;
}

/// I formati letti con Assimp, con quello che ognuno dice (o non dice) di unita' e assi.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Formato {
    /// unita' e assi scritti nel file
    Fbx,
    /// glTF 2.0, testo o binario (GLB): sempre metri, Y in alto
    Gltf,
    /// nessuna unita' ne' asse nel file: di solito Y in alto, l'unita' la sceglie l'utente (predefinito: metri)
    Obj,
    /// l'unita' e' scritta nel modello (predefinito millimetri), Z in alto
    Treemmf,
    /// nessuna unita' ne' asse nel file (scansioni): l'unita' la sceglie l'utente (predefinito: millimetri)
    Ply,
}

impl Formato {
    /// Il formato dall'estensione: .fbx .gltf .glb .obj .3mf .ply
    pub fn da_percorso(p: &Path) -> Option<Formato> {
        match p.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
            Some("fbx") => Some(Formato::Fbx),
            Some("gltf") | Some("glb") => Some(Formato::Gltf),
            Some("obj") => Some(Formato::Obj),
            Some("3mf") => Some(Formato::Treemmf),
            Some("ply") => Some(Formato::Ply),
            _ => None,
        }
    }

    /// Millimetri per unita' quando il file non li dice (FBX li dice sempre; 3MF: si leggono con `unita_3mf`).
    pub fn unita_predefinita_mm(self) -> f64 {
        match self {
            Formato::Fbx => 10.0,
            Formato::Gltf | Formato::Obj => 1000.0,
            Formato::Treemmf | Formato::Ply => 1.0,
        }
    }

    /// Se, senza altre scelte, gli assi del file si portano a quelli di SketchUp: FBX (gli assi scritti), glTF e OBJ (Y in alto).
    /// 3MF ha gia' Z in alto; un PLY li lascia come sono.
    pub fn assi_predefiniti(self) -> bool {
        matches!(self, Formato::Fbx | Formato::Gltf | Formato::Obj)
    }
}

/// Le scelte per un file letto con Assimp.
#[derive(Clone, Debug)]
pub struct OpzioniAssimp {
    /// millimetri per unita' scelti dall'utente; `None`: quelli scritti nel file o, se non li dice, `unita_predefinita_mm`
    pub unita_mm: Option<f64>,
    /// millimetri per unita' quando il file non li dice (quelli del formato)
    pub unita_predefinita_mm: f64,
    /// true: gli assi del file (o, se il file non li dice, Y in alto) diventano quelli di SketchUp, Z in alto; false: come sono
    pub assi_del_file: bool,
    /// una mesh usata da piu' nodi arriva come una definizione e le sue istanze
    pub istanze: bool,
    /// immagini dei materiali e mappatura
    pub uv: bool,
    /// dove scrivere le immagini incorporate nel file
    pub cartella_texture: PathBuf,
}

/// Quello che il file dice di se'.
#[derive(Clone, Debug, Default)]
pub struct InfoAssimp {
    /// millimetri per unita' del file (scritti nel file, o quelli del formato)
    pub unita_file_mm: f64,
    /// true: l'unita' e' scritta nel file
    pub unita_dal_file: bool,
    /// asse in alto del file: 0 x, 1 y, 2 z; -1 se il file non lo dice
    pub asse_alto: i32,
    pub mesh: u64,
    pub nodi: u64,
    /// triangoli del file: ogni uso di una mesh conta
    pub triangoli: u64,
    /// immagini incorporate scritte nella cartella delle texture
    pub incorporate: u64,
}

impl InfoAssimp {
    /// Il nome dell'unita' del file (mm, cm, m, in, ft), o i millimetri se non e' una di queste.
    pub fn nome_unita(&self) -> String {
        for (nome, mm) in [("mm", 1.0), ("cm", 10.0), ("m", 1000.0), ("in", 25.4), ("ft", 304.8)] {
            if (self.unita_file_mm - mm).abs() < mm * 1e-4 {
                return nome.to_string();
            }
        }
        format!("{}mm", self.unita_file_mm)
    }
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
    uv: *const f32,
    texture: *const c_char,
    matrice: *const f32,
) {
    // SAFETY: il ponte passa puntatori validi per la durata della chiamata; `user` e' il nostro Vec. Un'istanza ha la matrice e
    // nessun triangolo; le altre voci hanno colore e triangoli, e `uv` e `texture` insieme o nessuno dei due.
    unsafe {
        let voci = &mut *(user as *mut Vec<Voce>);
        let nome = CStr::from_ptr(nome).to_string_lossy().into_owned();
        if tipo == 2 {
            let mut m = [0.0f32; 12];
            m.copy_from_slice(std::slice::from_raw_parts(matrice, 12));
            voci.push(Voce::Istanza { definizione: id, nome, matrice: m });
            return;
        }
        let c = std::slice::from_raw_parts(rgba, 4);
        let colore = if c[0] < 0.0 { None } else { Some([c[0], c[1], c[2], c[3]]) };
        let vertici = std::slice::from_raw_parts(v, nv as usize * 3);
        let triangoli = std::slice::from_raw_parts(idx, nt as usize * 3);
        let coord = if uv.is_null() { None } else { Some(std::slice::from_raw_parts(uv, nv as usize * 2)) };
        let file = if texture.is_null() { None } else { Some(CStr::from_ptr(texture).to_string_lossy().into_owned()) };
        let parte = unisci(nome, colore, vertici, triangoli, coord.zip(file));
        voci.push(if tipo == 1 { Voce::Definizione(id, parte) } else { Voce::Geometria(parte) });
    }
}

/// Assimp divide i vertici dove la mappatura cambia (i bordi della texture): qui i punti nella stessa posizione tornano uno solo,
/// cosi' SketchUp fa una superficie unita; i triangoli che toccano un punto con coordinate diverse da un triangolo all'altro
/// portano le loro (`bordi`), come per i file .max.
fn unisci(nome: String, colore: Option<[f32; 4]>, vertici: &[f32], triangoli: &[u32], mappa: Option<(&[f32], String)>) -> Parte {
    let mut nuovo: Vec<u32> = Vec::with_capacity(vertici.len() / 3);
    let mut posti: HashMap<[u32; 3], u32> = HashMap::new();
    let mut punti: Vec<f32> = Vec::new();
    for p in vertici.chunks(3) {
        let chiave = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
        let n = posti.len() as u32;
        let k = *posti.entry(chiave).or_insert_with(|| {
            punti.extend_from_slice(p);
            n
        });
        nuovo.push(k);
    }
    let tri: Vec<u32> = triangoli.iter().map(|&i| nuovo[i as usize]).collect();
    let texture = mappa.map(|(uv, file)| {
        let mut per_punto: Vec<Option<[f32; 2]>> = vec![None; punti.len() / 3];
        let mut diverso = vec![false; punti.len() / 3];
        for (&vecchio, &k) in triangoli.iter().zip(&tri) {
            let u = [uv[2 * vecchio as usize], uv[2 * vecchio as usize + 1]];
            match per_punto[k as usize] {
                None => per_punto[k as usize] = Some(u),
                Some(a) if (a[0] - u[0]).abs() > 1e-5 || (a[1] - u[1]).abs() > 1e-5 => diverso[k as usize] = true,
                _ => {}
            }
        }
        let bordi = triangoli
            .chunks(3)
            .zip(tri.chunks(3))
            .enumerate()
            .filter(|(_, (_, t))| t.iter().any(|&k| diverso[k as usize]))
            .map(|(i, (vecchi, _))| {
                let u = |j: usize| [uv[2 * vecchi[j] as usize], uv[2 * vecchi[j] as usize + 1]];
                let (a, b, c) = (u(0), u(1), u(2));
                (i as u32, [a[0], a[1], b[0], b[1], c[0], c[1]])
            })
            .collect();
        Texture { file, uv: per_punto.iter().flat_map(|u| u.unwrap_or([0.0, 0.0])).collect(), bordi }
    });
    Parte { nome, colore, vertici: punti, triangoli: tri, texture }
}

/// Legge un file FBX, glTF/GLB, OBJ, 3MF o PLY.
pub fn carica(percorso: &Path, opzioni: &OpzioniAssimp) -> Result<(Vec<Voce>, InfoAssimp)> {
    if !percorso.is_file() {
        bail!("file non trovato: {}", percorso.display());
    }
    let testo = |p: &Path| -> Result<CString> { Ok(CString::new(p.to_str().ok_or_else(|| anyhow!("percorso non UTF-8: {}", p.display()))?)?) };
    let path = testo(percorso)?;
    let cartella = testo(&opzioni.cartella_texture)?;
    let mut voci: Vec<Voce> = Vec::new();
    let mut info = [0.0f64; 7];
    let mut err = vec![0u8; 1024];
    // SAFETY: tutti i puntatori restano validi durante la chiamata; `info` ha i sette posti che il ponte scrive.
    let codice = unsafe {
        rasm_load(
            path.as_ptr(),
            opzioni.unita_mm.unwrap_or(0.0),
            opzioni.unita_predefinita_mm,
            i32::from(opzioni.assi_del_file),
            i32::from(opzioni.istanze),
            i32::from(opzioni.uv),
            cartella.as_ptr(),
            raccogli,
            &mut voci as *mut _ as *mut c_void,
            info.as_mut_ptr(),
            err.as_mut_ptr() as *mut c_char,
            err.len() as u32,
        )
    };
    if codice != 0 {
        let fine = err.iter().position(|&b| b == 0).unwrap_or(err.len());
        bail!("{}", spiega(&String::from_utf8_lossy(&err[..fine])));
    }
    let info = InfoAssimp {
        unita_file_mm: info[0],
        unita_dal_file: info[6] > 0.5,
        asse_alto: info[1] as i32,
        mesh: info[2] as u64,
        nodi: info[3] as u64,
        triangoli: info[4] as u64,
        incorporate: info[5] as u64,
    };
    Ok((voci, info))
}

/// I messaggi di Assimp che capitano piu' spesso, detti in modo chiaro (con il messaggio originale tra parentesi).
fn spiega(messaggio: &str) -> String {
    let m = messaggio.to_lowercase();
    let chiaro = if m.contains("no meshes remaining") || m.contains("has no faces") {
        "il file contiene solo punti o linee, senza facce: non c'e' niente da importare (una nuvola di punti va prima trasformata in una mesh)"
    } else if m.contains("draco") {
        "il file glTF e' compresso con Draco, che questo motore non legge: esportalo (o convertilo) senza la compressione Draco"
    } else if m.contains("could not open referenced file") {
        "manca un file a cui il glTF rimanda (il .bin o un'immagine): tienilo nella stessa cartella del file .gltf"
    } else if m.contains("unable to open file") || m.contains("failed to open") {
        "il file non si apre: controlla che esista e che non sia aperto in un altro programma"
    } else {
        return messaggio.to_string();
    };
    format!("{chiaro} ({messaggio})")
}

/// Versione di Assimp, es. "6.0.2".
pub fn versione_assimp() -> String {
    // SAFETY: stringa statica terminata da zero.
    unsafe { CStr::from_ptr(rasm_assimp_version()).to_string_lossy().into_owned() }
}

/// L'unita' di un file 3MF: l'attributo `unit` del modello (`3D/3dmodel.model` dentro l'archivio ZIP), in millimetri per unita'
/// (micron, millimeter, centimeter, inch, foot, meter; senza attributo: millimeter). `None` se il modello non si legge.
pub fn unita_3mf(percorso: &Path) -> Option<f64> {
    let dati = std::fs::read(percorso).ok()?;
    let modello = voce_zip(&dati, |nome| nome.to_lowercase().ends_with(".model"))?;
    let testo = String::from_utf8_lossy(&modello);
    let inizio = testo.find("<model")?;
    let tag = &testo[inizio..inizio + testo[inizio..].find('>')?];
    let unita = tag.find("unit=").and_then(|k| {
        let resto = &tag[k + 5..];
        let virgolette = resto.chars().next()?;
        let resto = &resto[1..];
        Some(resto[..resto.find(virgolette)?].to_string())
    });
    Some(match unita.as_deref().map(str::to_lowercase).as_deref() {
        Some("micron") => 0.001,
        Some("centimeter") => 10.0,
        Some("inch") => 25.4,
        Some("foot") => 304.8,
        Some("meter") => 1000.0,
        _ => 1.0,
    })
}

/// L'inizio (al massimo 64 KB) della prima voce di un archivio ZIP il cui nome va bene a `scegli` (dall'elenco in fondo
/// all'archivio; voci salvate o compresse con deflate).
fn voce_zip(dati: &[u8], scegli: impl Fn(&str) -> bool) -> Option<Vec<u8>> {
    use std::io::Read;
    let u16_a = |i: usize| -> Option<usize> { Some(u16::from_le_bytes(dati.get(i..i + 2)?.try_into().ok()?) as usize) };
    let u32_a = |i: usize| -> Option<usize> { Some(u32::from_le_bytes(dati.get(i..i + 4)?.try_into().ok()?) as usize) };
    let fine = (0..dati.len().saturating_sub(21)).rev().take(70_000).find(|&i| dati[i..].starts_with(b"PK\x05\x06"))?;
    let voci = u16_a(fine + 10)?;
    let mut p = u32_a(fine + 16)?;
    for _ in 0..voci {
        if !dati.get(p..)?.starts_with(b"PK\x01\x02") {
            return None;
        }
        let metodo = u16_a(p + 10)?;
        let compressa = u32_a(p + 20)?;
        let (n, e, c) = (u16_a(p + 28)?, u16_a(p + 30)?, u16_a(p + 32)?);
        let locale = u32_a(p + 42)?;
        let nome = String::from_utf8_lossy(dati.get(p + 46..p + 46 + n)?).into_owned();
        p += 46 + n + e + c;
        if !scegli(&nome) {
            continue;
        }
        let inizio = locale + 30 + u16_a(locale + 26)? + u16_a(locale + 28)?;
        let grezzi = dati.get(inizio..inizio + compressa)?;
        return match metodo {
            0 => Some(grezzi[..grezzi.len().min(65_536)].to_vec()),
            8 => {
                let mut fuori = Vec::new();
                flate2::read::DeflateDecoder::new(grezzi).take(65_536).read_to_end(&mut fuori).ok()?;
                Some(fuori)
            }
            _ => None,
        };
    }
    None
}
