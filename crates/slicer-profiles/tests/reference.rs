//! Keeps `docs/settings.md` in step with the code. The descriptions come
//! from the doc comments of `Settings`, defaults and limits from the values.
//! After changing a setting, rewrite the file with
//! `UPDATE_GOLDEN=1 cargo test -p slicer-profiles --test reference`.
#![allow(clippy::unwrap_used, missing_docs)]

use std::path::Path;

use slicer_profiles::Settings;

/// Name and doc comment of every field of `Settings`, in source order.
fn described_fields() -> Vec<(String, String)> {
    let source = include_str!("../src/lib.rs").replace("\r\n", "\n");
    let start = source.find("pub struct Settings {").unwrap();
    let end = start + source[start..].find("\n}\n").unwrap();
    let mut fields = Vec::new();
    let mut comment = String::new();
    // The first line is the struct itself.
    for line in source[start..end].lines().skip(1).map(str::trim) {
        if let Some(text) = line.strip_prefix("///") {
            comment.push_str(text.trim());
            comment.push(' ');
        } else if let Some(rest) = line.strip_prefix("pub ") {
            let name = rest.split(':').next().unwrap().to_string();
            fields.push((name, std::mem::take(&mut comment).trim().to_string()));
        } else {
            comment.clear();
        }
    }
    fields
}

#[test]
fn settings_reference_is_current() {
    let defaults = Settings::default();
    let values = defaults.placeholder_values();
    let limits = defaults.limits();
    let fields = described_fields();
    for key in values.keys() {
        assert!(
            fields.iter().any(|(name, _)| name == key),
            "'{key}' was not found in the source"
        );
    }

    let mut text = String::from(
        "# Settings\n\nGenerated from the code by `cargo test -p slicer-profiles --test reference`; do not edit by hand.\n\nLengths are in mm, speeds in mm/s, temperatures in °C. A settings file is TOML and only needs the keys it changes. Limits marked with * depend on other settings; the values shown hold for the defaults.\n\n| Key | Default | Allowed | Meaning |\n|---|---|---|---|\n",
    );
    for (name, description) in &fields {
        assert!(!description.is_empty(), "'{name}' has no description");
        let default = values
            .get(name)
            .map_or("see `default-config`", String::as_str);
        let allowed = match limits.iter().find(|(key, ..)| key == name) {
            Some(&(_, _, min, max)) => {
                // A limit that moves when the nozzle or the layers change.
                let other = Settings {
                    nozzle_diameter: 0.8,
                    layer_height: 0.3,
                    first_layer_height: 0.3,
                    ..Settings::default()
                };
                let moved = other
                    .limits()
                    .iter()
                    .any(|&(key, _, a, b)| key == name && (a, b) != (min, max));
                format!("{min} to {max}{}", if moved { " *" } else { "" })
            }
            None => match name.as_str() {
                "flavor" => "`marlin`, `klipper`".to_string(),
                "infill_pattern" => "`rectilinear`, `grid`".to_string(),
                "wall_order" => "`inner_outer`, `outer_inner`".to_string(),
                "seam_position" => "`rear`, `nearest`".to_string(),
                "extrusion_mode" => "`relative`, `absolute`".to_string(),
                "fan_off_layers" => "0 or more".to_string(),
                "start_gcode" | "end_gcode" => "text with `{key}` placeholders".to_string(),
                other => panic!("'{other}' has neither limits nor a description of its values"),
            },
        };
        text.push_str(&format!(
            "| `{name}` | {default} | {allowed} | {description} |\n"
        ));
    }

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/settings.md");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    let current = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert_eq!(
        text, current,
        "docs/settings.md is out of date; see the comment in this file"
    );
}
