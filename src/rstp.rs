//! Formato di scambio «RSTP» tra il motore e il plugin di SketchUp (binario, little-endian).
//!
//! ```text
//! "RSTP"  u32 versione(=1)  u32 numero_parti
//! per ogni parte:
//!   u32 lunghezza_nome  nome (UTF-8)
//!   f32 r g b a          (tutti -1 se il file non dà un colore)
//!   u32 numero_vertici   u32 numero_triangoli
//!   f32 x y z  × numero_vertici        (millimetri)
//!   u32 i j k  × numero_triangoli      (indici da 0, senso antiorario)
//! ```
//!
//! Versione 2 (solo se un file 3ds Max ha oggetti ripetuti): ogni parte ha prima `u32 tipo`:
//! 0 = geometria come nella versione 1; 1 = definizione (`u32 id` poi nome, colore e geometria locale, da NON mettere nel modello ma da
//! riusare); 2 = istanza (`u32 id` della definizione, nome, `f32 × 12` = matrice 4x3 a vettori riga, traslazione in mm).
//! ```text
//! "RSTP"  u32 versione(=2)  u32 numero_voci
//! per ogni voce:  u32 tipo
//!   tipo 0: come una parte della versione 1
//!   tipo 1: u32 id   poi come una parte della versione 1 (vertici in unita' locali, in mm)
//!   tipo 2: u32 id   u32 lunghezza_nome  nome   f32 × 12
//! ```
//!
//! Versione 4 (quando il plugin chiede le linee morbide, `--soften`): come la 3 (con il blocco della texture), e dopo ogni parte
//! `u32 n` (= numero dei triangoli) e `n` byte: i lati morbidi di ogni triangolo (vedi `spigoli`).

use crate::ffi::Parte;
use anyhow::{bail, Result};

pub const MAGIC: &[u8; 4] = b"RSTP";
pub const VERSIONE: u32 = 1;

pub fn scrivi(parti: &[Parte]) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(MAGIC);
    o.extend_from_slice(&VERSIONE.to_le_bytes());
    o.extend_from_slice(&(parti.len() as u32).to_le_bytes());
    for p in parti {
        o.extend_from_slice(&(p.nome.len() as u32).to_le_bytes());
        o.extend_from_slice(p.nome.as_bytes());
        for c in p.colore.unwrap_or([-1.0; 4]) {
            o.extend_from_slice(&c.to_le_bytes());
        }
        o.extend_from_slice(&((p.vertici.len() / 3) as u32).to_le_bytes());
        o.extend_from_slice(&((p.triangoli.len() / 3) as u32).to_le_bytes());
        for v in &p.vertici {
            o.extend_from_slice(&v.to_le_bytes());
        }
        for i in &p.triangoli {
            o.extend_from_slice(&i.to_le_bytes());
        }
    }
    o
}

/// Una voce del file RSTP versione 2.
#[derive(Clone, Debug, PartialEq)]
pub enum Voce {
    Geometria(Parte),
    Definizione(u32, Parte),
    Istanza { definizione: u32, nome: String, matrice: [f32; 12] },
}

/// `v3`: dopo i triangoli c'e' il blocco della texture (versione 3 del formato).
fn scrivi_parte(o: &mut Vec<u8>, p: &Parte, v3: bool) {
    o.extend_from_slice(&(p.nome.len() as u32).to_le_bytes());
    o.extend_from_slice(p.nome.as_bytes());
    for c in p.colore.unwrap_or([-1.0; 4]) {
        o.extend_from_slice(&c.to_le_bytes());
    }
    o.extend_from_slice(&((p.vertici.len() / 3) as u32).to_le_bytes());
    o.extend_from_slice(&((p.triangoli.len() / 3) as u32).to_le_bytes());
    for v in &p.vertici {
        o.extend_from_slice(&v.to_le_bytes());
    }
    for i in &p.triangoli {
        o.extend_from_slice(&i.to_le_bytes());
    }
    if v3 {
        match &p.texture {
            None => o.extend_from_slice(&0u32.to_le_bytes()),
            Some(t) => {
                o.extend_from_slice(&1u32.to_le_bytes());
                o.extend_from_slice(&(t.file.len() as u32).to_le_bytes());
                o.extend_from_slice(t.file.as_bytes());
                for x in &t.uv {
                    o.extend_from_slice(&x.to_le_bytes());
                }
                o.extend_from_slice(&(t.bordi.len() as u32).to_le_bytes());
                for (i, uv) in &t.bordi {
                    o.extend_from_slice(&i.to_le_bytes());
                    for x in uv {
                        o.extend_from_slice(&x.to_le_bytes());
                    }
                }
            }
        }
    }
}

