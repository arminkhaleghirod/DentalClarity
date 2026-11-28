// Take-home booklet (docs/UI_SPEC.md section 7).
//
// Every piece of text comes from /data.json and is placed as a string, so
// nothing a model, a staff member or the practice records wrote is ever
// evaluated as Typst markup (docs/SECURITY.md, output handling).

#let data = json("/data.json")
#let l = data.labels
#let primary = rgb(data.primary)
#let highlight = rgb(data.highlight)
#let muted = luma(85)
#let rule = luma(190)

#set document(title: l.title, author: data.practice_name)
#set text(font: ("Noto Sans", "Noto Sans Arabic"), size: 11pt, lang: data.lang)
#set par(leading: 0.72em, spacing: 1.1em)

// The name of the section a page belongs to: the first section heading on
// the page, or else the last one before it.
#let section_name() = {
  let here_page = here().page()
  let on_page = query(heading.where(level: 1)).filter(h => h.location().page() == here_page)
  if on_page.len() > 0 {
    on_page.first().body
  } else {
    let before = query(heading.where(level: 1).before(here()))
    if before.len() > 0 { before.last().body }
  }
}

#set page(
  paper: data.paper,
  margin: 18mm,
  header: context {
    if counter(page).get().first() > 1 {
      set text(size: 8.5pt, fill: muted)
      section_name()
    }
  },
  footer: context {
    if counter(page).get().first() > 1 {
      set text(size: 8.5pt, fill: muted)
      grid(
        columns: (1fr, auto),
        column-gutter: 8pt,
        data.practice_name + "  ·  " + data.plan_line + "  ·  " + data.version_line,
        [#counter(page).get().first() #l.page_of #counter(page).final().first()],
      )
    }
  },
)

#show heading.where(level: 1): it => block(above: 0pt, below: 12pt, {
  set text(size: 18pt, weight: 700, fill: primary)
  it.body
})
#show heading.where(level: 2): it => block(above: 14pt, below: 6pt, sticky: true, {
  set text(size: 13pt, weight: 700)
  it.body
})

#let held_card() = block(
  width: 100%,
  inset: 12pt,
  radius: 6pt,
  fill: luma(242),
  stroke: 0.5pt + rule,
  l.held,
)

#let text_block(b) = {
  heading(level: 2, b.heading)
  par(b.sentences.join(" "))
}

#let question_list(questions) = {
  if questions.len() > 0 {
    heading(level: 2, l.questions_heading)
    list(..questions)
  }
}

// Cover ---------------------------------------------------------------------

#block(width: 100%, inset: (bottom: 10pt), stroke: (bottom: 3pt + primary))[
  #set text(size: 14pt, weight: 700, fill: primary)
  #data.practice_name
]
#v(30mm)
#text(size: 28pt, weight: 700, l.title)
#v(6mm)
#text(size: 13pt, data.prepared_line)
#v(2mm)
#text(size: 11pt, fill: muted, data.language_line)
#if data.provider_line != none {
  v(2mm)
  text(size: 11pt, fill: muted, data.provider_line)
}
#if data.demo {
  v(10mm)
  block(width: 100%, inset: 10pt, radius: 6pt, stroke: 0.5pt + rule, text(size: 10pt, l.demo_note))
}
#v(1fr)
#text(size: 9pt, fill: muted, l.not_advice)

// Costs come before what happens next (UI_SPEC section 7).

#let costs_page() = {
  pagebreak(weak: true)
  heading(level: 1, l.costs_title)
  table(
    columns: (2.4fr, auto, 1fr, 1fr, 1fr),
    stroke: 0.5pt + rule,
    inset: 6pt,
    align: (left, left, right, right, right),
    table.header(
      strong(l.cost_procedure),
      strong(l.cost_phase),
      strong(l.cost_fee),
      strong(l.cost_insurance),
      strong(l.cost_patient),
    ),
    ..data.costs.rows
      .map(r => (
        if r.location == "" { r.name } else { r.name + " · " + r.location },
        r.phase,
        r.fee,
        r.insurance,
        r.patient,
      ))
      .flatten(),
    strong(l.cost_total),
    [],
    strong(data.costs.fee),
    strong(data.costs.insurance),
    strong(data.costs.patient),
  )
  v(6pt)
  text(size: 10pt, data.cost_caveat)
}

// Sections ------------------------------------------------------------------

#for section in data.sections {
  if section.kind == "next_steps" {
    costs_page()
  }
  pagebreak(weak: true)
  heading(level: 1, section.title)
  if section.location != none {
    text(size: 10pt, fill: muted, section.location)
    v(4pt)
  }
  if section.kind == "at_a_glance" and data.teeth.len() > 0 {
    block(
      width: 100%,
      inset: 10pt,
      radius: 6pt,
      stroke: 1pt + highlight,
      [#strong(l.teeth_heading) #data.teeth.join(", ")],
    )
  }
  if section.held {
    held_card()
  } else {
    if section.illustration != none {
      figure(
        image(section.illustration, width: 62%),
        caption: text(size: 9pt, fill: muted, l.drawing_caption),
        numbering: none,
        gap: 6pt,
      )
    }
    for b in section.blocks {
      text_block(b)
    }
    question_list(section.questions)
  }
  if section.kind == "next_steps" {
    heading(level: 2, l.notes_heading)
    for _ in range(6) {
      v(9mm)
      line(length: 100%, stroke: 0.4pt + rule)
    }
  }
}
#if not data.sections.any(s => s.kind == "next_steps") {
  costs_page()
}

// Closing -------------------------------------------------------------------

#v(14pt)
#text(size: 9pt, fill: muted, l.not_advice)
