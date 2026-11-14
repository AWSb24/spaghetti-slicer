# Ticket-Breakdown: Phase 0–1 (Headless-FDM-MVP)

Grundlage: [PRD.md](PRD.md), Requirements R1–R13. Referenzdrucker: Bambu Lab X1C.

**Größen** (Schätzung für eine Person, ungeprüft): **S** = bis 1 Tag, **M** = 2–4 Tage, **L** = 1–2 Wochen.
**Typen:** Spike = zeitlich begrenzte Klärung mit Entscheidung als Ergebnis; Task = Umsetzung.

Summe: 40 Tickets (7 × S, 25 × M, 8 × L), also rechnerisch etwa 5–8 Monate Vollzeit-Äquivalent, ohne Puffer für Nacharbeit. Das ist eine Größenordnung, keine Zusage.

## Stand (2026-11-13)

| Ticket | Stand |
|---|---|
| T-001 | Erledigt |
| T-002 | Erledigt: MIT oder Apache-2.0, Regeln in `CONTRIBUTING.md` |
| T-003 | Erledigt: `i_overlay` bestätigt, festes 10-nm-Raster eingeführt, siehe `docs/decisions/0001-polygon-library.md` |
| T-004 | Offen, nur am Gerät möglich |
| T-005 | Erledigt: CI auf Linux, Windows, macOS; erster Lauf grün |
| T-006 | Erledigt: einheitliche Diagnosen mit stabilem Code, Meldung und Kontext; Warnungen werden gesammelt und als JSON ausgegeben |
| T-007 | Erledigt: sechs Referenzmodelle in `examples/`, 37 erzeugte Modelle als Korpus-Test, defekte Dateien als Test, 21 Dateien der Benchy-Familie (CC0) und 200 Modelle aus Thingi10K (CC BY, CC BY-SA, gemeinfrei; 160 saubere, 40 mit offenen oder nicht-mannigfaltigen Netzen) per Abrufskript mit Prüfsummen. 199 der 200 werden gesliced, eines ist flacher als eine Schicht. Die Lizenzen stammen aus den Metadaten des Datensatzes und sind nicht einzeln bei Thingiverse nachgeprüft |
| T-046 | Weitgehend erledigt: CI vergleicht den G-Code der sechs Referenzmodelle und des Benchy zwischen den drei Plattformen; Sollwerte (Größe, Zeilenzahl, Prüfsumme) liegen in `examples/golden.tsv` und werden bei jedem Testlauf geprüft, Aktualisierung mit `UPDATE_GOLDEN=1`. Die Sollwerte zeigen nur, dass sich die Ausgabe geändert hat, nicht wo |
| T-010, T-011 | Erledigt |
| T-012 | Erledigt: Geometrie, Einheiten, Komponenten, Build-Items und Transformationen; Verweise auf andere Modellteile im Paket (Production-Erweiterung); übrige Erweiterungen als Warnung. Lokal mit 16 Dateien aus Fusion 360 und Bambu Studio geprüft (private Modelle, nicht im Repository). Bei Bambu-Projekten mit mehreren Druckplatten wird eine Platte gewählt (`--plate`, Server-Feld `plate`); Farben und Einstellungen aus der Datei werden ignoriert |
| T-014 | Weitgehend erledigt: Fuzz-Ziele für STL und 3MF in `fuzz/` (cargo-fuzz), wöchentlicher und manuell startbarer Lauf `fuzz.yml`, Abstürze werden als Artefakt aufbewahrt; dazu Tests mit 7000 reproduzierbar beschädigten Dateien bei jedem Testlauf. Bisher kein Absturz gefunden. Das 3MF-Ziel bekommt rohe Bytes ohne gültige Startdatei und kommt deshalb selten über die ZIP-Prüfung hinaus |
| T-013 | Weitgehend erledigt: erkannt und mit Art und Anzahl gemeldet werden offene Kanten, Kanten mit mehr als zwei Dreiecken, nach innen gedrehte Modelle und nicht schließende Konturen; Löcher mit bis zu 64 Randkanten werden vor dem Schneiden geschlossen (Fächer aus Dreiecken, bei gebogenen oder einspringenden Löchern nur grob richtig). Offen: einzelne umgedrehte Flächen ausrichten |
| T-020 bis T-023, T-030 bis T-036 | Umgesetzt mit Lücken: Bahngeometrie je Schicht und das Zusammensetzen der Schnittlinien parallel auf allen Kernen; Umrisse werden nach dem Schnitt auf 0,005 mm ausgedünnt; Zacken ohne Breite aus der Versatz-Operation werden entfernt; Wandreihenfolge wählbar (`wall_order`); Naht wählbar hinten ausgerichtet oder am nächsten Punkt (`seam_position`), keine Naht an Ecken oder frei gesetzt; Wände werden auf 0,0125 mm ausgedünnt; Leerwege zum nächsten Start, Retraction nur wenn der Weg den Umriss der Schicht verlässt oder zu einer Außenwand führt; Infill-Winkel fest bei 45°/135°, weil frei wählbare Winkel Winkelfunktionen brauchen, die nicht auf jeder Plattform bitgleich rechnen. Überhänge und Brücken ohne Stütze werden als Warnung gemeldet (`slicing.unsupported_overhangs`); einfache Stützen (gerade Säulen aus Linien mit Rand, `supports = true`, Vorgriff auf Phase 3) mit dichter Kontaktschicht (`support_interface_layers`) und einer leeren Schicht zum Modell nach oben und unten, der Skirt umschließt sie; ohne Baumstützen. Brücken (massive Flächen ohne Material darunter, mindestens eine Linie breit) bekommen Linien in der Richtung, in der die meisten beidseitig aufliegen, gewählt aus vier Richtungen (X, Y, beide Diagonalen), mit zwei Linienbreiten Auflage; mit eigener Geschwindigkeit (`bridge_speed`) und eigener Bahnart im G-Code (`;TYPE:Bridge`), ohne eigene Flussrate und Kühlung, und mit Stützen werden sie weiterhin gestützt |
| T-041, T-043, T-050 | Umgesetzt: relative und absolute Extrusion (`extrusion_mode`), Kommentare zum Linientyp, Kopf mit Version und Einstellungen, Platzhalter mit Fehlermeldung. Marlin und Klipper erzeugen denselben G-Code, weil die verwendeten Befehle in beiden gleich sind; nicht an Hardware verifiziert |
| T-051, T-052, T-053 | Erledigt: Server mit Health, Konfiguration über Umgebungsvariablen, JSON-Logs, Auftrags-API unter `/v1/`, begrenzte Parallelität, Verfall, Token und Upload-Limit. Aufträge liegen nur im Speicher; ein laufender Slice lässt sich nicht unterbrechen; sauberes Herunterfahren ist nicht getestet |
| T-055 | Erledigt bis auf einen Test auf arm64: mehrstufiges `Dockerfile`, statische Binaries in einem leeren Image (2,5 MB), Nicht-Root; CI baut das Image für amd64 und slict ein Modell über die API; die Release-Pipeline baut amd64 und arm64. Das arm64-Image ist nie gelaufen |
| T-040 | Weitgehend erledigt: Einstellungsdateien lassen sich schichten (`-c` mehrfach, im Server mehrere `settings`-Felder); mitgelieferte Profile für Drucker, Material und Prozess in `profiles/`, ein Test lädt alle Kombinationen und prüft, dass sich die Rollen nicht überschneiden. `docs/settings.md` nennt Schlüssel, Standardwert, erlaubten Bereich und Bedeutung und wird von einem Test aus dem Code erzeugt und geprüft. Fehler nennen Schlüssel, Wert und Bereich; unbekannte Schlüssel werden abgelehnt. Offen: Technologie-Kennung (FDM/SLA), Einheit als eigenes Feld |
| T-044 | Teilweise: Zeitschätzung mit Beschleunigung und Abbremsen an Ecken (Einstellungen `acceleration`, `jerk`), Material in Länge, Volumen, Gewicht. Offen: Vergleich mit einem echten Druck; Aufheizen und Start-/End-G-Code zählen nicht mit |
| T-045 | Teilweise: `profiles/printers/bambu-x1c.toml` mit selbst gewählten Werten und einer Start-/Endsequenz aus allgemein üblichen Befehlen; nie am Gerät gelaufen, hängt an T-004 |
| T-054 | Weitgehend erledigt: OpenAPI-3.1-Beschreibung unter `/openapi.json`; von Hand neben dem Router geschrieben, nicht aus dem Code erzeugt. Ein Test ruft jede beschriebene Operation auf und prüft, dass die Antwort angekündigt ist |
| T-056 | Umgesetzt, Veröffentlichung ungetestet: `release.yml` baut bei einem Tag `v*` Archive für drei Betriebssysteme und das Image für amd64/arm64, legt Prüfsummen an und veröffentlicht ein Release mit erzeugten Notizen. Geprobt ist nur der Bau ohne Veröffentlichung |
| T-061 | Erledigt: die CI misst Zeit und Spitzenspeicher der Referenzmodelle und des Benchy und zeigt sie in der Zusammenfassung des Laufs (kein Verlauf über mehrere Läufe); Vergleich mit Orca Slicer 2.3.1 auf demselben Rechner mit dokumentierten Einstellungen in `docs/benchmarks.md`, Skripte in `tools/orca-compare/`. Die beiden dabei gefundenen Mängel (zu viele Retractions im Infill, sehr großer G-Code bei runden Modellen) sind behoben und nachgemessen |
| T-062 | Erledigt: README mit Schnellstart für CLI, Server und Docker, bekannten Einschränkungen, Hinweisen zum ersten Druck, Profilen und Release-Ablauf; Referenz aller Einstellungen in `docs/settings.md`; API-Beschreibung unter `/openapi.json` |
| T-042 | Vorbereitet, ungeprüft: die CLI schreibt bei einem Ausgabenamen auf `.gcode.3mf` eine Projektdatei mit G-Code, MD5-Prüfsumme und Beschreibungsdateien (der Server liefert dieselbe Datei unter `/v1/jobs/{id}/gcode.3mf`), Aufbau an einem Export aus Bambu Studio abgelesen. Ohne Vorschaubilder und ohne vollständige Einstellungsdatei. Ob der X1C die Datei annimmt und welche Startsequenz er braucht, klärt erst T-004 am Gerät |
| T-060 | Offen, nur am Gerät möglich |
| Alle übrigen | Offen |

