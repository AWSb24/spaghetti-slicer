# PRD: Spaghetti Slicer

**Ein 3D-Druck-Slicer nach dem Vorbild von Orca Slicer, vollständig in Rust, für Desktop und Server.**

| | |
|---|---|
| Status | Entwurf v0.1 |
| Stand | 2026-11-11 |
| Rahmen | Solo-Projekt, Open Source |
| Detailtiefe | Phase 0–1 ausspezifiziert, Phase 2–5 als Fahrplan |
| Referenzhardware | Bambu Lab X1C (FDM) und Anycubic Photon Mono X (Resin); kein Marlin-/Klipper-Gerät |

---

## 1. Problem Statement

Die verbreiteten Open-Source-Slicer (Orca Slicer, Bambu Studio, PrusaSlicer) sind gewachsene C++-Desktop-Anwendungen, in denen Slicing-Kern und GUI eng verwoben sind. Wer Slicing automatisieren will (Druckfarm, Web-Dienst, CI für Druckteile, Homelab), muss eine GUI-Anwendung mit ihren Abhängigkeiten in einen Container zwängen und über eine nachträglich angebaute Kommandozeile steuern. Wer beitragen will, steht vor einer großen C++-Codebasis mit langer Build-Kette.

**Betroffen sind** Betreiber mehrerer Drucker und Automatisierer (bei jedem Druckauftrag), Homelab-Nutzer, die von jedem Gerät im Browser slicen wollen, sowie Entwickler, die Slicing als Bibliothek einbetten möchten.

**Kosten des Nichtlösens:** Automatisierung bleibt fragil, Server-Betrieb schwergewichtig, und es gibt keinen Slicer-Kern, der sich als Bibliothek, CLI, Dienst und App gleichermaßen nutzen lässt.

> **Evidenz-Hinweis:** Diese Problembeschreibung beruht auf der Projektidee und allgemeiner Kenntnis des Ökosystems, nicht auf eigener Nutzerforschung. Siehe offene Frage Q8.

## 2. Goals

| # | Ziel | Woran wir es erkennen |
|---|---|---|
| G1 | **Ein Kern, vier Auslieferungen.** Derselbe Slicing-Kern läuft als Bibliothek, CLI, Server-Dienst und UI (Desktop + Browser). | Kein Slicing-Code außerhalb der Kern-Crates; UI und Server rufen dieselbe öffentliche API auf. |
| G2 | **Druckbare Ergebnisse.** G-Code aus dem MVP druckt Referenzmodelle erfolgreich. | ≥ 9 von 10 Referenzdrucken ohne Abbruch und ohne sichtbare Fehlstellen (Definition in Abschnitt 6). |
| G3 | **Reproduzierbarkeit.** Gleiche Eingabe, gleiches Profil, gleiche Version ergeben auf Windows, Linux und macOS byte-identischen G-Code. | CI-Vergleich der Ausgabe über alle drei Plattformen. |
| G4 | **Leichter Server-Betrieb.** Der Container braucht keine GUI-Bibliotheken, keinen X-Server, keine GPU. | Image ≤ 50 MB (Ziel), startet in < 1 s, läuft als Nicht-Root. |
| G5 | **Konkurrenzfähige Geschwindigkeit.** Slicing der Referenzmodelle dauert nicht länger als in Orca bei vergleichbaren Einstellungen. | Benchmark-Suite in CI, Vergleichswerte dokumentiert. |

G1–G4 sind Nutzerziele. Projektziel (statt Geschäftsziel): eine Codebasis, die eine Einzelperson warten kann und in die Außenstehende einsteigen können.

## 3. Non-Goals

