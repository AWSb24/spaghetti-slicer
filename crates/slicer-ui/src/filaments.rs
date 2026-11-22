//! Filament slots and their purge volumes, as the filament panel edits
//! them.
//!
//! A plate's filament slots are the extruders of the printer
//! (`extruder_count`); every slot has a colour (`filament_colours`) and a
//! kind (`filament_type`), and a tool change from one slot to another
//! purges `flush_volumes` into the prime tower. The window edits those
//! settings through the settings list, so the values are checked and saved
//! with the project like every other setting. This module turns the
//! settings into slots and the wheel on the extruder count into the
//! sequence of settings that changing it takes.

use slicer_core::thumbnail::DEFAULT_COLORS;
use slicer_profiles::{Settings, colour_hex};

/// Most extruders the slicer takes (`extruder_count`).
pub const MAX_SLOTS: u32 = 16;

/// One filament slot of the plate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// Colour as red, green and blue; a colour the settings do not name
    /// comes from the built-in palette.
    pub colour: [u8; 3],
    /// Kind of the filament, such as `PLA`; `None` where the settings name
    /// none, which the `.gcode.3mf` container writes as `PLA`.
    pub kind: Option<String>,
}

impl Slot {
    /// The kind for people: the one of the settings, or `PLA` where they
    /// name none.
    pub fn kind_label(&self) -> &str {
        self.kind
            .as_deref()
            .filter(|kind| !kind.is_empty())
            .unwrap_or("PLA")
    }
}

/// Number of filament slots of `settings`, at least one.
pub fn count(settings: &Settings) -> usize {
    settings.extruder_count.max(1) as usize
}

/// The slots of `settings`, in the order of the extruders.
pub fn slots(settings: &Settings) -> Vec<Slot> {
    // Settings are checked when they are read; a broken list leaves the
    // palette in place.
    let colours = settings.filament_colours().unwrap_or_default();
    let kinds = settings.filament_types().unwrap_or_default();
    (0..count(settings))
        .map(|index| Slot {
            colour: colours
                .get(index)
                .copied()
                .unwrap_or(DEFAULT_COLORS[index % DEFAULT_COLORS.len()]),
            kind: kinds.get(index).map(|kind| (*kind).to_owned()),
        })
        .collect()
}

/// The colour the slot `extruder` (from 0) shows.
pub fn slot_colour(settings: &Settings, extruder: usize) -> [u8; 3] {
    slots(settings)
        .get(extruder)
        .map_or(DEFAULT_COLORS[0], |slot| slot.colour)
}

/// The text of `filament_colours` for `colours`: `#RRGGBB` entries
/// separated by commas.
pub fn colours_text(colours: &[[u8; 3]]) -> String {
    colours
        .iter()
        .map(|colour| colour_hex(*colour))
        .collect::<Vec<_>>()
        .join(",")
}

/// `filament_colours` with the colour of slot `extruder` replaced. Slots
/// the settings do not name get their colour from the palette, so that
/// every slot shows its own colour afterwards.
pub fn set_slot_colour(settings: &Settings, extruder: usize, colour: [u8; 3]) -> String {
    let mut colours: Vec<[u8; 3]> = slots(settings)
        .into_iter()
        .map(|slot| slot.colour)
        .collect();
    if let Some(entry) = colours.get_mut(extruder) {
        *entry = colour;
    }
    colours_text(&colours)
}

/// `filament_type` with the kind of slot `extruder` replaced; the other
/// slots keep their kinds, and slots without one get `PLA`.
pub fn set_slot_type(settings: &Settings, extruder: usize, kind: &str) -> String {
    let mut kinds: Vec<String> = slots(settings)
        .into_iter()
        .map(|slot| slot.kind_label().to_owned())
        .collect();
    if let Some(entry) = kinds.get_mut(extruder) {
        let kind = kind.trim();
        // An empty field means the kind the container writes by default.
        *entry = if kind.is_empty() {
            "PLA".to_owned()
        } else {
            kind.to_owned()
        };
    }
    kinds.join(",")
}

/// The purge volumes of `flush_volumes` without `flush_multiplier`, as
/// `count` rows of `count` values; `None` if the text is empty or does not
/// name exactly `count` × `count` volumes of at least 0.
///
/// Values may be separated by commas and rows by semicolons; a plain list
/// of `count` × `count` numbers is read row by row, so `0,50,200,0` is the
/// table of two extruders.
pub fn parse_flush(text: &str, count: usize) -> Option<Vec<Vec<f64>>> {
    let text = text.trim();
    if text.is_empty() || count == 0 {
        return None;
    }
    let values: Vec<f64> = text
        .split([',', ';'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 0.0)
        })
        .collect::<Option<Vec<_>>>()?;
    if values.len() != count * count {
        return None;
    }
    let mut table: Vec<Vec<f64>> = values.chunks(count).map(<[f64]>::to_vec).collect();
    for (index, row) in table.iter_mut().enumerate() {
        row[index] = 0.0;
    }
    Some(table)
}