---

## Kritischer Pfad

```
T-001 → T-003 (Polygon-Spike) → T-010 → T-020 → T-022 → T-023 → T-030 → T-031 → T-041 → T-060
                                                                              ↑
                                              T-004 (X1C-Spike) → T-042 ──────┘
```

Die beiden Spikes **T-003** und **T-004** zuerst erledigen: Beide können den Plan umwerfen.

---

## Epic A: Fundament (Phase 0)

| ID | Typ | Titel | Größe | Hängt ab von | PRD |
|---|---|---|---|---|---|
| T-001 | Task | Cargo-Workspace und Crate-Gerüst anlegen | S | – | Leitplanken |
| T-002 | Spike | Lizenz festlegen und Clean-Room-Regeln schreiben | S | – | Q3 |
| T-003 | Spike | Polygon-Bibliothek evaluieren | L | T-001 | Q2 |
| T-004 | Spike | X1C: fremden G-Code von SD-Karte drucken | M | – | Q13 |
| T-005 | Task | CI auf Windows, Linux, macOS | M | T-001 | R13 |
| T-006 | Task | Fehler- und Warnungsmodell für den Kern | S | T-001 | R1, R9, R10 |
| T-007 | Task | Testkorpus und Referenzmodelle zusammenstellen | M | T-002 | R13, Metriken |

