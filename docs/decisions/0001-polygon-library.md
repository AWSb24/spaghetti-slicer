# Entscheidung 0001: Polygon-Bibliothek und Koordinatenraster

| | |
|---|---|
| Status | Entschieden |
| Datum | 2026-11-11 |
| Ticket | T-003, beantwortet Q2 im [PRD](../../PRD.md) |

## Entscheidung

1. **Bibliothek:** `i_overlay` bleibt die Bibliothek für boolesche Operationen und Offsets.
2. **Koordinaten:** Die öffentliche API des Kerns rechnet in `f64`-Millimetern. Alle Ergebnisse von Polygon-Operationen liegen auf einem festen Raster von **10 nm** (`1e-5` mm).
3. **Kapselung:** Nur `crates/slicer-core/src/polygon.rs` ruft die Bibliothek auf. Ein späterer Wechsel betrifft eine Datei.
4. **Wiedervorlage:** `clipper2-rs` wird neu bewertet, sobald das Projekt mindestens sechs Monate alt ist oder die Polygon-Operationen im Profil als Engpass auftauchen.

## Kandidaten

| Crate | Version | Lizenz | Art | Auf crates.io seit | Downloads | Im Test |
|---|---|---|---|---|---|---|
| `i_overlay` | 9.1.0 | MIT OR Apache-2.0 | Eigene Implementierung, reines Rust | August 2023, 132 Versionen | ca. 11,9 Mio. | Ja |
| `clipper2-rust` | 1.2.0 | BSL-1.0 | Rust-Portierung von Clipper2 | Februar 2026, 6 Versionen | ca. 252.000 | Ja |
| `clipper2-rs` | 0.3.0 | BSL-1.0 | Rust-Portierung von Clipper2, laut Autor bitgleich gegen das C++-Original geprüft | 30. Oktober 2026, 3 Versionen | ca. 1.700 | Ja |
| `clipper2` | 0.6.0 | MIT OR Apache-2.0 | Bindung an die C++-Bibliothek | – | – | Nein: kein reines Rust |
| `geo` | 0.33.1 | MIT OR Apache-2.0 | Nutzt für boolesche Operationen selbst `i_overlay` | – | – | Nein: kein eigener Kandidat |
| `cavalier_contours` | 0.9.0 | MIT OR Apache-2.0 | Offsets für Polylinien mit Bögen | – | – | Nein: anderer Schwerpunkt, nicht geprüft |
| Eigenbau | – | – | – | – | – | Nein: nicht nötig, siehe Ergebnis |

Zahlen von crates.io, Stand 2026-11-11. Alle getesteten Lizenzen sind nach [CONTRIBUTING.md](../../CONTRIBUTING.md) zulässig.

## Messung

Das Vergleichsprogramm liegt in [`tools/polyeval`](../../tools/polyeval/src/main.rs). Gemessen auf einem Windows-Rechner, Release-Build, bester von drei Läufen. Runde Ecken mit 2 µm Sehnenabweichung in allen Bibliotheken.

### Robustheit

26 Einzelfälle mit bekanntem oder gegenseitig verglichenem Ergebnis: Schrumpfen bis zum Verschwinden, Streifen schmaler als eine Linienbreite, Löcher, gemeinsame Kanten, Berührung an einer Ecke, identische Operanden, Selbstüberschneidung, doppelte und kollineare Punkte, Spitzen ohne Fläche, entartete Konturen, Überlappung und Lücke von 0,1 nm, Aufspaltung einer Engstelle, Geometrie bei 1900 mm, 0,01 mm kleines Quadrat, Stern mit 500 nadelförmigen Zacken, leere Eingaben.

| | `i_overlay` | `clipper2-rust` | `clipper2-rs` |
|---|---|---|---|
| Einzelfälle außerhalb der Toleranz | 0 | 0 | 0 |
| Abstürze oder Fehler | 0 | 0 | 0 |
| Zufallstest: 400 selbstüberschneidende Polygone, je bereinigen, schrumpfen, wachsen, schneiden – Fehler | 0 | 0 | 0 |
| Zufallstest – verletzte Invarianten (Fläche wächst, wo sie nicht darf) | 0 | 0 | 0 |
| Größte Flächenabweichung zwischen den Bibliotheken nach Bereinigung | 0,00004 mm² | Referenz | Referenz |

Unterschied im Detail: Die Clipper-Portierungen lassen nach einer Vereinigung kollineare Zwischenpunkte stehen (6 statt 4 Punkte beim Rechteck aus zwei Quadraten), `i_overlay` entfernt sie.