/// `flush_volumes` for `table`, rows separated by `;`.
pub fn flush_text(table: &[Vec<f64>]) -> String {
    table
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| volume_text(*value))
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// A purge volume as short text: whole numbers without a fraction, others
/// with one decimal.
fn volume_text(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if rounded.fract().abs() < f64::EPSILON {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.1}")
    }
}

/// The table the flush matrix shows: the values of `flush_volumes`, or the
/// ones derived from the colours when the settings derive them, without
/// `flush_multiplier`. Zeros where the settings name no purge volumes.
pub fn flush_table(settings: &Settings) -> Vec<Vec<f64>> {
    let count = count(settings);
    let empty = || vec![vec![0.0; count]; count];
    if let Some(table) = parse_flush(&settings.flush_volumes, count) {
        return table;
    }
    if !settings.flush_volumes.trim().is_empty() || !settings.flush_from_colours {
        return empty();
    }
    let Ok(derived) = settings.flush_matrix() else {
        return empty();
    };
    let factor = if settings.flush_multiplier > 0.0 {
        settings.flush_multiplier
    } else {
        1.0
    };
    (0..count)
        .map(|from| {
            (0..count)
                .map(|to| {
                    derived
                        .get(from)
                        .and_then(|row| row.get(to))
                        .copied()
                        .unwrap_or(0.0)
                        / factor
                })
                .collect()
        })
        .collect()
}

/// The explicit table of `flush_volumes`, whatever `extruder_count` says:
/// with one extruder any square table is accepted, so the side comes from
/// the number of values.
fn explicit_flush(settings: &Settings) -> Option<Vec<Vec<f64>>> {
    if settings.flush_volumes.trim().is_empty() {
        return None;
    }
    let values = settings
        .flush_volumes
        .split([',', ';'])
        .filter(|entry| !entry.trim().is_empty())
        .count();
    let side = (values as f64).sqrt() as usize;
    if side == 0 || side * side != values {
        return None;
    }
    parse_flush(&settings.flush_volumes, side)
}