| Nicht-Ziel | Begründung |
|---|---|
| **Feature-Parität mit Orca in v1** | Orca enthält viele Personenjahre Arbeit. v1 beweist den Kern, nicht die Vollständigkeit. |
| **Portierung von Orca/libslic3r-Code** | Entscheidung für Clean-Room; AGPL-Code würde die Lizenz des Projekts festlegen. |
| **Hersteller-Cloud-Anbindung** (Bambu Cloud, MakerWorld, Prusa Connect) | Proprietäre, instabile Schnittstellen; kein Beitrag zum Kernproblem. |
| **Drucker-Fernsteuerung und Farm-Management** (Warteschlangen über Drucker, Kamera, Überwachung) | Eigene Produktkategorie; dafür existieren Werkzeuge, die den Slicer per API ansprechen können. |
| **Benutzerverwaltung und Mandantenfähigkeit im Server** | Der Server ist ein Dienst hinter einem Reverse-Proxy bzw. im vertrauenswürdigen Netz. Nur einfache Token-Authentifizierung. |
| **Modellieren und Mesh-Bearbeitung** über Platzieren/Skalieren/Drehen hinaus (Schneiden, Boolesche Operationen, Bemalen) | Großer Aufwand, für den Kernnutzen nicht nötig. Kandidat für später. |

## 4. Personas und User Stories

### Automatisierer / Druckfarm-Betreiber (primär für Phase 1)

- Als Betreiber mehrerer Drucker möchte ich ein Modell und ein Profil an eine HTTP-API schicken und G-Code zurückbekommen, damit ich Aufträge ohne manuelles Slicen einplanen kann.
- Als Automatisierer möchte ich denselben Slicer als einzelnes CLI-Programm in Skripten nutzen, damit ich keinen Dienst betreiben muss.
- Als Betreiber möchte ich bei gleicher Eingabe immer gleichen G-Code erhalten, damit ich Ergebnisse cachen und Abweichungen nachvollziehen kann.
- Als Automatisierer möchte ich bei ungültigem Modell oder Profil eine maschinenlesbare Fehlermeldung mit Ursache erhalten, damit mein Ablauf sinnvoll reagieren kann.
- Als Betreiber möchte ich Druckzeit und Materialverbrauch als strukturierte Daten erhalten, damit ich kalkulieren kann.

### Hobby-Drucker am Desktop (Phase 2)

- Als Hobby-Drucker möchte ich ein Modell auf die Druckplatte ziehen, einen Drucker und ein Material wählen und slicen, damit ich ohne Vorwissen zu druckbarem G-Code komme.
- Als Hobby-Drucker möchte ich das Ergebnis Schicht für Schicht ansehen, damit ich Probleme vor dem Druck erkenne.
- Als Hobby-Drucker möchte ich gewarnt werden, wenn ein Modell nicht auf die Platte passt oder Mesh-Fehler hat, damit ich keinen Fehldruck starte.
- Als Umsteiger möchte ich meine Orca-Profile importieren, damit ich nicht bei null anfange. *(Phase 3)*

### Homelab-Nutzer im Browser (Phase 2)

- Als Homelab-Nutzer möchte ich die Slicer-Oberfläche im Browser öffnen, damit ich von jedem Gerät slicen kann, ohne etwas zu installieren.
- Als Homelab-Nutzer möchte ich den Dienst mit einem einzigen `docker run` starten, damit die Einrichtung in Minuten erledigt ist.

### Entwickler / Beitragende

- Als Rust-Entwickler möchte ich den Kern als Crate einbinden, damit ich Slicing in eigene Werkzeuge integrieren kann.
- Als Beitragender möchte ich das Projekt mit `cargo build` ohne weitere Systemabhängigkeiten bauen, damit der Einstieg unter zehn Minuten dauert.

### Bambu-Lab-Nutzer (Phase 4) und Resin-Nutzer (Phase 5)

- Als Bambu-Nutzer möchte ich mehrfarbige Drucke mit AMS slicen und im LAN an den Drucker senden, damit ich nicht auf Bambu Studio angewiesen bin.
- Als Resin-Nutzer möchte ich Modelle aushöhlen, stützen und in einem von meinem Drucker gelesenen Format ausgeben, damit ich ein Werkzeug für beide Technologien habe.

## 5. Requirements

### Architektur-Leitplanken

Ein Cargo-Workspace; der Kern kennt weder UI noch Netzwerk noch Dateisystem.

