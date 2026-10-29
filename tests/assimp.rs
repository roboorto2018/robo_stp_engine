//! Prova dei file FBX, glTF/GLB, OBJ, 3MF e PLY con Assimp, sui modelli di prova del sorgente di Assimp (cartella ASSIMP_MODELS,
//! predefinita C:/assimp/src/test/models): se la cartella non c'e' le prove non fanno niente.

use robo_stp_engine::assimp::{carica, unita_3mf, Formato, OpzioniAssimp};
use robo_stp_engine::ffi::Parte;
use robo_stp_engine::rstp::{self, Voce};
use std::path::PathBuf;

fn modello(nome: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("ASSIMP_MODELS").unwrap_or_else(|_| "C:/assimp/src/test/models".into()));
    let f = if nome.contains('/') { dir.join(nome) } else { dir.join("FBX").join(nome) };
    f.is_file().then_some(f)
}

/// Le scelte che il motore usa per quel formato (unita' e assi predefiniti; il 3MF con l'unita' letta dal file).
fn opzioni_di(f: &std::path::Path, uv: bool) -> OpzioniAssimp {
    let formato = Formato::da_percorso(f).expect("formato");
    let unita = if formato == Formato::Treemmf { unita_3mf(f).unwrap_or(1.0) } else { formato.unita_predefinita_mm() };
    OpzioniAssimp { unita_predefinita_mm: unita, assi_del_file: formato.assi_predefiniti(), ..opzioni(uv) }
}

fn lati(voci: &[Voce]) -> [f32; 3] {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for p in parti(voci) {
        let (a, b) = limiti(p);
        for i in 0..3 {
            min[i] = min[i].min(a[i]);
            max[i] = max[i].max(b[i]);
        }
    }
    [max[0] - min[0], max[1] - min[1], max[2] - min[2]]
}

fn opzioni(uv: bool) -> OpzioniAssimp {
    OpzioniAssimp { unita_mm: None, unita_predefinita_mm: Formato::Fbx.unita_predefinita_mm(), assi_del_file: true, istanze: true, uv, cartella_texture: std::env::temp_dir().join("robo_fbx_test_tex") }
}

fn parti(voci: &[Voce]) -> Vec<&Parte> {
    voci.iter().filter_map(|v| if let Voce::Geometria(p) = v { Some(p) } else { None }).collect()
}

fn limiti(p: &Parte) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for v in p.vertici.chunks(3) {
        for i in 0..3 {
            min[i] = min[i].min(v[i]);
            max[i] = max[i].max(v[i]);
        }
    }
    (min, max)
}

/// Volume dai triangoli: positivo se sono orientati verso l'esterno.
fn volume(p: &Parte) -> f64 {
    p.triangoli
        .chunks(3)
        .map(|t| {
            let v = |i: u32| {
                let k = i as usize * 3;
                [p.vertici[k] as f64, p.vertici[k + 1] as f64, p.vertici[k + 2] as f64]
            };
            let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.0
        })
        .sum()
}

#[test]
fn un_cubo_chiuso_in_millimetri() {
    let Some(f) = modello("box.fbx") else { return };
    let (voci, info) = carica(&f, &opzioni(false)).unwrap();
    let p = parti(&voci);
    assert_eq!(p.len(), 1);
    assert_eq!(info.nome_unita(), "m");
    // i punti uguali sono uno solo: un cubo ha 8 angoli e 12 triangoli
    assert_eq!(p[0].vertici.len() / 3, 8);
    assert_eq!(p[0].triangoli.len() / 3, 12);
    assert!(volume(p[0]) > 0.0, "facce verso l'esterno");
    let (min, max) = limiti(p[0]);
    println!("box.fbx: {min:?} {max:?}, volume {}", volume(p[0]));
}

#[test]
fn le_unita_scelte_cambiano_la_misura() {
    let Some(f) = modello("box.fbx") else { return };
    let (voci, _) = carica(&f, &opzioni(false)).unwrap();
    let (min, max) = limiti(parti(&voci)[0]);
    let mut o = opzioni(false);
    o.unita_mm = Some(10.0); // il file e' in metri: in centimetri arriva cento volte piu' piccolo
    let (voci, _) = carica(&f, &o).unwrap();
    let (min2, max2) = limiti(parti(&voci)[0]);
    assert!(((max[0] - min[0]) / (max2[0] - min2[0]) - 100.0).abs() < 1e-3);
}

#[test]
fn y_in_alto_diventa_z_in_alto() {
    let Some(f) = modello("spider.fbx") else { return };
    let (voci, info) = carica(&f, &opzioni(false)).unwrap();
    assert_eq!(info.asse_alto, 1);
    let (_, alto) = carica(&f, &OpzioniAssimp { assi_del_file: false, ..opzioni(false) }).unwrap();
    assert_eq!(alto.asse_alto, 1);
    let tutti = |voci: &[Voce]| {
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for p in parti(voci) {
            let (a, b) = limiti(p);
            for i in 0..3 {
                min[i] = min[i].min(a[i]);
                max[i] = max[i].max(b[i]);
            }
        }
        (min, max)
    };
    let (min, max) = tutti(&voci);
    let (vmin, vmax) = {
        let (voci, _) = carica(&f, &OpzioniAssimp { assi_del_file: false, ..opzioni(false) }).unwrap();
        tutti(&voci)
    };
    // l'altezza (Y nel file) diventa Z
    assert!(((max[2] - min[2]) - (vmax[1] - vmin[1])).abs() < 1e-2, "{min:?} {max:?} / {vmin:?} {vmax:?}");
    assert!(((max[1] - min[1]) - (vmax[2] - vmin[2])).abs() < 1e-2);
}

