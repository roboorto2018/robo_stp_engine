//! Prova completa con OpenCascade: scrive un assieme di due cubi in STEP e in IGES, lo rilegge e controlla
//! parti, colori, posizioni (il secondo cubo e' spostato di 30 mm) e orientamento dei triangoli.

use robo_stp_engine::ffi::{carica, scrivi_cubi_di_prova, Formato, Parte};
use robo_stp_engine::rstp;

/// OpenCascade ha impostazioni globali (per esempio l'unita' di lettura): i test che lo usano passano uno alla volta.
static OCCT: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn in_fila() -> std::sync::MutexGuard<'static, ()> {
    OCCT.lock().unwrap_or_else(|e| e.into_inner())
}

fn limiti(parti: &[Parte]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for p in parti {
        for v in p.vertici.chunks(3) {
            for i in 0..3 {
                min[i] = min[i].min(v[i]);
                max[i] = max[i].max(v[i]);
            }
        }
    }
    (min, max)
}

/// Volume del solido dai triangoli (teorema della divergenza): positivo se sono orientati verso l'esterno.
fn volume(p: &Parte) -> f64 {
    let mut vol = 0.0;
    for t in p.triangoli.chunks(3) {
        let v = |i: u32| {
            let k = i as usize * 3;
            [p.vertici[k] as f64, p.vertici[k + 1] as f64, p.vertici[k + 2] as f64]
        };
        let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
        vol += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.0;
    }
    vol
}

fn prova(formato: Formato, estensione: &str) -> Vec<Parte> {
    let dir = std::env::temp_dir().join(format!("robo_stp_test_{estensione}"));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("prova.{estensione}"));
    scrivi_cubi_di_prova(&file, formato).unwrap();
    let parti = carica(&file, 0.1, 0.5).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    parti
}

#[test]
fn step_assieme_con_nomi_colori_e_posizioni() {
    let _fila = in_fila();
    let parti = prova(Formato::Step, "stp");
    assert_eq!(parti.len(), 2);
    let rosso = parti.iter().find(|p| p.nome == "Assieme/CuboRosso").expect("nome del pezzo");
    let blu = parti.iter().find(|p| p.nome == "Assieme/CuboBlu").expect("nome del pezzo");
    assert_eq!(rosso.colore, Some([1.0, 0.0, 0.0, 1.0]));
    assert_eq!(blu.colore, Some([0.0, 0.0, 1.0, 1.0]));
    // cubo rosso 20x10x5 nell'origine, cubo blu 10x10x10 spostato di 30 mm in X
    let (min, max) = limiti(std::slice::from_ref(rosso));
    assert_eq!((min, max), ([0.0, 0.0, 0.0], [20.0, 10.0, 5.0]));
    let (min, max) = limiti(std::slice::from_ref(blu));
    assert_eq!((min, max), ([30.0, 0.0, 0.0], [40.0, 10.0, 10.0]));
    assert!((volume(rosso) - 1000.0).abs() < 1e-3, "volume rosso {}", volume(rosso));
    assert!((volume(blu) - 1000.0).abs() < 1e-3, "volume blu {}", volume(blu));
}

#[test]
fn iges_stessa_geometria() {
    let _fila = in_fila();
    let parti = prova(Formato::Iges, "igs");
    assert!(!parti.is_empty());
    let (min, max) = limiti(&parti);
    assert_eq!((min, max), ([0.0, 0.0, 0.0], [40.0, 10.0, 10.0]));
    let totale: f64 = parti.iter().map(volume).sum();
    assert!((totale - 2000.0).abs() < 1e-3, "volume {totale}");
    assert!(parti.iter().any(|p| p.colore == Some([1.0, 0.0, 0.0, 1.0])));
}

#[test]
fn il_formato_di_scambio_conserva_tutto() {
    let _fila = in_fila();
    let parti = prova(Formato::Step, "step");
    assert_eq!(rstp::leggi(&rstp::scrivi(&parti)).unwrap(), parti);
}

