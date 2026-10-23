//! Le linee morbide: per ogni triangolo, quali dei suoi tre lati stanno fra due facce che piegano meno di un angolo (la
//! triangolazione di una superficie liscia). Il plugin le crea gia' nascoste e lisce, senza controllarle una per una in Ruby.
//!
//! Un byte per triangolo: bit 0 = lato dal primo al secondo vertice, bit 1 = dal secondo al terzo, bit 2 = dal terzo al primo.
//! I punti uguali vengono uniti prima (in un file STEP ogni faccia ha i suoi vertici, anche sul bordo comune con la vicina).

use crate::ffi::Parte;
use std::collections::HashMap;

/// Due punti piu' vicini di cosi' (mm) sono lo stesso punto.
const UNIONE: f64 = 0.001;

/// I lati morbidi di ogni triangolo della parte: piegano meno di `gradi` (almeno 0.05, cosi' anche 0 ammorbidisce le facce
/// sullo stesso piano). Un lato con una sola faccia o con piu' di due non e' mai morbido.
pub fn morbidi(p: &Parte, gradi: f64) -> Vec<u8> {
    let n = p.triangoli.len() / 3;
    let mut flag = vec![0u8; n];
    if n == 0 {
        return flag;
    }
    let unico = unisci(&p.vertici);
    let punto = |i: u32| {
        let k = i as usize * 3;
        [f64::from(p.vertici[k]), f64::from(p.vertici[k + 1]), f64::from(p.vertici[k + 2])]
    };
    let normali: Vec<Option<[f64; 3]>> = (0..n)
        .map(|t| {
            let (a, b, c) = (punto(p.triangoli[3 * t]), punto(p.triangoli[3 * t + 1]), punto(p.triangoli[3 * t + 2]));
            let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            let nn = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            let l = (nn[0] * nn[0] + nn[1] * nn[1] + nn[2] * nn[2]).sqrt();
            (l > 1e-12).then(|| [nn[0] / l, nn[1] / l, nn[2] / l])
        })
        .collect();

    // ogni lato con i suoi due estremi uniti (il minore prima), il triangolo e quale dei tre lati e'
    let mut lati: Vec<(u64, u32)> = Vec::with_capacity(3 * n);
    for t in 0..n {
        for k in 0..3 {
            let a = unico[p.triangoli[3 * t + k] as usize];
            let b = unico[p.triangoli[3 * t + (k + 1) % 3] as usize];
            if a == b {
                continue;
            }
            let chiave = (u64::from(a.min(b)) << 32) | u64::from(a.max(b));
            lati.push((chiave, (t as u32) * 3 + k as u32));
        }
    }
    lati.sort_unstable_by_key(|x| x.0);

    let coseno = gradi.max(0.05).to_radians().cos();
    let mut i = 0;
    while i < lati.len() {
        let mut j = i + 1;
        while j < lati.len() && lati[j].0 == lati[i].0 {
            j += 1;
        }
        if j - i == 2 {
            let (s0, s1) = (lati[i].1, lati[i + 1].1);
            let (t0, t1) = ((s0 / 3) as usize, (s1 / 3) as usize);
            if let (Some(n0), Some(n1)) = (normali[t0], normali[t1]) {
                if n0[0] * n1[0] + n0[1] * n1[1] + n0[2] * n1[2] >= coseno {
                    flag[t0] |= 1 << (s0 % 3);
                    flag[t1] |= 1 << (s1 % 3);
                }
            }
        }
        i = j;
    }
    flag
}

/// Per ogni vertice, il numero del primo vertice uguale (entro UNIONE). Una griglia di celle grandi UNIONE: un punto cerca
/// nella sua cella e nelle 26 intorno, cosi' due punti vicinissimi ai lati di un confine fra celle si trovano lo stesso.
fn unisci(vertici: &[f32]) -> Vec<u32> {
    let n = vertici.len() / 3;
    let mut celle: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::with_capacity(n);
    let mut unico = Vec::with_capacity(n);
    let pos = |i: usize| [f64::from(vertici[3 * i]), f64::from(vertici[3 * i + 1]), f64::from(vertici[3 * i + 2])];
    for i in 0..n {
        let p = pos(i);
        let c = [(p[0] / UNIONE).floor() as i64, (p[1] / UNIONE).floor() as i64, (p[2] / UNIONE).floor() as i64];
        let mut trovato = None;
        'cerca: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(lista) = celle.get(&(c[0] + dx, c[1] + dy, c[2] + dz)) {
                        for &k in lista {
                            let q = pos(k as usize);
                            let d = (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2);
                            if d <= UNIONE * UNIONE {
                                trovato = Some(k);
                                break 'cerca;
                            }
                        }
                    }
                }
            }
        }
        match trovato {
            Some(k) => unico.push(k),
            None => {
                celle.entry((c[0], c[1], c[2])).or_default().push(i as u32);
                unico.push(i as u32);
            }
        }
    }
    unico
}

#[cfg(test)]
mod test {
    use super::*;

    fn parte(vertici: Vec<f32>, triangoli: Vec<u32>) -> Parte {
        Parte { nome: "p".into(), colore: None, vertici, triangoli, texture: None }
    }

    #[test]
    fn piano_diviso_in_due_triangoli_con_vertici_separati() {
        // un quadrato fatto di due triangoli che NON condividono i vertici (come due facce di un file STEP)
        let p = parte(
            vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
            vec![0, 1, 2, 3, 4, 5],
        );
        let f = morbidi(&p, 0.0);
        // la diagonale (dal terzo al primo nel primo triangolo, dal primo al secondo nel secondo) e' morbida; i bordi no
        assert_eq!(f, vec![0b100, 0b001]);
    }

    #[test]
    fn spigolo_di_un_cubo_resta_e_la_piega_leggera_sparisce() {
        // due triangoli piegati di 90 gradi lungo il lato comune (0,0,0)-(1,0,0)
        let p = parte(vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], vec![0, 1, 2, 1, 0, 3]);
        assert_eq!(morbidi(&p, 30.0), vec![0, 0]);
        assert_eq!(morbidi(&p, 89.0), vec![0, 0]);
        assert_eq!(morbidi(&p, 91.0), vec![0b001, 0b001]);
    }

    #[test]
    fn punti_vicinissimi_ai_lati_di_un_confine_si_uniscono() {
        // lo stesso punto scritto con una differenza minima che lo porta nella cella accanto
        let a = 0.002_f32;
        let b = 0.002_f32 - 0.000_000_5;
        let p = parte(vec![0.0, 0.0, 0.0, a, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, b, 1.0, 0.0], vec![0, 1, 2, 3, 4, 5]);
        let f = morbidi(&p, 10.0);
        assert!(f[0] != 0 && f[1] != 0, "{f:?}");
    }

    #[test]
    fn un_lato_con_tre_facce_non_e_morbido() {
        let p = parte(
            vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.5, 0.0, 1.0],
            vec![0, 1, 2, 1, 0, 3, 0, 1, 4],
        );
        assert_eq!(morbidi(&p, 179.0), vec![0, 0, 0]);
    }
}
