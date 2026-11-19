//! Command line interface for the slicer.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::json;
use slicer_core::{Diagnose, Diagnostic, Diagnostics};
use slicer_fdm::load;
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
        /// walls, seam, top and bottom layers, infill, supports and the
        /// speeds of walls, infill, bridges, overhangs and ironing. May be given
        /// several times, also for the same object
        #[arg(long = "object", value_name = "N:FILE")]
        object: Vec<String>,
        /// Settings file in TOML format for the layers whose middle lies
        /// from Z1 up to (not including) Z2 millimetres above the bed, of
        /// object N only (1 is the first model file) or, without N, of
        /// every object; layered over the object's settings. It may only
        /// set walls, seam, top and bottom layers, infill and supports. May
        /// be given several times
        #[arg(long = "range", value_name = "[N:]Z1-Z2:FILE")]
        range: Vec<String>,
        /// Modifier of object N (1 is the first model file): MESH (STL or
        /// 3MF, in the coordinates of the model file, moved with the
        /// object) is not printed; inside it the object prints with the
        /// settings file SETTINGS (TOML) over its own. It may only set top
        /// and bottom layers, sparse infill (density, pattern except
        /// lightning, angle, connecting) and supports. May be given several
        /// times; where modifiers overlap, the later one counts
        #[arg(long = "modifier", value_name = "N:MESH:SETTINGS")]
        modifier: Vec<String>,
        /// Negative volume of object N (1 is the first model file): MESH
        /// (STL or 3MF, in the coordinates of the model file, moved with
        /// the object) is cut out of the object before anything else, so
        /// walls go round the hole and surfaces form at it. May be given
        /// several times
        #[arg(long = "negative", value_name = "N:MESH")]
        negative: Vec<String>,
        /// Use the print settings stored in project files of Orca Slicer or
        /// Bambu Studio (3MF): the project's settings below the -c files
        /// and --set overrides, the settings of single objects over the
        /// plate's and below --object. Settings without a counterpart are
        /// reported as warnings
        #[arg(long)]
        project_settings: bool,
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
    /// by side, each with its own share of the flow ratio.
    /// pressure-advance: a hollow tower whose pressure advance factor
    /// changes every 4 mm of height. Each value is lettered on the model in
    /// raised digits. Start, end and step default to values that suit the
    /// settings.
    Calibrate {
        /// Kind of print: temperature, retraction, flow or pressure-advance
        #[arg(value_parser = parse_calibration_kind)]
        kind: slicer_gcode::calibration::Kind,
        /// First value: °C, mm, % of the flow ratio, or the pressure advance
        /// factor (bottom block or first flow block)
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

/// How messages name the file at `path`, as [`in_file`] does.
fn file_name(path: &Path) -> load::Name {
    load::Name::new(path.display().to_string(), "path", path.display())
}

/// An input/output failure of the command line tool itself.
fn io_error(code: &'static str, path: &Path, error: &std::io::Error) -> Diagnostic {
    Diagnostic::error(code, format!("{}: {error}", path.display())).with("path", path.display())
}

/// Why a picture of the plate could not be made.
fn thumbnail_error(error: &std::io::Error) -> Diagnostic {
    Diagnostic::error(
        "thumbnail.encode",
        format!("the picture of the plate cannot be made: {error}"),
    )
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
            range,
            modifier,
            negative,
            project_settings,
            plate,
            svg,
            send,
            printer,
            json,
        } => {
            let placement = load::Placing {
                keep_positions,
                gap,
            };
            report(
                send.map(|address| printer.target(&address))
                    .transpose()
                    .and_then(|send| {
                        let stored = SettingsArgs {
                            config: &config,
                            overrides: &set,
                            objects: &object,
                            ranges: &range,
                            modifiers: &modifier,
                            negatives: &negative,
                            project: project_settings,
                        };
                        slice(&input, output, placement, &stored, plate, svg, send)
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
        .ok_or_else(|| "expected temperature, retraction, flow or pressure-advance".to_owned())
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

    let base = read_settings(Settings::default(), config, overrides)?;
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
    slicer_gcode::thumbnails::add_thumbnails(
        &mut result.gcode,
        &meshes,
        slicer_fdm::Placement::AsGiven,
        &settings,
    )
    .map_err(|e| thumbnail_error(&e))?;

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

/// The settings files of every one of `count` model files, from the
/// `--object N:FILE` arguments in order, named by their paths. Every file
/// is checked by layering it over `settings`.
fn read_object_files(
    count: usize,
    objects: &[String],
    settings: &Settings,
) -> Result<Vec<Vec<load::ObjectLayer>>, Diagnostic> {
    let mut own: Vec<Vec<load::ObjectLayer>> = vec![Vec::new(); count];
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
        slicer_fdm::apply_object_toml(&mut settings.clone(), &text).map_err(in_file(path))?;
        slot.push(load::ObjectLayer {
            text,
            name: file_name(path),
            range: None,
            modifier: None,
            negative: None,
        });
    }
    Ok(own)
}

/// Adds the settings files of the `--range [N:]Z1-Z2:FILE` arguments to
/// `own`, the settings files of every model file: to those of model N, or
/// without N to those of every model. Every file is checked by layering it
/// over `settings`.
fn read_range_files(
    own: &mut [Vec<load::ObjectLayer>],
    ranges: &[String],
    settings: &Settings,
) -> Result<(), Diagnostic> {
    let count = own.len();
    for item in ranges {
        let invalid = || {
            Diagnostic::error(
                "cli.invalid_range",
                format!(
                    "--range expects [N:]Z1-Z2:FILE with N from 1 to {count} and heights 0 <= Z1 < Z2 in millimetres, got '{item}'"
                ),
            )
            .with("argument", item)
        };
        let (first, rest) = item.split_once(':').ok_or_else(invalid)?;
        // A number alone names the model; heights have a dash.
        let (model, heights, path) = if first.contains('-') {
            (None, first, rest)
        } else {
            let (heights, path) = rest.split_once(':').ok_or_else(invalid)?;
            let model = first
                .trim()
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .filter(|&index| index < count)
                .ok_or_else(invalid)?;
            (Some(model), heights, path)
        };
        let (from, to) = load::parse_height_range(heights).ok_or_else(invalid)?;
        let path = Path::new(path);
        let text = fs::read_to_string(path).map_err(|e| io_error("io.read", path, &e))?;
        let mut range = slicer_fdm::HeightRange::new(from, to);
        slicer_fdm::apply_range_toml(&mut range, settings, &text).map_err(in_file(path))?;
        for (index, layers) in own.iter_mut().enumerate() {
            if model.is_none_or(|model| model == index) {
                layers.push(load::ObjectLayer {
                    text: text.clone(),
                    name: file_name(path),
                    range: Some((from, to)),
                    modifier: None,
                    negative: None,
                });
            }
        }
    }
    Ok(())
}

/// Splits the `MESH:SETTINGS` of a `--modifier` argument after the mesh,
/// whose name ends in `.stl` or `.3mf`, so that both paths may hold
/// colons (drive letters).
fn split_modifier(text: &str) -> Option<(&str, &str)> {
    let lower = text.to_ascii_lowercase();
    let end = [".stl:", ".3mf:"]
        .iter()
        .filter_map(|ending| lower.find(ending))
        .min()?;
    let (mesh, settings) = (&text[..end + 4], &text[end + 5..]);
    (!settings.is_empty()).then_some((mesh, settings))
}

/// Adds the modifiers of the `--modifier N:MESH:SETTINGS` arguments to
/// `own`, the settings files of every model file: to those of model N.
/// Every settings file is checked by layering it over `settings`;
/// warnings about reading the meshes go to `warnings`.
fn read_modifier_files(
    own: &mut [Vec<load::ObjectLayer>],
    modifiers: &[String],
    settings: &Settings,
    warnings: &mut Diagnostics,
) -> Result<(), Diagnostic> {
    let count = own.len();
    if modifiers.len() > slicer_fdm::MAX_MODIFIERS {
        return Err(Diagnostic::error(
            "settings.too_many_modifiers",
            format!(
                "{} modifiers; at most {} are allowed",
                modifiers.len(),
                slicer_fdm::MAX_MODIFIERS
            ),
        )
        .with("modifiers", modifiers.len())
        .with("max", slicer_fdm::MAX_MODIFIERS));
    }
    for item in modifiers {
        let invalid = || {
            Diagnostic::error(
                "cli.invalid_modifier",
                format!(
                    "--modifier expects N:MESH:SETTINGS with N from 1 to {count}, MESH an .stl or .3mf file and SETTINGS a TOML file, got '{item}'"
                ),
            )
            .with("argument", item)
        };
        let (number, rest) = item.split_once(':').ok_or_else(invalid)?;
        let model = number
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|&index| index < count)
            .ok_or_else(invalid)?;
        let (mesh_path, settings_path) = split_modifier(rest).ok_or_else(invalid)?;
        let (mesh_path, settings_path) = (Path::new(mesh_path), Path::new(settings_path));
        let bytes = fs::read(mesh_path).map_err(|e| io_error("io.read", mesh_path, &e))?;
        let mesh =
            slicer_formats::parse_model(&bytes, None, warnings).map_err(in_file(mesh_path))?;
        let text = fs::read_to_string(settings_path)
            .map_err(|e| io_error("io.read", settings_path, &e))?;
        let mut modifier = slicer_fdm::Modifier::default();
        slicer_fdm::apply_modifier_toml(&mut modifier, settings, &text)
            .map_err(in_file(settings_path))?;
        if let Some(layers) = own.get_mut(model) {
            layers.push(load::ObjectLayer {
                text,
                name: file_name(settings_path),
                range: None,
                modifier: Some(mesh),
                negative: None,
            });
        }
    }
    Ok(())
}

/// Adds the negative volumes of the `--negative N:MESH` arguments to `own`,
/// the settings files of every model file: to those of model N. Warnings
/// about reading the meshes go to `warnings`.
fn read_negative_files(
    own: &mut [Vec<load::ObjectLayer>],
    negatives: &[String],
    warnings: &mut Diagnostics,
) -> Result<(), Diagnostic> {
    let count = own.len();
    if negatives.len() > slicer_fdm::MAX_NEGATIVES {
        return Err(Diagnostic::error(
            "model.too_many_negatives",
            format!(
                "{} negative volumes; at most {} are allowed",
                negatives.len(),
                slicer_fdm::MAX_NEGATIVES
            ),
        )
        .with("negatives", negatives.len())
        .with("max", slicer_fdm::MAX_NEGATIVES));
    }
    for item in negatives {
        let invalid = || {
            Diagnostic::error(
                "cli.invalid_negative",
                format!(
                    "--negative expects N:MESH with N from 1 to {count} and MESH an .stl or .3mf file, got '{item}'"
                ),
            )
            .with("argument", item)
        };
        // The number comes first, so the path may hold colons (drive
        // letters).
        let (number, path) = item.split_once(':').ok_or_else(invalid)?;
        let layers = number
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|index| own.get_mut(index))
            .filter(|_| !path.is_empty())
            .ok_or_else(invalid)?;
        let path = Path::new(path);
        let bytes = fs::read(path).map_err(|e| io_error("io.read", path, &e))?;
        let mesh = slicer_formats::parse_model(&bytes, None, warnings).map_err(in_file(path))?;
        layers.push(load::ObjectLayer {
            text: String::new(),
            name: file_name(path),
            range: None,
            modifier: None,
            negative: Some(mesh),
        });
    }
    Ok(())
}

/// Settings from the files in `config`, layered in order over `base`, with
/// the `--set` overrides on top.
fn read_settings(
    base: Settings,
    config: &[PathBuf],
    overrides: &[String],
) -> Result<Settings, Diagnostic> {
    let mut settings = base;
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

/// Where the settings of `slice` come from, in the order they are layered:
/// the project files (if `project`), the files in `config`, the
/// `overrides`, for single objects the `objects` arguments, for height
/// ranges of objects the `ranges` arguments and for modifiers the
/// `modifiers` arguments; the negative volumes of objects come from the
/// `negatives` arguments.
struct SettingsArgs<'a> {
    config: &'a [PathBuf],
    overrides: &'a [String],
    objects: &'a [String],
    ranges: &'a [String],
    modifiers: &'a [String],
    negatives: &'a [String],
    project: bool,
}

fn slice(
    inputs: &[PathBuf],
    output: Option<PathBuf>,
    placing: load::Placing,
    args: &SettingsArgs,
    plate: Option<u32>,
    svg: Option<PathBuf>,
    printer: Option<(Target, bool)>,
) -> Result<serde_json::Value, Diagnostic> {
    let mut warnings = Diagnostics::new();
    let names: Vec<load::Name> = inputs.iter().map(|input| file_name(input)).collect();
    let mut files = Vec::new();
    let mut base = Settings::default();
    let mut stored = Vec::new();
    if args.project {
        // The project files come first: their settings are the base.
        for (input, name) in inputs.iter().zip(&names) {
            let bytes = fs::read(input).map_err(|e| io_error("io.read", input, &e))?;
            let model = load::Model {
                bytes: &bytes,
                name: Some(name),
            };
            stored.push(load::project_base(model, &mut base, &mut warnings)?);
            files.push(bytes);
        }
    }
    let settings = read_settings(base, args.config, args.overrides)?;
    let mut object_files = read_object_files(inputs.len(), args.objects, &settings)?;
    read_range_files(&mut object_files, args.ranges, &settings)?;
    read_modifier_files(&mut object_files, args.modifiers, &settings, &mut warnings)?;
    read_negative_files(&mut object_files, args.negatives, &mut warnings)?;
    if files.is_empty() {
        for input in inputs {
            files.push(fs::read(input).map_err(|e| io_error("io.read", input, &e))?);
        }
    }

    let models: Vec<load::Model> = files
        .iter()
        .zip(&names)
        .map(|(bytes, name)| load::Model {
            bytes,
            name: Some(name),
        })
        .collect();
    // --object and --range files go over every object of their model file.
    let mut objects = load::Objects::read(
        &models,
        &stored,
        &settings,
        &object_files,
        plate,
        &mut warnings,
    )?;
    let placement = objects.place(inputs.len(), placing, &settings);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let own = slicer_fdm::OwnSettings {
        objects: &objects.settings,
        ranges: &objects.ranges,
        modifiers: &objects.modifiers,
        negatives: &objects.negatives,
    };
    let toolpaths = slicer_fdm::generate_plate_with_own_settings(
        &objects.meshes,
        own,
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
        ) => match objects.model.get(*object) {
            Some(&file) => in_file(&inputs[file])(e.diagnostic()),
            None => e.diagnostic(),
        },
        _ => e.diagnostic(),
    })?;
    let mut result =
        slicer_gcode::write_gcode(&toolpaths, &settings).map_err(|e| e.diagnostic())?;
    slicer_gcode::thumbnails::add_thumbnails(
        &mut result.gcode,
        &objects.meshes,
        placement,
        &settings,
    )
    .map_err(|e| thumbnail_error(&e))?;

    let first = inputs.first().map_or_else(PathBuf::new, PathBuf::clone);
    let output = output.unwrap_or_else(|| first.with_extension("gcode"));
    // A name ending in .gcode.3mf asks for the container of Bambu Lab
    // printers around the same G-code.
    let container = output
        .to_string_lossy()
        .to_lowercase()
        .ends_with(".gcode.3mf");
    let content = if container {
        let [large, small] = [
            slicer_formats::PLATE_THUMBNAIL_SIZE,
            slicer_formats::PLATE_THUMBNAIL_SMALL_SIZE,
        ];
        let mut pictures = slicer_gcode::thumbnails::render_thumbnails(
            &objects.meshes,
            placement,
            &[[large, large], [small, small]],
        )
        .map_err(|e| thumbnail_error(&e))?
        .into_iter()
        .map(|picture| picture.png);
        let info = slicer_formats::PrintInfo {
            seconds: result.stats.print_time_s,
            weight_g: result.stats.filament_weight_g,
            filament_m: result.stats.filament_length_mm / 1000.0,
            layers: result.stats.layers,
            nozzle_diameter: settings.nozzle_diameter,
            thumbnail: pictures.next().unwrap_or_default(),
            thumbnail_small: pictures.next().unwrap_or_default(),
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
        if stats.get("flush_weight_g").is_some() {
            println!(
                "  of that into the prime tower: {:.2} cm³, {:.1} g",
                number("flush_volume_cm3"),
                number("flush_weight_g")
            );
        }
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
