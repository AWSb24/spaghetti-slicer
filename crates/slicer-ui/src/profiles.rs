//! The profile folder: finding it, listing the printer, material and
//! process profiles in it, and importing Orca Slicer profiles.

use std::path::{Path, PathBuf};

use slicer_core::{Diagnose, Diagnostic};
use slicer_profiles::Settings;

/// The roles of profiles, in the order they are applied, with the
/// subfolder of the profile folder that holds them.
pub const ROLES: [(&str, &str); 3] = [
    ("Printer", "printers"),
    ("Material", "materials"),
    ("Process", "processes"),
];

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

/// The name a profile is shown with: its file name without `.toml`.
pub fn profile_name(path: &Path) -> String {
    path.file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
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
