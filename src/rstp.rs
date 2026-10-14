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
        parti.push(Parte { nome, colore: if c[0] < 0.0 { None } else { Some(c) }, vertici, triangoli });
    }
    Ok(parti)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn scrittura_e_lettura_coincidono() {
        let p = vec![
            Parte { nome: "Cubo/Parte àè".into(), colore: Some([1.0, 0.0, 0.5, 1.0]), vertici: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0], triangoli: vec![0, 1, 2] },
            Parte { nome: "Senza colore".into(), colore: None, vertici: vec![], triangoli: vec![] },
        ];
        assert_eq!(leggi(&scrivi(&p)).unwrap(), p);
    }

    #[test]
    fn file_rovinato_da_errore() {
        assert!(leggi(b"XXXX").is_err());
        let mut d = scrivi(&[Parte { nome: "a".into(), colore: None, vertici: vec![0.0; 3], triangoli: vec![] }]);
        d.truncate(d.len() - 2);
        assert!(leggi(&d).is_err());
    }
}