| Crate | Aufgabe |
|---|---|
| `slicer-core` | Geometrie, Mesh, Schichtung, Polygon-Operationen |
| `slicer-fdm` | Perimeter, Infill, Stützen, Pfadplanung |
| `slicer-gcode` | G-Code-Erzeugung mit austauschbaren Flavors |
| `slicer-sla` | Resin-Pipeline (Phase 5) |
| `slicer-formats` | Modell- und Projektformate lesen/schreiben |
| `slicer-profiles` | Drucker-, Material- und Prozessprofile |
| `slicer-cli` | Kommandozeile |
| `slicer-server` | HTTP-API, liefert später die Web-UI aus |
| `slicer-ui` | Eine UI-Codebasis für Desktop (nativ) und Browser (WASM) |

- **UI-Technik (Empfehlung):** egui + wgpu. Baut nativ für alle drei Betriebssysteme und nach WebAssembly, sodass Desktop-App und Web-UI derselbe Code sind und alles Rust bleibt. Bestätigung in Q5.
- **Lizenz (entschieden):** MIT/Apache-2.0 dual. Bedingung: Clean-Room. Kein Code aus AGPL-Slicern; Algorithmen werden aus Veröffentlichungen, Dokumentation und eigenem Entwurf abgeleitet.

### P0 – Must-Have (Phase 0 und 1: Headless-FDM-MVP)

**R1 Modellimport.** STL (binär und ASCII) und 3MF (Geometrie) werden gelesen.
- Given eine gültige STL- oder 3MF-Datei, when sie geladen wird, then stehen Dreiecksnetz und Abmessungen zur Verfügung.
- Given eine beschädigte oder leere Datei, when sie geladen wird, then gibt es einen Fehler mit Ursache und keinen Absturz.
- Given ein Modell mit Löchern oder umgedrehten Flächen, when es geladen wird, then wird eine Warnung mit Art und Anzahl der Fehler ausgegeben; einfache Fälle werden repariert.

**R2 Schichtung.** Das Netz wird in Schichten konstanter Höhe mit geschlossenen Konturen zerlegt; erste Schicht separat einstellbar.
- Given ein wasserdichtes Modell, when es geschichtet wird, then ist jede Kontur geschlossen und frei von Selbstüberschneidungen.
- Given ein Modell höher als der Bauraum, when gesliced wird, then bricht der Vorgang mit klarer Fehlermeldung ab.

**R3 Perimeter.** Einstellbare Anzahl Wandlinien mit fester Linienbreite, Reihenfolge innen/außen wählbar.
- [ ] Wandstärke am Kalibrierwürfel weicht höchstens ±0,05 mm vom Sollwert ab.
- [ ] Bereiche schmaler als eine Linienbreite führen nicht zu Absturz oder Überextrusion (dürfen in v1 ausgelassen werden, mit Warnung).

**R4 Infill.** Mindestens geradlinig (rectilinear) und Gitter (grid), Dichte 0–100 %.
- [ ] Extrudiertes Volumen entspricht bei 100 % dem Modellvolumen auf ±3 %.
- [ ] Infill überlappt Perimeter um einen einstellbaren Betrag.

**R5 Deck- und Bodenschichten.** Massive Schichten oben und unten, Anzahl einstellbar; nach oben oder unten freiliegende Flächen werden automatisch erkannt.
- [ ] Kein sichtbares Infill an Außenflächen der Referenzmodelle.

**R6 Haftungshilfen.** Skirt und Brim, Abstand und Linienzahl einstellbar.

**R7 Bewegungen.** Retraction (Länge, Geschwindigkeit), optionaler Z-Hop, Geschwindigkeiten je Linientyp, Lüftersteuerung, Temperaturen für Düse und Bett inklusive abweichender erster Schicht.
- [ ] Kein Leerweg über Außenflächen ohne Retraction.

