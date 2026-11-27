# Demo video script

Target length about 3 minutes, which fits both AI for Medicine (at least 60 seconds) and Teens in AI (2 to 5 minutes). Recorded on the live demo at https://dentalclarity-demo.fly.dev in a desktop browser at 1440 by 900, with Present mode shown in a tablet-sized window. Lines in quotes are voice-over. Times are approximate.

## 0:00 to 0:20, the problem

Screen: a raw treatment plan in the patient page (codes, tooth numbers, fees).

"This is what a dental treatment plan looks like: codes, tooth numbers and fees. Patients are often asked to accept a plan like this after a short conversation in the chair, and many leave unsure what they agreed to or what it costs them."

## 0:20 to 0:45, what DentalClarity is

Screen: dashboard, then pick Daniel Okafor and press Create presentation.

"DentalClarity reads the plan the dentist already saved and turns it into a presentation the patient can follow. It explains the plan. It doesn't diagnose, recommend treatment or try to sell anything."

## 0:45 to 1:15, how it stays accurate

Screen: the progress view while sections are written, checked and reviewed.

"The model never writes the facts. Teeth, procedures, phases and costs come from the practice's records as locked facts. The model writes plain-language sentences around them, and every sentence has to cite a fact, a passage from a curated knowledge base, or the clinical note. Seventeen deterministic checks then look for wrong teeth, invented costs or timing, and wording that diagnoses, promises or pressures. A separate review pass checks each section again. Anything that still fails is held for staff and never shown to the patient."

## 1:15 to 1:40, staff review

Screen: the editor. Point at the provenance labels, the checks panel, and press "Looks right" on a note-derived reason.

"Staff see where each block came from and which checks passed. A reason taken from the dentist's note stays hidden from the patient until someone on the team confirms it."

## 1:40 to 2:20, the patient's view

Screen: press Present. Show At a glance with the tooth chart, play narration for a few seconds with the sentence highlight, open one procedure with its illustration, then Journey and Costs.

"Then staff hand over the tablet. The patient sees which teeth are involved, what each procedure is and why it follows the one before, and what it is estimated to cost. The costs are built straight from the plan, with no AI involved. Every block can be read aloud, with the transcript on screen."

## 2:20 to 2:50, evaluation

Screen: the ClarityEval report table from docs/RESULTS.md.

"We measured this with ClarityEval, a held-out benchmark of 36 synthetic cases, committed by hash before the run. Compared with the same model given the same rules in a single prompt, the pipeline showed no unsafe or unsupported statements to patients in 36 presentations, against 36 per 100 presentations for the baseline, at well under one cent per presentation."

## 2:50 to 3:05, limits and close

Screen: README, the clinical use section, then the logo.

"DentalClarity is a prototype. The knowledge base still needs dentist review, and it isn't ready for real patient data. The code, the benchmark and the live demo are all open. Thanks for watching."

## Recording notes

- Generate the Daniel Okafor presentation before recording so the progress view can be cut short. Record one live generation separately for the 0:45 segment.
- Starting Present pre-warms narration for every generated block. Wait a few seconds after pressing Present before playing, and play any staff-edited block once beforehand.
- Leave the browser zoom at 100% and hide bookmarks and extensions.
