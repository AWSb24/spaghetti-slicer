# Entscheidung 0002: Referenz für die Oberfläche

| | |
|---|---|
| Status | Entschieden |
| Datum | 2026-11-22 |
| Ticket | – (betrifft alle Arbeiten an `slicer-ui`) |

## Entscheidung

1. **Kein fremder Quelltext.** Für die Oberfläche wird kein Quelltext von Orca Slicer,
   Bambu Studio, PrusaSlicer oder einem anderen Slicer gelesen, kopiert, übersetzt oder
   nachgebaut. Es gilt die Clean-Room-Regel aus [CONTRIBUTING.md](../../CONTRIBUTING.md).
2. **Referenz ist das Verhalten, nicht die Umsetzung.** Beobachtet werden die Programme
   als Nutzer: Bildschirmfotos, Arbeitsabläufe, Beschriftungen und Maße. Dazu kommen
   Nutzerdokumentation, Dateiformat-Spezifikationen (STL, 3MF), Firmware-Dokumentation
   und veröffentlichte Arbeiten.
3. **Eigene Gestaltung.** Farben werden an Bildschirmfotos gemessen und in
   `crates/slicer-ui/src/theme.rs` als eigene Werte geführt. Symbole zeichnet
   `crates/slicer-ui/src/icons.rs` selbst. Layout, Wortwahl und Bedienung sind eigene
   Arbeit.
4. **Nur das Ergebnis zählt.** Ein Arbeitsablauf darf dem bekannter Slicer entsprechen
   (Prepare/Preview, Objektliste mit Teilen, Prozessreiter, Vorschau mit Schichtregler).
   Die Umsetzung dahinter ist die dieses Projekts. Ein Test darf fremdes Verhalten
   nachbilden, aber nicht aus fremdem Quelltext oder fremden Testdaten stammen.
5. **Profile bleiben Daten.** Das Importieren von Orca-Slicer-Profilen liest die vom
   Nutzer gespeicherten Dateien als Daten; es liest keinen Quelltext und keine
   mitgelieferten Profile (siehe `slicer-profiles`).
6. **Bei Widerspruch** gilt [CONTRIBUTING.md](../../CONTRIBUTING.md). Eine Aufgabe, die
   fremden Quelltext verlangt, wird nicht umgesetzt; stattdessen wird die gewünschte
   Bedienung beschrieben und hier neu entworfen. Eine Aufgabe, die den Quelltext
   wirklich braucht, erfordert zuerst eine neue Entscheidung und einen Lizenzwechsel
   des ganzen Projekts.

## Begründung

- **Lizenz:** Orca Slicer und die meisten anderen Slicer stehen unter AGPL-3.0. Eine
  Übersetzung oder enge Anlehnung an ihren Quelltext wäre ein abgeleitetes Werk und
  könnte nicht unter MIT OR Apache-2.0 verbreitet werden.
- **Herkunft:** Die Herkunftstabelle in [CONTRIBUTING.md](../../CONTRIBUTING.md) hält für
  jede nichttriviale Stelle die Quelle fest. Diese Entscheidung hält die Oberfläche auf
  derselben Grundlage.
- **Es reicht:** Für Bedienung, Layout und Farben genügt das beobachtbare Verhalten. Die
  vorhandene Oberfläche (`slicer-ui`) ist so entstanden; `theme.rs` nennt die
  Bildschirmfotos als Quelle der Farbwerte.
- **Eigene Symbole:** Fremde Symbole sind urheberrechtlich geschützt, auch wenn der
  Quelltext frei wäre. Eigene Zeichnungen vermeiden die Frage.

## Risiken und offene Punkte

- **Mehr Arbeit:** Was in fremdem Quelltext nachgelesen werden könnte, muss hier
  entworfen, getestet und gegen das Verhalten geprüft werden. Das kostet Zeit.
- **Unbekanntes Verhalten:** Manches ist nur im Quelltext dokumentiert. Dann entscheidet
  das Projekt nach eigenen Tests und Bedürfnissen, nicht nach dem Vorbild.
- **Keine vollständige Prüfung:** Ob eine Änderung „zu nah“ an einer fremden Umsetzung
  liegt, lässt sich nicht automatisch prüfen. Die Regel gilt für Menschen und für
  KI-Assistenten gleichermaßen; im Zweifel wird neu entworfen.
- **Spätere Änderung:** Nur durch eine neue Entscheidung, mit Zustimmung der
  Rechteinhaber und einem Lizenzwechsel. Die bisherige Herkunftsdokumentation bleibt
  dann trotzdem gültig.
