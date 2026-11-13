"""Resolves the 'inherits' chain of Orca Slicer's shipped profiles into full files."""
import json
import pathlib
import sys

root = pathlib.Path(r"C:\Program Files\OrcaSlicer\resources\profiles\BBL")
out = pathlib.Path(sys.argv[1])


def load(kind, name):
    data = json.loads((root / kind / f"{name}.json").read_text(encoding="utf-8"))
    parent = data.get("inherits")
    if parent:
        merged = load(kind, parent)
        merged.update(data)
        data = merged
    return data


for kind, name, target in [
    ("machine", "Bambu Lab X1 Carbon 0.4 nozzle", "machine.json"),
    ("process", "0.20mm Standard @BBL X1C", "process.json"),
    ("filament", "Bambu PLA Basic @BBL X1C", "filament.json"),
]:
    data = load(kind, name)
    data.pop("inherits", None)
    data["from"] = "system"
    (out / target).write_text(json.dumps(data, indent=1), encoding="utf-8")
    print(target, len(data), "keys")
