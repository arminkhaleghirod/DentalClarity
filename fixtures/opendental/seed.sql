-- Synthetic Open Dental data mirroring fixtures/synthetic/practice.toml.
--
-- Every person, plan, note and amount here is invented. Do not add real
-- patient data. Procedure descriptions are the practice's own short names,
-- not CDT descriptor text.
--
-- Load after schema.sql. Ages and appointments are relative to the day the
-- seed is loaded, so the recommended list is never empty on that day.
-- Rows marked "excluded" exist to prove the connector leaves them out.

USE opendental;

INSERT INTO provider (ProvNum, Abbr, LName, FName, Suffix, IsSecondary, IsNotPerson) VALUES
  (1, 'DOC1', 'Patel', 'Anika', 'DDS', 0, 0),
  (2, 'DOC2', 'Nguyen', 'Minh', 'DMD', 0, 0),
  (3, 'HYG1', 'Lee', 'Jordan', 'RDH', 1, 0);

-- Treatment plan priorities (definition category 20).
INSERT INTO definition (DefNum, Category, ItemOrder, ItemName) VALUES
  (1, 20, 0, '1'),
  (2, 20, 1, '2'),
  (3, 16, 0, 'Caries');

-- TreatArea: 1 surface, 2 tooth, 3 mouth, 4 quadrant, 6 arch.
-- D5213 is an arch code in a stock install; it is a mouth code here so the
-- plan matches the TOML fixture, which has no arch field.
INSERT INTO procedurecode (CodeNum, ProcCode, Descript, AbbrDesc, TreatArea, LaymanTerm) VALUES
  (1, 'D3330', 'Root canal, molar', 'RCT molar', 2, ''),
  (2, 'D2950', 'Core buildup', 'Buildup', 2, ''),
  (3, 'D2740', 'Crown, porcelain', 'PFC', 2, ''),
  (4, 'D2392', 'Composite filling, 2 surfaces, back tooth', 'Comp 2s post', 1, ''),
  (5, 'D2391', 'Composite filling, 1 surface, back tooth', 'Comp 1s post', 1, ''),
  (6, 'D1110', 'Adult prophy', 'Pro', 3, 'Adult cleaning'),
  (7, 'D6010', 'Implant placement', 'Implant', 2, ''),
  (8, 'D6065', 'Implant crown, porcelain', 'Impl crown', 2, ''),
  (9, 'D1351', 'Sealant', 'Seal', 2, ''),
  (10, 'D4341', 'Deep cleaning, 4+ teeth per quadrant', 'SRP 4+', 4, ''),
  (11, 'D4910', 'Gum maintenance visit', 'Perio maint', 3, ''),
  (12, 'D7140', 'Extraction, erupted tooth', 'Ext', 2, ''),
  (13, 'D5213', 'Partial denture, upper, cast metal', 'Upper partial', 3, ''),
  (14, 'D6240', 'Bridge pontic, porcelain', 'Pontic', 2, ''),
  (15, 'D6750', 'Bridge crown, porcelain', 'Abut crown', 2, ''),
  (16, 'D7210', 'Surgical extraction', 'Surg ext', 2, ''),
  (17, 'D9944', 'Night guard, hard', 'Guard', 3, '');