**T-001 Cargo-Workspace und Crate-Gerüst**
- [ ] Workspace mit `slicer-core`, `-fdm`, `-gcode`, `-formats`, `-profiles`, `-cli`, `-server` (leer, aber baubar)
- [ ] `slicer-core` hat keine Abhängigkeit auf Dateisystem, Netzwerk oder UI
- [ ] Einheitliche Lint- und Format-Konfiguration; `cargo build` und `cargo test` laufen ohne weitere Schritte

**T-002 Lizenz und Clean-Room-Regeln**
- [ ] Lizenzentscheidung dokumentiert, Lizenzdateien im Repo
- [ ] `CONTRIBUTING.md` mit Regel: kein Quelltext aus AGPL-Slicern lesen oder übernehmen; Algorithmen mit Quellenangabe (Paper, Doku, eigener Entwurf)

**T-003 Polygon-Bibliothek evaluieren** (Zeitbox: 1 Woche)
- [ ] Kandidaten-Crates für Offset, Vereinigung, Differenz, Schnitt verglichen: Robustheit bei entarteten Eingaben, Ganzzahl-Koordinaten, Lizenz, Wartungsstand, Geschwindigkeit
- [ ] Prüfung mit realen Schichtkonturen (dünne Wände, spitze Winkel, sich berührende Konturen)
- [ ] Entscheidung „Crate X" oder „Eigenbau" schriftlich begründet; bei Eigenbau neue Tickets und neue Schätzung
- [ ] Koordinatendarstellung festgelegt (Ganzzahl-Einheit und Auflösung) als Grundlage für plattformgleiche Ausgabe