/// Scrive le voci: versione 1 se sono tutte geometrie semplici (compatibile con i plugin vecchi), versione 2 se ci sono
/// istanze, versione 3 se qualche parte ha una texture (come la 2, con il blocco della texture dopo ogni parte).
pub fn scrivi_voci(voci: &[Voce]) -> Vec<u8> {
    let v3 = voci.iter().any(|v| matches!(v, Voce::Geometria(p) | Voce::Definizione(_, p) if p.texture.is_some()));
    if !v3 && voci.iter().all(|v| matches!(v, Voce::Geometria(_))) {
        let parti: Vec<Parte> = voci.iter().filter_map(|v| if let Voce::Geometria(p) = v { Some(p.clone()) } else { None }).collect();
        return scrivi(&parti);
    }
    let mut o = Vec::new();
    o.extend_from_slice(MAGIC);
    o.extend_from_slice(&(if v3 { 3u32 } else { 2u32 }).to_le_bytes());
    o.extend_from_slice(&(voci.len() as u32).to_le_bytes());
    for v in voci {
        match v {
            Voce::Geometria(p) => {
                o.extend_from_slice(&0u32.to_le_bytes());
                scrivi_parte(&mut o, p, v3);
            }
            Voce::Definizione(id, p) => {
                o.extend_from_slice(&1u32.to_le_bytes());
                o.extend_from_slice(&id.to_le_bytes());
                scrivi_parte(&mut o, p, v3);
            }
            Voce::Istanza { definizione, nome, matrice } => {
                o.extend_from_slice(&2u32.to_le_bytes());
                o.extend_from_slice(&definizione.to_le_bytes());
                o.extend_from_slice(&(nome.len() as u32).to_le_bytes());
                o.extend_from_slice(nome.as_bytes());
                for m in matrice {
                    o.extend_from_slice(&m.to_le_bytes());
                }
            }
        }
    }
    o
}

/// Scrive le voci nella versione 4: con i lati morbidi di ogni parte, calcolati per `gradi` (in parallelo, una parte per volta).
pub fn scrivi_con_morbidi(voci: &[Voce], gradi: f64) -> Vec<u8> {
    let parti: Vec<&Parte> = voci
        .iter()
        .filter_map(|v| match v {
            Voce::Geometria(p) | Voce::Definizione(_, p) => Some(p),
            Voce::Istanza { .. } => None,
        })
        .collect();
    let mut morbidi = crate::maxfile::in_parallelo(&parti, |p| crate::spigoli::morbidi(p, gradi)).into_iter();
    let mut o = Vec::new();
    o.extend_from_slice(MAGIC);
    o.extend_from_slice(&4u32.to_le_bytes());
    o.extend_from_slice(&(voci.len() as u32).to_le_bytes());
    for v in voci {
        let (tipo, id, parte) = match v {
            Voce::Geometria(p) => (0u32, None, Some(p)),
            Voce::Definizione(id, p) => (1, Some(*id), Some(p)),
            Voce::Istanza { definizione, .. } => (2, Some(*definizione), None),
        };
        o.extend_from_slice(&tipo.to_le_bytes());
        if let Some(id) = id {
            o.extend_from_slice(&id.to_le_bytes());
        }
        match (v, parte) {
            (Voce::Istanza { nome, matrice, .. }, _) => {
                o.extend_from_slice(&(nome.len() as u32).to_le_bytes());
                o.extend_from_slice(nome.as_bytes());
                for m in matrice {
                    o.extend_from_slice(&m.to_le_bytes());
                }
            }
            (_, Some(p)) => {
                scrivi_parte(&mut o, p, true);
                let f = morbidi.next().unwrap_or_default();
                o.extend_from_slice(&(f.len() as u32).to_le_bytes());
                o.extend_from_slice(&f);
            }
            _ => {}
        }
    }
    o
}

struct Lettore<'a>(&'a [u8]);

impl<'a> Lettore<'a> {
    fn prendi(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            bail!("file RSTP troncato");
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.prendi(4)?.try_into()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.prendi(4)?.try_into()?))
    }
}

