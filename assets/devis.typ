// Mise en page du devis (PDF). Typst : https://typst.app/docs
// Ce fichier est suivi, à la fin, d'un appel `#devis((...))` écrit par le serveur avec les données
// du devis. Les textes arrivent comme valeurs (jamais comme balisage) : rien de ce que la personne
// tape ne peut changer la mise en page.

#let accent = rgb("#1f3a5f")
#let gris = rgb("#5b6470")
#let clair = rgb("#f1f4f8")
#let trait = luma(222)

// Un texte sur plusieurs lignes : chaque saut de ligne devient un retour à la ligne.
#let multi(s) = s.split("\n").join(linebreak())

#let bloc-adresse(b) = {
  text(weight: "bold", b.nom)
  for l in b.adresse { linebreak(); l }
  if b.tva != "" { linebreak(); text(fill: gris)[TVA ] + b.tva }
}

#let pied(d) = {
  set text(size: 7pt, fill: gris)
  line(length: 100%, stroke: 0.4pt + trait)
  v(2pt)
  grid(
    columns: (1fr, auto),
    {
      if d.societe != none {
        let morceaux = (d.societe.nom,)
        if d.societe.tva != "" { morceaux.push("TVA " + d.societe.tva) }
        if d.societe.iban != "" { morceaux.push("IBAN " + d.societe.iban) }
        morceaux.join(" · ")
      }
    },
    context [Page #counter(page).get().first() / #counter(page).final().first()],
  )
}