**T-004 X1C: fremden G-Code von SD-Karte drucken** (Zeitbox: wenige Tage)
- [ ] Geklärt, welche Dateiform der X1C annimmt (reine `.gcode`-Datei oder `.gcode.3mf`-Container) und was der Container mindestens enthalten muss
- [ ] Minimale Start-/Endsequenz selbst erarbeitet (Aufheizen, Homing, Nivellierung, Düse reinigen) und dokumentiert, mit Quellenangabe
- [ ] Ein von Hand geschriebener Test-G-Code (z. B. ein Quadrat, eine Schicht) druckt erfolgreich
- [ ] Ergebnis: Entscheidung, ob Phase 1 am X1C abnehmbar ist; falls nein, Rückmeldung ins PRD (Q14 wird dann blockierend)
- **Sicherheit:** erste Versuche mit reduzierten Temperaturen und Geschwindigkeiten, Drucker beaufsichtigen

**T-005 CI auf drei Plattformen**
- [ ] Build, Tests, Lints bei jedem Push auf Windows, Linux, macOS
- [ ] Build-Cache eingerichtet; Laufzeit dokumentiert

**T-006 Fehler- und Warnungsmodell**
- [ ] Einheitliche Fehlertypen mit stabilem Code, Meldung und Kontext (Datei, Schlüssel, Wert)
- [ ] Warnungen werden gesammelt und nicht nur geloggt; serialisierbar als JSON

**T-007 Testkorpus und Referenzmodelle**
- [ ] Fünf Referenzmodelle festgelegt (Benchy, 20-mm-Würfel, drei weitere mit Überhang-freier Geometrie, dünnen Wänden, Löchern)
- [ ] Korpus aus frei lizenzierten Modellen angelegt, Lizenzen vermerkt; Ziel 200, Start mit 30
- [ ] Bewusst defekte Dateien für Importer-Tests

---

## Epic B: Modellimport und Mesh (R1)

| ID | Typ | Titel | Größe | Hängt ab von |
|---|---|---|---|---|
| T-010 | Task | Mesh-Datenstruktur und Grundoperationen | M | T-003 |
| T-011 | Task | STL-Import (binär und ASCII) | S | T-010, T-006 |
| T-012 | Task | 3MF-Import (Geometrie) | M | T-010, T-006 |
| T-013 | Task | Mesh-Prüfung und einfache Reparatur | L | T-010 |
| T-014 | Task | Fuzzing der Importer | M | T-011, T-012 |

**T-010 Mesh-Datenstruktur**
- [ ] Indiziertes Dreiecksnetz, Bounding Box, Volumen, Transformation (verschieben, drehen, skalieren)
- [ ] Zusammenführen doppelter Eckpunkte

**T-011 STL-Import**
- [ ] Binär und ASCII werden erkannt und gelesen
- [ ] Abgeschnittene, leere oder falsch deklarierte Dateien ergeben einen Fehler mit Ursache, keinen Absturz

**T-012 3MF-Import**
- [ ] Geometrie und Objekt-Transformationen aus dem 3MF-Kernformat; mehrere Objekte pro Datei
- [ ] Unbekannte Erweiterungen werden ignoriert und als Warnung gemeldet

**T-013 Mesh-Prüfung und Reparatur**
- [ ] Erkennung: offene Kanten, umgedrehte Flächen, entartete Dreiecke, nicht-mannigfaltige Kanten; Warnung mit Art und Anzahl
- [ ] Reparatur einfacher Fälle (Normalen ausrichten, entartete Dreiecke entfernen, kleine Löcher schließen)
- [ ] Nicht reparierbare Modelle werden klar gemeldet

**T-014 Fuzzing der Importer**
- [ ] Fuzz-Ziele für STL und 3MF; läuft regelmäßig in CI
- [ ] Gefundene Abstürze als Regressionstests abgelegt

