# Comparison with Orca Slicer

Measured on 2026-11-13 on one Windows 11 machine (16 logical cores), with Orca Slicer 2.3.1 as installed and Spaghetti Slicer at commit `f1452a8`. Orca was only run as a program; its source code was not read.

## Method

- Orca was started from the command line with its own shipped profiles for the Bambu Lab X1 Carbon (0.4 mm nozzle), "0.20mm Standard" and "Bambu PLA Basic". `tools/orca-compare/flatten.py` resolves the `inherits` chain of these profiles into complete files, which the command line needs.
- Spaghetti Slicer was given the values Orca reported in its G-code where a matching setting exists: 0.2 mm layers, 0.42 mm lines, 2 walls, 5 top and 3 bottom layers, 15 % infill, the same speeds, density and flow ratio. Not matched: Orca fills with its "crosshatch" pattern, uses different line widths per feature and a 0.5 mm first layer line; Spaghetti Slicer used its grid pattern and one line width.
- Each program sliced each model five times. The table shows the median wall-clock time of the whole process, including start-up and writing the file, and the largest peak working set. `tools/orca-compare/bench.py` does the runs.

## Result

| Model | Slicer | Time [s] | Peak memory [MB] | Layers | Filament [g] | Print time (own estimate) | G-code [MB] |
|---|---|---|---|---|---|---|---|
| cube_20mm.stl | Orca | 0.33 | 90 | 100 | 3.93 | 8m 51s | 0.3 |
| cube_20mm.stl | Spaghetti | 0.02 | 8 | 100 | 3.38 | 4m 56s | 0.2 |
| gear_with_hub.stl | Orca | 0.39 | 100 | 50 | 3.82 | 8m 14s | 0.5 |
| gear_with_hub.stl | Spaghetti | 0.05 | 19 | 50 | 3.26 | 6m 2s | 0.7 |
| tube_24mm.stl | Orca | 0.50 | 102 | 50 | 2.35 | 5m 0s | 0.3 |
| tube_24mm.stl | Spaghetti | 0.10 | 23 | 50 | 1.97 | 3m 38s | 1.5 |
| 3dbenchy.stl | Orca | 3.11 | 224 | 240 | 11.61 | 30m 13s | 2.6 |
| 3dbenchy.stl | Spaghetti | 1.73 | 121 | 240 | 10.56 | 21m 40s | 18.3 |
| 32770.stl (Octocat, Thingi10K) | Orca | 1.02 | 185 | 168 | 11.59 | 28m 2s | 3.3 |
| 32770.stl (Octocat, Thingi10K) | Spaghetti | 0.33 | 48 | 168 | 10.30 | 19m 31s | 5.8 |

## What the numbers say

- **Speed and memory:** Spaghetti Slicer is faster and needs less memory on all five models, by a factor of 1.8 on the Benchy and more on small models, where Orca's start-up dominates. Orca does more per model (supports check, overhang handling, arc fitting, cooling logic), so this is not a like-for-like race.
- **Layers:** identical on every model.
- **Filament:** Spaghetti Slicer reports 9 to 16 % less. For the cube the G-code of both was added up per feature: Orca's total contains 107 mm of filament for the purge line of its start sequence, which Spaghetti Slicer does not have. On the print moves alone the difference is 6 % (1115 mm against 1189 mm): Orca puts more into sparse infill (other pattern, wider first layer), Spaghetti Slicer slightly more into walls. No sign of a gross error in either.
- **Print time:** not comparable. Both are estimates from different motion models, and Orca's includes more of what the printer does.

## Problems this showed in Spaghetti Slicer

1. **Far too many retractions.** On the 20 mm cube Spaghetti Slicer retracts about 780 times in sparse infill alone, Orca about 90 times. Every move between two infill lines longer than 1.5 mm retracts, although it never leaves the inside of the part.
2. **G-code is several times larger on curved models** (18.3 MB against 2.6 MB for the Benchy). Curves are written as very many short segments; Orca thins them out to a resolution of 0.012 mm and fits arcs. Files this dense can make a printer stutter.
