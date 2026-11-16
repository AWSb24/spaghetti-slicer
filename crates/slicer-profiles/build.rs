//! Generates the table of all settings (`SETTINGS_INFO`) from the fields of
//! `Settings` in `src/lib.rs`: name, group, kind of value, the values of
//! choices and the doc comment. The doc comments stay the only place where
//! a setting is described, and programs get the descriptions without
//! reading source files.

use std::fmt::Write;

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    let source = std::fs::read_to_string("src/lib.rs")
        .expect("src/lib.rs is readable")
        .replace("\r\n", "\n");
    let mut table = String::from("&[\n");
    for field in fields(&source) {
        let choices = choices(&source, &field.ty);
        let kind = match field.ty.as_str() {
            "f64" | "f32" => "Number",
            "u8" | "u16" | "u32" | "u64" | "usize" => "Count",
            "bool" => "Flag",
            _ if !choices.is_empty() => "Choice",
            _ => "Text",
        };
        let _ = writeln!(
            table,
            "    SettingInfo {{ key: {:?}, group: {:?}, kind: ValueKind::{kind}, choices: &{choices:?}, description: {:?} }},",
            field.name, field.group, field.description
        );
    }
    table.push(']');
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    std::fs::write(std::path::Path::new(&out).join("settings_info.rs"), table)
        .expect("OUT_DIR is writable");
}

struct Field {
    name: String,
    ty: String,
    group: String,
    description: String,
}

/// The fields of `Settings` in source order. A `// Group` comment starts a
/// group; `///` lines are the description of the next field.
fn fields(source: &str) -> Vec<Field> {
    let body = block(source, "pub struct Settings {").expect("src/lib.rs defines Settings");
    let mut fields = Vec::new();
    let mut group = String::new();
    let mut comment = String::new();
    for line in body.lines().map(str::trim) {
        if let Some(text) = line.strip_prefix("///") {
            comment.push_str(text.trim());
            comment.push(' ');
        } else if let Some(text) = line.strip_prefix("//") {
            group = text.trim().to_owned();
            comment.clear();
        } else if let Some((name, ty)) = line.strip_prefix("pub ").and_then(|l| l.split_once(':')) {
            fields.push(Field {
                name: name.trim().to_owned(),
                ty: ty.trim().trim_end_matches(',').trim().to_owned(),
                group: group.clone(),
                description: std::mem::take(&mut comment).trim().to_owned(),
            });
        } else {
            comment.clear();
        }
    }
    fields
}

/// The lines between `head` and the closing brace at the start of a line.
fn block<'a>(source: &'a str, head: &str) -> Option<&'a str> {
    let start = source.find(head)? + head.len();
    let end = start + source[start..].find("\n}")?;
    Some(&source[start..end])
}

/// The names of the variants of the enum `ty` as they are written in
/// settings files, following its `rename_all`; empty if `ty` is no enum
/// with unit variants in `src/lib.rs`.
fn choices(source: &str, ty: &str) -> Vec<String> {
    let head = format!("pub enum {ty} {{");
    let (Some(at), Some(body)) = (source.find(&head), block(source, &head)) else {
        return Vec::new();
    };
    // The attributes right above the enum.
    let attributes: Vec<&str> = source[..at]
        .lines()
        .rev()
        .map(str::trim)
        .skip_while(|line| line.is_empty())
        .take_while(|line| line.starts_with("#[") || line.starts_with("///"))
        .collect();
    let rule = attributes
        .iter()
        .find_map(|line| line.split("rename_all = \"").nth(1))
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("");
    let mut names = Vec::new();
    let mut rename = None;
    for line in body.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("#[serde(rename = \"") {
            rename = rest.split('"').next().map(str::to_owned);
            continue;
        }
        let variant = line.trim_end_matches(',');
        if variant.starts_with(|c: char| c.is_ascii_uppercase())
            && variant.chars().all(|c| c.is_ascii_alphanumeric())
        {
            names.push(rename.take().unwrap_or_else(|| renamed(variant, rule)));
        } else if !line.starts_with("///") && !line.is_empty() {
            rename = None;
        }
    }
    names
}

fn renamed(variant: &str, rule: &str) -> String {
    match rule {
        "lowercase" => variant.to_lowercase(),
        "snake_case" => {
            let mut name = String::new();
            for (index, c) in variant.chars().enumerate() {
                if c.is_ascii_uppercase() && index > 0 {
                    name.push('_');
                }
                name.push(c.to_ascii_lowercase());
            }
            name
        }
        _ => variant.to_owned(),
    }
}