### Geschwindigkeit

| Arbeitslast | `i_overlay` | `clipper2-rust` | `clipper2-rs` |
|---|---|---|---|
| 200 Schichten eines Rings: je 5 Offsets, 9 Schnitte, 1 Differenz | 645 ms | 752 ms | 337 ms |
| Kontur mit 20.000 Punkten: 5 × Offset, Schnitt, Differenz | 755 ms | 2266 ms | 646 ms |

`clipper2-rs` ist bei der schichttypischen Last etwa 1,9-mal so schnell wie `i_overlay`. Bei der großen Kontur liegen beide gleichauf, `clipper2-rust` ist dreimal langsamer.

### Raster

| Betriebsart von `i_overlay` | Ergebnis verschiebt sich, wenn sich nur die Umgebung ändert | 200 Schichten |
|---|---|---|
| Raster aus der Bounding Box (bisher) | bis 6 · 10⁻⁸ mm | 645 ms |
| Festes Raster 10 nm, 32-Bit | bis 9 · 10⁻¹⁴ mm (Gleitkomma-Rauschen) | 644 ms |
| Festes Raster 1 nm, 64-Bit | bis 3 · 10⁻¹⁴ mm | 825 ms |

Die Clipper-Portierungen rechnen von Haus aus auf einem festen Ganzzahlraster und liefern bitgleiche Ergebnisse.

## Begründung

- **Robustheit entscheidet nicht:** Alle drei Bibliotheken bestehen jeden Test. Ein Eigenbau ist damit nicht zu rechtfertigen.
- **Reife spricht für `i_overlay`:** Drei Jahre auf crates.io, über hundert Versionen, mit Abstand die größte Verbreitung. `clipper2-rs` ist zwölf Tage alt; Version 0.3.0 wurde einen Tag nach Erscheinen zurückgezogen und wieder freigegeben. Darauf baut man kein Fundament.
- **Geschwindigkeit spricht für `clipper2-rs`, reicht aber nicht:** Faktor 1,9 ist spürbar, im Moment aber kein Engpass. Ein 20-mm-Würfel mit 100 Schichten wird insgesamt in etwa 30 ms gesliced.
- **`clipper2-rust` hat keinen Vorteil:** langsamer als beide anderen und jünger als `i_overlay`.
- **API-Passung:** `i_overlay` liefert Außenkonturen mit ihren Löchern gruppiert, was der Kern überall braucht. Bei Clipper müsste die Zuordnung über den PolyTree oder einen zusätzlichen Durchlauf hergestellt werden.
- **Lizenzgleichheit:** `i_overlay` steht unter derselben Doppellizenz wie das Projekt.

### Raster

- **Festes statt angepasstes Raster:** Beim bisherigen Verfahren hing die Rundung von der Bounding Box jeder einzelnen Operation ab. Dieselbe Kontur konnte so in zwei Operationen leicht unterschiedlich gerundet werden. Das feste Raster kostet keine messbare Zeit.
- **10 nm in 32 Bit statt 1 nm in 64 Bit:** 10 nm liegt weit unter jeder Druckerauflösung, deckt mehrere Meter Bauraum ab und ist 28 % schneller als die 64-Bit-Variante.
- **Nachrunden:** Nach jeder Operation werden die Koordinaten auf das Raster gerundet. Damit verschwindet das Gleitkomma-Rauschen, und Ergebnisse sind unabhängig von der Lage auf dem Bett bitgleich. Ein Test in `polygon.rs` sichert das ab.
- **Ausweichpfad:** Geometrie, die für das feste Raster zu weit vom Ursprung liegt, fällt auf das angepasste Raster zurück, statt einen Fehler zu erzeugen. Ebenfalls durch einen Test abgedeckt.

## Risiken und offene Punkte

- **API-Wechsel bei `i_overlay`:** Neun Hauptversionen in drei Jahren. `Cargo.lock` hält die Version fest; Aktualisierungen brauchen jeweils eine Prüfung mit `tools/polyeval`.
- **Ein Maintainer:** Gilt für alle drei Kandidaten.
- **Nicht geprüft:** Verhalten mit realen Modellen (Benchy, organische Formen), offene Linienzüge gegen Flächen schneiden (für spätere Füllmuster nötig), Speicherbedarf, Plattformgleichheit auf Linux und macOS. Die Messung ersetzt nicht den Testkorpus aus T-007.
- **Nur ein Rechner, eine Messreihe:** Die Zeiten sind Größenordnungen, keine Benchmarks mit statistischer Absicherung.