**R8 G-Code-Ausgabe.** Flavors Bambu (X1C, ein Material, ohne AMS), Marlin und Klipper; benutzerdefinierter Start- und End-G-Code mit Platzhaltern, relative oder absolute Extrusion. Der Bambu-Flavor ist der an Hardware abgenommene; Marlin und Klipper werden in Phase 1 nur über Regressionstests und G-Code-Prüfung abgesichert und im Release als „nicht an Hardware verifiziert" gekennzeichnet.
- Given ein für den X1C geslictes Referenzmodell, when die Ausgabe per SD-Karte auf den Drucker gebracht und gestartet wird, then druckt der X1C es ohne Bambu Studio vollständig durch. *(Dateiform siehe Q13.)*
- Given gleiche Eingabe, gleiches Profil, gleiche Version, when auf Windows, Linux und macOS gesliced wird, then ist der G-Code byte-identisch.
- [ ] Die Ausgabe enthält Kommentare zum Linientyp und einen Kopf mit Druckzeit-Schätzung und Materialverbrauch.
- [ ] Die Flavor-Schnittstelle ist so geschnitten, dass ein weiterer Flavor ohne Änderung an `slicer-fdm` ergänzt werden kann.

**R9 Profile.** Drei Ebenen (Drucker, Material, Prozess) in einem dokumentierten Textformat, mit Vererbung und Validierung.
- Given ein Profil mit unbekanntem Schlüssel oder Wert außerhalb des gültigen Bereichs, when es geladen wird, then nennt der Fehler Schlüssel, Wert und erlaubten Bereich.
- [ ] Mindestens ein mitgeliefertes, getestetes Profil pro Referenzdrucker.

**R10 CLI.** Ein einzelnes Programm ohne Laufzeitabhängigkeiten.
- Given Modell und Profile als Argumente, when `slice` ausgeführt wird, then entsteht eine G-Code-Datei und der Exit-Code ist 0.
- Given ein Fehler, when `slice` ausgeführt wird, then ist der Exit-Code ungleich 0 und die Ursache steht auf stderr; mit `--json` maschinenlesbar.
- [ ] Statistik (Zeit, Material, Schichtzahl) ist als JSON abrufbar.

**R11 HTTP-API.** Asynchrone Aufträge: anlegen (Modell + Profile), Status abfragen, Ergebnis und Statistik abholen, Auftrag abbrechen.
- Given ein gültiger Auftrag, when er angelegt wird, then antwortet der Server sofort mit einer Auftrags-ID und sliced im Hintergrund.
- Given ein Upload über dem konfigurierten Größenlimit, when er eintrifft, then wird er mit passendem HTTP-Status abgelehnt.
- Given ein konfiguriertes API-Token, when eine Anfrage ohne Token eintrifft, then wird sie abgelehnt.
- [ ] Gleichzeitige Aufträge sind auf eine konfigurierbare Zahl begrenzt, weitere warten.
- [ ] Die API ist als OpenAPI-Dokument beschrieben und versioniert (`/v1/...`).
- [ ] Health-Endpunkt für Container-Orchestrierung.

**R12 Docker-Image.** Mehrstufiger Build, minimales Laufzeit-Image, Nicht-Root, für amd64 und arm64.
- Given `docker run` mit Port-Freigabe, when der Container startet, then ist die API ohne weitere Konfiguration erreichbar.
- [ ] Konfiguration ausschließlich über Umgebungsvariablen.

**R13 Plattformen und CI.** Build und Tests laufen bei jeder Änderung auf Windows, Linux und macOS; Release-Binaries für alle drei.
- [ ] Regressionstests vergleichen G-Code der Referenzmodelle gegen gespeicherte Sollausgaben.

### P1 – Nice-to-Have (Phase 2 und 3)

