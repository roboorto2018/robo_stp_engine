// robo_stp_engine: legge un file STEP o IGES con OpenCascade e scrive i triangoli in formato RSTP.
// Usato da robo importer (plugin di SketchUp); non apre finestre e non usa la rete.

use anyhow::{bail, Context, Result};
use robo_stp_engine::{ffi, maxfile, rstp, sldprt};
use std::path::PathBuf;

const AIUTO: &str = "\
robo_stp_engine - STEP/IGES con OpenCascade

  robo_stp_engine --input FILE.stp|.step|.igs|.iges|.max|.sldprt --output FILE.rstp
                  [--deflection MM] [--angle RADIANTI] [--error-file FILE]
                  [--unit mm|cm|m|in|ft] [--instances 1] [--max-triangles N] [--min-size MM] [--skip-heavy N]   (solo .max)
  robo_stp_engine --selftest      prova STEP e IGES con due cubi colorati
  robo_stp_engine --sample FILE   scrive un file di esempio (.stp o .igs) con due cubi colorati
  robo_stp_engine --version

--deflection  tolleranza della triangolazione in mm (predefinito 0.1; piu' piccola = piu' triangoli)
--angle       deviazione angolare in radianti (predefinito 0.5)
--unit        solo per i file .max: unita' del file (predefinito mm)
--instances   .max: 1 = gli oggetti ripetuti diventano istanze di una definizione
--max-triangles .max: semplifica gli oggetti piu' pesanti fino a questo totale di triangoli
--min-size    .max: ignora gli oggetti piu' piccoli di cosi' (mm, lato maggiore)
--skip-heavy  .max: ignora gli oggetti con piu' di questi triangoli
--preview N   anteprima leggera: al massimo N triangoli in tutto (per tutti i formati)
";

struct Opzioni {
    input: PathBuf,
    output: PathBuf,
    deflection: f64,
    angle: f64,
    max: maxfile::OpzioniMax,
    /// Anteprima leggera: al massimo questi triangoli in tutto, superfici grossolane.
    anteprima: Option<usize>,
}

