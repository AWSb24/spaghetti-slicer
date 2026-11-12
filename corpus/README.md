# Real-world test models

The models themselves are not stored in this repository (about 55 MB). Fetch them with:

```bash
bash tools/fetch-corpus.sh
```

Then run the tests that use them:

```bash
cargo test -p slicer-cli --test corpus -- --ignored
```

## 3DBenchy

| | |
|---|---|
| Author | Creative Tools |
| License | CC0 1.0 Universal (public domain), as stated on https://www.3dbenchy.com/license/ (checked 2026-11-12) |
| Source | https://github.com/CreativeTools/3DBenchy at commit `b542785f9642ab39db8a778a0ccc934b3ebd829a` |
| Files | 21 STL files: the complete boat, the 17-shell multi-part version, two dual-print halves and 17 single parts |

The repository itself carries no license file; the license statement comes from the author's website. File names are shortened on fetch; the mapping and full SHA-256 checksums are in [3dbenchy.tsv](3dbenchy.tsv).

| File | Size (KB) | SHA-256 |
|---|---|---|
| `3dbenchy.stl` | 11021 | `6ab57f1c3f8e86bc…` |
| `dualprint_gunwale_deck_plate_wheel_frames_roof_chimney_top.stl` | 6005 | `2e24ec47375b7670…` |
| `dualprint_hull_box_bridge_walls_rod_holder_chimney.stl` | 6814 | `5b7b86c2521608c9…` |
| `multi_part_complete_17_shells.stl` | 16144 | `f034e295a9316ed4…` |
| `multi_part_single_bridge_roof.stl` | 1199 | `9a5e4a1dee5c32b9…` |
| `multi_part_single_bridge_walls.stl` | 170 | `f459d2468f107aa9…` |
| `multi_part_single_cargo_box.stl` | 18 | `262989714519c019…` |
| `multi_part_single_chimney_body.stl` | 58 | `c17c7905a8d405d4…` |
| `multi_part_single_chimney_top.stl` | 783 | `c70d7fa5b82f237c…` |
| `multi_part_single_deck_surface.stl` | 228 | `ba8f11d078385dfa…` |
| `multi_part_single_doorframe_port.stl` | 35 | `ce4a01732552ba8b…` |
| `multi_part_single_doorframe_starboard.stl` | 35 | `d37674c3abc224b2…` |
| `multi_part_single_fishing_rod_holder.stl` | 441 | `884ef5902d72709b…` |
| `multi_part_single_gunwale.stl` | 2709 | `d6439d063a20708d…` |
| `multi_part_single_hawsepipe_port.stl` | 190 | `6f143cffa0b1e9cc…` |
| `multi_part_single_hawsepipe_starboard.stl` | 180 | `ccebc7327d127708…` |
| `multi_part_single_hull.stl` | 9117 | `6cc572a7fcfcf771…` |
| `multi_part_single_stern_name_plate.stl` | 484 | `c1fd99127bae8e20…` |
| `multi_part_single_stern_window.stl` | 68 | `edb429f92294de24…` |
| `multi_part_single_wheel.stl` | 73 | `d229ff677f3e5ac8…` |
| `multi_part_single_window.stl` | 22 | `60adae0cd5bc4645…` |

## Private models

STL and 3MF files placed directly in this folder are ignored by git. They are sliced by a separate test, on a 256 x 256 mm bed; models that do not fit are skipped:

```bash
cargo test --release -p slicer-cli --test corpus local_models -- --ignored --nocapture
```

## Adding models

Only add models whose license is stated by the author and allows redistribution and use for testing (CC0, CC BY, MIT or similar). Record author, license, where the license is stated, the source at a fixed revision and checksums. See [CONTRIBUTING.md](../CONTRIBUTING.md).