---

## Epic C: Schichtung (R2)

| ID | Typ | Titel | Größe | Hängt ab von |
|---|---|---|---|---|
| T-020 | Task | Mesh-Ebenen-Schnitt zu geschlossenen Konturen | L | T-010, T-003 |
| T-021 | Task | Schichthöhen-Planung und Bauraumprüfung | S | T-020 |
| T-022 | Task | Schicht-Datenmodell mit Regionen | M | T-020 |
| T-023 | Task | Klassifikation der Flächen (oben, unten, innen) | M | T-022 |

**T-020 Mesh-Ebenen-Schnitt**
- [ ] Jede Kontur geschlossen, korrekt orientiert (außen/Loch), ohne Selbstüberschneidung
- [ ] Sonderfälle: Eckpunkt oder Fläche genau in der Schnittebene, sich berührende Körper
- [ ] Schichten werden parallel berechnet; Ergebnis unabhängig von der Thread-Anzahl

**T-021 Schichthöhen-Planung**
- [ ] Konstante Schichthöhe, abweichende erste Schicht
- [ ] Modell außerhalb des Bauraums (X, Y oder Z) bricht mit klarer Meldung ab

**T-022 Schicht-Datenmodell**
- [ ] Technologieneutrale Schichtkonturen in `slicer-core`; FDM-Regionen in `slicer-fdm`
- [ ] Material-/Extruder-Zuordnung pro Objekt ist im Modell vorhanden (v1 nutzt nur einen Extruder)

**T-023 Flächenklassifikation**
- [ ] Nach oben und unten freiliegende Bereiche je Schicht durch Vergleich mit Nachbarschichten
- [ ] Ausdehnung über die eingestellte Zahl von Deck-/Bodenschichten

---

## Epic D: Pfaderzeugung (R3–R7)

| ID | Typ | Titel | Größe | Hängt ab von |
|---|---|---|---|---|
| T-030 | Task | Perimeter mit fester Linienbreite | L | T-022 |
| T-031 | Task | Infill geradlinig und Gitter | M | T-030 |
| T-032 | Task | Massive Deck- und Bodenschichten | M | T-023, T-031 |
| T-033 | Task | Skirt und Brim | S | T-030 |
| T-034 | Task | Extrusionsmodell (Fluss, Linienquerschnitt) | M | T-022 |
| T-035 | Task | Pfadreihenfolge und Leerwege | L | T-030, T-031 |
| T-036 | Task | Retraction, Z-Hop, Geschwindigkeiten, Lüfter, Temperaturen | M | T-035 |

**T-030 Perimeter**
- [ ] Einstellbare Wandzahl, Reihenfolge innen/außen wählbar
- [ ] Wandstärke am Kalibrierwürfel ±0,05 mm (rechnerisch im Test, am Druck in T-060)
- [ ] Bereiche schmaler als eine Linienbreite: kein Absturz, keine Überextrusion, Warnung

**T-031 Infill**
- [ ] Geradlinig und Gitter, Dichte 0–100 %, Winkel einstellbar und schichtweise wechselnd
- [ ] Einstellbare Überlappung mit den Perimetern

**T-032 Deck- und Bodenschichten**
- [ ] Massive Füllung in klassifizierten Bereichen, Anzahl einstellbar
- [ ] Kein sichtbares Infill an Außenflächen der Referenzmodelle (Prüfung in T-060)

**T-033 Skirt und Brim**
- [ ] Abstand, Linienzahl, Brim-Breite einstellbar; mehrere Objekte auf der Platte berücksichtigt

**T-034 Extrusionsmodell**
- [ ] Extrusionsmenge aus Linienbreite, Schichthöhe, Filamentdurchmesser, Flussfaktor
- [ ] Extrudiertes Volumen bei 100 % Infill entspricht dem Modellvolumen auf ±3 %

**T-035 Pfadreihenfolge und Leerwege**
- [ ] Kurze Leerwege innerhalb einer Schicht (Nearest-Neighbour als Start)
- [ ] Kein Leerweg über Außenflächen ohne Retraction
- [ ] Reihenfolge deterministisch

**T-036 Bewegungsparameter**
- [ ] Retraction (Länge, Geschwindigkeit, Mindestweg), optionaler Z-Hop
- [ ] Geschwindigkeit je Linientyp, erste Schicht separat; Lüfter und Temperaturen inklusive erster Schicht