**Phase 2: Oberfläche**
- **R14** Druckplatte mit 3D-Ansicht: Modelle laden (Dialog und Drag-and-drop), verschieben, drehen, skalieren, duplizieren, löschen, automatisch anordnen.
- **R15** Profilauswahl und Bearbeitung der Einstellungen mit Suche; geänderte Werte sind erkennbar.
- **R16** Slicen mit Fortschritt und Abbruch; die Oberfläche bleibt bedienbar.
- **R17** Schichtvorschau mit Schieberegler, Einfärbung nach Linientyp/Geschwindigkeit, Anzeige von Zeit und Material.
- **R18** Web-UI aus derselben Codebasis, vom Server ausgeliefert. *Abnahme: Der Ablauf Laden → Einstellen → Slicen → Vorschau → Export funktioniert ohne CLI auf allen drei Betriebssystemen und in aktuellen Versionen von Firefox und Chromium.*
- **R19** Projekt speichern und laden (3MF mit Platzierung und Einstellungen).
- **R20** Installierbare Pakete je Betriebssystem. macOS-Signierung und Notarisierung: siehe Q7.

**Phase 3: Druckqualität**
- **R21** Stützstrukturen (zuerst normal, dann baumartig), automatisch und mit Überhangswinkel.
- **R22** Nahtplatzierung (ausgerichtet, hinten, nächstgelegen, zufällig).
- **R23** Brückenerkennung mit eigenen Fluss- und Lüfterwerten.
- **R24** Variable Linienbreite für dünne Wände.
- **R25** Adaptive Schichthöhe.
- **R26** Weitere Füllmuster (Gyroid, Waben), Raft, Bügeln (ironing).
- **R27** Import von Orca-Profilen (JSON) mit Bericht über nicht übernommene Einstellungen.
- **R28** Kalibrierhilfen (Fluss, Temperaturturm, Pressure Advance).

*Abnahme Phase 3: Vergleichsdrucke gegen Orca auf einem festgelegten Satz von Testmodellen, Bewertung nach dokumentierter Checkliste.*

### P2 – Future Considerations (Phase 4 und 5)

Nicht in v1 gebaut, aber die Architektur muss sie zulassen:

| Thema | Was heute schon berücksichtigt wird |
|---|---|
| **Bambu Lab, erweitert** (kompatibles 3MF-Projekt, LAN-Versand; der einfarbige G-Code-Flavor ist bereits P0) | Ausgabe ist ein Container, nicht zwingend eine einzelne G-Code-Datei. |
| **Mehrmaterial / AMS** (Werkzeugwechsel, Wischturm, Spülmengen) | Datenmodell führt von Anfang an eine Material-/Extruder-Zuordnung pro Objekt und Bereich, auch wenn v1 nur einen Extruder nutzt. |
| **Resin/SLA** (Schichtbilder, Resin-Stützen, Aushöhlen, Ablauflöcher) | `slicer-core` liefert technologieneutrale Schichtkonturen; alles FDM-Spezifische liegt in `slicer-fdm`. Profile haben eine Technologie-Kennung. |
| **Mesh-Bearbeitung** (Schneiden, Bemalen von Naht/Stützen) | Modifikatoren pro Bereich sind im Datenmodell vorgesehen. |
| **STEP-Import** | Importer sind über eine gemeinsame Schnittstelle austauschbar. |

## 6. Success Metrics

Zielwerte sind **Hypothesen** und werden nach den ersten Messungen angepasst.

### Leading (pro Release messbar)

| Metrik | Erfolg | Stretch | Messung |
|---|---|---|---|
| Erfolgreiche Referenzdrucke | ≥ 9/10 | 10/10 | Benchy, 20-mm-Würfel und drei weitere Modelle, je zweimal auf dem Referenzdrucker; Erfolg = durchgedruckt, maßhaltig ±0,2 mm, keine Löcher/Fäden an Außenflächen |
| Slicing-Zeit Benchy (0,2 mm) | ≤ Orca | ≤ 50 % von Orca | Benchmark in CI, gleicher Rechner, vergleichbare Einstellungen |
| Speicherbedarf beim Slicen | ≤ Orca | ≤ 50 % von Orca | Spitzenwert im Benchmark |
| Plattformübergreifend identischer G-Code | 100 % der Referenzmodelle | – | CI-Vergleich |
| Docker-Image-Größe | ≤ 50 MB | ≤ 20 MB | Release-Pipeline |
| Robustheit | 0 Abstürze im Testkorpus | – | Korpus aus ≥ 200 frei lizenzierten Modellen plus Fuzzing der Importer |
| Build-Einstieg | `cargo build` ohne weitere Schritte | – | Frischer Checkout auf drei Betriebssystemen |

