//! Command line interface for the slicer.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::json;
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
    /// Slice an STL model into G-code
    Slice {
        /// Model file (binary or ASCII STL)
        input: PathBuf,
        /// G-code file to write [default: input with .gcode extension]
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Settings file in TOML format; missing keys use defaults
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Override a single setting, e.g. --set layer_height=0.1
        #[arg(short, long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
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

/// An error with a stable machine-readable code.
struct Failure {
    code: &'static str,
    message: String,
}

fn fail(code: &'static str) -> impl Fn(String) -> Failure {
    move |message| Failure { code, message }
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
            svg,
            json,
        } => match slice(input, output, config, &set, svg) {
            Ok(result) => {
                if json {
                    println!("{result:#}");
                } else {
                    print_summary(&result);
                }
                ExitCode::SUCCESS
            }
            Err(failure) => {
                if json {
                    eprintln!(
                        "{}",
                        json!({ "error": { "code": failure.code, "message": failure.message } })
                    );
                } else {
                    eprintln!("error: {}", failure.message);
                }
                ExitCode::FAILURE
            }
        },
    }
}

fn slice(
    input: PathBuf,
    output: Option<PathBuf>,
    config: Option<PathBuf>,
    overrides: &[String],
    svg: Option<PathBuf>,
) -> Result<serde_json::Value, Failure> {
    let mut settings = match &config {
        Some(path) => {
            let text = fs::read_to_string(path)
                .map_err(|e| fail("config_unreadable")(format!("{}: {e}", path.display())))?;
            Settings::from_toml(&text)
                .map_err(|e| fail("config_invalid")(format!("{}: {e}", path.display())))?
        }
        None => Settings::default(),
    };
    for item in overrides {
        let (key, value) = item.split_once('=').ok_or_else(|| {
            fail("setting_invalid")(format!("--set expects KEY=VALUE, got '{item}'"))
        })?;
        settings
            .set(key.trim(), value.trim())
            .map_err(|e| fail("setting_invalid")(e.to_string()))?;
    }

    let bytes = fs::read(&input)
        .map_err(|e| fail("model_unreadable")(format!("{}: {e}", input.display())))?;
    let mesh = slicer_formats::parse_stl(&bytes)
        .map_err(|e| fail("model_invalid")(format!("{}: {e}", input.display())))?;

    let mut warnings = Vec::new();
    let open_edges = mesh.open_edge_count();
    if open_edges > 0 {
        warnings.push(format!(
            "mesh is not watertight ({open_edges} open edges); affected layers may be incomplete"
        ));
    }

    let toolpaths = slicer_fdm::generate(&mesh, &settings)
        .map_err(|e| fail("slicing_failed")(e.to_string()))?;
    let result = slicer_gcode::write_gcode(&toolpaths, &settings)
        .map_err(|e| fail("gcode_invalid")(e.to_string()))?;

    let output = output.unwrap_or_else(|| input.with_extension("gcode"));
    fs::write(&output, &result.gcode)
        .map_err(|e| fail("output_unwritable")(format!("{}: {e}", output.display())))?;

    if let Some(dir) = &svg {
        let unwritable =
            |e: std::io::Error| fail("output_unwritable")(format!("{}: {e}", dir.display()));
        fs::create_dir_all(dir).map_err(unwritable)?;
        for (index, layer) in toolpaths.layers.iter().enumerate() {
            let image = slicer_fdm::preview::layer_svg(layer, settings.line_width);
            fs::write(dir.join(format!("layer_{:04}.svg", index + 1)), image)
                .map_err(unwritable)?;
        }
    }

    Ok(json!({
        "output": output.display().to_string(),
        "stats": result.stats,
        "warnings": warnings,
    }))
}

fn print_summary(result: &serde_json::Value) {
    for warning in result["warnings"].as_array().into_iter().flatten() {
        eprintln!("warning: {}", warning.as_str().unwrap_or_default());
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
        "  time:      {}h {:02}m {:02}s (estimate without acceleration, real prints take longer)",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
}
