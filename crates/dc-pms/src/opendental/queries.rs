//! Every SQL statement the Open Dental connector sends.
//!
//! All of them are `SELECT` or `SHOW` statements. Column meanings and enum
//! values come from the published Open Dental 24.1 schema; docs/OPENDENTAL.md
//! lists them with their sources and what is still unverified.
//!
//! Values that recur:
//! - `patient.PatStatus = 0` is a current patient.
//! - `appointment.AptStatus = 1` is a scheduled appointment.
//! - `treatplan.TPStatus` 1 is the active plan and 2 an inactive plan.
//!   Saved plans (0) hold frozen `proctp` copies and are not read.
//! - `procedurelog.ProcStatus` 1 is treatment planned and 8 is treatment
//!   planned on an inactive plan. `ProcNumLab <> 0` marks a Canadian lab fee
//!   row, which belongs to another procedure.
//! - `definition.Category = 20` holds the treatment plan priorities.
//! - `claimproc.Status` 6 is an estimate and 8 a capitation estimate.
//! - Birthdates before 1880 are Open Dental's blank date (`0001-01-01`).

/// Session guard run on every new connection, on top of the user's grants.
pub const READ_ONLY_SESSION: &str = "SET SESSION TRANSACTION READ ONLY";

pub const SHOW_GRANTS: &str = "SHOW GRANTS";

/// Patients matching a name fragment or an exact patient number.
///
/// Binds: the lowered query, a `LIKE` pattern escaped with `!`, and the
/// patient number or -1.
pub const SEARCH_PATIENTS: &str = r"
SELECT p.PatNum, p.FName, p.LName,
       CASE WHEN p.Birthdate > '1880-01-01'
            THEN TIMESTAMPDIFF(YEAR, p.Birthdate, CURDATE()) END AS Age,
       p.Language,
       (SELECT DATE_FORMAT(MIN(a.AptDateTime), '%H:%i')
          FROM appointment a
         WHERE a.PatNum = p.PatNum AND a.AptStatus = 1
           AND a.AptDateTime >= CURDATE()
           AND a.AptDateTime < CURDATE() + INTERVAL 1 DAY) AS AptTime
  FROM patient p
 WHERE p.PatStatus = 0
   AND (? = '' OR LOWER(CONCAT(p.FName, ' ', p.LName)) LIKE ? ESCAPE '!' OR p.PatNum = ?)
 ORDER BY p.LName, p.FName, p.PatNum
 LIMIT 100";

/// Patients with a scheduled appointment today and at least one planned
/// procedure on an active or inactive plan, in appointment order.
pub const RECOMMENDED_PATIENTS: &str = r"
SELECT p.PatNum, p.FName, p.LName,
       CASE WHEN p.Birthdate > '1880-01-01'
            THEN TIMESTAMPDIFF(YEAR, p.Birthdate, CURDATE()) END AS Age,
       p.Language,
       DATE_FORMAT(MIN(a.AptDateTime), '%H:%i') AS AptTime
  FROM patient p
  JOIN appointment a ON a.PatNum = p.PatNum
 WHERE p.PatStatus = 0
   AND a.AptStatus = 1
   AND a.AptDateTime >= CURDATE()
   AND a.AptDateTime < CURDATE() + INTERVAL 1 DAY
   AND EXISTS (
       SELECT 1
         FROM treatplan t
         JOIN treatplanattach ta ON ta.TreatPlanNum = t.TreatPlanNum
         JOIN procedurelog pl ON pl.ProcNum = ta.ProcNum AND pl.PatNum = t.PatNum
        WHERE t.PatNum = p.PatNum
          AND t.TPStatus IN (1, 2)
          AND pl.ProcStatus IN (1, 8)
          AND pl.ProcNumLab = 0)
 GROUP BY p.PatNum, p.FName, p.LName, p.Birthdate, p.Language
 ORDER BY MIN(a.AptDateTime), p.PatNum";

/// Binds: the patient number.
pub const PATIENT_EXISTS: &str = r"
SELECT p.PatNum FROM patient p WHERE p.PatNum = ? AND p.PatStatus = 0";

/// Active and inactive plans with at least one planned procedure, active
/// plan first. Binds: the patient number.
pub const TREATMENT_PLANS: &str = r"
SELECT t.TreatPlanNum, t.Heading, CAST(t.TPStatus AS SIGNED) AS TPStatus,
       COUNT(DISTINCT pl.ProcNum) AS Procs
  FROM treatplan t
  JOIN treatplanattach ta ON ta.TreatPlanNum = t.TreatPlanNum
  JOIN procedurelog pl ON pl.ProcNum = ta.ProcNum AND pl.PatNum = t.PatNum
 WHERE t.PatNum = ?
   AND t.TPStatus IN (1, 2)
   AND pl.ProcStatus IN (1, 8)
   AND pl.ProcNumLab = 0
 GROUP BY t.TreatPlanNum, t.Heading, t.TPStatus, t.DateTP
 ORDER BY t.TPStatus = 1 DESC, t.DateTP DESC, t.TreatPlanNum DESC";

