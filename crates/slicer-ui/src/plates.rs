//! Several build plates in one session. Each plate holds its own models
//! and keeps its own slice while another plate is shown.
//!
//! The plate shown (the current plate) lives outside this list, in the
//! hands of the app, so that the app can edit it like a single plate; the
//! list holds the others and takes the current one back when the user
//! switches. One history covers all plates, so that undo also brings back
//! a model moved to another plate, an added plate or a removed one.

use slicer_core::Mesh;

use crate::plate::Plate;

/// Steps that can be undone.
const UNDO_STEPS: usize = 100;

/// A plate with what the app keeps for it, such as its slice.
#[derive(Debug, Clone)]
struct Slot<S> {
    /// Stays the same while the plate exists, to match a plate to its
    /// state before and after an undo.
    id: u64,
    /// The models; empty for the current plate, which the app holds.
    plate: Plate,
    /// What the app keeps for this plate; `None` for the current plate.
    extra: Option<S>,
}

/// All plates at one moment, for undo and redo.
#[derive(Debug, Clone, Default)]
struct Snapshot {
    plates: Vec<(u64, Plate)>,
    /// Index of the plate shown.
    current: usize,
}

/// The plates of a session and which one is shown. `S` is what the app
/// keeps for a plate besides its models, for example its slice.
#[derive(Debug, Clone)]
pub struct Plates<S> {
    slots: Vec<Slot<S>>,
    current: usize,
    next_id: u64,
    /// The last recorded state of all plates.
    recorded: Snapshot,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl<S> Default for Plates<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Plates<S> {
    /// One empty plate, shown.
    pub fn new() -> Self {
        Self {
            slots: vec![Slot {
                id: 0,
                plate: Plate::default(),
                extra: None,
            }],
            current: 0,
            next_id: 1,
            recorded: Snapshot {
                plates: vec![(0, Plate::default())],
                current: 0,
            },
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// The plates `plates` (at least one), the first shown, with no steps
    /// to undo. Returns the list and the first plate, which the app holds.
    pub fn from_plates(plates: Vec<Plate>) -> (Self, Plate) {
        let mut list = Self::new();
        let mut plates = plates.into_iter();
        let first = plates.next().unwrap_or_default();
        for plate in plates {
            let index = list.push();
            list.slots[index].plate = plate;
        }
        list.recorded = list.snapshot(&first);
        (list, first)
    }

    /// Every plate in order; `plate` is the current one.
    pub fn all<'a>(&'a self, plate: &'a Plate) -> Vec<&'a Plate> {
        (0..self.slots.len())
            .map(|i| self.plate_at(i, plate))
            .collect()
    }

    /// Number of plates.
    pub fn count(&self) -> usize {
        self.slots.len()
    }

    /// Index of the plate shown, counted from 0.
    pub fn current(&self) -> usize {
        self.current
    }

    /// A number that stays with the plate shown while it exists and
    /// differs from that of every other plate, to tell when another plate
    /// is shown.
    pub fn current_id(&self) -> u64 {
        self.slots[self.current].id
    }

    /// Number of models on every plate; `plate` is the current one.
    pub fn model_counts(&self, plate: &Plate) -> Vec<usize> {
        (0..self.slots.len())
            .map(|i| self.plate_at(i, plate).items.len())
            .collect()
    }

    /// The plates that are not shown, for changes that apply to every
    /// plate, such as moving models to the middle of a new bed.
    pub fn others_mut(&mut self) -> impl Iterator<Item = &mut Plate> {
        let current = self.current;
        self.slots
            .iter_mut()
            .enumerate()
            .filter(move |(i, _)| *i != current)
            .map(|(_, slot)| &mut slot.plate)
    }

    /// The meshes of plate `index`, each where it is placed; `plate` is the
    /// current one.
    pub fn meshes_of(&self, index: usize, plate: &Plate) -> Vec<Mesh> {
        if index < self.slots.len() {
            self.plate_at(index, plate).meshes()
        } else {
            Vec::new()
        }
    }

    /// Plate `index`, with `plate` standing in for the current one.
    fn plate_at<'a>(&'a self, index: usize, plate: &'a Plate) -> &'a Plate {
        if index == self.current {
            plate
        } else {
            &self.slots[index].plate
        }
    }

    /// Adds an empty plate after the last one without showing it and
    /// returns its index.
    pub fn push(&mut self) -> usize {
        self.slots.push(Slot {
            id: self.next_id,
            plate: Plate::default(),
            extra: None,
        });
        self.next_id += 1;
        self.slots.len() - 1
    }

    /// Shows plate `to`: the current plate and its `extra` go back into the
    /// list and `to` with its own comes out. Nothing happens for the
    /// current plate or an index past the end.
    pub fn switch(&mut self, to: usize, plate: &mut Plate, extra: &mut Option<S>) {
        if to == self.current || to >= self.slots.len() {
            return;
        }
        self.put_back(plate, extra);
        self.current = to;
        self.take_out(plate, extra);
    }

    /// Removes plate `index` with its models. The last plate cannot be
    /// removed. Removing the current plate shows the one that took its
    /// place, or the one before it at the end. Returns whether a plate was
    /// removed.
    pub fn remove(&mut self, index: usize, plate: &mut Plate, extra: &mut Option<S>) -> bool {
        if self.slots.len() < 2 || index >= self.slots.len() {
            return false;
        }
        self.put_back(plate, extra);
        self.slots.remove(index);
        if index < self.current || self.current == self.slots.len() {
            self.current -= 1;
        }
        self.take_out(plate, extra);
        true
    }

    /// Moves model `item` of the current plate to plate `to`, where it
    /// keeps its place and is selected. Returns false and changes nothing
    /// for the current plate or a model or plate that does not exist.
    pub fn move_item(&mut self, item: usize, to: usize, plate: &mut Plate) -> bool {
        if to == self.current || to >= self.slots.len() || item >= plate.items.len() {
            return false;
        }
        let moved = plate.items.remove(item);
        plate.selected = None;
        plate.serial += 1;
        let target = &mut self.slots[to].plate;
        target.items.push(moved);
        target.selected = Some(target.items.len() - 1);
        target.serial += 1;
        true
    }

    /// Records the state of all plates as a new step if it differs from
    /// the last one; `plate` is the current one. Call it when no change is
    /// in progress. Switching plates and selecting models is no step.
    pub fn settle(&mut self, plate: &Plate) {
        let same = self.recorded.plates.len() == self.slots.len()
            && self
                .recorded
                .plates
                .iter()
                .enumerate()
                .all(|(i, (id, old))| {
                    *id == self.slots[i].id && self.plate_at(i, plate).same_as(old)
                });
        if same {
            // The plate shown counts for the next undo.
            self.recorded.current = self.current;
            return;
        }
        let now = self.snapshot(plate);
        self.undo.push(std::mem::replace(&mut self.recorded, now));
        if self.undo.len() > UNDO_STEPS {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Whether there is a step to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is a step to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Takes all plates back one step; `plate` and `extra` are the current
    /// plate's. Shows the first plate the step changes, if any.
    pub fn undo(&mut self, plate: &mut Plate, extra: &mut Option<S>) {
        if let Some(previous) = self.undo.pop() {
            let now = std::mem::replace(&mut self.recorded, previous);
            self.redo.push(now);
            self.restore(plate, extra);
        }
    }

    /// Takes all plates forward one undone step, like [`Plates::undo`].
    pub fn redo(&mut self, plate: &mut Plate, extra: &mut Option<S>) {
        if let Some(next) = self.redo.pop() {
            let now = std::mem::replace(&mut self.recorded, next);
            self.undo.push(now);
            self.restore(plate, extra);
        }
    }

    /// The state of all plates now.
    fn snapshot(&self, plate: &Plate) -> Snapshot {
        Snapshot {
            plates: (0..self.slots.len())
                .map(|i| (self.slots[i].id, self.plate_at(i, plate).clone()))
                .collect(),
            current: self.current,
        }
    }

    /// Puts the recorded state on the plates. A plate that changes gets a
    /// higher serial, so that views and slices count it as changed, and
    /// keeps what the app stored for it; a plate that comes back has
    /// nothing stored.
    fn restore(&mut self, plate: &mut Plate, extra: &mut Option<S>) {
        self.put_back(plate, extra);
        let shown = self.slots[self.current].id;
        let mut old: Vec<Slot<S>> = std::mem::take(&mut self.slots);
        let mut changed = None;
        for (index, (id, recorded)) in self.recorded.plates.iter().enumerate() {
            let before = old
                .iter()
                .position(|slot| slot.id == *id)
                .map(|at| old.swap_remove(at));
            let slot = match before {
                Some(slot) if slot.plate.same_as(recorded) => slot,
                before => {
                    changed.get_or_insert(index);
                    let serial = before.as_ref().map_or(0, |slot| slot.plate.serial + 1);
                    let mut plate = recorded.clone();
                    plate.serial = plate.serial.max(serial);
                    if plate.selected.is_some_and(|i| i >= plate.items.len()) {
                        plate.selected = None;
                    }
                    Slot {
                        id: *id,
                        plate,
                        extra: before.and_then(|slot| slot.extra),
                    }
                }
            };
            self.slots.push(slot);
        }
        let still = self.slots.iter().position(|slot| slot.id == shown);
        self.current = changed
            .or(still)
            .unwrap_or(self.recorded.current)
            .min(self.slots.len().saturating_sub(1));
        self.recorded.current = self.current;
        self.take_out(plate, extra);
    }

    /// Stores the current plate and its extra in its slot.
    fn put_back(&mut self, plate: &mut Plate, extra: &mut Option<S>) {
        let slot = &mut self.slots[self.current];
        slot.plate = std::mem::take(plate);
        slot.extra = extra.take();
    }

    /// Hands out the current plate and its extra.
    fn take_out(&mut self, plate: &mut Plate, extra: &mut Option<S>) {
        let slot = &mut self.slots[self.current];
        *plate = std::mem::take(&mut slot.plate);
        *extra = slot.extra.take();
    }
}

/// Suggested name of the output file of a plate: like
/// [`crate::job::output_name`], with the plate number (counted from 1)
/// added when there are several plates.
pub fn output_name(model_name: &str, plate: usize, plates: usize, gcode_3mf: bool) -> String {
    let name = crate::job::output_name(model_name, gcode_3mf);
    if plates < 2 {
        return name;
    }
    let extension = if gcode_3mf { ".gcode.3mf" } else { ".gcode" };
    let stem = name.strip_suffix(extension).unwrap_or(&name);
    format!("{stem} plate {plate}{extension}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::plate::Model;
    use slicer_profiles::Settings;

    fn cube(size: f64) -> Model {
        Model::from_mesh("cube", Mesh::cuboid(size, size, size)).unwrap()
    }

    #[test]
    fn output_names_carry_the_plate() {
        assert_eq!(output_name("benchy.stl", 1, 1, false), "benchy.gcode");
        assert_eq!(
            output_name("benchy.stl", 2, 3, false),
            "benchy plate 2.gcode"
        );
        assert_eq!(
            output_name("Part.3mf", 1, 2, true),
            "Part plate 1.gcode.3mf"
        );
    }

    #[test]
    fn plates_keep_their_models_and_extras() {
        let settings = Settings::default();
        let mut plates: Plates<&str> = Plates::new();
        let (mut plate, mut extra) = (Plate::default(), Some("slice of plate 1"));
        plate.add(cube(10.0), &settings);
        assert_eq!(plates.count(), 1);

        let second = plates.push();
        assert_eq!(second, 1);
        assert_eq!(plates.current(), 0);
        let first_id = plates.current_id();
        plates.switch(second, &mut plate, &mut extra);
        assert_eq!(plates.current(), 1);
        assert_ne!(plates.current_id(), first_id);
        assert!(plate.items.is_empty());
        assert_eq!(extra, None);
        plate.add(cube(20.0), &settings);
        plate.add(cube(5.0), &settings);
        assert_eq!(plates.model_counts(&plate), [1, 2]);
        assert_eq!(plates.meshes_of(0, &plate).len(), 1);
        assert_eq!(plates.meshes_of(1, &plate).len(), 2);
        assert_eq!(plates.others_mut().count(), 1);

        plates.switch(0, &mut plate, &mut extra);
        assert_eq!(plate.items.len(), 1);
        assert_eq!(extra, Some("slice of plate 1"));
        // Switching to the plate shown or past the end does nothing.
        plates.switch(0, &mut plate, &mut extra);
        plates.switch(5, &mut plate, &mut extra);
        assert_eq!(plates.current(), 0);
        assert_eq!(extra, Some("slice of plate 1"));
    }

    #[test]
    fn removing_plates_shows_a_neighbour() {
        let settings = Settings::default();
        let mut plates: Plates<u32> = Plates::new();
        let (mut plate, mut extra) = (Plate::default(), None);
        // Plate i holds i + 1 models.
        for i in 0..3 {
            if i > 0 {
                let index = plates.push();
                plates.switch(index, &mut plate, &mut extra);
            }
            for _ in 0..=i {
                plate.add(cube(10.0), &settings);
            }
            extra = Some(i);
        }
        assert_eq!(plates.model_counts(&plate), [1, 2, 3]);
        // Removing the last plate while it is shown shows the one before.
        assert!(plates.remove(2, &mut plate, &mut extra));
        assert_eq!(plates.current(), 1);
        assert_eq!((plate.items.len(), extra), (2, Some(1)));
        // Removing one before the shown plate keeps showing it.
        assert!(plates.remove(0, &mut plate, &mut extra));
        assert_eq!(plates.current(), 0);
        assert_eq!((plate.items.len(), extra), (2, Some(1)));
        // The last plate stays.
        assert!(!plates.remove(0, &mut plate, &mut extra));
        assert!(!plates.remove(3, &mut plate, &mut extra));
        assert_eq!(plates.count(), 1);
    }

    #[test]
    fn models_move_between_plates_and_back_with_undo() {
        let settings = Settings::default();
        let mut plates: Plates<&str> = Plates::new();
        let (mut plate, mut extra) = (Plate::default(), Some("slice"));
        plates.settle(&plate);
        assert!(!plates.can_undo());
        plate.add(cube(10.0), &settings);
        plate.add(cube(20.0), &settings);
        plate.items[1].placement.center = [50.0, 60.0];
        plates.settle(&plate);
        let second = plates.push();
        plates.settle(&plate);

        let serial = plate.serial;
        assert!(plates.move_item(1, second, &mut plate));
        assert_eq!(plate.items.len(), 1);
        assert_eq!(plate.selected, None);
        assert!(plate.serial > serial);
        // Not to the plate it is on, and only models that exist.
        assert!(!plates.move_item(0, 0, &mut plate));
        assert!(!plates.move_item(4, second, &mut plate));
        plates.settle(&plate);
        assert_eq!(plates.model_counts(&plate), [1, 1]);

        // Have a look at the other plate: the model kept its place.
        plates.switch(second, &mut plate, &mut extra);
        assert_eq!(plate.items[0].placement.center, [50.0, 60.0]);
        assert_eq!(plate.selected, Some(0));
        plates.settle(&plate);
        assert!(plates.can_undo(), "switching is no step");

        // Undo puts the model back and shows the first plate that changed.
        plates.undo(&mut plate, &mut extra);
        assert_eq!(plates.current(), 0);
        assert_eq!(plates.model_counts(&plate), [2, 0]);
        assert!(plate.serial > serial + 1);
        // The first plate kept its (now old) slice.
        assert_eq!(extra, Some("slice"));
        // Redo moves it again.
        plates.redo(&mut plate, &mut extra);
        assert_eq!(plates.model_counts(&plate), [1, 1]);
        // Undoing the move and the added plate leaves one plate.
        plates.undo(&mut plate, &mut extra);
        plates.undo(&mut plate, &mut extra);
        assert_eq!(plates.count(), 1);
        assert_eq!(plates.current(), 0);
        assert_eq!(plate.items.len(), 2);
        // A new change drops what could be redone.
        plate.selected = Some(0);
        plate.remove_selected();
        plates.settle(&plate);
        assert!(!plates.can_redo());
    }

    #[test]
    fn undo_brings_back_a_removed_plate() {
        let settings = Settings::default();
        let mut plates: Plates<&str> = Plates::new();
        let (mut plate, mut extra) = (Plate::default(), None);
        plate.add(cube(10.0), &settings);
        let second = plates.push();
        plates.switch(second, &mut plate, &mut extra);
        plate.add(cube(20.0), &settings);
        plate.add(cube(20.0), &settings);
        extra = Some("slice of plate 2");
        plates.settle(&plate);
        // Back on the first plate, remove the second.
        plates.switch(0, &mut plate, &mut extra);
        assert!(plates.remove(1, &mut plate, &mut extra));
        plates.settle(&plate);
        assert_eq!(plates.count(), 1);

        plates.undo(&mut plate, &mut extra);
        assert_eq!(plates.count(), 2);
        // The plate that came back is shown, without its slice.
        assert_eq!(plates.current(), 1);
        assert_eq!(plate.items.len(), 2);
        assert_eq!(extra, None);
        plates.redo(&mut plate, &mut extra);
        assert_eq!(plates.count(), 1);
        assert_eq!(plates.current(), 0);
        assert_eq!(plate.items.len(), 1);
    }

    #[test]
    fn unchanged_plates_keep_their_serial() {
        let settings = Settings::default();
        let mut plates: Plates<()> = Plates::new();
        let (mut plate, mut extra) = (Plate::default(), None);
        plate.add(cube(10.0), &settings);
        plates.settle(&plate);
        let second = plates.push();
        plates.settle(&plate);
        plates.switch(second, &mut plate, &mut extra);
        plate.add(cube(10.0), &settings);
        plates.settle(&plate);
        let first_serial = plates.slots[0].plate.serial;
        // Undo changes only the second plate, which stays shown.
        plates.undo(&mut plate, &mut extra);
        assert_eq!(plates.current(), 1);
        assert!(plate.items.is_empty());
        assert_eq!(plates.slots[0].plate.serial, first_serial);
    }
}
