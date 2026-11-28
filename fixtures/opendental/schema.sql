-- Subset of the Open Dental 24.1 schema that the DentalClarity connector reads.
--
-- Table and column names, types and defaults follow the published schema
-- (https://www.opendental.com/OpenDentalDocumentation24-1.xml). Only the
-- columns the connector uses are kept, plus a few it must not read (such as
-- patient.SSN and procnote.Signature) so the read-only grants can be tested.
-- This is not a copy of Open Dental's own create scripts.

CREATE DATABASE IF NOT EXISTS opendental CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci;
USE opendental;

CREATE TABLE patient (
  PatNum bigint(20) NOT NULL AUTO_INCREMENT,
  LName varchar(100) NOT NULL DEFAULT '',
  FName varchar(100) NOT NULL DEFAULT '',
  MiddleI varchar(100) NOT NULL DEFAULT '',
  Preferred varchar(100) NOT NULL DEFAULT '',
  PatStatus tinyint(4) NOT NULL DEFAULT 0,
  Gender tinyint(4) NOT NULL DEFAULT 0,
  Birthdate date NOT NULL DEFAULT '0001-01-01',
  SSN varchar(100) NOT NULL DEFAULT '',
  Address varchar(100) NOT NULL DEFAULT '',
  Guarantor bigint(20) NOT NULL DEFAULT 0,
  PriProv bigint(20) NOT NULL DEFAULT 0,
  Language varchar(100) NOT NULL DEFAULT '',
  ClinicNum bigint(20) NOT NULL DEFAULT 0,
  PRIMARY KEY (PatNum),
  KEY indexLName (LName(10)),
  KEY indexFName (FName(10))
);

CREATE TABLE provider (
  ProvNum bigint(20) NOT NULL AUTO_INCREMENT,
  Abbr varchar(255) NOT NULL DEFAULT '',
  LName varchar(100) NOT NULL DEFAULT '',
  FName varchar(100) NOT NULL DEFAULT '',
  MI varchar(100) NOT NULL DEFAULT '',
  Suffix varchar(100) NOT NULL DEFAULT '',
  IsSecondary tinyint(4) NOT NULL DEFAULT 0,
  IsHidden tinyint(4) NOT NULL DEFAULT 0,
  IsNotPerson tinyint(4) NOT NULL DEFAULT 0,
  ProvStatus tinyint(4) NOT NULL DEFAULT 0,
  PreferredName varchar(100) NOT NULL DEFAULT '',
  StateLicense varchar(50) NOT NULL DEFAULT '',
  PRIMARY KEY (ProvNum)
);

CREATE TABLE definition (
  DefNum bigint(20) NOT NULL AUTO_INCREMENT,
  Category tinyint(3) unsigned NOT NULL DEFAULT 0,
  ItemOrder smallint(5) unsigned NOT NULL DEFAULT 0,
  ItemName varchar(255) NOT NULL DEFAULT '',
  ItemValue varchar(255) NOT NULL DEFAULT '',
  ItemColor int(11) NOT NULL DEFAULT 0,
  IsHidden tinyint(3) unsigned NOT NULL DEFAULT 0,
  PRIMARY KEY (DefNum)
);

CREATE TABLE procedurecode (
  CodeNum bigint(20) NOT NULL AUTO_INCREMENT,
  ProcCode varchar(15) NOT NULL DEFAULT '',
  Descript varchar(255) NOT NULL DEFAULT '',
  AbbrDesc varchar(50) NOT NULL DEFAULT '',
  ProcCat bigint(20) NOT NULL DEFAULT 0,
  TreatArea tinyint(3) unsigned NOT NULL DEFAULT 0,
  IsProsth tinyint(3) unsigned NOT NULL DEFAULT 0,
  LaymanTerm varchar(255) NOT NULL DEFAULT '',
  PRIMARY KEY (CodeNum),
  KEY ProcCode (ProcCode)
);

CREATE TABLE appointment (
  AptNum bigint(20) NOT NULL AUTO_INCREMENT,
  PatNum bigint(20) NOT NULL DEFAULT 0,
  AptStatus tinyint(3) unsigned NOT NULL DEFAULT 0,
  Note text,
  ProvNum bigint(20) NOT NULL DEFAULT 0,
  AptDateTime datetime NOT NULL DEFAULT '0001-01-01 00:00:00',
  ProcDescript varchar(255) NOT NULL DEFAULT '',
  ClinicNum bigint(20) NOT NULL DEFAULT 0,
  PRIMARY KEY (AptNum),
  KEY indexPatNum (PatNum),
  KEY AptDateTime (AptDateTime)
);

CREATE TABLE procedurelog (
  ProcNum bigint(20) NOT NULL AUTO_INCREMENT,
  PatNum bigint(20) NOT NULL DEFAULT 0,
  AptNum bigint(20) NOT NULL DEFAULT 0,
  ProcDate date NOT NULL DEFAULT '0001-01-01',
  ProcFee double NOT NULL DEFAULT 0,
  Surf varchar(10) NOT NULL DEFAULT '',
  ToothNum varchar(2) NOT NULL DEFAULT '',
  ToothRange varchar(100) NOT NULL DEFAULT '',
  Priority bigint(20) NOT NULL DEFAULT 0,
  ProcStatus tinyint(3) unsigned NOT NULL DEFAULT 0,
  ProvNum bigint(20) NOT NULL DEFAULT 0,
  Dx bigint(20) NOT NULL DEFAULT 0,
  PlannedAptNum bigint(20) NOT NULL DEFAULT 0,
  ClinicNum bigint(20) NOT NULL DEFAULT 0,
  CodeNum bigint(20) NOT NULL DEFAULT 0,
  DateTP date NOT NULL DEFAULT '0001-01-01',
  ProcNumLab bigint(20) NOT NULL DEFAULT 0,
  UnitQty int(11) NOT NULL DEFAULT 1,
  BaseUnits int(11) NOT NULL DEFAULT 0,
  Discount double NOT NULL DEFAULT 0,
  PRIMARY KEY (ProcNum),
  KEY indexPatNum (PatNum),
  KEY indexAptNum (AptNum)
);

CREATE TABLE procnote (
  ProcNoteNum bigint(20) NOT NULL AUTO_INCREMENT,
  PatNum bigint(20) NOT NULL DEFAULT 0,
  ProcNum bigint(20) NOT NULL DEFAULT 0,
  EntryDateTime datetime NOT NULL DEFAULT '0001-01-01 00:00:00',
  UserNum bigint(20) NOT NULL DEFAULT 0,
  Note text,
  SigIsTopaz tinyint(4) NOT NULL DEFAULT 0,
  Signature text,
  PRIMARY KEY (ProcNoteNum),
  KEY ProcNum (ProcNum)
);

CREATE TABLE treatplan (
  TreatPlanNum bigint(20) NOT NULL AUTO_INCREMENT,
  PatNum bigint(20) NOT NULL DEFAULT 0,
  DateTP date NOT NULL DEFAULT '0001-01-01',
  Heading varchar(255) NOT NULL DEFAULT '',
  Note text,
  TPStatus tinyint(4) NOT NULL DEFAULT 0,
  TPType tinyint(4) NOT NULL DEFAULT 0,
  PRIMARY KEY (TreatPlanNum),
  KEY PatNum (PatNum)
);

CREATE TABLE treatplanattach (
  TreatPlanAttachNum bigint(20) NOT NULL AUTO_INCREMENT,
  TreatPlanNum bigint(20) NOT NULL DEFAULT 0,
  ProcNum bigint(20) NOT NULL DEFAULT 0,
  Priority bigint(20) NOT NULL DEFAULT 0,
  PRIMARY KEY (TreatPlanAttachNum),
  KEY TreatPlanNum (TreatPlanNum),
  KEY ProcNum (ProcNum)
);

-- Saved plans keep frozen copies here. The connector does not read it yet.
CREATE TABLE proctp (
  ProcTPNum bigint(20) NOT NULL AUTO_INCREMENT,
  TreatPlanNum bigint(20) NOT NULL DEFAULT 0,
  PatNum bigint(20) NOT NULL DEFAULT 0,
  ProcNumOrig bigint(20) NOT NULL DEFAULT 0,
  ItemOrder smallint(5) unsigned NOT NULL DEFAULT 0,
  Priority bigint(20) NOT NULL DEFAULT 0,
  ToothNumTP varchar(255) NOT NULL DEFAULT '',
  Surf varchar(255) NOT NULL DEFAULT '',
  ProcCode varchar(15) NOT NULL DEFAULT '',
  Descript varchar(255) NOT NULL DEFAULT '',
  FeeAmt double NOT NULL DEFAULT 0,
  PriInsAmt double NOT NULL DEFAULT 0,
  SecInsAmt double NOT NULL DEFAULT 0,
  PatAmt double NOT NULL DEFAULT 0,
  PRIMARY KEY (ProcTPNum),
  KEY TreatPlanNum (TreatPlanNum)
);

CREATE TABLE claimproc (
  ClaimProcNum bigint(20) NOT NULL AUTO_INCREMENT,
  ProcNum bigint(20) NOT NULL DEFAULT 0,
  ClaimNum bigint(20) NOT NULL DEFAULT 0,
  PatNum bigint(20) NOT NULL DEFAULT 0,
  ProvNum bigint(20) NOT NULL DEFAULT 0,
  InsPayEst double NOT NULL DEFAULT 0,
  Status tinyint(3) unsigned NOT NULL DEFAULT 0,
  PlanNum bigint(20) NOT NULL DEFAULT 0,
  WriteOff double NOT NULL DEFAULT 0,
  CopayAmt double NOT NULL DEFAULT -1,
  NoBillIns tinyint(3) unsigned NOT NULL DEFAULT 0,
  BaseEst double NOT NULL DEFAULT 0,
  InsEstTotal double NOT NULL DEFAULT 0,
  InsEstTotalOverride double NOT NULL DEFAULT -1,
  WriteOffEst double NOT NULL DEFAULT -1,
  WriteOffEstOverride double NOT NULL DEFAULT -1,
  InsSubNum bigint(20) NOT NULL DEFAULT 0,
  PRIMARY KEY (ClaimProcNum),
  KEY indexProcNum (ProcNum)
);
