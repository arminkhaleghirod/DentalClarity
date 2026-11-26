# Procedure illustrations

One SVG per procedure category id from `knowledge/codes.toml`, plus `fallback.svg`
for procedures with no category or an unknown one. The patient app shows them on
the procedure detail screen at `/illustrations/<category>.svg`, with the caption
"Educational drawing, not your X-ray."

These drawings are original works made for DentalClarity and are licensed with
the rest of the repository under Apache-2.0. They were drawn from scratch as
simple shapes; they are not traced from or based on any existing illustration.

They are educational and schematic. Proportions are simplified, and they do not
show any patient's mouth. They need review by a licensed clinician before they
are used in clinical care.

## Style

- Canvas `viewBox="0 0 480 300"`, panel corner radius 16, outline stroke 3 px.
- Cross-sections or side views of a lower tooth, with enamel (white), dentin
  (ivory), pulp (soft pink), gum and bone. Dentures are drawn from above.
- Restorations in light blue, metal parts and amalgam in grey, root canal
  filling in orange, motion arrows in muted blue.
- No text, fonts, scripts, external references or raster images. Labels and
  alt text live in the app (`crates/dc-web/src/present/screens/procedures.rs`)
  so they can be translated.
- Colors are baked in as presentation attributes. An SVG loaded through `<img>`
  cannot inherit `currentColor`, so practice branding does not tint them.

## Review status

| File | Shows | review_status |
| --- | --- | --- |
| exam.svg | Two molars and a dental mirror | unreviewed |
| radiographs.svg | X-ray head, tooth and sensor behind it | unreviewed |
| cleaning.svg | Scaler removing tartar at the gum line | unreviewed |
| periodontal_scaling.svg | Gum pocket and bone loss, scaler cleaning tartar on the root below the gum line | unreviewed |
| sealant.svg | Sealant covering the grooves on a molar's chewing surface | unreviewed |
| fluoride.svg | Varnish coating the crown of a tooth | unreviewed |
| filling_composite.svg | Tooth-colored filling in a cavity on the chewing surface | unreviewed |
| filling_amalgam.svg | Silver filling in a cavity on the chewing surface | unreviewed |
| core_buildup.svg | Broken tooth rebuilt with core material, future crown outline | unreviewed |
| post_and_core.svg | Root-filled tooth with a post in the canal and a core on top | unreviewed |
| crown.svg | Crown lowered onto a shaped tooth | unreviewed |
| root_canal.svg | Molar with cleaned and filled canals and a sealed chamber | unreviewed |
| pulpotomy.svg | Baby molar with healing material in the pulp chamber, root pulp kept | unreviewed |
| extraction.svg | Tooth lifted out of its socket | unreviewed |
| surgical_extraction.svg | Tilted tooth under the gum and bone, opening above it | unreviewed |
| bone_graft.svg | Socket filled with graft granules and covered by a membrane | unreviewed |
| implant.svg | Implant post in the bone with a healing cap | unreviewed |
| implant_crown.svg | Crown on an abutment over an implant post | unreviewed |
| bridge.svg | Bridge with crowns on two teeth holding a replacement tooth over a gap | unreviewed |
| denture_complete.svg | Full upper denture seen from below | unreviewed |
| denture_partial.svg | Partial lower denture with metal bar and clasps | unreviewed |
| night_guard.svg | Guard over the upper teeth, upper and lower teeth closed | unreviewed |
| fallback.svg | A plain molar in gum and bone | unreviewed |
