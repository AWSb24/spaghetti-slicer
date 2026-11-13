# Profiles

Settings files to layer with `-c`, in the order printer, material, process:

```bash
spaghetti-slicer slice model.stl -c profiles/printers/generic-220.toml -c profiles/materials/pla.toml -c profiles/processes/standard-0.20.toml
```

Each file only sets the keys that belong to its role; everything else keeps its default (`spaghetti-slicer default-config` prints all keys). A test loads every file here and every combination of printer, material and process.

| File | Status |
|---|---|
| `printers/generic-220.toml` | Generic Marlin printer with a 220 x 220 mm bed. Not run on a printer. |
| `printers/bambu-x1c.toml` | Bambu Lab X1C. **Never run on the printer.** The start and end sequence uses only common commands and was written without knowing what the firmware needs; watch the first print and keep a hand on the power switch. |
| `materials/pla.toml`, `materials/petg.toml` | Typical temperatures from filament makers' data sheets; adjust to your filament. |
| `processes/*.toml` | Layer heights 0.12, 0.20 and 0.28 mm for a 0.4 mm nozzle. |

All values were chosen for this project from manufacturers' public specifications and general practice. Nothing was copied from the profiles of other slicers.
