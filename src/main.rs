// robo_stp_engine: legge un file STEP o IGES con OpenCascade e scrive i triangoli in formato RSTP.
// Usato da robo STP/IGES importer (plugin di SketchUp); non apre finestre e non usa la rete.

use anyhow::{bail, Context, Result};
use robo_stp_engine::{ffi, rstp};
use std::path::PathBuf;

const AIUTO: &str = "\
robo_stp_engine - STEP/IGES con OpenCascade

  robo_stp_engine --input FILE.stp|.step|.igs|.iges --output FILE.rstp
                  [--deflection MM] [--angle RADIANTI] [--error-file FILE]
  robo_stp_engine --selftest      prova STEP e IGES con due cubi colorati
  robo_stp_engine --sample FILE   scrive un file di esempio (.stp o .igs) con due cubi colorati
  robo_stp_engine --version

--deflection  tolleranza della triangolazione in mm (predefinito 0.1; piu' piccola = piu' triangoli)
--angle       deviazione angolare in radianti (predefinito 0.5)
";

struct Opzioni {
    input: PathBuf,
    output: PathBuf,
    deflection: f64,
    angle: f64,
}

fn leggi_argomenti(args: &[String]) -> Result<Opzioni> {
    let mut input = None;
    let mut output = None;
    let (mut deflection, mut angle) = (0.1, 0.5);
    let mut i = 0;
    while i < args.len() {
        let valore = |i: usize| args.get(i + 1).with_context(|| format!("manca il valore dopo {}", args[i]));
        match args[i].as_str() {
            "--input" => input = Some(PathBuf::from(valore(i)?)),
            "--output" => output = Some(PathBuf::from(valore(i)?)),
            "--deflection" => deflection = valore(i)?.replace(',', ".").parse().context("--deflection non e' un numero")?,
            "--angle" => angle = valore(i)?.replace(',', ".").parse().context("--angle non e' un numero")?,
            "--error-file" => {}
            altro => bail!("opzione sconosciuta: {altro}"),
        }
        i += 2;
    }
    if !(deflection > 0.0 && deflection <= 1000.0) {
        bail!("--deflection deve essere tra 0 e 1000 mm");
    }
    if !(angle > 0.0 && angle <= 3.2) {
        bail!("--angle deve essere tra 0 e 3.2 radianti");
    }
    Ok(Opzioni { input: input.context("manca --input")?, output: output.context("manca --output")?, deflection, angle })
}

fn converti(o: &Opzioni) -> Result<()> {
    let parti = ffi::carica(&o.input, o.deflection, o.angle)?;
    if parti.is_empty() {
        bail!("il file non ha prodotto nessuna superficie da importare");
    }
    std::fs::write(&o.output, rstp::scrivi(&parti)).with_context(|| format!("scrittura di {}", o.output.display()))?;
    let triangoli: usize = parti.iter().map(|p| p.triangoli.len() / 3).sum();
    println!("{{\"parts\":{},\"triangles\":{},\"occt\":\"{}\"}}", parti.len(), triangoli, ffi::versione_occt());
    Ok(())
}

/// Scrive due cubi colorati in STEP e in IGES, li rilegge e controlla il risultato.
fn autoprova() -> Result<()> {
    let dir = std::env::temp_dir().join("robo_stp_selftest");
    std::fs::create_dir_all(&dir)?;
    for (nome, formato) in [("prova.stp", ffi::Formato::Step), ("prova.igs", ffi::Formato::Iges)] {
        let file = dir.join(nome);
        ffi::scrivi_cubi_di_prova(&file, formato)?;
        let parti = ffi::carica(&file, 0.1, 0.5)?;
        let triangoli: usize = parti.iter().map(|p| p.triangoli.len() / 3).sum();
        println!("{nome}: {} parti, {triangoli} triangoli", parti.len());
        for p in &parti {
            println!("  {} colore {:?} ({} vertici)", p.nome, p.colore, p.vertici.len() / 3);
        }
        if parti.len() < 2 || triangoli < 24 {
            bail!("{nome}: risultato inatteso");
        }
    }
    std::fs::remove_dir_all(&dir).ok();
    println!("Autoprova riuscita (OpenCascade {}).", ffi::versione_occt());
    Ok(())
}

/// Scrive un file di esempio da provare con il plugin.
fn campione(file: Option<&String>) -> Result<()> {
    let file = PathBuf::from(file.context("manca il nome del file")?);
    ffi::scrivi_cubi_di_prova(&file, ffi::Formato::da_percorso(&file)?)?;
    println!("Creato {}", file.display());
    Ok(())
}

fn scrivi_errore(args: &[String], messaggio: &str) {
    if let Some(i) = args.iter().position(|a| a == "--error-file") {
        if let Some(f) = args.get(i + 1) {
            let _ = std::fs::write(f, messaggio);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        None | Some("--help") | Some("-h") => {
            print!("{AIUTO}");
            Ok(())
        }
        Some("--version") => {
            println!("robo_stp_engine {} (OpenCascade {})", env!("CARGO_PKG_VERSION"), ffi::versione_occt());
            Ok(())
        }
        Some("--selftest") => autoprova(),
        Some("--sample") => campione(args.get(1)),
        Some(_) => leggi_argomenti(&args).and_then(|o| converti(&o)),
    };
    if let Err(e) = r {
        let m = format!("{e:#}");
        eprintln!("Errore: {m}");
        scrivi_errore(&args, &m);
        std::process::exit(1);
    }
}
