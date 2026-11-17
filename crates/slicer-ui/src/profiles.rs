//! The profile folder: finding it, listing the printer, material and
//! process profiles in it, and importing Orca Slicer profiles. Besides the
//! files, the profiles built into the program (`slicer_profiles::catalog`)
//! stand in the lists under paths starting with [`BUILTIN`]; the setup
//! assistant chooses which of them are shown.

use std::path::{Path, PathBuf};

use slicer_core::{Diagnose, Diagnostic};
use slicer_profiles::Settings;
use slicer_profiles::catalog;

/// The roles of profiles, in the order they are applied, with the
/// subfolder of the profile folder that holds them.
pub const ROLES: [(&str, &str); 3] = [
    ("Printer", "printers"),
    ("Material", "materials"),
    ("Process", "processes"),
];

/// Start of the paths that stand for profiles built into the program.
pub const BUILTIN: &str = "builtin:";

/// The path that stands for the built-in profile `id` of a role, given by
/// its subfolder in [`ROLES`].
pub fn builtin_path(sub: &str, id: &str) -> PathBuf {
    PathBuf::from(format!("{BUILTIN}{sub}/{id}"))
}

/// Role subfolder and identifier of a built-in profile's path.
fn builtin_parts(path: &Path) -> Option<(&str, &str)> {
    path.to_str()?.strip_prefix(BUILTIN)?.split_once('/')
}

/// The settings file of a built-in profile, or `None` if `path` is a file.
pub fn builtin_text(path: &Path) -> Option<Result<String, String>> {
    let (sub, id) = builtin_parts(path)?;
    let text = match sub {
        "printers" => catalog::printer_by_id(id).map(|(printer, nozzle)| printer.toml(nozzle)),
        "materials" => catalog::material_by_id(id).map(catalog::Material::toml),
        "processes" => catalog::process_by_id(id).map(|(process, nozzle)| process.toml(nozzle)),
        _ => None,
    };
    Some(text.ok_or_else(|| format!("{}: no such built-in profile", path.display())))
}

/// What the setup assistant chose: the built-in printers (one entry per
/// nozzle, see `catalog::Printer::id`) and materials to list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Setup {
    /// Whether the assistant has run, so it does not open by itself again.
    pub done: bool,
    /// Identifiers of the chosen printer profiles.
    pub printers: Vec<String>,
    /// Identifiers of the chosen materials.
    pub materials: Vec<String>,
}

impl Setup {
    /// One line per entry, to be stored between runs.
    pub fn to_text(&self) -> String {
        let mut text = format!("done={}\n", self.done);
        for id in &self.printers {
            text.push_str(&format!("printer={id}\n"));
        }
        for id in &self.materials {
            text.push_str(&format!("material={id}\n"));
        }
        text
    }

    /// Reads what [`Setup::to_text`] wrote; unknown lines and profiles
    /// that no longer exist are left out.
    pub fn from_text(text: &str) -> Self {
        let mut setup = Setup::default();
        for line in text.lines() {
            match line.split_once('=') {
                Some(("done", value)) => setup.done = value == "true",
                Some(("printer", id)) if catalog::printer_by_id(id).is_some() => {
                    setup.printers.push(id.to_owned());
                }
                Some(("material", id)) if catalog::material_by_id(id).is_some() => {
                    setup.materials.push(id.to_owned());
                }
                _ => {}
            }
        }
        setup
    }

    /// The built-in profiles to list for each role: the chosen printers
    /// and materials, and the processes for a `nozzle` this wide.
    pub fn builtin(&self, nozzle: f64) -> [Vec<PathBuf>; 3] {
        let [printers, materials, processes] = ROLES.map(|(_, sub)| sub);
        [
            self.printers
                .iter()
                .map(|id| builtin_path(printers, id))
                .collect(),
            self.materials
                .iter()
                .map(|id| builtin_path(materials, id))
                .collect(),
            catalog::processes(nozzle)
                .iter()
                .map(|process| builtin_path(processes, &process.id()))
                .collect(),
        ]
    }
}

/// The path of the standard process for a nozzle, the one chosen with a
/// new printer.
pub fn standard_process(nozzle: f64) -> PathBuf {
    let id = catalog::processes(nozzle)
        .iter()
        .find(|process| process.label == "Standard")
        .map(catalog::Process::id)
        .unwrap_or_default();
    builtin_path(ROLES[2].1, &id)
}