/// The settings that change the number of filament slots from the one of
/// `settings` to `count`, in the order they stay valid step by step.
///
/// Colours and kinds are kept for the slots that remain and grow to the
/// palette and `PLA`; an explicit flush table is cut or filled with zeros.
/// A table derived from the colours stays derived. Empty if the count does
/// not change.
pub fn resize_plan(settings: &Settings, count: u32) -> Vec<(&'static str, String)> {
    let old = settings.extruder_count.max(1);
    let new = count.clamp(1, MAX_SLOTS);
    if new == old {
        return Vec::new();
    }
    let wanted = new as usize;
    let colours = settings.filament_colours().unwrap_or_default();
    let kinds = settings.filament_types().unwrap_or_default();
    let explicit = explicit_flush(settings);
    let mut plan = Vec::new();
    if new < old {
        // The old table and the longer lists are not valid for fewer
        // slots once the count changes, so they go first.
        if !settings.flush_volumes.trim().is_empty() {
            plan.push(("flush_volumes", String::new()));
        }
        if !colours.is_empty() {
            let kept: Vec<[u8; 3]> = colours.iter().take(wanted).copied().collect();
            plan.push(("filament_colours", colours_text(&kept)));
        }
        if !kinds.is_empty() {
            let kept: Vec<String> = kinds
                .iter()
                .take(wanted)
                .map(|kind| (*kind).to_owned())
                .collect();
            plan.push(("filament_type", kept.join(",")));
        }
        plan.push(("extruder_count", new.to_string()));
        if let Some(table) = explicit {
            let mut cut = table;
            cut.truncate(wanted);
            for row in &mut cut {
                row.truncate(wanted);
            }
            plan.push(("flush_volumes", flush_text(&cut)));
        }
    } else {
        // The old table does not fit more slots either, so it goes first
        // and comes back grown once the count is right.
        if !settings.flush_volumes.trim().is_empty() {
            plan.push(("flush_volumes", String::new()));
        }
        // Derived volumes need one colour per slot, which the longer list
        // only has once the count is right: derive again afterwards.
        let derived = settings.flush_from_colours
            && settings.flush_volumes.trim().is_empty()
            && !colours.is_empty();
        if derived {
            plan.push(("flush_from_colours", "false".to_owned()));
        }
        plan.push(("extruder_count", new.to_string()));
        if !colours.is_empty() {
            let grown: Vec<[u8; 3]> = (0..wanted)
                .map(|index| {
                    colours
                        .get(index)
                        .copied()
                        .unwrap_or(DEFAULT_COLORS[index % DEFAULT_COLORS.len()])
                })
                .collect();
            plan.push(("filament_colours", colours_text(&grown)));
        }
        if !kinds.is_empty() {
            let grown: Vec<String> = (0..wanted)
                .map(|index| kinds.get(index).copied().unwrap_or("PLA").to_owned())
                .collect();
            plan.push(("filament_type", grown.join(",")));
        }
        if let Some(table) = explicit {
            let mut grown = table;
            grown.resize(wanted, vec![0.0; old as usize]);
            for row in &mut grown {
                row.resize(wanted, 0.0);
            }
            plan.push(("flush_volumes", flush_text(&grown)));
        }
        if derived {
            plan.push(("flush_from_colours", "true".to_owned()));
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn two() -> Settings {
        let mut settings = Settings::default();
        settings
            .set_all([
                ("extruder_count", "2"),
                ("filament_colours", "#FF8000, #202020"),
                ("filament_type", "PLA,PETG"),
            ])
            .unwrap();
        settings
    }

    #[test]
    fn slots_follow_the_settings_and_the_palette() {
        let settings = Settings::default();
        let one = slots(&settings);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].colour, DEFAULT_COLORS[0]);
        assert_eq!(one[0].kind, None);
        assert_eq!(one[0].kind_label(), "PLA");

        let settings = two();
        let two_slots = slots(&settings);
        assert_eq!(two_slots.len(), 2);
        assert_eq!(two_slots[0].colour, [0xFF, 0x80, 0]);
        assert_eq!(two_slots[1].kind.as_deref(), Some("PETG"));
        assert_eq!(slot_colour(&settings, 5), DEFAULT_COLORS[0]);
    }

    #[test]
    fn changing_one_slot_keeps_the_others() {
        let settings = two();
        let text = set_slot_colour(&settings, 1, [0x00, 0xC0, 0x00]);
        assert_eq!(text, "#FF8000,#00C000");
        assert_eq!(colours_text(&[[1, 2, 3]]), "#010203");

        assert_eq!(set_slot_type(&settings, 0, "PLA-CF"), "PLA-CF,PETG");
        assert_eq!(set_slot_type(&settings, 0, "  "), "PLA,PETG");
        assert_eq!(set_slot_type(&Settings::default(), 0, "PETG"), "PETG");
        // A slot without a kind gets the one the container writes.
        let mut bare = two();
        bare.set("filament_type", "PLA").unwrap();
        assert_eq!(set_slot_type(&bare, 1, "ABS"), "PLA,ABS");
    }

    #[test]
    fn flush_tables_are_read_and_written_without_the_multiplier() {
        assert_eq!(
            parse_flush("0,50;200,0", 2),
            Some(vec![vec![0.0, 50.0], vec![200.0, 0.0]])
        );
        // Commas alone are read row by row, and the diagonal is dropped.
        let table = parse_flush("9, 50, 200, 9;", 2).unwrap();
        assert_eq!(table, vec![vec![0.0, 50.0], vec![200.0, 0.0]]);
        assert_eq!(parse_flush("", 2), None);
        assert_eq!(parse_flush("1,2,3", 2), None);
        assert_eq!(parse_flush("1,2,3,-4", 2), None);
        assert_eq!(flush_text(&table), "0,50;200,0");
        assert_eq!(flush_text(&[vec![0.0, 123.46]]), "0,123.5");

        let mut settings = two();
        settings.set("flush_volumes", "0,50;200,0").unwrap();
        settings.set("flush_multiplier", "2").unwrap();
        assert_eq!(flush_table(&settings)[0][1], 50.0);
        settings.set("flush_volumes", "").unwrap();
        assert_eq!(flush_table(&settings), vec![vec![0.0; 2]; 2]);
    }

    #[test]
    fn flush_tables_come_from_the_colours_when_asked() {
        let mut settings = two();
        settings.set("flush_from_colours", "true").unwrap();
        settings.set("flush_multiplier", "2").unwrap();
        let table = flush_table(&settings);
        // Orange to almost black purges less than back (see
        // `colour_flush_volume`), and the multiplier is not applied here.
        assert!(table[0][1] > 0.0 && table[1][0] > table[0][1]);
        assert_eq!(table[0][0], 0.0);
        assert!(settings.flush_matrix().unwrap()[1][0] > 0.0);
    }

    #[test]
    fn growing_keeps_the_values_and_fills_with_defaults() {
        let settings = two();
        let mut with_flush = settings.clone();
        with_flush.set("flush_volumes", "0,50;200,0").unwrap();
        let plan = resize_plan(&with_flush, 3);
        let keys: Vec<&str> = plan.iter().map(|(key, _)| *key).collect();
        assert_eq!(
            keys,
            [
                "flush_volumes",
                "extruder_count",
                "filament_colours",
                "filament_type",
                "flush_volumes"
            ]
        );
        assert_eq!(plan[1].1, "3");
        assert_eq!(plan[2].1, "#FF8000,#202020,#3BA55C");
        assert_eq!(plan[3].1, "PLA,PETG,PLA");
        assert_eq!(plan[4].1, "0,50,0;200,0,0;0,0,0");

        // Every step stays valid in order.
        let mut step = with_flush.clone();
        for (key, value) in resize_plan(&step, 3) {
            step.set(key, &value).unwrap();
        }
        assert_eq!(step.extruder_count, 3);
        assert_eq!(step.filament_colours().unwrap().len(), 3);
        assert_eq!(flush_table(&step)[0][1], 50.0);

        // With one extruder any square table is accepted, and it is kept
        // when a second slot comes.
        let mut single = Settings::default();
        single.set("flush_volumes", "0,50;200,0").unwrap();
        let mut step = single.clone();
        for (key, value) in resize_plan(&single, 2) {
            step.set(key, &value).unwrap();
        }
        assert_eq!(step.extruder_count, 2);
        assert_eq!(flush_table(&step), vec![vec![0.0, 50.0], vec![200.0, 0.0]]);

        // A table derived from the colours is switched off while the
        // longer list of colours is written, then on again.
        let mut derived = two();
        derived.set("flush_from_colours", "true").unwrap();
        let plan = resize_plan(&derived, 3);
        assert_eq!(
            plan.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            [
                "flush_from_colours",
                "extruder_count",
                "filament_colours",
                "filament_type",
                "flush_from_colours"
            ]
        );
        assert_eq!(plan[0].1, "false");
        assert_eq!(plan[4].1, "true");
        let mut step = derived.clone();
        for (key, value) in plan {
            step.set(key, &value).unwrap();
        }
        assert!(step.flush_from_colours && step.extruder_count == 3);
        assert_eq!(step.flush_matrix().unwrap().len(), 3);
    }

    #[test]
    fn shrinking_keeps_the_remaining_slots() {
        let mut settings = Settings::default();
        settings
            .set_all([
                ("extruder_count", "3"),
                ("filament_colours", "#FF8000,#202020,#00C000"),
                ("filament_type", "PLA,PETG,ABS"),
                ("flush_volumes", "0,10,20;30,0,40;50,60,0"),
            ])
            .unwrap();
        let plan = resize_plan(&settings, 2);
        assert_eq!(
            plan.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            [
                "flush_volumes",
                "filament_colours",
                "filament_type",
                "extruder_count",
                "flush_volumes"
            ]
        );
        assert_eq!(plan[1].1, "#FF8000,#202020");
        assert_eq!(plan[2].1, "PLA,PETG");
        assert_eq!(plan[4].1, "0,10;30,0");
        let mut step = settings.clone();
        for (key, value) in plan {
            step.set(key, &value).unwrap();
        }
        assert_eq!(step.extruder_count, 2);
        assert_eq!(step.filament_types().unwrap(), ["PLA", "PETG"]);
        assert_eq!(flush_table(&step), vec![vec![0.0, 10.0], vec![30.0, 0.0]]);

        // A table derived from colours stays derived.
        let mut derived = settings.clone();
        derived.set("flush_volumes", "").unwrap();
        derived.set("flush_from_colours", "true").unwrap();
        assert!(
            !resize_plan(&derived, 2)
                .iter()
                .any(|(key, _)| *key == "flush_volumes")
        );
        assert!(resize_plan(&settings, 3).is_empty());
        let one = resize_plan(&settings, 1);
        assert_eq!(
            one.iter()
                .find(|(key, _)| *key == "extruder_count")
                .unwrap()
                .1,
            "1"
        );
        assert_eq!(resize_plan(&Settings::default(), 99)[0].1, "16");
    }
}