#[test]
fn pezzi_ripetuti_come_definizione_e_istanze() {
    let _fila = in_fila();
    use robo_stp_engine::ffi::{carica_voci, scrivi_cubi_ripetuti};
    use robo_stp_engine::rstp::Voce;
    let dir = std::env::temp_dir().join("robo_stp_test_ripetuti");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ripetuti.stp");
    scrivi_cubi_ripetuti(&file, Formato::Step).unwrap();
    // senza istanze: tre parti, il rosso due volte (il secondo 50 mm piu' in alto)
    let parti = carica(&file, 0.1, 0.5).unwrap();
    assert_eq!(parti.len(), 3);
    // con le istanze: il rosso e' una definizione (nelle sue coordinate) con due istanze, il blu resta una parte
    let voci = carica_voci(&file, 0.1, 0.5, None, true, &[]).unwrap().voci;
    std::fs::remove_dir_all(&dir).ok();
    let definizioni: Vec<_> = voci.iter().filter_map(|v| if let Voce::Definizione(id, p) = v { Some((*id, p)) } else { None }).collect();
    assert_eq!(definizioni.len(), 1);
    let (id, rosso) = definizioni[0];
    assert_eq!(rosso.colore, Some([1.0, 0.0, 0.0, 1.0]));
    assert_eq!(limiti(std::slice::from_ref(rosso)), ([0.0, 0.0, 0.0], [20.0, 10.0, 5.0]));
    let istanze: Vec<_> = voci.iter().filter_map(|v| if let Voce::Istanza { definizione, matrice, .. } = v { Some((*definizione, *matrice)) } else { None }).collect();
    assert_eq!(istanze.len(), 2);
    assert!(istanze.iter().all(|(d, _)| *d == id));
    let mut alture: Vec<f32> = istanze.iter().map(|(_, m)| m[11]).collect();
    alture.sort_by(f32::total_cmp);
    assert_eq!(alture, vec![0.0, 50.0]);
    assert_eq!(istanze[0].1[..9], [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    assert_eq!(voci.iter().filter(|v| matches!(v, Voce::Geometria(_))).count(), 1);
}

#[test]
fn il_tetto_dei_triangoli_allarga_la_tolleranza() {
    let _fila = in_fila();
    use robo_stp_engine::ffi::carica_voci;
    use robo_stp_engine::rstp::Voce;
    // due cubi: 24 triangoli in tutto, qualunque tolleranza; il tetto non puo' scendere sotto, ma non deve rompere nulla
    let dir = std::env::temp_dir().join("robo_stp_test_tetto");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("tetto.stp");
    scrivi_cubi_di_prova(&file, Formato::Step).unwrap();
    let letto = carica_voci(&file, 0.1, 0.5, Some(10), false, &[]).unwrap();
    let (voci, usata) = (letto.voci, letto.tolleranza);
    std::fs::remove_dir_all(&dir).ok();
    let triangoli: usize = voci.iter().map(|v| if let Voce::Geometria(p) = v { p.triangoli.len() / 3 } else { 0 }).sum();
    assert_eq!(triangoli, 24);
    assert!(usata > 0.1, "la tolleranza doveva allargarsi: {usata}");
}

#[test]
fn le_stime_dei_dettagli_contano_i_triangoli() {
    let _fila = in_fila();
    use robo_stp_engine::ffi::{carica_voci, scrivi_cubi_ripetuti};
    let dir = std::env::temp_dir().join("robo_stp_test_stime");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("stime.stp");
    scrivi_cubi_ripetuti(&file, Formato::Step).unwrap();
    // tre cubi (il rosso due volte): 36 triangoli nel modello, 24 da costruire, a qualunque dettaglio (sono piani)
    let letto = carica_voci(&file, 5.0, 1.5, None, false, &[(0.5, 0.8), (0.1, 0.5), (0.02, 0.3)]).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(letto.stime.len(), 3);
    // Bozza e Normale triangolate per intero, Alta stimata dal campione (qui il rapporto e' 1: i cubi sono piani)
    let conti: Vec<_> = letto.stime.iter().map(|s| (s.nel_modello, s.da_costruire, s.misurata)).collect();
    assert_eq!(conti, vec![(36, 24, true), (36, 24, true), (36, 24, false)]);
    // dopo le stime l'anteprima resta quella grossolana chiesta
    assert_eq!(letto.tolleranza, 5.0);
}

#[test]
fn errori_chiari() {
    let _fila = in_fila();
    let dir = std::env::temp_dir().join("robo_stp_test_errori");
    std::fs::create_dir_all(&dir).unwrap();
    let rovinato = dir.join("rovinato.stp");
    std::fs::write(&rovinato, "questo non e' un file STEP").unwrap();
    assert!(carica(&rovinato, 0.1, 0.5).is_err());
    assert!(carica(&dir.join("manca.stp"), 0.1, 0.5).unwrap_err().to_string().contains("non trovato"));
    assert!(carica(&dir.join("x.txt"), 0.1, 0.5).unwrap_err().to_string().contains("estensione"));
    std::fs::remove_dir_all(&dir).ok();
}
