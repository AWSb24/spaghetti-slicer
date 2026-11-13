//! The profiles shipped in `profiles/` must load, alone and combined.
#![allow(clippy::unwrap_used, missing_docs)]

use std::path::Path;

use slicer_profiles::Settings;

fn files(kind: &str) -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../profiles")
        .join(kind);
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(path).unwrap())
        })
        .collect();
    files.sort();
    files
}

#[test]
fn every_shipped_profile_loads_in_every_combination() {
    let (printers, materials, processes) =
        (files("printers"), files("materials"), files("processes"));
    assert!(printers.len() >= 2 && materials.len() >= 2 && processes.len() >= 3);
    for (printer_name, printer) in &printers {
        for (material_name, material) in &materials {
            for (process_name, process) in &processes {
                let mut settings = Settings::default();
                for (name, text) in [
                    (printer_name, printer),
                    (material_name, material),
                    (process_name, process),
                ] {
                    settings
                        .apply_toml(text)
                        .unwrap_or_else(|e| panic!("{name}: {e}"));
                }
            }
        }
    }

    // The roles do not overlap: no key is set by files of two kinds.
    let keys = |group: &[(String, String)]| -> Vec<String> {
        group
            .iter()
            .flat_map(|(_, text)| text.parse::<toml::Table>().unwrap().into_iter())
            .map(|(key, _)| key)
            .collect()
    };
    let (printer, material, process) = (keys(&printers), keys(&materials), keys(&processes));
    for key in &printer {
        // A printer may bring its own speeds and skirt.
        let shared = material.contains(key) || process.contains(key);
        assert!(!shared, "'{key}' is set by a printer and by another kind");
    }
    for key in &material {
        assert!(
            !process.contains(key),
            "'{key}' is set by material and process"
        );
    }

    let x1c = &printers
        .iter()
        .find(|(name, _)| name == "bambu-x1c.toml")
        .unwrap()
        .1;
    let settings = Settings::from_toml(x1c).unwrap();
    assert_eq!((settings.bed_size_x, settings.bed_size_y), (256.0, 256.0));
}