### Lagging (über Monate)

| Metrik | Ziel 6 Monate nach erstem Release | Ziel 12 Monate |
|---|---|---|
| Externe Beitragende mit gemergter Änderung | ≥ 3 | ≥ 10 |
| Getestete Druckerprofile | ≥ 5 | ≥ 20 |
| Meldungen „Fehldruck durch Slicer-Fehler" je Release | fallend | – |
| Bekannte Einbindungen der API/Bibliothek in andere Projekte | ≥ 1 | ≥ 3 |

## 7. Open Questions

| # | Frage | Wer | Blockierend? |
|---|---|---|---|
| Q1 | ~~Welche Drucker stehen als Referenz zur Verfügung?~~ **Beantwortet:** Bambu Lab X1C (FDM) und Anycubic Photon Mono X (Resin). Kein Marlin-/Klipper-Gerät. | Projektinhaber | Erledigt |
| Q13 | In welcher Dateiform nimmt der X1C fremden G-Code von der SD-Karte an (reine `.gcode`-Datei oder `.gcode.3mf`-Container), und welche Start-/Endsequenz braucht er mindestens (Aufheizen, Homing, Bettnivellierung)? Die Sequenz muss selbst erarbeitet oder aus eigenen Nutzerprofilen stammen, nicht aus AGPL-Quellen kopiert werden. | Engineering (Versuch am Gerät) | **Ja, für Phase 1** – Zeitbox: wenige Tage |
| Q14 | Soll für die Hardware-Abnahme von Marlin/Klipper ein günstiger Zweitdrucker beschafft werden, oder reicht Verifikation durch die Community? | Projektinhaber | Nein |
| Q2 | Polygon-Operationen (Offset, Vereinigung, Schnitt): vorhandenes Rust-Crate oder Eigenbau? Robustheit und Ganzzahl-Koordinaten sind Voraussetzung für G3. | Engineering | **Ja, für Phase 0** – Zeitbox: eine Woche Evaluierung |
| Q3 | ~~Lizenz endgültig: MIT/Apache-2.0 oder bewusst Copyleft?~~ **Beantwortet:** MIT oder Apache-2.0 (Doppellizenz). Clean-Room-Regeln stehen in `CONTRIBUTING.md`. | Projektinhaber | Erledigt |
| Q4 | Was ist an Bambu-Druckern ohne das proprietäre Netzwerk-Plugin möglich (LAN-Modus), und wie stabil ist das angesichts der Firmware- und Autorisierungsänderungen des Herstellers? | Engineering (Recherche) | Ja, für Phase 4 |
| Q5 | UI-Technik bestätigen: Reicht egui für eine Oberfläche mit sehr vielen Einstellungen, und ist die 3D-Leistung im Browser (WebGPU/WebGL-Fallback) ausreichend? | Engineering | Ja, für Phase 2 – Prototyp |
| Q6 | Dürfen die mit Orca ausgelieferten Druckerprofile übernommen werden, oder nur der Import von Nutzerprofilen? | Recht/Lizenz | Nein (betrifft R27) |
| Q7 | macOS-Signierung/Notarisierung und Windows-Code-Signing kosten Geld. Budget vorhanden, oder unsignierte Builds? | Projektinhaber | Nein (R20) |
| Q8 | Gibt es konkrete Nutzer (eigene Drucker, Bekannte, Community), deren Bedarf die Priorisierung prüfen kann? | Projektinhaber | Nein |
| Q9 | Resin-Ausgabeformat: Durch den Photon Mono X ist das Zielformat das Anycubic-eigene Dateiformat dieses Druckers (nach aktuellem Kenntnisstand `.pwmx`, am Gerät zu bestätigen). Gibt es dafür eine frei nutzbare Formatbeschreibung, oder muss das Format aus eigenen Beispieldateien erschlossen werden? Vorhandene Community-Werkzeuge auf ihre Lizenz prüfen, bevor deren Quelltext gelesen wird (Clean-Room). | Engineering (Recherche) | Ja, für Phase 5 |
| Q10 | Rechnet die Web-UI im Browser (WASM) oder schickt sie Aufträge an den Server? Beides hat Folgen für Speicherlimits und Offline-Fähigkeit. | Engineering | Nein (Phase 2) |
| Q11 | STEP-Import gewünscht? In Rust derzeit nur eingeschränkt abgedeckt. | Projektinhaber | Nein |
| Q12 | Projektname: Bleibt es bei „Spaghetti Slicer"? | Projektinhaber | Nein |