/// Rilegge un file RSTP (serve ai test e a chi vuole controllare il risultato).
pub fn leggi(dati: &[u8]) -> Result<Vec<Parte>> {
    let mut l = Lettore(dati);
    if l.prendi(4)? != MAGIC {
        bail!("non e' un file RSTP");
    }
    if l.u32()? != VERSIONE {
        bail!("versione RSTP non supportata");
    }
    let n = l.u32()?;
    let mut parti = Vec::new();
    for _ in 0..n {
        let len = l.u32()? as usize;
        let nome = String::from_utf8(l.prendi(len)?.to_vec())?;
        let c = [l.f32()?, l.f32()?, l.f32()?, l.f32()?];
        let nv = l.u32()? as usize;
        let nt = l.u32()? as usize;
        let mut vertici = Vec::with_capacity(nv * 3);
        for _ in 0..nv * 3 {
            vertici.push(l.f32()?);
        }
        let mut triangoli = Vec::with_capacity(nt * 3);
        for _ in 0..nt * 3 {
            triangoli.push(l.u32()?);
        }
        parti.push(Parte { nome, colore: if c[0] < 0.0 { None } else { Some(c) }, vertici, triangoli, texture: None });
    }
    Ok(parti)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn scrittura_e_lettura_coincidono() {
        let p = vec![
            Parte { nome: "Cubo/Parte àè".into(), colore: Some([1.0, 0.0, 0.5, 1.0]), vertici: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0], triangoli: vec![0, 1, 2], texture: None },
            Parte { nome: "Senza colore".into(), colore: None, vertici: vec![], triangoli: vec![], texture: None },
        ];
        assert_eq!(leggi(&scrivi(&p)).unwrap(), p);
    }

    #[test]
    fn versione_2_con_istanze() {
        let def = Parte { nome: "Sedia".into(), colore: None, vertici: vec![0.0; 9], triangoli: vec![0, 1, 2], texture: None };
        let voci = vec![
            Voce::Definizione(7, def.clone()),
            Voce::Istanza { definizione: 7, nome: "G/Sedia 1".into(), matrice: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 5.0, 6.0, 7.0] },
        ];
        let d = scrivi_voci(&voci);
        assert_eq!(&d[4..8], &2u32.to_le_bytes());
        // solo geometrie semplici: resta la versione 1
        let v1 = scrivi_voci(&[Voce::Geometria(def)]);
        assert_eq!(&v1[4..8], &1u32.to_le_bytes());
    }

    #[test]
    fn versione_3_con_texture() {
        let t = crate::ffi::Texture { file: "a.jpg".into(), uv: vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0], bordi: vec![(0, [0.0, 0.0, 1.0, 0.0, 0.0, 1.0])] };
        let p = Parte { nome: "P".into(), colore: None, vertici: vec![0.0; 9], triangoli: vec![0, 1, 2], texture: Some(t) };
        let d = scrivi_voci(&[Voce::Geometria(p)]);
        assert_eq!(&d[4..8], &3u32.to_le_bytes());
        // 12 byte di intestazione, u32 tipo, parte: 4 + 1 nome, 16 colore, 8 conteggi, 36 vertici, 12 triangoli, poi la texture
        let inizio = 12 + 4 + 4 + 1 + 16 + 8 + 36 + 12;
        assert_eq!(&d[inizio..inizio + 4], &1u32.to_le_bytes());
        assert_eq!(&d[inizio + 8..inizio + 13], b"a.jpg");
    }

    #[test]
    fn versione_4_con_i_lati_morbidi() {
        // un quadrato in due triangoli: la diagonale e' morbida
        let q = Parte { nome: "Q".into(), colore: None, vertici: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0], triangoli: vec![0, 1, 2, 0, 2, 3], texture: None };
        let voci = vec![
            Voce::Definizione(3, q.clone()),
            Voce::Istanza { definizione: 3, nome: "I".into(), matrice: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0] },
            Voce::Geometria(q),
        ];
        let d = scrivi_con_morbidi(&voci, 30.0);
        assert_eq!(&d[4..8], &4u32.to_le_bytes());
        assert_eq!(&d[8..12], &3u32.to_le_bytes());
        // la definizione: tipo, id, nome, colore, conteggi, 4 vertici, 2 triangoli, texture assente, poi i 2 byte dei lati
        let fine_def = 12 + 4 + 4 + 4 + 1 + 16 + 8 + 48 + 24 + 4;
        assert_eq!(&d[fine_def..fine_def + 4], &2u32.to_le_bytes());
        assert_eq!(&d[fine_def + 4..fine_def + 6], &[0b100, 0b001]);
        // l'istanza dopo, poi la geometria con i suoi lati alla fine del file
        assert_eq!(&d[fine_def + 6..fine_def + 10], &2u32.to_le_bytes());
        assert_eq!(&d[d.len() - 2..], &[0b100, 0b001]);
    }

    #[test]
    fn file_rovinato_da_errore() {
        assert!(leggi(b"XXXX").is_err());
        let mut d = scrivi(&[Parte { nome: "a".into(), colore: None, vertici: vec![0.0; 3], triangoli: vec![], texture: None }]);
        d.truncate(d.len() - 2);
        assert!(leggi(&d).is_err());
    }
}
