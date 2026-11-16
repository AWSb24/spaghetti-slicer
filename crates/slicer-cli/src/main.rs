//! Command line interface for the slicer.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::json;
use slicer_core::{Diagnose, Diagnostic, Diagnostics};
use slicer_profiles::Settings;

#[derive(Parser)]
#[command(
    name = "spaghetti-slicer",
    version,
    about = "Slices 3D models into G-code"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Slice an STL or 3MF model into G-code
    Slice {
        /// Model file (STL or 3MF)
        input: PathBuf,
        /// G-code file to write [default: input with .gcode extension]; a name
        /// ending in .gcode.3mf writes a project file for Bambu Lab printers
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Settings file in TOML format; may be given several times, later
        /// files override earlier ones, missing keys use defaults
        #[arg(short, long)]
        config: Vec<PathBuf>,
        /// Override a single setting, e.g. --set layer_height=0.1
        #[arg(short, long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        /// Build plate to slice from a project file with several plates
        #[arg(long, value_name = "NUMBER")]
        plate: Option<u32>,
        /// Write one SVG preview per layer into this directory
        #[arg(long, value_name = "DIR")]
        svg: Option<PathBuf>,
        /// Print the result (or the error) as JSON
        #[arg(long)]
        json: bool,
    },
    /// Slice an STL or 3MF model into layer images for a resin (MSLA) printer
    ///
    /// Writes a ZIP archive with one PNG per layer and a JSON manifest
    /// (docs/sla-format.md). No supports and no hollowing yet.
    SliceSla {
        /// Model file (STL or 3MF)
        input: PathBuf,
        /// Archive to write [default: input with .sla.zip extension]
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Override a resin setting, e.g. --set exposure_time=2.5
        #[arg(short, long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        /// Print the result (or the error) as JSON
        #[arg(long)]
        json: bool,
    },
    /// Print all settings with their default values as TOML
    DefaultConfig,
    /// Turn profiles saved by Orca Slicer (JSON) into a settings file
    ///
    /// Give printer, material and process profile in this order; later files
    /// override earlier ones. Settings without a counterpart are listed.
    ImportOrca {
        /// Orca Slicer profiles (JSON)
        #[arg(required = true)]
        profiles: Vec<PathBuf>,
        /// Settings file to write [default: print to standard output]
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

/// Names the file a diagnostic concerns.
fn in_file(path: &Path) -> impl Fn(Diagnostic) -> Diagnostic + '_ {
    move |diagnostic| {
        let mut diagnostic = diagnostic.with("path", path.display());
        diagnostic.message = format!("{}: {}", path.display(), diagnostic.message);
        diagnostic
    }
}

/// An input/output failure of the command line tool itself.
fn io_error(code: &'static str, path: &Path, error: &std::io::Error) -> Diagnostic {
    Diagnostic::error(code, format!("{}: {error}", path.display())).with("path", path.display())
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::DefaultConfig => {
            print!("{}", Settings::default().to_toml());
            ExitCode::SUCCESS
        }
        Command::ImportOrca { profiles, output } => match import_orca(&profiles, output) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Slice {
            input,
            output,
            config,
            set,
            plate,
            svg,
            json,
        } => report(
            slice(input, output, &config, &set, plate, svg),
            json,
            print_summary,
        ),
        Command::SliceSla {
            input,
            output,
            set,
            json,
        } => report(slice_sla(input, output, &set), json, print_sla_summary),
    }
}