/// `paths` with every built-in process made for another nozzle than the
/// built-in printer among them swapped for the process of the same step
/// (for example Standard) for that nozzle, so that changing the printer
/// keeps the process valid.
pub fn fit_processes(paths: &[PathBuf]) -> Vec<PathBuf> {
    let nozzle = paths.iter().find_map(|path| match builtin_parts(path) {
        Some(("printers", id)) => catalog::printer_by_id(id).map(|(_, nozzle)| nozzle),
        _ => None,
    });
    let Some(nozzle) = nozzle else {
        return paths.to_vec();
    };
    let fitting = catalog::processes(nozzle);
    paths
        .iter()
        .map(|path| match builtin_parts(path) {
            Some(("processes", id)) => catalog::process_by_id(id)
                .filter(|(process, _)| !fitting.contains(process))
                .and_then(|(process, _)| fitting.iter().find(|p| p.label == process.label))
                .map_or_else(|| path.clone(), |p| builtin_path(ROLES[2].1, &p.id())),
            _ => path.clone(),
        })
        .collect()
}

/// Whether `dir` looks like a profile folder.
fn is_profile_dir(dir: &Path) -> bool {
    ROLES.iter().any(|(_, sub)| dir.join(sub).is_dir())
}

/// The profile folder shipped with the program: `profiles` next to the
/// executable or in one of its parent folders, else in the working folder
/// or one of its parents, else in the source tree this program was built
/// from.
pub fn find_profiles_dir(exe: Option<&Path>, cwd: Option<&Path>) -> Option<PathBuf> {
    let built_from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let starts = [exe.and_then(Path::parent), cwd, Some(built_from.as_path())];
    starts
        .into_iter()
        .flatten()
        .flat_map(Path::ancestors)
        .map(|dir| dir.join("profiles"))
        .find(|dir| is_profile_dir(dir))
}

/// The profiles of one folder, by role.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileSet {
    /// The folder.
    pub dir: PathBuf,
    /// Settings files of each role in the order of [`ROLES`], sorted by name.
    pub files: [Vec<PathBuf>; 3],
}

impl ProfileSet {
    /// Lists the `.toml` files in the role subfolders of `dir`. Missing
    /// subfolders are empty.
    pub fn scan(dir: &Path) -> Self {
        let files = ROLES.map(|(_, sub)| {
            let mut found: Vec<PathBuf> = std::fs::read_dir(dir.join(sub))
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
                })
                .collect();
            found.sort();
            found
        });
        Self {
            dir: dir.to_path_buf(),
            files,
        }
    }

    /// The built-in profiles `builtin` (by role) followed by the files of
    /// this folder.
    pub fn with_builtin(&self, builtin: &[Vec<PathBuf>; 3]) -> Self {
        let mut files = builtin.clone();
        for (all, own) in files.iter_mut().zip(&self.files) {
            all.extend(own.iter().cloned());
        }
        Self {
            dir: self.dir.clone(),
            files,
        }
    }

    /// The chosen file of every role in order; `None` means no file of
    /// that role.
    pub fn chosen(&self, choice: &[Option<usize>; 3]) -> Vec<PathBuf> {
        (0..ROLES.len())
            .filter_map(|role| choice[role].and_then(|index| self.files[role].get(index)))
            .cloned()
            .collect()
    }

    /// Which file of every role is among `paths`, so a selection survives
    /// a new scan.
    pub fn choice_of(&self, paths: &[PathBuf]) -> [Option<usize>; 3] {
        [0, 1, 2].map(|role| {
            self.files[role]
                .iter()
                .position(|file| paths.contains(file))
        })
    }
}

/// The name a profile is shown with: its file name without `.toml`, or
/// the name of a built-in profile.
pub fn profile_name(path: &Path) -> String {
    let builtin = builtin_parts(path).and_then(|(sub, id)| match sub {
        "printers" => catalog::printer_by_id(id).map(|(printer, nozzle)| printer.name(nozzle)),
        "materials" => catalog::material_by_id(id).map(catalog::Material::display_name),
        "processes" => catalog::process_by_id(id).map(|(process, _)| process.name()),
        _ => None,
    });
    builtin.unwrap_or_else(|| {
        path.file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
    })
}