#let tableau-lignes(lignes) = {
  let cellules = ()
  for l in lignes {
    let corps = {
      multi(l.description)
      if l.remarque != "" {
        linebreak()
        text(size: 7.5pt, fill: gris, style: "italic", multi(l.remarque))
      }
    }
    if l.type == "titre" {
      cellules.push(table.cell(fill: clair)[*#l.numero*])
      cellules.push(table.cell(fill: clair, colspan: 5)[*#corps*])
      cellules.push(table.cell(fill: clair, align: right)[*#l.sous_total*])
    } else if l.type == "sous_titre" {
      cellules.push[#l.numero]
      cellules.push(table.cell(colspan: 6)[*#corps*])
    } else if l.type == "texte" {
      cellules.push[#l.numero]
      cellules.push(table.cell(colspan: 6)[#text(style: "italic", corps)])
    } else {
      cellules.push[#l.numero]
      cellules.push[#corps]
      cellules.push[#l.quantite]
      cellules.push[#l.unite]
      cellules.push[#l.prix_unitaire]
      cellules.push[#l.tva]
      cellules.push[#l.total]
    }
  }
  table(
    columns: (auto, 1fr, auto, auto, auto, auto, auto),
    stroke: (x: none, y: none, bottom: 0.3pt + trait),
    inset: (x: 4pt, y: 5pt),
    align: (left + top, left + top, right + top, left + top, right + top, right + top, right + top),
    table.header(
      ..([N°], [Description], [Qté], [Unité], [P.U.], [TVA], [Total]).map(t => table.cell(
        stroke: (bottom: 0.8pt + accent),
        text(weight: "bold", fill: accent, t),
      )),
    ),
    ..cellules,
  )
}

#let devis(d) = {
  set document(
    title: d.genre + " N° " + d.numero,
    author: if d.societe != none { d.societe.nom } else { "" },
  )
  set page(
    paper: "a4",
    margin: (top: 1.8cm, bottom: 2.3cm, x: 1.8cm),
    footer: pied(d),
    footer-descent: 6pt,
  )
  set text(font: "Inter", size: 8.5pt, lang: "fr", region: "BE", fill: rgb("#1c2430"))
  set par(leading: 0.62em, spacing: 0.9em)

  // En-tête : la société à gauche, le client à droite.
  grid(
    columns: (1fr, 1fr),
    column-gutter: 1.2cm,
    {
      if d.societe != none {
        if d.societe.logo != none {
          image(d.societe.logo, height: 1.7cm, fit: "contain")
          v(6pt)
        }
        text(weight: "bold", size: 10pt, d.societe.nom)
        for l in d.societe.adresse { linebreak(); l }
        if d.societe.telephone != "" { linebreak(); d.societe.telephone }
        if d.societe.email != "" { linebreak(); d.societe.email }
        if d.societe.tva != "" { linebreak(); text(fill: gris)[TVA ] + d.societe.tva }
      }
    },
    {
      if d.client != none {
        block(width: 100%, inset: 10pt, fill: clair, radius: 3pt, bloc-adresse(d.client))
      } else {
        text(fill: gris, style: "italic")[Client à compléter]
      }
      if d.facturation != none {
        v(6pt)
        text(size: 7.5pt, fill: gris)[Facturé à]
        linebreak()
        bloc-adresse(d.facturation)
      }
    },
  )

  v(0.9cm)
  text(size: 17pt, weight: "bold", fill: accent)[#upper(d.genre) N° #d.numero]
  v(4pt)
  grid(
    columns: (auto, 1fr),
    column-gutter: 10pt,
    row-gutter: 4pt,
    ..if d.corrige != "" { (text(fill: gris)[Corrige], d.corrige) } else { () },
    text(fill: gris)[Référence], d.reference,
    text(fill: gris)[Date], d.date,
    text(fill: gris)[Responsable], d.responsable,
    ..if d.chantier != "" { (text(fill: gris)[Chantier], d.chantier) } else { () },
  )
  v(0.6cm)

  // Les lignes. Un « saut de page » du devis coupe le tableau et passe à la page suivante.
  for (i, segment) in d.segments.enumerate() {
    if i > 0 { pagebreak() }
    if segment.len() > 0 { tableau-lignes(segment) }
  }
  if d.segments.flatten().len() == 0 {
    align(center, text(fill: gris, style: "italic")[Aucune ligne.])
  }

  v(0.5cm)
  // Les totaux et ce qui les suit restent groupés.
  block(breakable: false, width: 100%)[
    #align(right)[
      #block(width: 8.2cm)[
        #grid(
          columns: (1fr, auto),
          align: (left, right),
          row-gutter: 5pt,
          ..if d.totaux.a_ajustement {
            (
              [Sous-total HT], d.totaux.sous_total,
              [#d.totaux.libelle_ajustement], d.totaux.ajustement,
            )
          } else { () },
          [Total HT], d.totaux.ht,
          ..d.totaux.tva.map(t => (
            text(fill: gris)[TVA #t.taux (sur #t.base)], text(fill: gris, t.montant),
          )).flatten(),
        )
        #v(4pt)
        #block(width: 100%, inset: (x: 8pt, y: 7pt), fill: accent, radius: 3pt)[
          #set text(fill: white, weight: "bold", size: 10.5pt)
          #grid(columns: (1fr, auto), align: (left, right), [Total TTC à payer], d.totaux.ttc)
        ]
      ]
    ]
  ]

  v(0.4cm)
  if d.conditions != "" {
    [#text(fill: gris)[Conditions de paiement :] #d.conditions]
    v(2pt)
  }
  for r in (d.remarque_1, d.remarque_2) {
    if r != "" { par(multi(r)) }
  }

  v(0.7cm)
  // Le bon pour accord du client.
  block(breakable: false, width: 100%, inset: 10pt, stroke: 0.6pt + trait, radius: 3pt)[
    #text(weight: "bold", fill: accent)[Bon pour accord]
    #v(2pt)
    #text(size: 7.5pt, fill: gris)[Mention manuscrite : « Lu et approuvé, bon pour accord ». Date et signature du client.]
    #v(1.7cm)
    #grid(
      columns: (1fr, 1fr),
      column-gutter: 1cm,
      row-gutter: 4pt,
      align: left,
      line(length: 100%, stroke: 0.4pt + gris),
      line(length: 100%, stroke: 0.4pt + gris),
      text(size: 7pt, fill: gris)[Date],
      text(size: 7pt, fill: gris)[Signature],
    )
  ]
}
