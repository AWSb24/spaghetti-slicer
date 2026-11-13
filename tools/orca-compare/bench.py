"""Times Orca Slicer and Spaghetti Slicer on the same models; wall clock and peak memory."""
import ctypes
import ctypes.wintypes as wt
import pathlib
import re
import statistics
import subprocess
import sys
import time

here = pathlib.Path(__file__).resolve().parent
orca = r"C:\Program Files\OrcaSlicer\orca-slicer.exe"
ours = sys.argv[1]
models = sys.argv[2:]


class Counters(ctypes.Structure):
    _fields_ = [("cb", wt.DWORD), ("PageFaultCount", wt.DWORD)] + [
        (name, ctypes.c_size_t)
        for name in [
            "PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage", "QuotaPagedPoolUsage",
            "QuotaPeakNonPagedPoolUsage", "QuotaNonPagedPoolUsage", "PagefileUsage", "PeakPagefileUsage",
        ]
    ]


def run(command):
    started = time.perf_counter()
    process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    code = process.wait()
    seconds = time.perf_counter() - started
    counters = Counters()
    counters.cb = ctypes.sizeof(counters)
    ctypes.windll.psapi.GetProcessMemoryInfo(wt.HANDLE(int(process._handle)), ctypes.byref(counters), counters.cb)
    return code, seconds, counters.PeakWorkingSetSize / 2**20


OURS = [
    "bed_size_x=256", "bed_size_y=256", "layer_height=0.2", "first_layer_height=0.2", "line_width=0.42",
    "wall_loops=2", "top_layers=5", "bottom_layers=3", "infill_density=15", "infill_pattern=grid",
    "skirt_loops=0", "filament_density=1.26", "flow_ratio=0.98", "outer_wall_speed=200",
    "inner_wall_speed=300", "infill_speed=270", "first_layer_speed=50", "travel_speed=500",
    "acceleration=10000", "jerk=9",
]


def stats(path, patterns):
    text = pathlib.Path(path).read_text(encoding="utf-8", errors="replace")
    return [m.group(1) if (m := re.search(p, text)) else "?" for p in patterns]


print("| Model | Slicer | Time [s] (median of 5) | Peak memory [MB] | Layers | Filament [g] | Print time | G-code [MB] |")
print("|---|---|---|---|---|---|---|---|")
for model in models:
    name = pathlib.Path(model).name
    out = here / "out"
    commands = {
        "Orca 2.3.1": [orca, "--slice", "0", "--load-settings", f"{here}\\machine.json;{here}\\process.json",
                       "--load-filaments", f"{here}\\filament.json", "--outputdir", str(out), model],
        "Spaghetti": [ours, "slice", model, "-o", str(here / "ours.gcode")] + [x for s in OURS for x in ("--set", s)],
    }
    for label, command in commands.items():
        runs = [run(command) for _ in range(5)]
        if any(code != 0 for code, _, _ in runs):
            print(f"| {name} | {label} | failed (exit {runs[0][0]}) | | | | | |")
            continue
        seconds = statistics.median(r[1] for r in runs)
        memory = max(r[2] for r in runs)
        if label.startswith("Orca"):
            path = out / "plate_1.gcode"
            layers, grams, duration = stats(path, [r"total layer number: (\d+)", r"filament used \[g\] = ([\d.]+)", r"model printing time: ([^;]+);"])
        else:
            path = here / "ours.gcode"
            layers, grams, secs = stats(path, [r"; layers: (\d+)", r"filament used \[g\]: ([\d.]+)", r"printing time \[s\]: (\d+)"])
            duration = f"{int(secs) // 60}m {int(secs) % 60}s" if secs != "?" else "?"
        size = path.stat().st_size / 2**20
        print(f"| {name} | {label} | {seconds:.2f} | {memory:.0f} | {layers} | {grams} | {duration} | {size:.1f} |")
