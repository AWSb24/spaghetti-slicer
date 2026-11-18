//! Command line interface for the slicer.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::json;
use slicer_core::{Diagnose, Diagnostic, Diagnostics};
use slicer_profiles::Settings;
use slicer_send::Target;

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
    /// Slice STL or 3MF models into G-code
    ///
    /// One model is centred on the bed; several are arranged side by side
    /// unless --keep-positions is given.
    Slice {
        /// Model files (STL or 3MF); each file is one object on the plate
        #[arg(required = true)]
        input: Vec<PathBuf>,
        /// G-code file to write [default: first input with .gcode extension];
        /// a name ending in .gcode.3mf writes a project file for Bambu Lab
        /// printers
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Keep every object where its file puts it, in bed coordinates
        /// (only put down on the bed), instead of centring or arranging
        #[arg(long)]
        keep_positions: bool,
        /// Distance between arranged objects in millimetres, plus the brim
        #[arg(long, value_name = "MM", default_value_t = slicer_fdm::ARRANGE_GAP)]
        gap: f64,
        /// Settings file in TOML format; may be given several times, later
        /// files override earlier ones, missing keys use defaults
        #[arg(short, long)]
        config: Vec<PathBuf>,
        /// Override a single setting, e.g. --set layer_height=0.1
        #[arg(short, long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        /// Settings file in TOML format for object N only (1 is the first
        /// model file), layered over the plate's settings; it may only set
        /// walls, top and bottom layers, infill and supports. May be given
        /// several times, also for the same object
        #[arg(long = "object", value_name = "N:FILE")]
        object: Vec<String>,
        /// Build plate to slice from a project file with several plates
        #[arg(long, value_name = "NUMBER")]
        plate: Option<u32>,
        /// Write one SVG preview per layer into this directory
        #[arg(long, value_name = "DIR")]
        svg: Option<PathBuf>,
        /// Also upload the G-code to a printer, e.g. moonraker://host:7125
        /// or octoprint://host (moonraker+https:// and octoprint+https://
        /// for TLS)
        #[arg(long, value_name = "PRINTER")]
        send: Option<String>,
        #[command(flatten)]
        printer: PrinterOptions,
        /// Print the result (or the error) as JSON
        #[arg(long)]
        json: bool,
    },
    /// Upload an existing G-code file to a printer (Moonraker or OctoPrint)
    Send {
        /// G-code file
        input: PathBuf,
        /// Printer, e.g. moonraker://host:7125 or octoprint://host
        #[arg(long, value_name = "PRINTER")]
        to: String,
        #[command(flatten)]
        printer: PrinterOptions,
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
    /// Write the G-code of a calibration print
    ///
    /// temperature: a tower of blocks, one per nozzle temperature (M104 at
    /// the first layer of each block). retraction: two towers whose
    /// retraction length changes per band of height. flow: flat blocks side
    /// by side, each with its own share of the flow ratio. Start, end and
    /// step default to values that suit the settings.
    Calibrate {
        /// Kind of print: temperature, retraction or flow
        #[arg(value_parser = parse_calibration_kind)]
        kind: slicer_gcode::calibration::Kind,
        /// First value: °C, mm, or % of the flow ratio (bottom block or
        /// first flow block)
        #[arg(long, allow_hyphen_values = true)]
        start: Option<f64>,
        /// Last value
        #[arg(long, allow_hyphen_values = true)]
        end: Option<f64>,
        /// Distance between two values (positive)
        #[arg(long)]
        step: Option<f64>,
        /// G-code file to write [default: calibration-KIND.gcode]
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Settings file in TOML format; may be given several times
        #[arg(short, long)]
        config: Vec<PathBuf>,
        /// Override a single setting, e.g. --set nozzle_temp=200
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

/// How G-code is handed to a printer.
#[derive(clap::Args)]
struct PrinterOptions {
    /// Start printing the file once it is uploaded
    #[arg(long)]
    start: bool,
    /// API key of the printer [default: environment variable
    /// SPAGHETTI_PRINTER_API_KEY, which keeps it out of the process list]
    #[arg(long, value_name = "KEY")]
    api_key: Option<String>,
}

impl PrinterOptions {
    /// The printer at `address` with the API key from the options or the
    /// environment.
    fn target(self, address: &str) -> Result<(Target, bool), Diagnostic> {
        let key = self
            .api_key
            .or_else(|| std::env::var("SPAGHETTI_PRINTER_API_KEY").ok());
        Ok((Target::parse(address)?.with_api_key(key), self.start))
    }
}

/// Uploads G-code; the `sent` part of the report.
fn send(
    (target, start): &(Target, bool),
    file_name: &str,
    gcode: &[u8],
    warnings: &mut Diagnostics,
) -> Result<serde_json::Value, Diagnostic> {
    let sent = slicer_send::upload(target, file_name, gcode, *start, warnings)?;
    Ok(json!({ "printer": target.to_string(), "file": sent.file, "started": sent.started }))
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
            keep_positions,
            gap,
            config,
            set,
            object,
            plate,
            svg,
            send,
            printer,
            json,
        } => {
            let placement = Placing {
                keep_positions,
                gap,
            };
            report(
                send.map(|address| printer.target(&address))
                    .transpose()
                    .and_then(|send| {
                        slice(
                            &input, output, placement, &config, &set, &object, plate, svg, send,
                        )
                    }),
                json,
                print_summary,
            )
        }
        Command::Send {
            input,
            to,
            printer,
            json,
        } => report(
            printer
                .target(&to)
                .and_then(|target| send_file(&input, &target)),
            json,
            print_summary,
        ),
        Command::SliceSla {
            input,
            output,
            set,
            json,
        } => report(slice_sla(input, output, &set), json, print_sla_summary),
        Command::Calibrate {
            kind,
            start,
            end,
            step,
            output,
            config,
            set,
            json,
        } => report(
            calibrate(kind, [start, end, step], output, &config, &set),
            json,
            print_calibration_summary,
        ),
    }
}