---

## Epic E: G-Code und Profile (R8, R9)

| ID | Typ | Titel | Größe | Hängt ab von |
|---|---|---|---|---|
| T-040 | Task | Profilformat, Vererbung, Validierung | L | T-006 |
| T-041 | Task | G-Code-Writer mit Flavor-Schnittstelle (Marlin, Klipper) | M | T-036, T-040 |
| T-042 | Task | Bambu-Flavor für X1C (ein Material) | M | T-041, T-004 |
| T-043 | Task | Platzhalter in Start-/End-G-Code | M | T-041 |
| T-044 | Task | Druckzeit- und Materialschätzung | M | T-041 |
| T-045 | Task | Profile für den X1C (Drucker, PLA, Standardprozess) | M | T-042 |
| T-046 | Task | Plattformgleiche Ausgabe absichern | M | T-041, T-005 |

**T-040 Profile**
- [ ] Drei Ebenen (Drucker, Material, Prozess), dokumentiertes Textformat, Vererbung
- [ ] Schema mit Typ, Einheit, Bereich, Standardwert je Einstellung; daraus erzeugte Referenzdokumentation
- [ ] Fehler nennen Schlüssel, Wert und erlaubten Bereich; unbekannte Schlüssel werden abgelehnt
- [ ] Profile tragen eine Technologie-Kennung (FDM/SLA)

**T-041 G-Code-Writer**
- [ ] Flavor-Schnittstelle; Marlin und Klipper umgesetzt; relative und absolute Extrusion
- [ ] Kommentare zum Linientyp, Kopf mit Version und Einstellungen
- [ ] Neuer Flavor erfordert keine Änderung an `slicer-fdm`
- [ ] Im Release als „nicht an Hardware verifiziert" gekennzeichnet

**T-042 Bambu-Flavor**
- [ ] Ausgabe in der in T-004 ermittelten Dateiform; Start-/Endsequenz aus T-004
- [ ] Ohne AMS, ein Material

**T-043 Platzhalter**
- [ ] Dokumentierte Variablen (Temperaturen, Bauraum, Schichthöhe, erste-Schicht-Werte) in Start-/End-G-Code
- [ ] Unbekannter Platzhalter ergibt einen Fehler mit Position

**T-044 Schätzung**
- [ ] Druckzeit unter Berücksichtigung von Beschleunigung; Materialverbrauch in Länge, Volumen, Gewicht
- [ ] Abweichung der Zeit gegen echten Druck gemessen und dokumentiert (Zielwert nach erster Messung festlegen)

**T-045 X1C-Profile**
- [ ] Ein getestetes Profil-Set, Werte selbst ermittelt oder aus Herstellerangaben, nicht aus AGPL-Profilen kopiert
- [ ] Auswirkung der offenen Lizenzfrage Q6 vermerkt

