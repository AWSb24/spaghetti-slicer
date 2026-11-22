# Ticket-Breakdown: Phase 0–1 (Headless-FDM-MVP)

Grundlage: [PRD.md](PRD.md), Requirements R1–R13. Referenzdrucker: Bambu Lab X1C.

**Größen** (Schätzung für eine Person, ungeprüft): **S** = bis 1 Tag, **M** = 2–4 Tage, **L** = 1–2 Wochen.
**Typen:** Spike = zeitlich begrenzte Klärung mit Entscheidung als Ergebnis; Task = Umsetzung.

Summe: 40 Tickets (7 × S, 25 × M, 8 × L), also rechnerisch etwa 5–8 Monate Vollzeit-Äquivalent, ohne Puffer für Nacharbeit. Das ist eine Größenordnung, keine Zusage.

## Stand (2026-11-15)

| Ticket | Stand |
|---|---|
| T-001 | Erledigt |
| T-002 | Erledigt: MIT oder Apache-2.0, Regeln in `CONTRIBUTING.md` |
| T-003 | Erledigt: `i_overlay` bestätigt, festes 10-nm-Raster eingeführt, siehe `docs/decisions/0001-polygon-library.md` |
| T-004 | Offen, nur am Gerät möglich |
| T-005 | Erledigt: CI auf Linux, Windows, macOS; erster Lauf grün |
| T-006 | Erledigt: einheitliche Diagnosen mit stabilem Code, Meldung und Kontext; Warnungen werden gesammelt und als JSON ausgegeben |
| T-007 | Erledigt: sechs Referenzmodelle in `examples/`, 37 erzeugte Modelle als Korpus-Test, defekte Dateien als Test, 21 Dateien der Benchy-Familie (CC0) und 200 Modelle aus Thingi10K (CC BY, CC BY-SA, gemeinfrei; 160 saubere, 40 mit offenen oder nicht-mannigfaltigen Netzen) per Abrufskript mit Prüfsummen. 199 der 200 werden gesliced, eines ist flacher als eine Schicht. Die Lizenzen stammen aus den Metadaten des Datensatzes und sind nicht einzeln bei Thingiverse nachgeprüft |
| T-046 | Weitgehend erledigt: CI vergleicht den G-Code der sechs Referenzmodelle und des Benchy zwischen den drei Plattformen; Sollwerte (Größe, Zeilenzahl, Prüfsumme) liegen in `examples/golden.tsv` und werden bei jedem Testlauf geprüft, Aktualisierung mit `UPDATE_GOLDEN=1`. Zusätzlich liegt je Schicht eine Prüfsumme in `examples/golden-layers.tsv`; ein Fehlschlag nennt Modell, Schicht und Höhe der ersten Abweichung |
| T-010, T-011 | Erledigt |
| T-012 | Erledigt: Geometrie, Einheiten, Komponenten, Build-Items und Transformationen; Verweise auf andere Modellteile im Paket (Production-Erweiterung); übrige Erweiterungen als Warnung. Lokal mit 16 Dateien aus Fusion 360 und Bambu Studio geprüft (private Modelle, nicht im Repository). Bei Bambu-Projekten mit mehreren Druckplatten wird eine Platte gewählt (`--plate`, Server-Feld `plate`); Farben und Einstellungen aus der Datei werden ignoriert |
| T-014 | Weitgehend erledigt: Fuzz-Ziele für STL und 3MF in `fuzz/` (cargo-fuzz), wöchentlicher und manuell startbarer Lauf `fuzz.yml`, Abstürze werden als Artefakt aufbewahrt; dazu Tests mit 7000 reproduzierbar beschädigten Dateien bei jedem Testlauf. Bisher kein Absturz gefunden. Das 3MF-Ziel bekommt rohe Bytes und kommt deshalb selten über die ZIP-Prüfung hinaus; ein drittes Ziel (`threemf_model`) packt die erzeugten Daten als Modellteil in ein gültiges Paket und startet mit einem gültigen Modell, damit der XML-Leser erreicht wird |
| T-013 | Erledigt: Vor dem Schneiden wird jedes Netz repariert (`Mesh::repair`): umgedrehte Dreiecke werden an ihren Nachbarn ausgerichtet (der kleinere Teil gibt nach), Dreiecke ohne Fläche entfernt (kurze Kanten zusammengezogen, Kantenflip), doppelte Dreiecke entfernt (gegenläufige Paare heben sich auf, so werden an einer Fläche anliegende Körper eins), Löcher mit bis zu 100000 Randkanten geschlossen und nach innen gewandte geschlossene Hüllen nach außen gedreht; eine Hülle in einer anderen (Hohlraum) behält ihre Richtung zur umgebenden. Ränder, die sich in einer Ecke treffen, werden dort getrennt; gefüllt wird durch Ohrenschneiden in der am besten passenden Ebene (Normale nach Newell), wenn sich der Rand dort nicht selbst schneidet, sonst durch einen Fächer um einen neuen Punkt in der Mitte, wenn der Rand ihn genau einmal umläuft, sonst bis 64 Kanten wie bisher durch einen Fächer von einer Ecke; andere Löcher bleiben offen. Der Aufwand ist begrenzt (Sortieren statt Hash-Reihenfolge, Budget für Ohrenschneiden und verschachtelte Hüllen). Gemeldet wird eine Warnung: `mesh.repaired` mit den Zahlen des Getanen oder `mesh.not_repairable` mit den verbliebenen offenen und mehrfach benutzten Kanten und einem Hinweis; sie ersetzt `mesh.reoriented`, `mesh.degenerate_removed`, `mesh.inside_out`, `mesh.non_manifold` und `mesh.not_watertight`. Von den 40 defekten Thingi10K-Modellen sind danach 22 sauber (vorher 11); die anderen 18 behalten Kanten, an denen sich Teile berühren. Die Reparatur braucht etwa 1 ms je 1000 Dreiecke. Objekte mit eigenen Einstellungen werden für ihren Bereich jeder Schicht ebenso mit `Mesh::repair` repariert (vorher ohne Entfernen doppelter Dreiecke und ohne Drehen ganzer Hüllen), die Warnung kommt einmal von der ganzen Platte; Objekte ohne eigene Einstellungen bleiben unverändert. Kopien eines Dreiecks gelten beim Ausrichten nicht als Nachbarn, sodass eine doppelte Kopie am Rand eines Lochs nicht mehr umgedreht wird und sich mit ihrem Original aufhebt (Test `a_repeated_triangle_at_the_rim_of_a_hole_goes_alone`). Offen: Flicken werden nicht gegen das übrige Netz geprüft |
| T-020 bis T-023, T-030 bis T-036 | Umgesetzt mit Lücken: Bahngeometrie je Schicht und das Zusammensetzen der Schnittlinien parallel auf allen Kernen; Umrisse werden nach dem Schnitt auf 0,005 mm ausgedünnt; Zacken ohne Breite aus der Versatz-Operation werden entfernt; Wandreihenfolge wählbar (`wall_order`); Naht wählbar hinten ausgerichtet, an der schärfsten Ecke (`corner`; nur nach außen zeigende Ecken, nach innen zeigende sind in den Wänden gerundet) oder am nächsten Punkt (`seam_position`), nicht frei gesetzt; Wände werden auf 0,0125 mm ausgedünnt; Leerwege zum nächsten Start, Retraction nur wenn der Weg den Umriss der Schicht verlässt oder zu einer Außenwand führt; Infill-Winkel frei wählbar (`infill_angle`, jede zweite Schicht um 90° gedreht), mit eigenem Sinus und Kosinus aus Grundrechenarten, damit jede Plattform bitgleich rechnet; Stützen und Brücken nutzen weiter feste Richtungen. Überhänge und Brücken ohne Stütze werden als Warnung gemeldet (`slicing.unsupported_overhangs`); einfache Stützen (gerade Säulen aus Linien mit Rand, `supports = true`, Vorgriff auf Phase 3) mit dichter Kontaktschicht (`support_interface_layers`) und einstellbarem Abstand zum Modell (Standard eine leere Schicht nach oben und unten, eine Linienbreite zur Seite), der Skirt umschließt sie; ohne Baumstützen. Brücken (massive Flächen ohne Material darunter, mindestens eine Linie breit) bekommen Linien in der Richtung, in der die meisten beidseitig aufliegen, gewählt aus vier Richtungen (X, Y, beide Diagonalen), mit zwei Linienbreiten Auflage; mit eigener Geschwindigkeit (`bridge_speed`) und eigener Bahnart im G-Code (`;TYPE:Bridge`), eigener Flussrate (`bridge_flow`) und eigener Lüfterdrehzahl (`bridge_fan_speed`); Brücken bis `support_bridge_length` (Standard 10 mm) bekommen keine Stützen |
| T-041, T-043, T-050 | Umgesetzt: relative und absolute Extrusion (`extrusion_mode`), Kommentare zum Linientyp, Kopf mit Version und Einstellungen, Platzhalter mit Fehlermeldung. Marlin und Klipper erzeugen denselben G-Code, weil die verwendeten Befehle in beiden gleich sind; nicht an Hardware verifiziert |
| T-051, T-052, T-053 | Erledigt: Server mit Health, Konfiguration über Umgebungsvariablen, JSON-Logs, Auftrags-API unter `/v1/`, begrenzte Parallelität, Verfall, Token und Upload-Limit. fertige Aufträge lassen sich in einem Verzeichnis aufbewahren und überstehen einen Neustart (`SPAGHETTI_DATA_DIR`), wartende und laufende Aufträge gehen beim Beenden verloren; ein abgebrochener Auftrag beendet auch den laufenden Slice (zwischen den Schritten und vor jeder Schicht der Bahnberechnung und des Ordnens geprüft, nicht im Netzschnitt; ein Test bricht einen laufenden Slice ab und prüft, dass der Arbeitsplatz in weniger als der halben Slice-Zeit wieder frei ist); ein Test prüft, dass der Server auf das Stoppsignal hin sauber endet (ohne laufende Anfrage) |
| T-055 | Erledigt: mehrstufiges `Dockerfile`, statische Binaries in einem leeren Image (2,5 MB), Nicht-Root; CI baut das Image für amd64 und slict ein Modell über die API; die Release-Pipeline baut amd64 und arm64. Ein wöchentlicher und von Hand startbarer Lauf (`fuzz.yml`) baut das Image auf einem arm64-Rechner, slict ein Modell über die API und vergleicht Größe und Zeilenzahl des G-Codes mit den Sollwerten; am 2026-11-15 erstmals grün |
| T-040 | Erledigt: Einstellungsdateien lassen sich schichten (`-c` mehrfach, im Server mehrere `settings`-Felder); mitgelieferte Profile für Drucker, Material und Prozess in `profiles/`, ein Test lädt alle Kombinationen und prüft, dass sich die Rollen nicht überschneiden. `docs/settings.md` nennt Schlüssel, Standardwert, erlaubten Bereich und Bedeutung und wird von einem Test aus dem Code erzeugt und geprüft. Fehler nennen Schlüssel, Wert und Bereich; unbekannte Schlüssel werden abgelehnt. Einstellungen tragen eine Technologie-Kennung (`technology`, bisher nur `fdm`; alles andere wird abgelehnt), die Druckerprofile setzen sie. Jede Einstellung hat ihre Einheit als eigenes Feld (`SettingInfo::unit`, Tabelle in `crates/slicer-profiles/src/units.rs`; fehlt eine Einstellung dort, bricht der Build ab). `docs/settings.md` zeigt sie als eigene Spalte, die App neben dem Wert; ein Test prüft sie gegen Beschreibung und Namen (Lüfter in %, Zeiten in s) |
| T-044 | Teilweise: Zeitschätzung mit Beschleunigung und Abbremsen an Ecken (Einstellungen `acceleration`, `jerk`), Material in Länge, Volumen, Gewicht. Start- und End-G-Code zählen mit (nach dem Ersetzen der Platzhalter): Bewegungen `G0`–`G3` nach demselben Modell, `G4` mit `P`/`S`, `G28` mit `homing_time` (10 s). Aufheizen zählt mit Heizraten (`nozzle_heat_rate`, `bed_heat_rate` in °C/s, Standard 0 = nicht gezählt) ab `ambient_temp` (25 °C): Heizungen folgen ihrem Ziel auch während der Fahrten, Zeit kostet nur das Warten (`M109`, `M190`, `M116`, mit `S` nur beim Aufheizen, mit `R` auch beim Abkühlen), auch im Werkzeugwechsel-G-Code. Die Statistik nennt `time_start_end_s` und `time_heating_s` als Teile von `print_time_s`; Kopfzeile und `prediction` im `.gcode.3mf` zeigen dieselbe Summe. Mit den Standardwerten 11 s mehr als vorher (Referenzmodelle: nur Zeitzeile und Einstellungsliste geändert); Benchy 3268 s, mit 2 und 0,5 °C/s 3353 s. Offen: Vergleich mit einem echten Druck; Höchstgeschwindigkeiten je Achse, Bewegungen im Werkzeugwechsel-G-Code |
| T-045 | Teilweise: `profiles/printers/bambu-x1c.toml` mit selbst gewählten Werten und einer Start-/Endsequenz aus allgemein üblichen Befehlen; nie am Gerät gelaufen, hängt an T-004 |
| T-054 | Weitgehend erledigt: OpenAPI-3.1-Beschreibung unter `/openapi.json`; von Hand geschrieben, der Router entsteht aus einer einzigen Liste der Operationen; ein Test prüft, dass Beschreibung und Liste genau dieselben Operationen nennen, und ruft jede auf |
| T-056 | Umgesetzt, Veröffentlichung ungetestet: `release.yml` baut bei einem Tag `v*` Archive für drei Betriebssysteme und das Image für amd64/arm64, legt Prüfsummen an und veröffentlicht ein Release mit erzeugten Notizen. Geprobt ist nur der Bau ohne Veröffentlichung |
| T-061 | Erledigt: die CI misst Zeit und Spitzenspeicher der Referenzmodelle und des Benchy und zeigt sie in der Zusammenfassung des Laufs (kein Verlauf über mehrere Läufe); Vergleich mit Orca Slicer 2.3.1 auf demselben Rechner mit dokumentierten Einstellungen in `docs/benchmarks.md`, Skripte in `tools/orca-compare/`. Die beiden dabei gefundenen Mängel (zu viele Retractions im Infill, sehr großer G-Code bei runden Modellen) sind behoben und nachgemessen |
| T-062 | Erledigt: README mit Schnellstart für CLI, Server und Docker, bekannten Einschränkungen, Hinweisen zum ersten Druck, Profilen und Release-Ablauf; Referenz aller Einstellungen in `docs/settings.md`; API-Beschreibung unter `/openapi.json` |
| T-042 | Vorbereitet, ungeprüft: die CLI schreibt bei einem Ausgabenamen auf `.gcode.3mf` eine Projektdatei mit G-Code, MD5-Prüfsumme und Beschreibungsdateien (der Server liefert dieselbe Datei unter `/v1/jobs/{id}/gcode.3mf`), Aufbau an einem Export aus Bambu Studio abgelesen. Mit Vorschaubildern der Platte (`Metadata/plate_1.png`, 512×512, und `plate_1_small.png`, 128×128, wie in gespeicherten Bambu-Projekten verknüpft), ohne vollständige Einstellungsdatei; aus der App gespeichert noch ohne Bild. Die Schichtliste (`layer_filament_lists`) nennt je Folge von Schichten mit denselben Extrudern diese Extruder (ab 0) und die erste und letzte Schicht, statt jede Schicht mit Filament 0; der Aufbau ist aus der bisherigen Liste erschlossen, eine gespeicherte Datei mit mehreren Filamenten zum Vergleich fehlt. Ob der X1C die Datei annimmt und welche Startsequenz er braucht, klärt erst T-004 am Gerät |
| R27 (Phase 3, vorgezogen) | Umgesetzt: `import-orca` übersetzt in Orca Slicer gespeicherte Profile (JSON) in eine Einstellungsdatei und listet jede nicht übernommene Einstellung mit Grund. Werte aus dem Orca-Vorgänger (`inherits`) fehlen, weil die mitgelieferten Orca-Profile nicht gelesen werden (Q6); an den eigenen Orca-Profilen dieses Rechners ausprobiert |
| T-022 | Teilweise: Extruder je Objekt im Modell (je Dreieck, Standard 0), aus Bambu-Projekten je Objekt und je Teil eines Objekts gelesen (der Teil vor dem Objekt), aus 3MF-Materialien (Basismaterialien und Farbgruppen) je Material in der Reihenfolge der ersten Verwendung; jede Bahn trägt ihren Extruder. Gedruckt wird mit dem niedrigsten, bei mehreren kommt `slicing.multiple_extruders`. Spülmengen je Filamentpaar (`flush_volumes`) bestimmen, wie viel jeder Werkzeugwechsel in den Prime-Tower spült. Offen: Bereiche und Werkzeugwechsel je Extruder, Trennung in technologieneutrale Schichtkonturen und FDM-Bereiche |
| Phase 3 (vorgezogen) | Brücken in jedem Winkel (15°-Schritte und längs/quer zur längsten Kante), auch für die Erkennung kurzer Brücken; Naht in gerundeten und nach innen zeigenden Ecken; Infill-Muster `triangles` und `gyroid` (Gyroid ignoriert `infill_angle`, G-Code etwa 1,7-mal so groß wie mit Gitter); adaptive Schichthöhe (`adaptive_layers`, Stufenhöhe nach Dolenc und Mäkelä 1994; Qualität über `adaptive_layer_quality`; weiche Übergänge mit `layer_height_max_change`: die Schichthöhe ändert sich je Schicht höchstens um den Prozentsatz, vor dünnen Schichten wird früher dünner, Standard aus und damit gleicher G-Code; Benchy bei 20 %: 403 statt 388 Schichten, größter Sprung 1,2 statt 3; offen: Stützabstand als Dicke, Höhe je Objekt, ebene Flächen, die mit der Grenze nicht erreichbar sind, werden überquert) |
| Phase 5 (vorgezogen) | Erste Resin-Pipeline: Crate `slicer-sla` mit geglätteten Schichtbildern, offenes Archivformat (ZIP mit PNG und JSON, `docs/sla-format.md`), Befehl `slice-sla`. Dazu Floß, Anhebung, Stützen (Raster- und Minimumpunkte, Kegelspitze, Säule, Kreuzstreben) und Aushöhlen mit Ablauflöchern. Offen: Herstellerformat des Photon Mono X (Q9), Bildausrichtung und Belichtung am Gerät, Stützen auf dem Modell, automatische Ausrichtung, Ablauflöcher an Wunschpositionen |
| Phase 3 (vorgezogen, Fortsetzung) | Dünne Wände und Lückenfüllung: Bereiche zwischen einer halben und einer Linienbreite bekommen eine Linie eigener Breite (`Path.width`), schmale Lücken zwischen Wänden und zum Infill werden gefüllt (`;TYPE:Gap infill`). Offen: Bereiche von ein bis zwei Linienbreiten bekommen eine Wandschleife, die sich selbst überlappt und zu viel Material abgibt (Thingi10K 40176/40184 bis 1,23-faches Volumen); keine echte Mittelachse für verzweigte dünne Wände; Benchy braucht rund 35 % mehr Rechenzeit |
| Mehrere Extruder (Phase 4 vorgezogen) | Bereich je Extruder (der höhere gewinnt bei Überlappung), Reihenfolge je Schicht beginnt mit dem zuletzt aktiven Extruder, Werkzeugwechsel mit `T<n>` und `toolchange_gcode`, einfacher Prime-Tower, Filament und Zeit je Extruder. Spülmengen je Filamentpaar (`flush_volumes`, `flush_multiplier`, aus Orca-/Bambu-Projekten übernommen): jeder Werkzeugwechsel spült seine Menge als Abschnitt einer Zickzacklinie in den Prime-Tower, der so dicht wie nötig gefüllt wird; was nicht in eine Schicht passt, meldet `slicing.flush_too_large`. Filamentfarben und -typen je Extruder (`filament_colours`, `filament_type`, aus Orca-/Bambu-Projekten übernommen) färben die Vorschaubilder je Extruder und stehen je benutztem Extruder mit Verbrauch im `.gcode.3mf`, dazu welche Schichten welche Extruder benutzen (als Schichtbereiche); mit `flush_from_colours` werden fehlende Spülmengen nach einer eigenen Faustregel aus der Helligkeit und dem Abstand der Farben abgeleitet (dunkel → hell spült mehr). Teile eines Objekts in Orca-/Bambu-Projekten drucken mit ihrem eigenen Extruder, und 3MF-Materialien (Basismaterialien, Farbgruppen) bekommen je Material einen Extruder in der Reihenfolge der ersten Verwendung; ihre Farben meldet `model.materials`, ohne `filament_colours` zu ändern. Offen: Extruder von Modifiern, Farbverläufe und Texturen aus 3MF, Materialfarben als `filament_colours` übernehmen, Prüfung der Teil-Extruder an einer echten Projektdatei, Farben in der App und im Projektbild der App, Bambu-Flavor/AMS, Test am Drucker || Mehrere Objekte | Platzierung im Kern (mittig oder wie angegeben), automatisches Anordnen nach umschließenden Rechtecken oder Umrissen, CLI mit mehreren Dateien und `--keep-positions`, App mit mehreren Modellen. Einstellungen je Objekt: Wandanzahl und -reihenfolge, Naht (`seam_position`), Deck- und Bodenschichten, Infill (Dichte, Muster, Winkel, Verbinden, Überlappung) und Stützen an/aus gelten je Objektgruppe mit gleichen Werten in ihrem eigenen Bereich jeder Schicht. Auch die Geschwindigkeiten der Linien (Außen- und Innenwand, Infill, Brücken, Überhänge in beiden Stufen, Bügeln) gelten je Objekt: der G-Code-Schreiber nimmt sie für jede Bahn aus den Einstellungen ihres Objekts (`Toolpaths::object_settings`, über `Path::object`), Stützen, Skirt, Brim, Prime Tower und die erste Schicht behalten die der Platte, die Mindestschichtzeit verlangsamt alle gleich, und Objekte, die sich nur in Geschwindigkeiten unterscheiden, werden genauso angelegt wie ohne. Ebenso je Objekt: Lüfter (`fan_speed`, `bridge_fan_speed`, `overhang_fan_speed`, `fan_off_layers`, `fan_layer_time`, `fan_max_speed`; `M106` nur bei Wertwechsel), Fließrate (`flow_ratio`, auch in der Filamentstatistik) und Rückzug (`retraction_length`, `retraction_speed`, `z_hop`, `wipe_distance`): der Rückzug vor einer Leerfahrt folgt dem Objekt der zuletzt gedruckten Bahn, das Vorschieben danach gibt genau so viel zurück, wie zurückgezogen wurde; Stützen haben eine eigene Geschwindigkeit (`support_speed`, 0 = `infill_speed`, für die ganze Platte). Schichthöhen, Skirt, Brim, Stützform, Fahrwege, kürzeste Fahrt mit Rückzug, Brücken- und Bügelfluss und die Mindestschichtzeit bleiben für die Platte gleich, Objekte ohne eigene Werte ergeben denselben G-Code wie zuvor (`slicer_fdm::generate_plate_with_object_settings`, in der App „Object settings“ am gewählten Modell, Modelle mit eigenen Werten mit ✱ markiert). CLI und Server nehmen sie ebenfalls an: `--object N:DATEI` bzw. das Feld `object_settings_<n>` legt eine Einstellungsdatei nur über das n-te Modell (der Server nimmt dafür mehrere `model`-Felder an und ordnet sie an), Schlüssel außerhalb von `OBJECT_SETTINGS` werden mit `settings.not_per_object` abgewiesen. Objekt für Objekt drucken (`sequential_print`): jedes Objekt wird für sich erzeugt und ganz gedruckt, das niedrigste zuerst, dazwischen hebt die Düse 2 mm über alles Gedruckte; Objekte näher als `extruder_clearance_radius` (konvexe Umrisse) oder außer dem letzten höher als `gantry_height` werden mit `slicing.objects_too_close` bzw. `slicing.object_too_tall` abgewiesen, Anordnen hält den Abstand ein, Brim je Objekt, Skirt nur um das erste, kein Prime Tower. Einstellungen aus Projekten von Orca Slicer und Bambu Studio: `--project-settings` nimmt die Projekteinstellungen (`Metadata/project_settings.config`, über die Orca-Zuordnung) als unterste Schicht unter `-c` und `--set` und die Einstellungen einzelner Objekte (`Metadata/model_settings.config`, nur `OBJECT_SETTINGS`) über die der Platte und unter `--object`; ein Objekt mit eigenen Werten wird dafür als eigenes Objekt geschnitten, nicht übertragbare Schlüssel werden als Warnungen gemeldet (`project.settings_skipped`, `project.object_settings_skipped`, `project.part_settings_skipped`), ohne die Option bleibt der G-Code gleich. Der Server kann das ebenso mit dem Formularfeld `project_settings=true` (gleiche Reihenfolge unter `settings`/`set` bzw. unter `object_settings_<n>`, gleiche Warnungen im Auftrag); CLI und Server nutzen dafür denselben Code (`slicer_fdm::load`). Anordnen nach Umrissen und mit Drehen: `arrange_outlines` packt die konvexen Umrisse von oben (Bottom-Left-Fill mit No-Fit-Polygonen, größtes Objekt zuerst, nie schlechter als die Rechteck-Reihen), `arrange_rotation` dreht zusätzlich in 45°-Schritten um Z, wo das dichter packt; beides in CLI, Server und App (dort als Drehung des Modells), standardmäßig aus, sodass der G-Code gleich bleibt. Mit `arrange_nonconvex` packt das Anordnen die echten Umrisse von oben (Vereinigung der projizierten Dreiecke, um den halben Abstand gewachsen und in wenige konvexe Stücke zerlegt), sodass L-Formen ineinandergreifen und kleine Teile in einem U oder Ring stehen; die Überlappungswarnung vergleicht dann die echten Umrisse, Objekt für Objekt gedruckt bleibt es beim konvexen Abstand für den Druckkopf. Mehrere Läufe (größtes, tiefstes, breitestes Objekt zuerst; Bottom-Left oder kleinstes umschließendes Rechteck, von der Mitte oder der Ecke aus) konkurrieren, der beste gewinnt, bei Gleichstand der bisherige: acht rechtwinklige Dreiecke mit 90 mm Schenkeln passen jetzt aufs 220-mm-Bett statt sieben. Höhenbereiche: Schichten, deren Mitte in einem Bereich [Z1, Z2) über dem Bett liegt, drucken mit dessen Werten über denen des Objekts (`RANGE_SETTINGS`: Wände, Naht, Deck-/Bodenschichten, Infill, Stützen an/aus); der Bereich gehört in jeder Schicht zum Bereich seines Objekts, sodass an seinen Grenzen keine falschen Deck- oder Bodenflächen entstehen, und Objekte ohne Bereiche ergeben denselben G-Code (`slicer_fdm::generate_plate_with_own_settings`, CLI `--range [N:]Z1-Z2:DATEI`, Server-Feld `range_settings_[<n>_]<z1>-<z2>`, mit `--project-settings` auch aus `<range min_z max_z>` der Objekte in `Metadata/model_settings.config`, deren Aufbau mangels echter Projektdatei mit Höhenbereichen nur erschlossen ist). Auch Geschwindigkeiten, Lüfter, Fließrate und Rückzug gelten je Höhenbereich: der G-Code-Schreiber nimmt sie für jede Bahn nach Objekt und Schicht (`Toolpaths::range_settings`, `Toolpaths::settings_on`, auch für die Farbe nach Geschwindigkeit in der Vorschau), die Schichten werden genauso angelegt wie ohne, die Mindestschichtzeit verlangsamt sie wie alle anderen; die Schichthöhe eines Höhenbereichs gilt für die ganze Platte, die Strecke zwischen den Bereichsgrenzen wird in gleich dicke Schichten geteilt, keine dicker als verlangt, Deck- und Bodenschalen behalten ihre Dicke in mm (gilt ein solcher Bereich nur für einige von mehreren Objekten, meldet das `slicing.range_layer_height_plate`; mit adaptiven Schichten behält der Bereich seine Schichthöhe und außerhalb bleiben sie adaptiv; mit `layer_height_max_change` verteilt sich der Wechsel auf mehrere Schichten statt einer dünnen Schicht an der Grenze; Objekt für Objekt gedruckt hat jedes seine eigenen Schichten). Modifier: ein nicht gedrucktes Netz, in dem das Objekt mit eigenen Werten druckt (`MODIFIER_SETTINGS`: Deck-/Bodenschichten, Infill-Dichte, -Muster außer Lightning, -Winkel und -Verbinden, Stützen an/aus); das Objekt behält seinen Bereich, seine Wände und seine Deck- und Bodenflächen, nur die Fläche innerhalb der Wände wird geteilt, sodass etwa ein Kern mit 100 % Infill keine zusätzlichen Schalen bekommt, und Objekte ohne Modifier ergeben denselben G-Code (`slicer_fdm::Modifier`, CLI `--modifier N:NETZ:DATEI` in den Koordinaten der Modelldatei, Server-Felder `modifier_<n>` und `modifier_settings_<n>`, mit `--project-settings` auch aus Teilen mit `subtype="modifier_part"` in `Metadata/model_settings.config`, deren Aufbau mangels echter Projektdatei mit Modifiern nur erschlossen ist); Modifier-, Negativ- und Stützblocker-Teile von Projekten werden nicht mehr mitgedruckt. Negativvolumen: ein Netz, das gleich nach dem Schneiden aus jeder Schicht seines Objekts abgezogen wird, sodass Wände um das Loch laufen, an seinen Enden Deck- und Bodenflächen entstehen und eine nach unten offene Tasche Stützen bekommt; es trifft nur sein eigenes Objekt, Objekte ohne Negativvolumen ergeben denselben G-Code (`OwnSettings::negatives`, CLI `--negative N:NETZ`, Server-Feld `negative_<n>`, aus Projekten die Teile mit `subtype="negative_part"` immer, auch ohne `--project-settings`); die App liest Projekte als ein Netz und zieht sie noch nicht ab. Stützblocker und -verstärker: nicht gedruckte Netze eines Objekts; Überhänge in einem Blocker oder darüber bekommen keine Stützen (so läuft keine Stütze durch ihn hindurch), Überhänge in einem Verstärker bekommen welche, auch mit `supports` aus und auch unter kurzen Brücken; wo sich beide überlappen, gilt der Blocker; sie wirken nur auf die Überhänge ihres eigenen Objekts, auch wo ein anderes Objekt in sie hineinreicht (Säulen unter dessen Überhängen laufen durch einen Blocker hindurch), und Äste von Baumstützen weichen jedem Blocker aus wie dem Modell, außer senkrecht unter gestützten Überhängen (`slicer_fdm::SupportVolume`, CLI `--support-blocker N:NETZ` und `--support-enforcer N:NETZ` in den Koordinaten der Modelldatei, Server-Felder `support_blocker_<n>` und `support_enforcer_<n>`, mit `--project-settings` auch aus Teilen mit `subtype="support_blocker"`/`"support_enforcer"`, deren Namen nur aus der Dokumentation erschlossen sind; ohne die Option meldet `model.support_parts_not_applied` sie). Offen: Überlappung je 3MF-Objekt, Anordnen mit exakten No-Fit-Polygonen nicht-konvexer Umrisse (Umrisse mit vielen Kerben wie Zahnräder werden bis auf zwölf Stücke aufgefüllt), Anordnen schneller auf Rechnern mit wenigen Kernen, Projekteinstellungen in der App, Wände und Naht je Modifier, Modifier in der App, Stützblocker und -verstärker in der App, Prüfung der Stützblocker-Teile an einer echten Projektdatei, Prüfung des Modifier-Aufbaus an einer echten Projektdatei, Schichten verschiedener Höhe nebeneinander (Schichthöhe nur für das Objekt des Höhenbereichs), Höhenbereiche in der App, Prüfung des Höhenbereich-Aufbaus an einer echten Projektdatei || Baumstützen | `support_style = tree`: Kontaktpunkte im Raster, Äste neigen sich je Schicht begrenzt zum Nachbarn, weichen dem Modell aus und verschmelzen; Dicke nach getragener Fläche. Benchy: 61 % weniger Stützmaterial, aber etwa 4 min längere Druckzeit wegen vieler dünner Spitzen. Äste weichen dem Modell der nächsten 16 Schichten schon vorher aus, ein festgefahrener Ast rückt je Schicht aus dem Modell heraus statt beschnitten senkrecht weiterzulaufen, und nach dem Verschmelzen wird ein Ast allmählich so dick, wie seine Last verlangt, und rückt dabei aus dem Modell heraus (eine Verdickung nach Länge wurde ausprobiert und wieder entfernt, sie kostete beim Benchy 5 cm Filament und 2 min); Benchy: festgefahrene Ast-Schichten 43 statt 93, Äste auf dem Modell 8 statt 12, Filament gesamt 3,93 statt 3,89 m, Druckzeit 1:13:21 statt 1:10:21, Slicen 0,6–0,8 s. Kontaktschichten sind ein eigener Linientyp (`PathKind::SupportInterface`, „Support interface“ im G-Code, eigene Farbe in Vorschau und Legende, eigene Zeit in der Statistik), für Säulen- wie Baumstützen, mit eigener Geschwindigkeit (`support_interface_speed`, 0 = `support_speed`), eigenem Lüfter (`support_interface_fan_speed`, -1 = Lüfter der Schicht), Muster (`support_interface_pattern`: `lines`, `concentric`, `grid`), Abstand (`support_interface_spacing`, 0 = zwei Linienbreiten) und optionalen Bodenkontaktschichten, wo eine Stütze auf dem Modell steht (`support_bottom_interface_layers`, Standard 0); mit den Standardwerten bleibt die Geometrie gleich, nur die Typkommentare ändern sich. Kontaktlinien drehen sich von Schicht zu Schicht um 90°, sodass jede Kontaktschicht die darunter kreuzt. Abstände zum Modell für Säulen- wie Baumstützen einstellbar: oben `support_top_z_distance`, unten `support_bottom_z_distance` (in mm, auf ganze Schichten von `layer_height` gerundet, mindestens eine; 0 = eine Schicht wie bisher) und seitlich `support_xy_distance` (0 = eine Linienbreite), aus Orca-Projekten übernommen; mit den Standardwerten bleiben die Stützen gleich. Benchy mit Stützen: 0,4 mm statt einer Schicht oben lässt eine Schicht mehr frei (oberste Stützschicht 181 statt 182, Stützfilament 614 statt 620 mm). Bei Schichten verschiedener Höhe (adaptiv oder Schichthöhe je Höhenbereich) zählen die Abstände oben und unten die Schichten am Modell, bis ihre Höhen den Abstand in mm erreichen; bei gleich hohen Schichten bleibt alles wie zuvor. Lösliche Stützen (`support_soluble`): 0 heißt dann gar kein Abstand oben, unten und zur Seite, die Kontaktschicht liegt direkt unter der getragenen Fläche, und der Randring einer Stütze unter einer Fläche wird Kontaktschicht; vor einer senkrechten Wand direkt unter der Fläche bleibt eine halbe Linienbreite frei. Stützen und Kontaktschichten drucken auf Wunsch mit eigenem Extruder (`support_extruder`, `support_interface_extruder`, ab 1 gezählt, 0 = wie bisher; aus Orca `support_filament`/`support_interface_filament`), mit Werkzeugwechseln und Prime-Tower; ein eigener Kontaktschicht-Extruder nimmt ebenfalls die Randringe unter Flächen. Benchy mit löslichen Stützen auf Extruder 2: 1:45:02 statt 1:09:02 mit einem Extruder, 5,46 m Filament auf Extruder 1 und 2,52 m auf Extruder 2 (zusammen 7,98 statt 4,36 m: der Unterschied und der größte Teil der längeren Zeit gehen in den Prime-Tower), 185 Werkzeugwechsel; nur die Kontaktschichten auf Extruder 2: 15 Wechsel, 0,16 m. Offen: Druckzeit der vielen dünnen Äste, eine halbe Linienbreite Abstand löslicher Stützen vor senkrechten Wänden, Randring als Kontaktschicht ohne lösliche Stützen oder eigenen Extruder, Säulenstützen anderer Objekte durch einen Blocker, unterste Kontaktschicht auf jeder zweiten Schicht parallel zu den Stützlinien darunter |
| Infill-Feinschliff | Gyroid ausgedünnt (Benchy 4,1 → 3,4 MB) und mit `infill_angle`; Rectilinear-Linien entlang der Wände verbunden (`infill_connect`, abschaltbar; Benchy: 3 % mehr Filament, deutlich weniger Leerwege); entlang massiven Infills und Brücken bleiben die Linien getrennt, damit sich nichts überlappt (Benchy gegenüber vorher 7 % mehr Leerwege, 1 % weniger Filament). Neue Muster `honeycomb` (Sechsecke, auf jeder Schicht gleich: Zickzacklinien plus kurze Querstücke, jede Kante einmal), `cubic` (drei Richtungen, die mit der Höhe seitlich wandern: auf der Ecke stehende Würfel) und `concentric`; dazu `top_surface_pattern`/`bottom_surface_pattern` mit `monotonic` (alle Linien gleich gerichtet, jede neben der vorigen) und `concentric`. Benchy (Grid 0,52 s, 2,49 MB): Honeycomb 0,52 s/2,62 MB, Cubic 0,53 s/2,49 MB, Concentric 0,63 s/2,66 MB, monotone Oberflächen 0,53 s/2,52 MB, konzentrische 0,59 s/2,56 MB. Neues Muster `lightning`: stützt nur die Oberflächen von unten. Von oben nach unten bekommt jede Schicht Stützpunkte im Linienabstand der Dichte unter den Flächen, über denen kein Sparse-Infill mehr liegt; jeder Punkt wird gerade mit der nächsten Wand bzw. dem massiven Infill oder einer schon gezogenen Linie verbunden (Umweg höchstens 1,5-fach), und nach unten wandert jeder Punkt pro Schicht um 45° auf sein Ziel zu, bis er es erreicht. So werden die Äste nach unten kürzer, und wo nichts darüber liegt, gibt es kein Infill. Benchy gegenüber Grid (15 %): Sparse-Infill 634 → 81 mm Filament, gesamt 3,65 → 3,25 m, Druckzeit 54:17 → 48:54 min, 2,49 → 2,44 MB, Slicen gleich schnell (0,63/0,64 s). |
| Druckoptionen (vorgezogen) | Mindestschichtzeit (`min_layer_time`, verlangsamt kleine Schichten bis `min_print_speed`; Probelauf der Schicht mit Zeitschätzung, bis zu vier Durchgänge wegen der Beschleunigung), Pressure Advance (`M900 K` bzw. `SET_PRESSURE_ADVANCE`), Elefantenfuß-Ausgleich (erste Schicht kleiner), Vasenmodus (`spiral_vase`: eine Wand, Z steigt entlang der Außenwand stetig, über dem Boden keine Füllung und kein Deckel) und Bügeln (`ironing`: neuer Linientyp über freiliegenden Oberseiten, wenig Material, eigene Geschwindigkeit). Dazu Wände nicht überqueren (`avoid_crossing_walls`: Leerfahrten folgen dem Rand des Teils um Löcher und Kerben, eine Viertel-Linienbreite innen, ohne Rückzug; Umwege über dem Dreifachen der geraden Strecke plus 10 mm fahren gerade), Fuzzy Skin (Punkte der Außenwand zufällig, aber reproduzierbar versetzt) und Wischen (`wipe_distance`: Rückzug während der Rückfahrt entlang der letzten Linie), Lüfter nach Schichtzeit (`fan_layer_time`: kürzere Schichten bekommen anteilig mehr Lüfter bis `fan_max_speed`, gemessen mit derselben Zeitschätzung wie die Mindestschichtzeit) und Überhänge (`overhang_speed`: Wandstücke, deren Linienmitte über den Rand der Schicht darunter ragt, also mehr als die halbe Linie frei hängt, werden in Stücken von höchstens 0,5 mm erkannt und als neuer Linientyp „Overhang wall“ langsam und mit `overhang_fan_speed` gedruckt; Benchy rechnet damit 0,56 statt 0,52 s; Überhänge unter 1 mm werden normale Wand, normale Stücke unter 1 mm zwischen zwei Überhängen werden Überhang, beim Benchy 123 statt 295 Überhangstücke); dazu eine zweite Stufe (`overhang_speed_full`, neuer Linientyp `OverhangWallFull`, im G-Code ebenfalls „Overhang wall“) für Wandstücke, deren Linienmitte mehr als eine halbe Linienbreite über den Rand ragt, also ganz frei hängt, mit demselben Ausgleich kurzer Stücke (beim Benchy ein Drittel der Überhangbewegungen). Bügeln nur der obersten Fläche (`ironing_type = top`): nur Teile des Modells, die auf der Schicht enden, also nichts mehr darüber haben; beim Benchy bügelt das nichts, weil der Schornstein auf dem Dach steht und sein Rand zu schmal ist (`all`: 28 Schichten). Bögen (`arc_fitting`): Folgen von mindestens drei kurzen Linien, deren Punkte und Linienmitten höchstens `arc_tolerance` (0,01 mm) von einem Kreis abweichen, werden gierig zu einem `G2`/`G3` zusammengefasst, ohne Winkelfunktionen; Filament und Zeitschätzung bleiben die der Linien, Klipper braucht `[gcode_arcs]`, und der G-Code von Benchy schrumpft damit von 2,49 auf 1,66 MB (von 85 321 auf 53 341 Zeilen, 8170 Bögen). Objekte benennen (`label_objects`): jede Bahn kennt ihr Objekt (`Path::object`, bei mehreren Objekten über die Bereiche je Objekt in jeder Schicht), Klipper bekommt `EXCLUDE_OBJECT_DEFINE` mit der konvexen Hülle der ersten Schicht und `EXCLUDE_OBJECT_START`/`_END` um die Bahnen jedes Objekts in jeder Schicht, Marlin `M486 T`/`S<n>`/`S-1`; die Fahrwege bleiben gleich. Alle standardmäßig aus; der G-Code der Referenzmodelle bleibt bis auf die Liste der Einstellungen am Dateiende gleich. Vasenmodus mit mehreren Inseln: jede Insel steigt als eigene Spirale von der Höhe der Schicht darunter auf ihre eigene, dazwischen Rückzug, Leerfahrt (mit `z_hop` angehoben) und Absenken; jede Insel beginnt neben ihrem Start auf der Schicht darunter (Zuordnung über die größte Überlappung, sonst den nächsten Schwerpunkt), jede Schicht mit der Insel, mit der die Schicht darunter endete; Löcher im Umriss fallen weg (`slicing.vase_holes`), Schichten mit Linien in zu schmalen Teilen werden flach gedruckt (`slicing.vase_islands`); Zeitschätzung und Vorschau steigen mit, eine einzelne Insel bleibt bytegleich. Offen: Umfahren von Inseln in Löchern, Geschwindigkeit je Überhangwinkel, Test am Drucker |
| Überlappende Wandschleifen | Bereiche von 1–2 Linienbreiten bekommen statt einer sich selbst überlappenden Schleife eine Linie eigener Breite (bis zwei Linienbreiten); Thingi10K 40176/40184 von 1,23- auf 1,00-faches Volumen. Offen: ob so breite Linien sauber drucken. Nebenbei geprüft: `gear_with_hub` schien nur das 0,83-fache auszugeben; tatsächlich zählt die Volumenformel des Netzes die Überlappung von Zahnrad und Nabe doppelt, gedruckt wird die Vereinigung zum 1,00-fachen (Test `overlapping_shells_extrude_their_union_once`) |
| Senden an Drucker | Moonraker (Klipper) und OctoPrint: CLI `--send`/`send`, Server `POST /v1/jobs/{id}/send` nur an freigegebene Drucker (`SPAGHETTI_PRINTERS`); nur gegen einen nachgebauten Drucker getestet. Bambu-LAN weiter bei T-004 |
| Phase 2 (vorgezogen) | Erste Desktop-App `slicer-ui` (egui/eframe mit OpenGL): STL/3MF laden, auf der Platte verschieben, drehen, skalieren, Einstellungsdateien und gängige Werte, Slicen im Hintergrund mit Abbrechen, Schichtvorschau, G-Code oder `.gcode.3mf` speichern; CI baut sie auf allen drei Systemen. Dazu 3D-Ansicht (Orbit-Kamera, schattiertes Modell, Bahnen bis zur gewählten Schicht), Browser-Version unter `/app/` (WASM, `SPAGHETTI_WEB_DIR`), Profilauswahl aus `profiles/`, Orca-Import, durchsuchbare Liste aller Einstellungen, Fortschrittsbalken (Fortschritt auch im Server-Status). Die 3D-Ansicht zeigt alle Modelle der Platte (ausgewähltes heller, nicht passende rot). Aufbau und Farben nach Orca Slicer (an Bildschirmfotos des laufenden Programms abgelesen, eigene Symbole, kein fremder Code und keine fremden Grafiken): Kopfleiste mit Datei-Menü, Prepare/Preview und Slice-/Export-Knöpfen, Seitenleiste mit Drucker, Filament, Objekten und Prozess-Einstellungen in Reitern und Gruppen mit Einheiten, 3D-Platte mit Werkzeugleiste, Auswahl per Klick und Verschieben per Ziehen, Zoom zum Mauszeiger, Vorschau mit Bahnen als beleuchtete 3D-Stränge auf der Grafikkarte, senkrechtem Schichtregler, Regler für die Bewegungen innerhalb der Schicht und Legende zum Ausblenden einzelner Linientypen, Meldungen unten rechts. Die 2D-Ansicht ist entfallen. Dazu Rückgängig/Wiederholen, Kopieren/Einfügen/Duplizieren, Tastenkürzel, Kontextmenü am Modell, Info-Box zum gewählten Objekt, Vorschau nach Linientyp, Geschwindigkeit, Schichthöhe oder Linienbreite, geschätzte Zeit je Linientyp (der Zeitschätzer zählt je Bewegung den Linientyp mit), Dialoge für Drucker- und Filament-Einstellungen mit „Als Profil speichern“. Einrichtungsassistent beim ersten Start (und im Datei-Menü) mit eingebautem Katalog: 66 Druckermodelle von 12 Herstellern mit ihren Düsengrößen, 11 Filamenttypen, Prozesse je Düse (Bauraum, Firmware, Extruder und Bewegungssystem nach Herstellerangaben; Geschwindigkeiten, Rückzug und Temperaturen als eigene Startwerte, an keinem dieser Drucker erprobt); Auswahl und benutzte Profile bleiben über den Neustart erhalten. Spiegeln (X/Y/Z), Auf Fläche legen, automatisches Ausrichten (wenig Überhang, viel Auflage) und Größe in mm. Griffe am Modell (Ring zum Drehen, Eckwürfel zum Skalieren), Schneiden in der Höhe mit geschlossenen Teilen (Deckel durch Triangulierung der Schnittkontur, auch bei überlappenden Körpern) und G-Code-Ansicht der Schicht in der Vorschau (Zeile zur Bewegung angenähert). Mehrere Platten: Leiste mit nummerierten Platten über der 3D-Ansicht (Hinzufügen, Wechseln, Entfernen per Rechtsklick), jede Platte mit eigenen Modellen und eigenem Slice-Ergebnis, „Auf Platte verschieben“ im Kontextmenü, Anordnen, Slicen und Export je Platte, ein gemeinsamer Rückgängig-Verlauf über alle Platten. Eigene Einstellungen je Modell („Object settings“ unter der Objektliste, mit Rückgängig, beim Kopieren und Schneiden übernommen). Offen: Griffe zum Verschieben in Z und Drehen um X/Y, schräge Schnitte und Verbinder, alle Platten auf einmal slicen, Geräte- und Projektseite, deutsche Oberfläche. Im Browser slict ein Web Worker mit demselben WebAssembly-Modul (Anfrage und Ergebnis als Bytes, Fortschritt als Nachrichten, Abbrechen beendet den Worker), die Seite bleibt bedienbar. Offen: im Browser gibt es keine Ordnerwahl und keinen Orca-Import; Fortschritt innerhalb des Netzschnitts; Installationspakete |
| R28 Kalibrierhilfen (vorgezogen) | Temperaturturm (ein Block je Temperatur, `M104` an der ersten Schicht jedes Blocks), Retraction-Test (zwei Türme, Rückzugslänge je Höhenband) und Flusstest (flache Blöcke, Fluss je Block): `calibrate temperature`, `retraction` oder `flow` in der CLI und Menü „Calibration“ in der App. Die Modelle werden normal gesliced, danach ändert ein Schritt den fertigen G-Code (Befehle nach dem Schichtwechsel, Extrusionsmengen je Band oder Block); anderer G-Code bleibt unverändert. Dazu ein Pressure-Advance-Turm (`calibrate pressure-advance`: hohler Turm 30 × 30 mm ohne Füllung und Deckel, der Faktor wechselt alle 4 mm mit `M900 K` bzw. `SET_PRESSURE_ADVANCE`; Standard 0 bis 0,08 in Schritten von 0,005, bei Bowden 0 bis 0,8 in Schritten von 0,05). Jeder Wert steht als erhabene Ziffern (eigene Balkenschrift, 5–6 mm hoch) am Modell: vorne auf jedem Block des Temperaturturms, auf einer Lasche neben jedem Retraction-Turm (Bänder abwechselnd links und rechts), oben auf jedem Flussblock und am Pressure-Advance-Turm abwechselnd vorne und rechts; die Bandgrenzen bleiben, wo sie waren. Offen: Test am Drucker |
| Projekte speichern | Erledigt: Datei → „Projekt speichern…“ (Strg+S) schreibt ein normales 3MF mit jedem Modell, wie es auf der Platte liegt (nach Schneiden, Spiegeln, Auf-Fläche-Legen), und seiner Lage als Build-Item-Transformation; Kopien teilen sich ein Objekt. Einstellungsdateien (Pfad oder `builtin:` und Inhalt), Werte der Seitenleiste, Änderungen in der Liste aller Einstellungen, Zusatzzeilen, Extruder je Dreieck und die genaue Lage stehen in `Metadata/spaghetti-project.toml` (TOML mit `version`; unbekannte Schlüssel werden ignoriert, fehlende ergänzt, damit Platten und Einstellungen je Objekt dazukommen können). „Projekt öffnen…“ (Strg+O) ersetzt Platte und Einstellungen; ein 3MF mit diesem Teil öffnet als Projekt, sonst als Modell; im Browser wird gespeichert per Download. Das Vorschaubild der ersten Platte (256×256, in Software gezeichnet) steht als `Metadata/thumbnail.png` mit der Thumbnail-Beziehung des 3MF-Formats im Projekt. Offen: Speichern ohne Nachfrage in die zuletzt geöffnete Datei, Projekte im CLI und Server |
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

## Später (nach den offenen Slicer-Aufgaben)

| ID | Typ | Titel | Größe | Hängt ab von |
|---|---|---|---|---|
| T-070 | Task | Nicht-planare Deckschichten: die obersten Schichten folgen flach geneigten Oberseiten statt Stufen zu bilden; Bahnen mit echten 3D-Punkten, Neigung begrenzt durch einen Düsenwinkel je Drucker, Kollisionsprüfung mit Düse und Lüfterhaube | XL | T-060 (Test am Drucker) |
| T-071 | Task | CMYK-Druck: Farben und Texturen aus 3MF/OBJ lesen und aus wenigen Grundfarben (CMY(K) und Weiß) durch Rastern und Schichten nachbilden; Mischdüse (`M163`/`M164`) oder Werkzeugwechsel (AMS) mit Spülmengen je Filamentpaar | XL | Farben in 3MF/OBJ, Spülmengen, T-004 (AMS) |

## Nicht in diesem Breakdown

Stützstrukturen (über die einfache Form hinaus), Nahtsteuerung, Brücken, UI, AMS, LAN-Versand, Resin: Phase 2–5 laut PRD. Ohne Stützen müssen die Referenzmodelle in T-007 so gewählt werden, dass sie ohne druckbar sind.