/// Reads the kind of a calibration print from the command line.
fn parse_calibration_kind(name: &str) -> Result<slicer_gcode::calibration::Kind, String> {
    slicer_gcode::calibration::Kind::from_name(name)
        .ok_or_else(|| "expected temperature, retraction or flow".to_owned())
}

/// Builds the test models of a calibration print, slices them and writes
/// the calibration G-code. `range` holds start, end and step; missing ones
/// come from [`slicer_gcode::calibration::Kind::default_range`].
fn calibrate(
    kind: slicer_gcode::calibration::Kind,
    range: [Option<f64>; 3],
    output: Option<PathBuf>,
    config: &[PathBuf],
    overrides: &[String],
) -> Result<serde_json::Value, Diagnostic> {
    use slicer_gcode::calibration::Calibration;

    let base = read_settings(config, overrides)?;
    let defaults = kind.default_range(&base);
    let [start, end, step] = [0, 1, 2].map(|i| range[i].unwrap_or(defaults[i]));
    let calibration = Calibration::new(kind, start, end, step).map_err(|e| e.diagnostic())?;
    let settings = calibration.settings(&base);
    let (names, meshes): (Vec<String>, Vec<slicer_core::Mesh>) =
        calibration.placed_models(&settings).into_iter().unzip();
    let mut warnings = Diagnostics::new();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let toolpaths = slicer_fdm::generate_plate(
        &meshes,
        slicer_fdm::Placement::AsGiven,
        &settings,
        &mut warnings,
        &stop,
    )
    .map_err(|e| e.diagnostic())?;
    let mut result =
        slicer_gcode::write_gcode(&toolpaths, &settings).map_err(|e| e.diagnostic())?;
    let objects: Vec<(usize, &slicer_core::Mesh)> = meshes.iter().enumerate().collect();
    calibration.apply(&mut result, &objects, &settings);

    let output =
        output.unwrap_or_else(|| PathBuf::from(format!("calibration-{}.gcode", kind.name())));
    fs::write(&output, &result.gcode).map_err(|e| io_error("io.write", &output, &e))?;
    let objects: Vec<serde_json::Value> = names
        .iter()
        .zip(&meshes)
        .map(|(name, mesh)| {
            let center = mesh.bounds().map_or([0.0, 0.0], |(lo, hi)| {
                [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]
            });
            json!({ "name": name, "x": center[0], "y": center[1] })
        })
        .collect();
    Ok(json!({
        "output": output.display().to_string(),
        "stats": result.stats,
        "warnings": warnings.as_slice(),
        "calibration": {
            "kind": kind.name(),
            "unit": kind.unit(),
            "values": calibration.values(),
            "summary": format!("{}: {}", kind.title(), calibration.summary()),
            "objects": objects,
        },
    }))
}