**T-046 Plattformgleiche Ausgabe**
- [ ] CI vergleicht G-Code der Referenzmodelle zwischen Windows, Linux, macOS byteweise
- [ ] Zahlenformatierung und Rundung zentral an einer Stelle
- [ ] Sollausgaben („Golden Files") mit dokumentiertem Verfahren zur Aktualisierung

---

## Epic F: CLI, API, Docker (R10–R12)

| ID | Typ | Titel | Größe | Hängt ab von |
|---|---|---|---|---|
| T-050 | Task | CLI `slice` mit JSON-Ausgabe | M | T-041 |
| T-051 | Task | Server-Grundgerüst, Health, Konfiguration | M | T-001 |
| T-052 | Task | Auftrags-API mit Warteschlange | L | T-051, T-050 |
| T-053 | Task | Token-Authentifizierung und Upload-Limits | S | T-052 |
| T-054 | Task | OpenAPI-Beschreibung | M | T-052 |
| T-055 | Task | Docker-Image (amd64, arm64) | M | T-052 |
| T-056 | Task | Release-Pipeline für Binaries und Image | M | T-055, T-005 |

**T-050 CLI**
- [ ] `slice` mit Modell und Profilen erzeugt die Ausgabedatei, Exit-Code 0
- [ ] Fehler: Exit-Code ungleich 0, Ursache auf stderr; mit `--json` maschinenlesbar
- [ ] Statistik (Zeit, Material, Schichtzahl) und Warnungen als JSON; einzelne Einstellungen per Argument überschreibbar

**T-051 Server-Grundgerüst**
- [ ] Konfiguration ausschließlich über Umgebungsvariablen; Health-Endpunkt
- [ ] Strukturierte Logs; sauberes Herunterfahren

**T-052 Auftrags-API**
- [ ] Anlegen (Modell + Profile), Status, Ergebnis, Statistik, Abbrechen unter `/v1/`
- [ ] Anlegen antwortet sofort mit Auftrags-ID; Slicing im Hintergrund
- [ ] Begrenzte Parallelität, weitere Aufträge warten; Ergebnisse verfallen nach konfigurierbarer Zeit
- [ ] Fehler im gleichen Format wie die CLI

**T-053 Authentifizierung und Limits**
- [ ] Mit konfiguriertem Token werden Anfragen ohne Token abgelehnt
- [ ] Zu große Uploads werden mit passendem Status abgelehnt, ohne die Datei vollständig zu puffern

**T-054 OpenAPI**
- [ ] Beschreibung wird aus dem Code erzeugt und vom Server ausgeliefert; CI prüft Übereinstimmung

**T-055 Docker-Image**
- [ ] Mehrstufiger Build, minimales Laufzeit-Image, Nicht-Root, amd64 und arm64
- [ ] `docker run` mit Port-Freigabe genügt; Größe gemessen gegen Ziel ≤ 50 MB

**T-056 Release-Pipeline**
- [ ] Getaggte Version erzeugt Binaries für drei Betriebssysteme und das Image
- [ ] Prüfsummen und Änderungsprotokoll

---

## Epic G: Abnahme (G2, G5)

| ID | Typ | Titel | Größe | Hängt ab von |
|---|---|---|---|---|
| T-060 | Task | Referenzdrucke auf dem X1C | L | T-042, T-045, T-032 |
| T-061 | Task | Benchmark-Suite gegen Orca | M | T-050 |
| T-062 | Task | Dokumentation für das erste Release | M | T-056 |

**T-060 Referenzdrucke**
- [ ] Fünf Modelle je zweimal; Erfolg = durchgedruckt, maßhaltig ±0,2 mm, keine Löcher oder Fäden an Außenflächen
- [ ] Ziel ≥ 9/10; Ergebnisse mit Fotos und Messwerten dokumentiert
- [ ] Gefundene Fehler als Tickets; Wiederholung nach Korrektur einplanen (Puffer: diese Schleife läuft erfahrungsgemäß mehrfach)

**T-061 Benchmarks**
- [ ] Slicing-Zeit und Spitzenspeicher für die Referenzmodelle, in CI verfolgt
- [ ] Vergleichsmessung mit Orca auf demselben Rechner mit dokumentierten, möglichst vergleichbaren Einstellungen (nur als Programm ausführen, Quelltext nicht lesen)

**T-062 Dokumentation**
- [ ] README mit Schnellstart für CLI und Docker; Profil-Referenz; API-Beispiele
- [ ] Bekannte Einschränkungen: keine Stützen, Marlin/Klipper nicht an Hardware verifiziert

---

## Empfohlene Reihenfolge

1. **Woche 1–2:** T-001, T-002, T-004, T-003 – die Spikes klären, ob Plan und Hardware tragen.
2. **Fundament:** T-005, T-006, T-007, dann Epic B.
3. **Kern:** Epic C, danach Epic D. Früh einen durchgehenden „dünnen" Pfad anstreben: Würfel → eine Wandlinie → G-Code (T-020, T-030, T-041 jeweils in Minimalform), dann verbreitern.
4. **Erster Druck so früh wie möglich:** T-042 und T-045 direkt nach dem dünnen Pfad, damit Fehler am Gerät auffallen, bevor alles darauf aufbaut.
5. **Parallel nebenher:** T-040 (Profile) und T-051 (Server-Gerüst) hängen kaum am Kern.
6. **Abschluss:** Epic F, dann Epic G.

## Nicht in diesem Breakdown

Stützstrukturen (über die einfache Form hinaus), Nahtsteuerung, Brücken, UI, AMS, LAN-Versand, Resin: Phase 2–5 laut PRD. Ohne Stützen müssen die Referenzmodelle in T-007 so gewählt werden, dass sie ohne druckbar sind.
