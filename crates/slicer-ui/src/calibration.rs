//! Calibration prints on the plate: the test models of a [`Calibration`],
//! and the step that turns the G-code of their slice into the calibration
//! G-code.
//!
//! The test models stay ordinary models on the plate: they can be moved,
//! copied or deleted. Copies keep their test value; a model that is
//! changed (mirrored, cut, laid on a face) becomes a model of its own and
//! prints like any other.

use std::sync::Arc;

use slicer_core::{Diagnostic, Mesh};
use slicer_gcode::Output;
use slicer_gcode::calibration::Calibration;
use slicer_profiles::Settings;

use crate::plate::{Model, Plate, placed_mesh};

/// A calibration whose test models were put on the plate.
#[derive(Debug, Clone)]
pub struct PlateCalibration {
    calibration: Calibration,
    /// The test models in the order of [`Calibration::models`]; plate items
    /// showing them share them.
    models: Vec<Arc<Model>>,
}

impl PlateCalibration {
    /// Replaces everything on `plate` with the test models of
    /// `calibration`, arranged in the middle of the bed of `settings`.
    pub fn put_on(
        calibration: Calibration,
        plate: &mut Plate,
        settings: &Settings,
    ) -> Result<Self, Diagnostic> {
        plate.items.clear();
        let mut models = Vec::new();
        for (name, mesh) in calibration.placed_models(&calibration.settings(settings)) {
            let center = mesh
                .bounds()
                .map(|(lo, hi)| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]);
            plate.add(Model::from_mesh(name, mesh)?, settings);
            if let (Some(item), Some(center)) = (plate.items.last_mut(), center) {
                item.placement.center = center;
                models.push(Arc::clone(&item.model));
            }
        }
        plate.clear_selection();
        Ok(Self {
            calibration,
            models,
        })
    }

    /// The calibration.
    pub fn calibration(&self) -> &Calibration {
        &self.calibration
    }

    /// For a slice of `plate` with `settings`: the settings to slice with
    /// instead, and the step for its G-code. `None` if no test model is on
    /// the plate any more.
    pub fn prepare(&self, plate: &Plate, settings: &Settings) -> Option<(Settings, Finish)> {
        let objects: Vec<(usize, Mesh)> = plate
            .items
            .iter()
            .filter_map(|item| {
                let index = (self.models.iter()).position(|m| Arc::ptr_eq(m, &item.model))?;
                Some((index, placed_mesh(&item.model, &item.placement)))
            })
            .collect();
        if objects.is_empty() {
            return None;
        }
        let settings = self.calibration.settings(settings);
        let finish = Finish {
            calibration: self.calibration.clone(),
            objects,
            settings: settings.clone(),
        };
        Some((settings, finish))
    }
}

/// Turns the G-code of a slice of test models into calibration G-code.
/// Made by [`PlateCalibration::prepare`] when the slice starts, so later
/// changes to the plate do not matter.
#[derive(Debug, Clone)]
pub struct Finish {
    calibration: Calibration,
    /// The test models as sliced, with their index in
    /// [`Calibration::models`].
    objects: Vec<(usize, Mesh)>,
    /// The settings the slice was made with.
    settings: Settings,
}

impl Finish {
    /// Changes the G-code (see [`Calibration::apply`]).
    pub fn apply(&self, output: &mut Output) {
        let objects: Vec<(usize, &Mesh)> = self.objects.iter().map(|(i, m)| (*i, m)).collect();
        self.calibration.apply(output, &objects, &self.settings);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::AtomicBool;

    use slicer_fdm::Progress;
    use slicer_gcode::calibration::Kind;

    use super::*;
    use crate::job::{SliceRequest, slice};
    use crate::plate::PlateOwnSettings;

    #[test]
    fn test_models_replace_the_plate_and_slice_to_calibration_gcode() {
        let settings = Settings::default();
        let mut plate = Plate::default();
        plate.add(
            Model::from_mesh("cube", Mesh::cuboid(5.0, 5.0, 5.0)).unwrap(),
            &settings,
        );
        let serial = plate.serial;
        let calibration = Calibration::new(Kind::Temperature, 220.0, 210.0, 10.0).unwrap();
        let active = PlateCalibration::put_on(calibration.clone(), &mut plate, &settings).unwrap();
        assert_eq!(active.calibration(), &calibration);
        assert_eq!(plate.items.len(), 1);
        assert_eq!(plate.items[0].model.name, "Temperature tower");
        assert!(plate.serial > serial && plate.selected.is_none());
        assert!(plate.check(&settings).is_ok());

        let (sliced_with, finish) = active.prepare(&plate, &settings).unwrap();
        assert_eq!(sliced_with.nozzle_temp_first_layer, 220.0);
        let request = SliceRequest {
            objects: plate.meshes(),
            settings: sliced_with,
            own: PlateOwnSettings::default(),
        };
        let mut result = slice(&request, &AtomicBool::new(false), &Progress::new()).unwrap();
        assert!(!result.output.gcode.contains("M104 S210\n"));
        finish.apply(&mut result.output);
        assert!(result.output.gcode.contains("\nM104 S210\n"));

        // Without test models on the plate there is nothing to do.
        plate.selected = Some(0);
        plate.remove_selected();
        assert!(active.prepare(&plate, &settings).is_none());
    }

    #[test]
    fn copies_keep_their_value_and_changed_models_lose_it() {
        let settings = Settings::default();
        let mut plate = Plate::default();
        let calibration = Calibration::new(Kind::Flow, -5.0, 5.0, 5.0).unwrap();
        let active = PlateCalibration::put_on(calibration, &mut plate, &settings).unwrap();
        assert_eq!(plate.items.len(), 3);
        let names: Vec<&str> = plate.items.iter().map(|i| i.model.name.as_str()).collect();
        assert_eq!(names, ["Flow -5 %", "Flow +0 %", "Flow +5 %"]);
        // Placed as arranged, left to right.
        let xs: Vec<f64> = plate.items.iter().map(|i| i.placement.center[0]).collect();
        assert!(xs[0] < xs[1] && xs[1] < xs[2], "{xs:?}");

        let copy = plate.items[2].clone();
        plate.add_copy(&copy, &settings);
        let mirrored = plate.items[0].model.mirrored(0, 0.0);
        plate.set_model(0, mirrored);
        let (_, finish) = active.prepare(&plate, &settings).unwrap();
        let indices: Vec<usize> = finish.objects.iter().map(|(i, _)| *i).collect();
        assert_eq!(indices, [1, 2, 2]);
    }
}
