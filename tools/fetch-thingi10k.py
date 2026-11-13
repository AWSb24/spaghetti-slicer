#!/usr/bin/env python3
"""Fetches the Thingi10K models listed in corpus/thingi10k.tsv.

Each model is downloaded from the dataset at a fixed revision, checked
against its SHA-256 and written as a binary STL into corpus/thingi10k/.
Needs only the Python standard library. Files that are already there are
kept.
"""
import csv
import hashlib
import io
import pathlib
import struct
import sys
import urllib.request
import zipfile

REVISION = "2d5d3b2f3cd3711028ad75b12788c13b25559ec6"
SOURCE = f"https://huggingface.co/datasets/Thingi10K/Thingi10K/resolve/{REVISION}/npz"
ROOT = pathlib.Path(__file__).resolve().parent.parent / "corpus"


def array(package, name, kind, size):
    """Reads an N x 3 array of little-endian values from an .npy member."""
    data = package.read(name)
    if data[:6] != b"\x93NUMPY" or data[6] != 1:
        raise ValueError(f"{name}: not a version 1 .npy file")
    header_end = 10 + struct.unpack("<H", data[8:10])[0]
    header = data[10:header_end].decode("latin-1")
    if f"'descr': '{kind}'" not in header or "'fortran_order': False" not in header:
        raise ValueError(f"{name}: unexpected layout {header.strip()}")
    body = data[header_end:]
    count = len(body) // size
    values = struct.unpack(f"<{count}{'d' if kind == '<f8' else 'i'}", body)
    return [values[i : i + 3] for i in range(0, count, 3)]


def to_stl(npz):
    with zipfile.ZipFile(io.BytesIO(npz)) as package:
        vertices = array(package, "vertices.npy", "<f8", 8)
        facets = array(package, "facets.npy", "<i4", 4)
    out = bytearray(80) + struct.pack("<I", len(facets))
    for facet in facets:
        out += struct.pack("<3f", 0.0, 0.0, 0.0)
        for index in facet:
            out += struct.pack("<3f", *vertices[index])
        out += b"\0\0"
    return bytes(out)


def main():
    target = ROOT / "thingi10k"
    target.mkdir(exist_ok=True)
    with open(ROOT / "thingi10k.tsv", encoding="utf-8", newline="") as manifest:
        rows = list(csv.DictReader(manifest, delimiter="\t"))
    fetched = 0
    for row in rows:
        path = target / f"{row['file_id']}.stl"
        if path.exists():
            continue
        with urllib.request.urlopen(f"{SOURCE}/{row['file_id']}.npz", timeout=60) as response:
            npz = response.read()
        if hashlib.sha256(npz).hexdigest() != row["sha256"]:
            sys.exit(f"checksum mismatch for {row['file_id']}")
        path.write_bytes(to_stl(npz))
        fetched += 1
    print(f"{len(rows)} models in {target} ({fetched} downloaded)")


if __name__ == "__main__":
    main()
