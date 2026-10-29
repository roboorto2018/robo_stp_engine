// Compila il ponte C++ verso OpenCascade e collega le librerie statiche di OCCT.
//
// OCCT va compilato prima (vedi README). Cartella di installazione: variabile OCCT_ROOT
// (predefinita C:/occt/install), con `inc` per le intestazioni e le .lib in `win64/vc14/lib`.

use std::path::{Path, PathBuf};

/// Cerca ricorsivamente la cartella che contiene TKernel.lib.
fn trova_lib(dir: &Path, livello: u32) -> Option<PathBuf> {
    if dir.join("TKernel.lib").exists() || dir.join("libTKernel.a").exists() {
        return Some(dir.to_path_buf());
    }
    if livello > 4 {
        return None;
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        // le librerie di debug (…d) non servono
        if p.is_dir() && p.file_name().is_some_and(|n| n != "libd" && n != "bind") {
            if let Some(t) = trova_lib(&p, livello + 1) {
                return Some(t);
            }
        }
    }
    None
}

/// Il ponte verso Assimp (file FBX, glTF/GLB, OBJ, 3MF, PLY) e le sue librerie statiche: variabile ASSIMP_ROOT (predefinita C:/assimp/install), con
/// `include` per le intestazioni e in `lib` la libreria di Assimp (assimp-vc…-mt.lib) e zlibstatic.lib (vedi README).
fn assimp() {
    println!("cargo:rerun-if-changed=cpp/assimp_bridge.cpp");
    println!("cargo:rerun-if-changed=cpp/assimp_bridge.h");
    println!("cargo:rerun-if-env-changed=ASSIMP_ROOT");
    let root = PathBuf::from(std::env::var("ASSIMP_ROOT").unwrap_or_else(|_| "C:/assimp/install".into()));
    let inc = root.join("include");
    if !inc.join("assimp").join("Importer.hpp").exists() {
        panic!("intestazioni di Assimp non trovate in {}: imposta ASSIMP_ROOT", inc.display());
    }
    let lib = root.join("lib");
    let nomi: Vec<String> = std::fs::read_dir(&lib)
        .unwrap_or_else(|_| panic!("librerie di Assimp non trovate in {}: compila prima Assimp", lib.display()))
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let base = n.strip_suffix(".lib").or_else(|| n.strip_suffix(".a")).map(|s| s.to_string())?;
            // la libreria di debug (…d.lib) non serve
            ((base.starts_with("assimp") && !base.ends_with('d')) || base.starts_with("zlibstatic")).then_some(base)
        })
        .collect();
    if !nomi.iter().any(|n| n.starts_with("assimp")) {
        panic!("assimp*.lib non trovata in {}: compila prima Assimp", lib.display());
    }

    let mut b = cc::Build::new();
    b.cpp(true).file("cpp/assimp_bridge.cpp").include(&inc).include("cpp");
    if b.get_compiler().is_like_msvc() {
        b.flag("/EHsc").flag("/std:c++17").flag("/utf-8");
    } else {
        b.flag("-std=c++17");
    }
    b.warnings(false).compile("rasm_bridge");
    println!("cargo:rustc-link-search=native={}", lib.display());
    for n in &nomi {
        println!("cargo:rustc-link-lib=static={n}");
    }
}

fn main() {
    println!("cargo:rerun-if-changed=cpp/bridge.cpp");
    println!("cargo:rerun-if-changed=cpp/bridge.h");
    println!("cargo:rerun-if-env-changed=OCCT_ROOT");

    let root = PathBuf::from(std::env::var("OCCT_ROOT").unwrap_or_else(|_| "C:/occt/install".into()));
    let inc = ["inc", "include/opencascade", "include"].iter().map(|s| root.join(s)).find(|p| p.join("Standard.hxx").exists());
    let inc = inc.unwrap_or_else(|| panic!("intestazioni di OpenCascade non trovate in {}: imposta OCCT_ROOT", root.display()));
    let lib = trova_lib(&root, 0).unwrap_or_else(|| panic!("TKernel.lib non trovata in {}: compila prima OpenCascade", root.display()));

    let mut b = cc::Build::new();
    b.cpp(true).file("cpp/bridge.cpp").include(&inc).include("cpp");
    if b.get_compiler().is_like_msvc() {
        b.flag("/EHsc").flag("/std:c++17").flag("/utf-8");
    } else {
        b.flag("-std=c++17");
    }
    b.warnings(false).compile("rstp_bridge");

    println!("cargo:rustc-link-search=native={}", lib.display());
    // si collegano le librerie che servono alla lettura dei file e alla triangolazione; il resto
    // (grafica OpenGL, Draw, VTK) non serve e non si collega
    let escluse = ["TKOpenGl", "TKOpenGles", "TKD3DHost", "TKD3DHostTest", "TKIVtk", "TKIVtkDraw", "TKDraw", "TKTopTest", "TKViewerTest",
        "TKXSDRAW", "TKXSDRAWDE", "TKXSDRAWIGES", "TKXSDRAWOBJ", "TKXSDRAWPLY", "TKXSDRAWSTEP", "TKXSDRAWSTL", "TKXSDRAWVRML", "TKXSDRAWGLTF",
        "TKDCAF", "TKQADraw", "TKService_Tcl", "TKTObjDRAW", "TKXDEDRAW", "TKMeshVS"];
    let mut nomi: Vec<String> = std::fs::read_dir(&lib)
        .expect("cartella delle librerie illeggibile")
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let base = n.strip_suffix(".lib").or_else(|| n.strip_suffix(".a")).map(|s| s.trim_start_matches("lib").to_string())?;
            (base.starts_with("TK") && !escluse.contains(&base.as_str())).then_some(base)
        })
        .collect();
    nomi.sort();
    for n in &nomi {
        println!("cargo:rustc-link-lib=static={n}");
    }
    assimp();
    if cfg!(windows) {
        for s in ["user32", "gdi32", "advapi32", "ws2_32", "shell32", "ole32"] {
            println!("cargo:rustc-link-lib=dylib={s}");
        }
    }
}
