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
        /// G-code file to write [default: input with .gcode extension]
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
    /// Print all settings with their default values as TOML
    DefaultConfig,
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
        Command::Slice {
            input,
            output,
            config,
            set,
            plate,
            svg,
            json,
        } => match slice(input, output, &config, &set, plate, svg) {
            Ok(result) => {
                if json {
                    println!("{result:#}");
                } else {
                    print_summary(&result);
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
        },
    }
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
        let (key, value) = item.split_once('=').ok_or_else(|| {
            Diagnostic::error(
                "cli.invalid_override",
                format!("--set expects KEY=VALUE, got '{item}'"),
            )
            .with("argument", item)
        })?;
        settings
            .set(key.trim(), value.trim())
            .map_err(|e| e.diagnostic())?;
    }

    let bytes = fs::read(&input).map_err(|e| io_error("io.read", &input, &e))?;
    let mut warnings = Diagnostics::new();
    let mesh =
        slicer_formats::parse_model(&bytes, plate, &mut warnings).map_err(in_file(&input))?;

    let toolpaths = slicer_fdm::generate_reporting(&mesh, &settings, &mut warnings)
        .map_err(|e| e.diagnostic())?;
    let result = slicer_gcode::write_gcode(&toolpaths, &settings).map_err(|e| e.diagnostic())?;

    let output = output.unwrap_or_else(|| input.with_extension("gcode"));
    fs::write(&output, &result.gcode).map_err(|e| io_error("io.write", &output, &e))?;

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