#[test]
fn copie_specchiate_restano_verso_l_esterno() {
    let Some(f) = modello("cubes_with_mirroring_and_pivot.fbx") else { return };
    let (voci, _) = carica(&f, &opzioni(false)).unwrap();
    for p in parti(&voci) {
        assert!(volume(p) > 0.0, "{}: volume {}", p.nome, volume(p));
    }
}

#[test]
fn colori_e_immagini_dei_materiali() {
    for nome in ["phong_cube.fbx", "box_orphant_embedded_texture.fbx", "maxPbrMaterial_metalRough.fbx", "spider.fbx"] {
        let Some(f) = modello(nome) else { continue };
        let (voci, info) = carica(&f, &opzioni(true)).unwrap();
        for p in parti(&voci) {
            let t = p.texture.as_ref().map(|t| (t.file.clone(), t.uv.len() / 2, t.bordi.len()));
            println!("{nome}: {} colore {:?} texture {:?}", p.nome, p.colore, t);
            if let Some(t) = &p.texture {
                assert_eq!(t.uv.len() / 2, p.vertici.len() / 3);
            }
        }
        println!("  incorporate {}", info.incorporate);
    }
}

#[test]
fn il_formato_di_scambio_si_rilegge() {
    let Some(f) = modello("spider.fbx") else { return };
    let (voci, _) = carica(&f, &opzioni(true)).unwrap();
    let dati = rstp::scrivi_con_morbidi(&voci, 30.0);
    assert!(dati.starts_with(b"RSTP"));
}

#[test]
fn gltf_in_metri_e_con_l_immagine_incorporata() {
    let Some(f) = modello("glTF2/BoxTextured-glTF-Binary/BoxTextured.glb") else { return };
    let (voci, info) = carica(&f, &opzioni_di(&f, true)).unwrap();
    assert_eq!(info.nome_unita(), "m");
    assert!(!info.unita_dal_file);
    assert_eq!(info.incorporate, 1, "l'immagine del GLB si scrive nella cartella delle texture");
    let p = parti(&voci);
    assert!(p[0].texture.is_some());
    assert!(volume(p[0]) > 0.0);
    // il cubo di prova e' di 1 m di lato: 1000 mm
    for l in lati(&voci) {
        assert!((l - 1000.0).abs() < 1.0, "{:?}", lati(&voci));
    }
}

#[test]
fn gltf_pezzi_ripetuti_come_istanze() {
    let Some(f) = modello("glTF2/2CylinderEngine-glTF-Binary/2CylinderEngine.glb") else { return };
    let (voci, _) = carica(&f, &opzioni_di(&f, false)).unwrap();
    let istanze = voci.iter().filter(|v| matches!(v, Voce::Istanza { .. })).count();
    assert!(istanze >= 50, "istanze: {istanze}");
}

#[test]
fn obj_con_i_materiali_del_mtl() {
    let Some(f) = modello("OBJ/spider.obj") else { return };
    let (voci, info) = carica(&f, &opzioni_di(&f, true)).unwrap();
    assert_eq!(info.nome_unita(), "m", "un OBJ senza unita' scelta e' in metri");
    let p = parti(&voci);
    assert!(p.len() > 5);
    assert!(p.iter().any(|x| x.colore.is_some()), "i colori del file .mtl");
}

#[test]
fn obj_y_in_alto_diventa_z_in_alto() {
    let Some(f) = modello("OBJ/spider.obj") else { return };
    let (girato, _) = carica(&f, &opzioni_di(&f, false)).unwrap();
    let (come_e, _) = carica(&f, &OpzioniAssimp { assi_del_file: false, ..opzioni_di(&f, false) }).unwrap();
    let (a, b) = (lati(&girato), lati(&come_e));
    assert!((a[2] - b[1]).abs() < 1e-2 && (a[1] - b[2]).abs() < 1e-2, "{a:?} {b:?}");
}

#[test]
fn tremmf_con_l_unita_del_file() {
    let Some(f) = modello("3MF/box.3mf") else { return };
    assert_eq!(unita_3mf(&f), Some(1.0), "box.3mf e' in millimetri");
    let (voci, _) = carica(&f, &opzioni_di(&f, false)).unwrap();
    let p = parti(&voci);
    assert_eq!(p[0].triangoli.len() / 3, 12);
    assert!(volume(p[0]) > 0.0);
}

#[test]
fn ply_testo_e_binario_uguali() {
    let (Some(a), Some(b)) = (modello("PLY/cube.ply"), modello("PLY/cube_binary.ply")) else { return };
    let (va, _) = carica(&a, &opzioni_di(&a, false)).unwrap();
    let (vb, _) = carica(&b, &opzioni_di(&b, false)).unwrap();
    assert_eq!(parti(&va)[0].triangoli.len(), parti(&vb)[0].triangoli.len());
    assert_eq!(lati(&va), lati(&vb));
}

#[test]
fn solo_punti_dice_perche() {
    let Some(f) = modello("PLY/points.ply") else { return };
    let e = carica(&f, &opzioni_di(&f, false)).unwrap_err().to_string();
    assert!(e.contains("solo punti o linee"), "{e}");
}
