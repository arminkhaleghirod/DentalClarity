# Open Dental connector

The `opendental` connector in `crates/dc-pms` reads treatment plans from an Open Dental MySQL or MariaDB database over a read-only user (DECISIONS D7, D13, D14). This page records how each Open Dental table is read, where that reading comes from, and what still has to be checked on a live install.

## Status

Every answer below comes from Open Dental's published database documentation for version 24.1. None of it has been checked against a running Open Dental install with its demo database yet, so the M2 acceptance item for that check is still open. Until it is done, treat each rule as "from the published schema, not yet verified against a live install".

Supported versions: the queries are written against the 24.1 schema. The intended range is Open Dental 24.1 and later 24.x releases, on MySQL 5.7 or 8.0 or on MariaDB 10.x. That range is a target, not a tested claim. The `PreferredName` column on `provider` and `ProcStatus` 8 (treatment planned, inactive) may not exist in older versions; check both before widening the range.

## Sources

- Open Dental database documentation, version 24.1 (XML): https://www.opendental.com/OpenDentalDocumentation24-1.xml. Read on 2026-11-28. The tables used here are `appointment`, `claimproc`, `definition`, `patient`, `procedurecode`, `procedurelog`, `procnote`, `proctp`, `provider`, `treatplan` and `treatplanattach`, and the enums `ApptStatus`, `ClaimProcStatus`, `DefCat`, `PatientStatus`, `ProcStat`, `TreatmentArea`, `TreatPlanStatus` and `TreatPlanType`.

## Checklist

Each item is the M2 checklist from [IMPLEMENTATION.md](IMPLEMENTATION.md). Every answer is from the published schema, not yet verified against a live install.

### Treatment plans and their procedures

`treatplan` holds three kinds of plan, told apart by `TPStatus`: 0 saved, 1 active (one per patient) and 2 inactive. Active and inactive plans point to live procedures through `treatplanattach` (`TreatPlanNum`, `ProcNum`, `Priority`). Open Dental removes attach rows for procedures that are completed or deleted when the treatment plan or chart module opens. Saved plans hold frozen copies in `proctp` (`ProcNumOrig` points back to `procedurelog`), with the tooth already converted to a display string and the amounts copied as they were when saved.

The connector lists active and inactive plans with at least one planned procedure, active first. Saved plans are not read yet. The plan name is `treatplan.Heading`, or "Active treatment plan" or "Inactive treatment plan" when the heading is blank.

To check on a live install: that a signed plan stays reachable as an active plan, or whether practices present from saved plans and the connector needs to read `proctp`.

### Procedure status

`procedurelog.ProcStatus` (`ProcStat`): 1 treatment planned, 2 complete, 3 existing (current provider), 4 existing (other provider), 5 referred out, 6 deleted, 7 condition, 8 treatment planned on an inactive plan. Open Dental has no "scheduled" status. A planned procedure is scheduled when `AptNum` points to an appointment; `PlannedAptNum` is a separate link to a planned appointment.

The connector reads procedures with status 1 or 8. It reports a procedure as scheduled when its `AptNum` appointment has `AptStatus` 1 (scheduled) and as planned otherwise. Completed procedures never reach a snapshot. Rows with `ProcNumLab` set are Canadian lab fees that belong to another procedure and are skipped.

To check: whether procedures on an inactive plan carry status 8, 1 or either.

### Tooth, surface and area

`procedurelog.ToothNum` holds a Universal number, 1 to 32 or A to T, or is blank. `Surf` holds surfaces, or a quadrant (`UR`, `UL`, `LR`, `LL`), a sextant (`1` to `6`) or an arch (`U`, `L`) depending on the code. `ToothRange` is a comma-separated list of tooth numbers without dashes. `procedurecode.TreatArea` (`TreatmentArea`) says which applies: 0 none, 1 surface, 2 tooth, 3 mouth, 4 quadrant, 5 sextant, 6 arch, 7 tooth range.

The connector maps a quadrant code's `Surf` to a quadrant and an arch code's `Surf` to an arch. Every other code uses `ToothNum`, with surfaces kept only for surface codes. Sextant, tooth range and whole-mouth codes have no tooth or area the app can draw, so they show as no specific tooth. A tooth number outside 1 to 32 and A to T, such as a supernumerary tooth, fails the snapshot instead of showing no tooth.

To check: the quadrant and arch strings on real rows, how international tooth numbering is stored (the schema says `ToothNum` is always stored in Universal form), and supernumerary numbers.

### Priority and phase

`procedurelog.Priority` and `treatplanattach.Priority` are foreign keys to `definition.DefNum` in category 20 (`TxPriorities`, "treatment plan priority names"). The attach row's priority lets one procedure have a different priority in each plan.

The connector uses the attach row's priority, then the procedure's own. The phase is the definition's `ItemName` when that is a number, otherwise its position in the list (`ItemOrder` plus 1). Procedures are ordered by priority, with no priority last, then by `ProcNum`.

To check: the default priority names in a fresh install, and the order Open Dental's treatment plan module uses within a priority.

### Insurance estimates, write-offs and patient portion

Estimates for planned procedures live in `claimproc` rows with `Status` 6 (estimate), one row per insurance plan; `Status` 8 is a capitation estimate. `InsEstTotal` is the final insurance estimate after deductible, other insurance and annual maximum; `InsEstTotalOverride` replaces it unless it is -1. `WriteOffEst` is the estimated write-off, usually for PPO plans, and `WriteOffEstOverride` replaces it unless it is -1. `NoBillIns` marks a row not billed to that plan. The fee is `procedurelog.ProcFee`, `Discount` is a dollar amount, and `UnitQty` and `BaseUnits` give the units.

