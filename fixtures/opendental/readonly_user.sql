-- Least-privilege user for the synthetic database, the same grants
-- docs/SECURITY.md asks practices to create. Column-level grants keep
-- identifiers such as patient.SSN and procnote.Signature out of reach.
--
-- The password below is for the throwaway CI and local test database only.
-- A practice generates its own, 24 characters or longer, and restricts the
-- user to the DentalClarity host instead of '%', which is used here only
-- because a test container sees clients from varying addresses.

CREATE USER IF NOT EXISTS 'dentalclarity_ro'@'%' IDENTIFIED BY 'synthetic-test-only';

GRANT SELECT (PatNum, LName, FName, PatStatus, Birthdate, Language)
  ON opendental.patient TO 'dentalclarity_ro'@'%';
GRANT SELECT (ProvNum, LName, PreferredName, IsSecondary, IsNotPerson)
  ON opendental.provider TO 'dentalclarity_ro'@'%';
GRANT SELECT (ProcNoteNum, ProcNum, EntryDateTime, Note)
  ON opendental.procnote TO 'dentalclarity_ro'@'%';
GRANT SELECT (AptNum, PatNum, AptStatus, AptDateTime)
  ON opendental.appointment TO 'dentalclarity_ro'@'%';
GRANT SELECT ON opendental.treatplan TO 'dentalclarity_ro'@'%';
GRANT SELECT ON opendental.treatplanattach TO 'dentalclarity_ro'@'%';
GRANT SELECT ON opendental.procedurelog TO 'dentalclarity_ro'@'%';
GRANT SELECT ON opendental.procedurecode TO 'dentalclarity_ro'@'%';
GRANT SELECT ON opendental.claimproc TO 'dentalclarity_ro'@'%';
GRANT SELECT (DefNum, Category, ItemOrder, ItemName)
  ON opendental.definition TO 'dentalclarity_ro'@'%';