/// The settings files after the profile choice changed from `old` to
/// `new`: the new profiles first, then the files loaded by hand.
pub fn replace_profiles(current: &[PathBuf], old: &[PathBuf], new: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = new.to_vec();
    files.extend(
        current
            .iter()
            .filter(|path| !old.contains(path) && !new.contains(path))
            .cloned(),
    );
    files
}

/// Orca Slicer profiles turned into one of our settings files.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// The settings file (TOML) with the taken values only.
    pub toml: String,
    /// Suggested file name, from the name of the last profile.
    pub name: String,
    /// What was left out, one line each.
    pub notes: Vec<String>,
}

/// Converts Orca Slicer profiles (JSON, as `(file name, text)`) into one
/// settings file. Later profiles override earlier ones. Like the
/// `import-orca` command of the command line tool.
pub fn import_orca_files(files: &[(String, String)]) -> Result<Imported, Diagnostic> {
    let mut settings = Settings::default();
    let mut keys: Vec<String> = Vec::new();
    let mut header = String::from("# Imported from Orca Slicer profiles by spaghetti-slicer.\n");
    let mut notes = Vec::new();
    let mut last_name = String::from("imported");
    for (file, text) in files {
        let import = slicer_profiles::import_orca(text, &mut settings).map_err(|e| {
            let mut diagnostic = e.diagnostic().with("file", file);
            diagnostic.message = format!("{file}: {}", diagnostic.message);
            diagnostic
        })?;
        let name = import.name.unwrap_or_else(|| file.clone());
        header.push_str(&format!(
            "# {name}: {} setting(s) taken",
            import.taken.len()
        ));
        if let Some(parent) = &import.inherits {
            header.push_str(&format!(
                "; values inherited from '{parent}' are not included"
            ));
            notes.push(format!(
                "{name}: values inherited from '{parent}' are not included"
            ));
        }
        header.push('\n');
        notes.extend(
            import
                .skipped
                .iter()
                .map(|(key, reason)| format!("{name}: skipped {key}: {reason}")),
        );
        for (ours, _, _) in import.taken {
            if !keys.contains(&ours) {
                keys.push(ours);
            }
        }
        last_name = name;
    }
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let stem: String = last_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(Imported {
        toml: format!("{header}\n{}", settings.to_toml_keys(&keys)),
        name: format!("{stem}.toml"),
        notes,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn shipped() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles")
    }

    #[test]
    fn the_shipped_profiles_are_found_and_listed() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"));
        // From a working folder inside the source tree, and from an
        // executable in a folder next to it.
        let found = find_profiles_dir(None, Some(&source.join("src"))).unwrap();
        assert!(found.ends_with("profiles"));
        let exe = source.join("../../target/release/app.exe");
        assert_eq!(
            find_profiles_dir(Some(&exe), None)
                .unwrap()
                .canonicalize()
                .unwrap(),
            shipped().canonicalize().unwrap()
        );

        let set = ProfileSet::scan(&shipped());
        let names: Vec<Vec<String>> = set
            .files
            .iter()
            .map(|files| files.iter().map(|f| profile_name(f)).collect())
            .collect();
        assert!(names[0].contains(&"generic-220".to_owned()), "{names:?}");
        assert!(names[1].contains(&"pla".to_owned()));
        assert!(names[2].contains(&"standard-0.20".to_owned()));
        // Every combination loads.
        let choice = [Some(0), Some(1), Some(2)];
        let chosen = set.chosen(&choice);
        assert_eq!(chosen.len(), 3);
        assert_eq!(set.choice_of(&chosen), choice);
        let mut settings = Settings::default();
        for path in &chosen {
            settings
                .apply_toml(&std::fs::read_to_string(path).unwrap())
                .unwrap();
        }
        assert_eq!(set.chosen(&[None, Some(0), None]).len(), 1);
        assert_eq!(ProfileSet::scan(Path::new("no/such/dir")).files[0].len(), 0);
    }

    #[test]
    fn built_in_profiles_are_listed_named_and_read() {
        let setup = Setup {
            done: true,
            printers: vec!["bambu-lab-x1-carbon-0.4".to_owned()],
            materials: vec!["generic-petg".to_owned(), "generic-pla".to_owned()],
        };
        let text = setup.to_text();
        assert_eq!(Setup::from_text(&text), setup);
        // Profiles that are gone are dropped.
        let old = Setup::from_text("done=true\nprinter=no-such-printer\nmaterial=generic-pla\n");
        assert!(old.done && old.printers.is_empty() && old.materials.len() == 1);

        let builtin = setup.builtin(0.4);
        assert_eq!(
            builtin[0],
            [PathBuf::from("builtin:printers/bambu-lab-x1-carbon-0.4")]
        );
        assert_eq!(builtin[2].len(), 6);
        assert!(builtin[2].contains(&standard_process(0.4)));
        assert_eq!(
            profile_name(&builtin[0][0]),
            "Bambu Lab X1 Carbon 0.4 nozzle"
        );
        assert_eq!(profile_name(&builtin[1][0]), "Generic PETG");
        assert_eq!(profile_name(&standard_process(0.4)), "0.20mm Standard");
        assert_eq!(profile_name(Path::new("a/pla.toml")), "pla");

        // Built-in profiles stack like files.
        let mut settings = Settings::default();
        for path in [&builtin[0][0], &builtin[1][0], &standard_process(0.4)] {
            settings
                .apply_toml(&builtin_text(path).unwrap().unwrap())
                .unwrap();
        }
        assert_eq!(settings.bed_size_x, 256.0);
        assert_eq!(settings.nozzle_temp, 240.0);
        assert_eq!(settings.layer_height, 0.2);
        assert!(builtin_text(Path::new("a/pla.toml")).is_none());
        assert!(
            builtin_text(Path::new("builtin:printers/nothing"))
                .unwrap()
                .is_err()
        );

        // A printer with another nozzle takes the same step of process.
        let fine = builtin_path("printers", "bambu-lab-x1-carbon-0.6");
        let fitted = fit_processes(&[fine.clone(), standard_process(0.4), "own.toml".into()]);
        assert_eq!(fitted, [fine, standard_process(0.6), "own.toml".into()]);
        assert_eq!(profile_name(&fitted[1]), "0.30mm Standard");
        assert_eq!(
            fit_processes(&[standard_process(0.4)]),
            [standard_process(0.4)]
        );

        let set = ProfileSet::scan(&shipped()).with_builtin(&builtin);
        assert_eq!(set.files[0][0], builtin[0][0]);
        assert!(set.files[0].len() > 1);
    }

    #[test]
    fn profiles_replace_profiles_and_keep_other_files() {
        let p = |name: &str| PathBuf::from(name);
        let current = [p("a.toml"), p("own.toml"), p("b.toml")];
        assert_eq!(
            replace_profiles(&current, &[p("a.toml"), p("b.toml")], &[p("c.toml")]),
            [p("c.toml"), p("own.toml")]
        );
        assert_eq!(replace_profiles(&[], &[], &[p("c.toml")]), [p("c.toml")]);
    }

    #[test]
    fn orca_profiles_become_a_settings_file() {
        let process = r#"{"name": "My 0.16mm", "inherits": "0.16mm Standard", "layer_height": "0.16", "wall_loops": "3", "ironing_type": "top"}"#;
        let material = r#"{"name": "My PLA", "nozzle_temperature": ["205"]}"#;
        let imported = import_orca_files(&[
            ("process.json".to_owned(), process.to_owned()),
            ("pla.json".to_owned(), material.to_owned()),
        ])
        .unwrap();
        assert_eq!(imported.name, "My_PLA.toml");
        let settings = Settings::from_toml(&imported.toml).unwrap();
        assert_eq!(settings.layer_height, 0.16);
        assert_eq!(settings.wall_loops, 3);
        assert_eq!(settings.nozzle_temp, 205.0);
        assert!(imported.notes.iter().any(|n| n.contains("0.16mm Standard")));
        assert!(imported.notes.iter().any(|n| n.contains("ironing_type")));
        let error = import_orca_files(&[("bad.json".to_owned(), "{".to_owned())]).unwrap_err();
        assert!(error.message.starts_with("bad.json: "), "{}", error.message);
    }
}