/// Binds: the plan number.
pub const PLAN_HEADER: &str = r"
SELECT t.Heading, CAST(t.TPStatus AS SIGNED) AS TPStatus,
       p.PatNum, p.FName, p.LName,
       CASE WHEN p.Birthdate > '1880-01-01'
            THEN TIMESTAMPDIFF(YEAR, p.Birthdate, CURDATE()) END AS Age,
       p.Language
  FROM treatplan t
  JOIN patient p ON p.PatNum = t.PatNum
 WHERE t.TreatPlanNum = ?
   AND t.TPStatus IN (1, 2)
   AND p.PatStatus = 0";

/// Planned procedures on one plan, in priority order. The per-plan priority
/// on `treatplanattach` wins over the procedure's own. Binds: the plan number.
pub const PLAN_PROCEDURES: &str = r"
SELECT pl.ProcNum, pc.ProcCode, pc.Descript, pc.LaymanTerm,
       CAST(pc.TreatArea AS SIGNED) AS TreatArea,
       pl.ToothNum, pl.Surf, pl.ProcFee,
       CAST(pl.UnitQty AS SIGNED) AS UnitQty,
       CAST(pl.BaseUnits AS SIGNED) AS BaseUnits,
       pl.Discount,
       CAST(pl.ProcStatus AS SIGNED) AS ProcStatus,
       CAST(a.AptStatus AS SIGNED) AS AptStatus,
       d.ItemName AS PriorityName,
       CAST(d.ItemOrder AS SIGNED) AS PriorityOrder,
       pl.ProvNum
  FROM treatplanattach ta
  JOIN treatplan t ON t.TreatPlanNum = ta.TreatPlanNum
  JOIN procedurelog pl ON pl.ProcNum = ta.ProcNum AND pl.PatNum = t.PatNum
  JOIN procedurecode pc ON pc.CodeNum = pl.CodeNum
  LEFT JOIN appointment a ON a.AptNum = pl.AptNum AND pl.AptNum <> 0
  LEFT JOIN definition d
         ON d.Category = 20
        AND d.DefNum = CASE WHEN ta.Priority <> 0 THEN ta.Priority ELSE pl.Priority END
 WHERE ta.TreatPlanNum = ?
   AND pl.ProcStatus IN (1, 8)
   AND pl.ProcNumLab = 0
 ORDER BY d.DefNum IS NULL, d.ItemOrder, pl.ProcNum";

/// Insurance estimates per procedure on one plan, summed over every
/// insurance plan. A `-1` override means the override is blank.
/// Binds: the plan number.
pub const PLAN_ESTIMATES: &str = r"
SELECT cp.ProcNum,
       SUM(CASE WHEN cp.NoBillIns <> 0 THEN 0
                WHEN cp.InsEstTotalOverride <> -1 THEN cp.InsEstTotalOverride
                ELSE cp.InsEstTotal END) AS InsEst,
       SUM(CASE WHEN cp.WriteOffEstOverride <> -1 THEN cp.WriteOffEstOverride
                WHEN cp.WriteOffEst <> -1 THEN cp.WriteOffEst
                ELSE 0 END) AS WriteOffEst,
       CAST(SUM(cp.Status = 8) AS SIGNED) AS CapRows
  FROM claimproc cp
 WHERE cp.Status IN (6, 8)
   AND cp.ProcNum IN (SELECT ta.ProcNum FROM treatplanattach ta WHERE ta.TreatPlanNum = ?)
 GROUP BY cp.ProcNum";

/// Procedure notes on one plan, newest first per procedure. Open Dental
/// never edits a note; an edit adds a row. Binds: the plan number.
pub const PLAN_NOTES: &str = r"
SELECT n.ProcNum, n.ProcNoteNum, n.Note
  FROM procnote n
 WHERE n.ProcNum IN (SELECT ta.ProcNum FROM treatplanattach ta WHERE ta.TreatPlanNum = ?)
 ORDER BY n.ProcNum, n.EntryDateTime DESC, n.ProcNoteNum DESC";

/// Binds: the provider number.
pub const PROVIDER: &str = r"
SELECT pv.LName, pv.PreferredName,
       CAST(pv.IsSecondary AS SIGNED) AS IsSecondary,
       CAST(pv.IsNotPerson AS SIGNED) AS IsNotPerson
  FROM provider pv
 WHERE pv.ProvNum = ?";