fn leggi_argomenti(args: &[String]) -> Result<Opzioni> {
    let mut input = None;
    let mut output = None;
    let (mut deflection, mut angle) = (0.1_f64, 0.5_f64);
    let mut unit = "mm".to_string();
    let (mut istanze, mut max_triangoli, mut dim_minima, mut oggetto_max) = (false, None, None, None);
    let mut anteprima = None;
    let mut i = 0;
    while i < args.len() {
        let valore = |i: usize| args.get(i + 1).with_context(|| format!("manca il valore dopo {}", args[i]));
        match args[i].as_str() {
            "--input" => input = Some(PathBuf::from(valore(i)?)),
            "--output" => output = Some(PathBuf::from(valore(i)?)),
            "--deflection" => deflection = valore(i)?.replace(',', ".").parse().context("--deflection non e' un numero")?,
            "--angle" => angle = valore(i)?.replace(',', ".").parse().context("--angle non e' un numero")?,
            "--unit" => unit = valore(i)?.clone(),
            "--preview" => anteprima = Some(valore(i)?.parse::<usize>().context("--preview non e' un numero intero")?.max(1000)),
            "--instances" => istanze = valore(i)? == "1",
            "--max-triangles" => max_triangoli = Some(valore(i)?.parse::<usize>().context("--max-triangles non e' un numero intero")?),
            "--min-size" => dim_minima = Some(valore(i)?.replace(',', ".").parse::<f64>().context("--min-size non e' un numero")?),
            "--skip-heavy" => oggetto_max = Some(valore(i)?.parse::<usize>().context("--skip-heavy non e' un numero intero")?),
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
    let mut max = maxfile::OpzioniMax::semplici(maxfile::mm_per_unita(&unit)?);
    max.istanze = istanze;
    max.max_triangoli = max_triangoli.filter(|&n| n > 0);
    max.dimensione_minima_mm = dim_minima.filter(|&d| d > 0.0);
    max.oggetto_max_triangoli = oggetto_max.filter(|&n| n > 0);
    if let Some(n) = anteprima {
        // l'anteprima e' sempre leggera: oggetti ripetuti come copie, tetto ai triangoli, superfici grossolane
        max.istanze = false;
        max.max_triangoli = Some(n);
        max.dimensione_minima_mm = None;
        max.oggetto_max_triangoli = None;
        max.piccoli_relativi = Some(0.004);
        deflection = deflection.max(2.0);
        angle = angle.max(1.0);
    }
    Ok(Opzioni { input: input.context("manca --input")?, output: output.context("manca --output")?, deflection, angle, max, anteprima })
}

fn converti(o: &Opzioni) -> Result<()> {
    if o.input.extension().is_some_and(|e| e.eq_ignore_ascii_case("max")) {
        return converti_max(o);
    }
    // SolidWorks (.sldprt): si legge la mesh di visualizzazione salvata nel file; gli altri formati vanno a OpenCascade
    let mut parti = if o.input.extension().is_some_and(|e| e.eq_ignore_ascii_case("sldprt")) {
        sldprt::carica(&o.input)?
    } else {
        ffi::carica(&o.input, o.deflection, o.angle)?
    };
    if parti.is_empty() {
        bail!("il file non ha prodotto nessuna superficie da importare");
    }
    let origine: usize = parti.iter().map(|p| p.triangoli.len() / 3).sum();
    // anteprima o limite di triangoli scelto dall'utente (per esempio per un .sldprt): si tagliano le parti piu' pesanti
    if let Some(n) = o.anteprima.or(o.max.max_triangoli) {
        maxfile::semplifica_parti(&mut parti, n);
    }
    std::fs::write(&o.output, rstp::scrivi(&parti)).with_context(|| format!("scrittura di {}", o.output.display()))?;
    let triangoli: usize = parti.iter().map(|p| p.triangoli.len() / 3).sum();
    println!("{{\"parts\":{},\"triangles\":{},\"source_triangles\":{},\"occt\":\"{}\"}}", parti.len(), triangoli, origine, ffi::versione_occt());
    Ok(())
}

/// File .max di 3ds Max: geometria degli oggetti Editable Mesh e Editable Poly, con nomi e posizioni dei nodi.
fn converti_max(o: &Opzioni) -> Result<()> {
    let (voci, r) = maxfile::carica(&o.input, &o.max)?;
    if voci.is_empty() {
        bail!(
            "il file .max non ha prodotto nessuna geometria importabile: servono oggetti Editable Mesh o Editable Poly ({} nodi, {} oggetti di tipo non supportato, {} scartati dai filtri)",
            r.nodi,
            r.non_supportati,
            r.scartati_piccoli + r.scartati_pesanti
        );
    }
    std::fs::write(&o.output, rstp::scrivi_voci(&voci)).with_context(|| format!("scrittura di {}", o.output.display()))?;
    // triangoli che finiscono nel modello: ogni istanza ripete quelli della sua definizione
    let mut per_definizione = std::collections::HashMap::new();
    for v in &voci {
        if let rstp::Voce::Definizione(id, p) = v {
            per_definizione.insert(*id, p.triangoli.len() / 3);
        }
    }
    let (mut unici, mut totali) = (0usize, 0usize);
    for v in &voci {
        match v {
            rstp::Voce::Geometria(p) => {
                unici += p.triangoli.len() / 3;
                totali += p.triangoli.len() / 3;
            }
            rstp::Voce::Definizione(_, p) => unici += p.triangoli.len() / 3,
            rstp::Voce::Istanza { definizione, .. } => totali += per_definizione.get(definizione).copied().unwrap_or(0),
        }
    }
    println!(
        "{{\"parts\":{},\"triangles\":{},\"source_triangles\":{},\"unique_triangles\":{},\"nodes\":{},\"unsupported\":{},\"unreadable_faces\":{},\"instances\":{},\"skipped_small\":{},\"skipped_heavy\":{},\"simplified\":{}}}",
        r.con_geometria, totali, r.triangoli_origine, unici, r.nodi, r.non_supportati, r.facce_illeggibili, r.istanze, r.scartati_piccoli, r.scartati_pesanti, r.semplificati
    );
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
