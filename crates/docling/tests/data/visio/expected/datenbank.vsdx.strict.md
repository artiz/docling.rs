# Zeichenblatt-1

Mitarbeiter

Adresse

Kontakt

ID: INT

Ortskennzahl: varchar

Telefonnummer: varchar

ID: INT

Vorname: varchar

ID: INT

Straße: varchar

Nachname: varchar

Ort: varchar

Startdatum: datetime

Land: varchar

Enddatum: datetime

PLZ: varchar

Gehalt: dezimal

VorgesetztenID: INT

Projekt

Typ: varchar

Name: varchar

Budget: dezimal

LeiterID: INT

ProjektMitarbeiter

ID: INT

ProjektID: INT

MitarbeiterID: INT

ID: INT

| From            | To                 | Label   |
|-----------------|--------------------|---------|
| Mitarbeiter     | Kontakt            | 1 *     |
| Straße: varchar | Mitarbeiter        | 1 1     |
| Mitarbeiter     | Projekt            | 1 *     |
| Mitarbeiter     | ProjektMitarbeiter | *       |