## 8. Timeline Considerations

**Keine harten Termine.** Die Phasen sind eine Reihenfolge, kein Kalender.

| Phase | Inhalt | Abnahme | Abhängigkeiten |
|---|---|---|---|
| **0 Fundament** | Workspace, CI, Modellimport, Polygon-Grundlage | CI grün auf drei Plattformen, R1 erfüllt | Q2 |
| **1 Headless-FDM-MVP** | R2–R13, inklusive einfarbigem Bambu-Flavor | Referenzdrucke auf dem X1C bestanden (G2) | Q13 |
| **2 Oberfläche** | R14–R20 | Ablauf ohne CLI auf drei OS und im Browser | Q5, Q10 |
| **3 Druckqualität** | R21–R28 | Vergleichsdrucke gegen Orca | Phase 1 stabil |
| **4 Bambu Lab, erweitert** | 3MF-Projekt, AMS-Mehrfarbdruck, LAN-Versand | Mehrfarbdruck ohne Bambu Studio | Q4; AMS am X1C nötig |
| **5 Resin** | SLA-Pipeline, Ausgabe im Format des Photon Mono X | Erfolgreicher Druck auf dem Anycubic Photon Mono X | Q9 |

### Risiken

- **Umfang für eine Person (größtes Risiko).** Der Gesamtumfang entspricht einem mehrjährigen Vorhaben. Gegenmaßnahme: Jede Phase liefert für sich Nutzen; nach Phase 1 existiert ein eigenständig brauchbares Produkt (Slicing-API/CLI). Phase 4 und 5 sind die ersten Streichkandidaten.
- **Geometrische Robustheit.** Reale Modelle sind fehlerhaft; die meisten Slicer-Fehler entstehen hier. Gegenmaßnahme: Testkorpus und Fuzzing ab Phase 0.
- **Clean-Room-Disziplin.** Wer zum Nachschlagen in AGPL-Quelltext schaut, gefährdet die Lizenzwahl. Gegenmaßnahme: Regel in den Beitragsrichtlinien, Quellenangaben für Algorithmen.
- **Bambu als bewegliches Ziel.** Der Hersteller kann Schnittstellen jederzeit ändern (Q4). Weil der X1C die einzige Referenzhardware ist, trifft das bereits das MVP: Die Abnahme von Phase 1 hängt daran, dass der Drucker fremden G-Code von der SD-Karte annimmt (Q13).
- **Marlin/Klipper ohne Hardware-Abnahme.** Die generischen Flavors können Fehler enthalten, die erst an einem echten Gerät auffallen. Gegenmaßnahme: klare Kennzeichnung im Release, Prüfung durch Community-Tester oder Zweitdrucker (Q14).
- **Scope Creep.** Regel: Jede Ergänzung zu P0 braucht eine Streichung in P0 oder wandert nach P1. Ideen landen im Parkplatz unten.

### Parkplatz

Ideen außerhalb des aktuellen Umfangs: Plugin-System, Skriptbare Nachbearbeitung, Kostenkalkulation, G-Code-Viewer als eigenständiges Werkzeug, Anbindung an Klipper/Moonraker und OctoPrint zum Hochladen, Belt-Drucker, nicht-planares Slicen.