/// Prints the result of a slicing command as JSON or as a summary.
fn report(
    result: Result<serde_json::Value, Diagnostic>,
    json: bool,
    summary: fn(&serde_json::Value),
) -> ExitCode {
    match result {
        Ok(result) => {
            if json {
                println!("{result:#}");
            } else {
                summary(&result);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            if json {
                eprintln!("{}", json!({ "error": error }));
            } else {
                eprintln!("error: {error}");
            }
            ExitCode::FAILURE
        }
    }
}

/// Splits a `--set` argument into key and value.
fn split_override(item: &str) -> Result<(&str, &str), Diagnostic> {
    let (key, value) = item.split_once('=').ok_or_else(|| {
        Diagnostic::error(
            "cli.invalid_override",
            format!("--set expects KEY=VALUE, got '{item}'"),
        )
        .with("argument", item)
    })?;
    Ok((key.trim(), value.trim()))
}

fn slice(
    input: PathBuf,
    output: Option<PathBuf>,
    config: &[PathBuf],
    overrides: &[String],
    plate: Option<u32>,
    svg: Option<PathBuf>,
) -> Result<serde_json::Value, Diagnostic> {
    let mut settings = Settings::default();
    for path in config {
        let text = fs::read_to_string(path).map_err(|e| io_error("io.read", path, &e))?;
        settings
            .apply_toml(&text)
            .map_err(|e| in_file(path)(e.diagnostic()))?;
    }
    for item in overrides {
        let (key, value) = split_override(item)?;
        settings.set(key, value).map_err(|e| e.diagnostic())?;
    }

    let bytes = fs::read(&input).map_err(|e| io_error("io.read", &input, &e))?;
    let mut warnings = Diagnostics::new();
    let mesh =
        slicer_formats::parse_model(&bytes, plate, &mut warnings).map_err(in_file(&input))?;

    let toolpaths = slicer_fdm::generate_reporting(&mesh, &settings, &mut warnings)
        .map_err(|e| e.diagnostic())?;
    let result = slicer_gcode::write_gcode(&toolpaths, &settings).map_err(|e| e.diagnostic())?;

    let output = output.unwrap_or_else(|| input.with_extension("gcode"));
    // A name ending in .gcode.3mf asks for the container of Bambu Lab
    // printers around the same G-code.
    let container = output
        .to_string_lossy()
        .to_lowercase()
        .ends_with(".gcode.3mf");
    let content = if container {
        let info = slicer_formats::PrintInfo {
            seconds: result.stats.print_time_s,
            weight_g: result.stats.filament_weight_g,
            filament_m: result.stats.filament_length_mm / 1000.0,
            layers: result.stats.layers,
            nozzle_diameter: settings.nozzle_diameter,
        };
        slicer_formats::write_gcode_3mf(&result.gcode, &info).map_err(|e| e.diagnostic())?
    } else {
        result.gcode.clone().into_bytes()
    };
    fs::write(&output, content).map_err(|e| io_error("io.write", &output, &e))?;

    if let Some(dir) = &svg {
        fs::create_dir_all(dir).map_err(|e| io_error("io.write", dir, &e))?;
        for (index, layer) in toolpaths.layers.iter().enumerate() {
            let image = slicer_fdm::preview::layer_svg(layer, settings.line_width);
            let file = dir.join(format!("layer_{:04}.svg", index + 1));
            fs::write(&file, image).map_err(|e| io_error("io.write", &file, &e))?;
        }
    }

    Ok(json!({
        "output": output.display().to_string(),
        "stats": result.stats,
        "warnings": warnings.as_slice(),
    }))
}

fn slice_sla(
    input: PathBuf,
    output: Option<PathBuf>,
    overrides: &[String],
) -> Result<serde_json::Value, Diagnostic> {
    let mut settings = slicer_sla::SlaSettings::default();
    for item in overrides {
        let (key, value) = split_override(item)?;
        settings.set(key, value).map_err(|e| e.diagnostic())?;
    }
    let bytes = fs::read(&input).map_err(|e| io_error("io.read", &input, &e))?;
    let mut warnings = Diagnostics::new();
    let mesh = slicer_formats::parse_model(&bytes, None, &mut warnings).map_err(in_file(&input))?;
    let print = slicer_sla::slice(&mesh, &settings, &mut warnings).map_err(|e| e.diagnostic())?;
    let archive = slicer_sla::write_archive(&print).map_err(|e| e.diagnostic())?;
    let output = output.unwrap_or_else(|| input.with_extension("sla.zip"));
    fs::write(&output, archive).map_err(|e| io_error("io.write", &output, &e))?;
    Ok(json!({
        "output": output.display().to_string(),
        "stats": print.stats,
        "warnings": warnings.as_slice(),
    }))
}

fn print_sla_summary(result: &serde_json::Value) {
    for warning in result["warnings"].as_array().into_iter().flatten() {
        eprintln!(
            "warning: {}",
            warning["message"].as_str().unwrap_or_default()
        );
    }
    let stats = &result["stats"];
    let seconds = stats["print_time_s"].as_f64().unwrap_or(0.0) as u64;
    println!("Wrote {}", result["output"].as_str().unwrap_or_default());
    println!("  layers:    {}", stats["layers"]);
    println!(
        "  resin:     {:.2} ml",
        stats["resin_volume_ml"].as_f64().unwrap_or(0.0)
    );
    println!(
        "  time:      {}h {:02}m {:02}s (exposure and plate moves)",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
}

fn import_orca(profiles: &[PathBuf], output: Option<PathBuf>) -> Result<(), Diagnostic> {
    let mut settings = Settings::default();
    let mut keys: Vec<String> = Vec::new();
    let mut header = String::from("# Imported from Orca Slicer profiles by spaghetti-slicer.\n");
    for path in profiles {
        let text = fs::read_to_string(path).map_err(|e| io_error("io.read", path, &e))?;
        let import = slicer_profiles::import_orca(&text, &mut settings)
            .map_err(|e| in_file(path)(e.diagnostic()))?;
        let name = import.name.unwrap_or_else(|| path.display().to_string());
        header.push_str(&format!(
            "# {name}: {} setting(s) taken",
            import.taken.len()
        ));
        if let Some(parent) = &import.inherits {
            // The parent is one of Orca's own presets, which are not read.
            header.push_str(&format!(
                "; values inherited from '{parent}' are not included"
            ));
            eprintln!("note: {name}: values inherited from '{parent}' are not included");
        }
        header.push('\n');
        for (key, reason) in &import.skipped {
            eprintln!("skipped: {name}: {key}: {reason}");
        }
        for (ours, _, _) in import.taken {
            if !keys.contains(&ours) {
                keys.push(ours);
            }
        }
    }
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let text = format!("{header}\n{}", settings.to_toml_keys(&keys));
    match output {
        Some(path) => {
            fs::write(&path, text).map_err(|e| io_error("io.write", &path, &e))?;
            eprintln!("Wrote {} with {} setting(s)", path.display(), keys.len());
        }
        None => print!("{text}"),
    }
    Ok(())
}

fn print_summary(result: &serde_json::Value) {
    for warning in result["warnings"].as_array().into_iter().flatten() {
        eprintln!(
            "warning: {}",
            warning["message"].as_str().unwrap_or_default()
        );
    }
    let stats = &result["stats"];
    let number = |key: &str| stats[key].as_f64().unwrap_or(0.0);
    let seconds = number("print_time_s") as u64;
    println!("Wrote {}", result["output"].as_str().unwrap_or_default());
    println!("  layers:    {}", stats["layers"]);
    println!(
        "  filament:  {:.2} m, {:.1} g",
        number("filament_length_mm") / 1000.0,
        number("filament_weight_g")
    );
    println!(
        "  time:      {}h {:02}m {:02}s (estimate with acceleration, without heating)",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
}