INSERT INTO patient (PatNum, LName, FName, PatStatus, Birthdate, Language, PriProv, SSN) VALUES
  (10482, 'Okafor', 'Daniel', 0, CURDATE() - INTERVAL 42 YEAR - INTERVAL 30 DAY, 'eng', 1, '000000001'),
  (10517, 'Ruiz', 'Maria Elena', 0, CURDATE() - INTERVAL 67 YEAR - INTERVAL 30 DAY, 'spa', 1, ''),
  (10603, 'Tran', 'Leo', 0, CURDATE() - INTERVAL 8 YEAR - INTERVAL 30 DAY, 'vie', 2, ''),
  (10655, 'Shah', 'Priya', 0, CURDATE() - INTERVAL 35 YEAR - INTERVAL 30 DAY, 'eng', 1, ''),
  (10702, 'Brooks', 'Walter', 0, CURDATE() - INTERVAL 71 YEAR - INTERVAL 30 DAY, 'eng', 1, ''),
  (10744, 'Ortiz', 'Samuel', 0, CURDATE() - INTERVAL 54 YEAR - INTERVAL 30 DAY, 'eng', 1, ''),
  (10790, 'Abernathy', 'Ruth', 0, CURDATE() - INTERVAL 81 YEAR - INTERVAL 30 DAY, 'eng', 1, ''),
  (10811, 'Holloway', 'Grace', 0, CURDATE() - INTERVAL 29 YEAR - INTERVAL 30 DAY, 'eng', 2, ''),
  -- Excluded: a deleted patient (PatStatus 4) with the same name as a current one.
  (10900, 'Okafor', 'Daniel', 4, CURDATE() - INTERVAL 50 YEAR, 'eng', 1, '');

-- AptStatus 1 is scheduled.
INSERT INTO appointment (AptNum, PatNum, AptStatus, ProvNum, AptDateTime) VALUES
  (1, 10482, 1, 1, TIMESTAMP(CURDATE(), '08:30:00')),
  (2, 10517, 1, 1, TIMESTAMP(CURDATE(), '09:15:00')),
  (3, 10603, 1, 2, TIMESTAMP(CURDATE(), '10:00:00')),
  (4, 10655, 1, 1, TIMESTAMP(CURDATE(), '11:30:00')),
  (5, 10702, 1, 1, TIMESTAMP(CURDATE(), '13:45:00')),
  (6, 10744, 1, 1, TIMESTAMP(CURDATE(), '14:30:00')),
  (7, 10790, 1, 1, TIMESTAMP(CURDATE(), '15:15:00')),
  -- Excluded: a broken appointment today and a scheduled one tomorrow.
  (8, 10811, 5, 2, TIMESTAMP(CURDATE(), '16:00:00')),
  (9, 10811, 1, 2, TIMESTAMP(CURDATE() + INTERVAL 1 DAY, '09:00:00')),
  -- Excluded: the deleted patient's appointment.
  (10, 10900, 1, 1, TIMESTAMP(CURDATE(), '07:45:00')),
  -- Later appointment the same day; the earliest one is shown.
  (11, 10482, 1, 1, TIMESTAMP(CURDATE(), '16:30:00'));

-- TPStatus 1 is the active plan, 0 a saved plan.
INSERT INTO treatplan (TreatPlanNum, PatNum, DateTP, Heading, TPStatus) VALUES
  (1, 10482, CURDATE() - INTERVAL 14 DAY, 'Active', 1),
  (2, 10517, CURDATE() - INTERVAL 14 DAY, 'Active', 1),
  (3, 10603, CURDATE() - INTERVAL 14 DAY, 'Active', 1),
  (4, 10655, CURDATE() - INTERVAL 14 DAY, 'Active', 1),
  (5, 10702, CURDATE() - INTERVAL 14 DAY, 'Active', 1),
  (6, 10744, CURDATE() - INTERVAL 14 DAY, 'Active', 1),
  (7, 10790, CURDATE() - INTERVAL 14 DAY, 'Active', 1),
  (8, 10811, CURDATE() - INTERVAL 14 DAY, 'Active', 1),
  -- Excluded: a saved plan, and the deleted patient's plan.
  (9, 10482, CURDATE() - INTERVAL 60 DAY, 'Signed plan', 0),
  (10, 10900, CURDATE() - INTERVAL 14 DAY, 'Active', 1);