The connector computes:
- fee: `ProcFee` times `UnitQty + BaseUnits` (at least 1);
- insurance estimate: the sum over estimate rows of the override or `InsEstTotal`, counting `NoBillIns` rows as 0;
- patient estimate: fee minus discount, insurance estimate and estimated write-off.

With no estimate row, a capitation estimate, or a negative result, both estimates are unknown and the app shows them as such instead of guessing. Amounts are doubles in dollars and are rounded to whole cents.

To check: that the patient estimate matches the "Pat" column in Open Dental's treatment plan module for primary plus secondary insurance, PPO write-offs, discount plans and capitation, and how a patient with no insurance looks (no estimate rows makes both estimates unknown today).

### Patient language

`patient.Language` holds "typically eng (English), fra (French), spa (Spanish), or similar", which are ISO 639-2 codes, or free text for a custom language. The connector shortens codes with an ISO 639-1 form to it (`eng` to `en`), keeps other three-letter codes, and drops free text.

To check: the values Open Dental writes for its language list and for custom languages.

### Procedure notes

`procnote` holds notes for one procedure (`ProcNum`). Open Dental never edits or deletes a note; an edit adds a newer row. The connector uses the newest row per procedure by `EntryDateTime`, then `ProcNoteNum`. A blank newest note means the note was cleared, and no note is used. Only notes on procedures in the plan are read (DECISIONS P11). `procnote.Signature` is never read.

To check: whether staff write the reasoning for planned treatment in procedure notes, or elsewhere (commlog, chart notes), and whether notes on planned procedures exist before the procedure is completed.

### Appointments and the recommended list

`appointment` has `PatNum`, `AptDateTime` and `AptStatus` (`ApptStatus`): 1 scheduled, 2 complete, 3 unscheduled list, 5 broken, 6 planned, 7 and 8 patient notes. The recommended list holds current patients (`PatStatus` 0) with a scheduled appointment today and at least one planned procedure on an active or inactive plan, in order of their earliest appointment today. "Today" is the database server's date (`CURDATE()`), which is the practice's local date when Open Dental's database runs in the office.

To check: whether checked-in or seated patients keep `AptStatus` 1, and the server time zone on a hosted database.

### Patients and providers

`patient.PatStatus` (`PatientStatus`): 0 patient, 1 non-patient, 2 inactive, 3 archived, 4 deleted, 5 deceased, 6 prospective. The connector shows status 0 only. Age is computed from `Birthdate`; Open Dental's blank date is `0001-01-01`. A patient under 18 gets a guardian presentation. The first name is `FName`; the nickname in `Preferred` is not used yet.

`provider.PreferredName` is the name Open Dental shows patients. The plan's provider is the one on most of its procedures. It is shown by `PreferredName`, or as "Dr." and `LName`, and left out for hygienists (`IsSecondary`) and placeholder providers (`IsNotPerson`).

## Read-only access

The connector refuses to start when `SHOW GRANTS` lists any privilege other than `SELECT` and `USAGE`, a grant option, or a role grant. Each connection runs `SET SESSION TRANSACTION READ ONLY`. The grants it needs are in `fixtures/opendental/readonly_user.sql` and in [SECURITY.md](SECURITY.md), with column-level grants on `patient`, `provider`, `procnote`, `appointment` and `definition` so identifiers such as the SSN and signatures stay out of reach. Every statement is in `crates/dc-pms/src/opendental/queries.rs`.

## Configuration

Clinical mode only; demo mode refuses it.

| Variable | Default |
|---|---|
| `DC_PMS` | `synthetic`; set `opendental` |
| `DC_OPENDENTAL_HOST` | `127.0.0.1` |
| `DC_OPENDENTAL_PORT` | `3306` |
| `DC_OPENDENTAL_DATABASE` | `opendental` |
| `DC_OPENDENTAL_USER` | required |
| `DC_OPENDENTAL_PASSWORD` | required |
| `DC_OPENDENTAL_TLS` | `preferred` on this machine, `verify-identity` for another host |
| `DC_OPENDENTAL_TLS_CA` | unset (the operating system's trust store) |
| `DC_PRACTICE_NAME` | `Your practice` until set in Settings |

A database on another host needs `required`, `verify-ca` or `verify-identity`. The password moves to the encrypted `secrets` table when that lands.

## Synthetic database and tests

`fixtures/opendental` holds a schema subset (`schema.sql`), synthetic data that mirrors `fixtures/synthetic/practice.toml` (`seed.sql`), and the read-only user (`readonly_user.sql`). The seed adds rows the connector must leave out: a deleted patient with a current patient's name, a saved plan, completed and deleted procedures, a lab fee row, a broken appointment, a superseded and a cleared note, a received claim row and a do-not-bill row.

`crates/dc-pms/tests/contract.rs` runs the same contract against the TOML connector and, when `DC_TEST_MYSQL_URL` is set, against the synthetic database, and checks that both return the same patients and plans apart from source ids. `DC_TEST_MYSQL_ADMIN_URL`, if set, checks that a user who can write is refused and that the session guard blocks writes. CI loads the seed into MySQL 8.0 and MariaDB 10.11 and runs both.

To run them locally:

```sh
docker run -d --name od-test -e MYSQL_ROOT_PASSWORD=local-root -p 3306:3306 mysql:8.0
for f in schema seed readonly_user; do
  mysql -h 127.0.0.1 -u root -plocal-root < fixtures/opendental/$f.sql
done
DC_TEST_MYSQL_URL=mysql://dentalclarity_ro:synthetic-test-only@127.0.0.1:3306/opendental \
DC_TEST_MYSQL_ADMIN_URL=mysql://root:local-root@127.0.0.1:3306/opendental \
  cargo test -p dc-pms --features opendental
```

The seed dates ages and appointments from the day it is loaded, so reload it on a later day.