/// Prints the result of `calibrate` as a summary.
fn print_calibration_summary(result: &serde_json::Value) {
    print_summary(result);
    let calibration = &result["calibration"];
    println!("{}", calibration["summary"].as_str().unwrap_or_default());
    if calibration["kind"] == "flow" {
        for object in calibration["objects"].as_array().into_iter().flatten() {
            println!(
                "  {} at X {:.0}, Y {:.0}",
                object["name"].as_str().unwrap_or_default(),
                object["x"].as_f64().unwrap_or(0.0),
                object["y"].as_f64().unwrap_or(0.0)
            );
        }
        println!(
            "Set flow_ratio to the current value times (1 + percent of the best block / 100)."
        );
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

/// How the command line puts the objects on the bed.
#[derive(Clone, Copy)]
struct Placing {
    keep_positions: bool,
    gap: f64,
}

/// Puts the objects on the bed: as the files place them, centred (one
/// object) or arranged side by side (several).
fn place(
    meshes: &mut [slicer_core::Mesh],
    placing: Placing,
    settings: &Settings,
) -> slicer_fdm::Placement {
    if placing.keep_positions {
        return slicer_fdm::Placement::AsGiven;
    }
    if meshes.len() < 2 {
        return slicer_fdm::Placement::Centre;
    }
    slicer_fdm::arrange_objects(meshes, settings, placing.gap);
    slicer_fdm::Placement::AsGiven
}

/// The settings of every one of `count` objects of its own: the files of
/// the `--object N:FILE` arguments layered in order over `settings`, or
/// `None` for an object without.
fn read_object_settings(
    count: usize,
    objects: &[String],
    settings: &Settings,
) -> Result<Vec<Option<Settings>>, Diagnostic> {
    let mut own: Vec<Option<Settings>> = vec![None; count];
    for item in objects {
        let invalid = || {
            Diagnostic::error(
                "cli.invalid_object",
                format!("--object expects N:FILE with N from 1 to {count}, got '{item}'"),
            )
            .with("argument", item)
        };
        let (number, path) = item.split_once(':').ok_or_else(invalid)?;
        let slot = number
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|index| own.get_mut(index))
            .ok_or_else(invalid)?;
        let path = Path::new(path);
        let text = fs::read_to_string(path).map_err(|e| io_error("io.read", path, &e))?;
        let object = slot.get_or_insert_with(|| settings.clone());
        slicer_fdm::apply_object_toml(object, &text).map_err(in_file(path))?;
    }
    Ok(own)
}

/// Settings from the files in `config`, layered in order, with the
/// `--set` overrides on top.
fn read_settings(config: &[PathBuf], overrides: &[String]) -> Result<Settings, Diagnostic> {
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
    Ok(settings)
}

// The command line arguments of `slice`, passed on one to one.
#[allow(clippy::too_many_arguments)]
fn slice(
    inputs: &[PathBuf],
    output: Option<PathBuf>,
    placing: Placing,
    config: &[PathBuf],
    overrides: &[String],
    objects: &[String],
    plate: Option<u32>,
    svg: Option<PathBuf>,
    printer: Option<(Target, bool)>,
) -> Result<serde_json::Value, Diagnostic> {
    let settings = read_settings(config, overrides)?;
    let own = read_object_settings(inputs.len(), objects, &settings)?;

    let mut warnings = Diagnostics::new();
    let mut meshes = Vec::new();
    for input in inputs {
        let bytes = fs::read(input).map_err(|e| io_error("io.read", input, &e))?;
        meshes.push(
            slicer_formats::parse_model(&bytes, plate, &mut warnings).map_err(in_file(input))?,
        );
    }
    let placement = place(&mut meshes, placing, &settings);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let toolpaths = slicer_fdm::generate_plate_with_object_settings(
        &meshes,
        &own,
        placement,
        &settings,
        &mut warnings,
        &stop,
        &slicer_fdm::Progress::new(),
    )
    .map_err(|e| match (&e, inputs.len()) {
        // Name the file rather than its number.
        (
            slicer_fdm::FdmError::OutsideBed { object, .. }
            | slicer_fdm::FdmError::ObjectTooTall { object, .. },
            2..,
        ) => in_file(&inputs[*object])(e.diagnostic()),
        _ => e.diagnostic(),
    })?;
    let result = slicer_gcode::write_gcode(&toolpaths, &settings).map_err(|e| e.diagnostic())?;

    let first = inputs.first().map_or_else(PathBuf::new, PathBuf::clone);
    let output = output.unwrap_or_else(|| first.with_extension("gcode"));
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

    // A printer gets plain G-code, also when a project file was written.
    let sent = match &printer {
        Some(printer) => {
            let name = output
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = if container {
                name[..name.len() - ".3mf".len()].to_string()
            } else {
                name
            };
            Some(send(
                printer,
                &name,
                result.gcode.as_bytes(),
                &mut warnings,
            )?)
        }
        None => None,
    };

    let mut report = json!({
        "output": output.display().to_string(),
        "stats": result.stats,
        "warnings": warnings.as_slice(),
    });
    if let Some(sent) = sent {
        report["sent"] = sent;
    }
    Ok(report)
}

/// Uploads an existing G-code file.
fn send_file(input: &Path, printer: &(Target, bool)) -> Result<serde_json::Value, Diagnostic> {
    let gcode = fs::read(input).map_err(|e| io_error("io.read", input, &e))?;
    let name = input
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut warnings = Diagnostics::new();
    let sent = send(printer, &name, &gcode, &mut warnings)?;
    Ok(json!({ "warnings": warnings.as_slice(), "sent": sent }))
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
    if let Some(output) = result["output"].as_str() {
        println!("Wrote {output}");
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
    let sent = &result["sent"];
    if let Some(printer) = sent["printer"].as_str() {
        println!(
            "Sent {} to {printer}{}",
            sent["file"].as_str().unwrap_or_default(),
            if sent["started"] == true {
                "; the print started"
            } else {
                ""
            }
        );
    }
}