-- ProcStatus 1 is treatment planned, 2 complete, 6 deleted. Fees in dollars.
-- Daniel's procedures carry the priority on procedurelog only, to cover the
-- fallback from treatplanattach.Priority.
INSERT INTO procedurelog
  (ProcNum, PatNum, CodeNum, ProcStatus, ToothNum, Surf, ProcFee, Priority, ProvNum, DateTP, ProcNumLab)
VALUES
  (1, 10482, 1, 1, '30', '', 1150.00, 1, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (2, 10482, 2, 1, '30', '', 310.00, 1, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (3, 10482, 3, 1, '30', '', 1380.00, 1, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (4, 10482, 4, 1, '14', 'MO', 265.00, 2, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (5, 10482, 5, 1, '19', 'O', 210.00, 2, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (6, 10482, 6, 1, '', '', 135.00, 2, 3, CURDATE() - INTERVAL 14 DAY, 0),
  (7, 10517, 7, 1, '19', '', 2100.00, 0, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (8, 10517, 8, 1, '19', '', 1650.00, 1, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (9, 10603, 9, 1, '3', '', 55.00, 0, 2, CURDATE() - INTERVAL 14 DAY, 0),
  (10, 10603, 9, 1, '14', '', 55.00, 0, 2, CURDATE() - INTERVAL 14 DAY, 0),
  (11, 10603, 9, 1, '19', '', 55.00, 0, 2, CURDATE() - INTERVAL 14 DAY, 0),
  (12, 10603, 5, 1, 'K', 'O', 175.00, 0, 2, CURDATE() - INTERVAL 14 DAY, 0),
  (13, 10603, 4, 1, 'T', 'DO', 220.00, 0, 2, CURDATE() - INTERVAL 14 DAY, 0),
  (14, 10655, 10, 1, '', 'UR', 240.00, 0, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (15, 10655, 10, 1, '', 'LR', 240.00, 0, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (16, 10655, 11, 1, '', '', 165.00, 0, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (17, 10702, 12, 1, '3', '', 225.00, 0, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (18, 10702, 13, 1, '', '', 1850.00, 0, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (19, 10744, 14, 1, '13', '', 1250.00, 0, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (20, 10744, 15, 1, '12', '', 1300.00, 0, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (21, 10744, 15, 1, '14', '', 1300.00, 0, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (22, 10790, 16, 1, '2', '', 385.00, 0, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (23, 10811, 17, 1, '', '', 520.00, 0, 2, CURDATE() - INTERVAL 14 DAY, 0),
  -- Excluded: a completed procedure, a procedure on the saved plan only,
  -- a deleted procedure still attached to the active plan, the deleted
  -- patient's procedure, and a Canadian lab fee row.
  (24, 10482, 6, 2, '', '', 135.00, 0, 3, CURDATE() - INTERVAL 200 DAY, 0),
  (25, 10482, 5, 1, '3', 'O', 210.00, 0, 1, CURDATE() - INTERVAL 60 DAY, 0),
  (26, 10482, 3, 6, '3', '', 1380.00, 1, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (27, 10900, 3, 1, '8', '', 1380.00, 1, 1, CURDATE() - INTERVAL 14 DAY, 0),
  (28, 10744, 15, 1, '', '', 80.00, 1, 1, CURDATE() - INTERVAL 14 DAY, 20);

-- Maria's crown has priority 1 on procedurelog and 2 on this plan; the
-- plan's priority wins.
INSERT INTO treatplanattach (TreatPlanNum, ProcNum, Priority) VALUES
  (1, 1, 0), (1, 2, 0), (1, 3, 0), (1, 4, 0), (1, 5, 0), (1, 6, 0), (1, 26, 0),
  (2, 7, 1), (2, 8, 2),
  (3, 9, 1), (3, 10, 1), (3, 11, 1), (3, 12, 1), (3, 13, 1),
  (4, 14, 1), (4, 15, 1), (4, 16, 2),
  (5, 17, 1), (5, 18, 2),
  (6, 19, 1), (6, 20, 1), (6, 21, 1), (6, 28, 1),
  (7, 22, 1),
  (8, 23, 1),
  (10, 27, 1);

INSERT INTO proctp (TreatPlanNum, PatNum, ProcNumOrig, ItemOrder, ToothNumTP, Surf, ProcCode, Descript, FeeAmt, PatAmt) VALUES
  (9, 10482, 25, 0, '3', 'O', 'D2391', 'Composite filling, 1 surface, back tooth', 210.00, 42.00);

-- Notes are never edited in Open Dental; an edit adds a newer row.
INSERT INTO procnote (PatNum, ProcNum, EntryDateTime, Note, Signature) VALUES
  (10482, 1, NOW() - INTERVAL 15 DAY, 'Draft note, superseded.', ''),
  (10482, 1, NOW() - INTERVAL 14 DAY, '#30 deep caries into pulp, symptomatic. Recommend RCT, buildup, crown.', 'sig'),
  (10482, 4, NOW() - INTERVAL 15 DAY, 'Note later cleared.', ''),
  (10482, 4, NOW() - INTERVAL 14 DAY, '', ''),
  (10517, 7, NOW() - INTERVAL 14 DAY, '#19 missing since extraction 2024. Bone width adequate on CBCT. Plan implant, then crown after healing.', ''),
  (10655, 14, NOW() - INTERVAL 14 DAY, 'Generalized 5-6 mm pockets with bleeding on probing, UR and LR. SRP by quadrant.', ''),
  (10702, 17, NOW() - INTERVAL 14 DAY, '#3 fractured below gumline, nonrestorable.', '');

-- Status 6 is an estimate. Overrides of -1 are blank.
-- Daniel's root canal is split across primary and secondary insurance, his
-- buildup uses an override, and one filling has a do-not-bill row and a
-- received claim row that must not count. Priya's maintenance visit has no
-- estimate yet, so both estimates stay unknown.
INSERT INTO claimproc (ProcNum, PatNum, Status, PlanNum, InsEstTotal, InsEstTotalOverride, WriteOffEst, NoBillIns) VALUES
  (1, 10482, 6, 1, 500.00, -1, -1, 0),
  (1, 10482, 6, 2, 190.00, -1, -1, 0),
  (2, 10482, 6, 1, 150.00, 186.00, -1, 0),
  (3, 10482, 6, 1, 690.00, -1, -1, 0),
  (3, 10482, 1, 1, 999.00, -1, -1, 0),
  (4, 10482, 6, 1, 212.00, -1, -1, 0),
  (5, 10482, 6, 1, 168.00, -1, -1, 0),
  (5, 10482, 6, 2, 50.00, -1, -1, 1),
  (6, 10482, 6, 1, 135.00, -1, -1, 0),
  (7, 10517, 6, 1, 840.00, -1, -1, 0),
  (8, 10517, 6, 1, 660.00, -1, -1, 0),
  (9, 10603, 6, 1, 55.00, -1, -1, 0),
  (10, 10603, 6, 1, 55.00, -1, -1, 0),
  (11, 10603, 6, 1, 55.00, -1, -1, 0),
  (12, 10603, 6, 1, 140.00, -1, -1, 0),
  (13, 10603, 6, 1, 176.00, -1, -1, 0),
  (14, 10655, 6, 1, 192.00, -1, -1, 0),
  (15, 10655, 6, 1, 192.00, -1, -1, 0),
  (17, 10702, 6, 1, 180.00, -1, -1, 0),
  (18, 10702, 6, 1, 925.00, -1, -1, 0),
  (19, 10744, 6, 1, 625.00, -1, -1, 0),
  (20, 10744, 6, 1, 650.00, -1, -1, 0),
  (21, 10744, 6, 1, 650.00, -1, -1, 0),
  (22, 10790, 6, 1, 308.00, -1, -1, 0),
  (23, 10811, 6, 1, 0.00, -1, -1, 0);
