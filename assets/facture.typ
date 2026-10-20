// Mise en page d'une facture ou d'une note de crédit (PDF). Typst : https://typst.app/docs
// Ce fichier est suivi, à la fin, d'un appel `#facture((...))` écrit par le serveur avec les données de la pièce.
// Les textes arrivent comme valeurs (jamais comme balisage) : rien de ce que la personne tape ne peut changer
// la mise en page. Mentions légales à faire valider par le comptable.

#let accent = rgb("#1f3a5f")
#let gris = rgb("#5b6470")
#let clair = rgb("#f1f4f8")
#let trait = luma(222)

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
    if l.type == "titre" {
      cellules.push(table.cell(fill: clair)[*#l.numero*])
      cellules.push(table.cell(fill: clair, colspan: 6)[*#multi(l.description)*])
    } else if l.type == "texte" {
      cellules.push[#l.numero]
      cellules.push(table.cell(colspan: 6)[#text(style: "italic", multi(l.description))])
    } else {
      cellules.push[#l.numero]
      cellules.push[#multi(l.description)]
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

#let facture(d) = {
  set document(title: if d.numero != "" { d.titre + " " + d.numero } else { d.titre }, author: if d.societe != none { d.societe.nom } else { "" })
  set page(
    paper: "a4",
    margin: (top: 1.8cm, bottom: 2.3cm, x: 1.8cm),
    footer: pied(d),
    footer-descent: 6pt,
    background: if d.brouillon { rotate(-35deg, text(size: 70pt, fill: luma(235), weight: "bold")[BROUILLON]) },
  )
  set text(font: "Inter", size: 8.5pt, lang: "fr", region: "BE", fill: rgb("#1c2430"))
  set par(leading: 0.62em, spacing: 0.9em)

  // En-tête : la société à gauche, le client facturé à droite.
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
    },
  )

  // Comme dans Vertuoza : le type en titre (« FACTURE FINALE »), puis « N° 2026-114 », référence, date,
  // responsable et échéance du paiement ; le devis et l'adresse du chantier en dessous.
  v(0.9cm)
  text(size: 17pt, weight: "bold", fill: accent, upper(d.titre))
  v(4pt)
  grid(
    columns: (auto, 1fr),
    column-gutter: 10pt,
    row-gutter: 4pt,
    ..if d.numero != "" { (text(fill: gris)[N°], text(weight: "bold", d.numero)) } else { () },
    ..if d.corrige != "" { (text(fill: gris)[Corrige], d.corrige) } else { () },
    ..if d.reference != "" { (text(fill: gris)[Référence], d.reference) } else { () },
    text(fill: gris)[Date], d.date,
    ..if d.responsable != "" { (text(fill: gris)[Responsable], d.responsable) } else { () },
    ..if d.echeance != "" { (text(fill: gris)[Échéance du paiement], d.echeance) } else { () },
    ..if d.devis != "" { (text(fill: gris)[Devis N°], d.devis) } else { () },
    ..if d.chantier != "" { (text(fill: gris)[#d.mot_chantier], d.chantier) } else { () },
    ..if d.adresse_chantier != "" { (text(fill: gris)[#d.mot_adresse_chantier], d.adresse_chantier) } else { () },
  )
  v(0.6cm)

  if d.lignes.len() > 0 { tableau-lignes(d.lignes) } else {
    align(center, text(fill: gris, style: "italic")[Aucune ligne.])
  }

  v(0.5cm)
  block(breakable: false, width: 100%)[
    #align(right)[
      #block(width: 8.2cm)[
        #grid(
          columns: (1fr, auto),
          align: (left, right),
          row-gutter: 5pt,
          [Total HT], d.totaux.ht,
          ..d.totaux.tva.map(t => (
            text(fill: gris)[TVA #t.taux (sur #t.base)], text(fill: gris, t.montant),
          )).flatten(),
        )
        #v(4pt)
        #block(width: 100%, inset: (x: 8pt, y: 7pt), fill: accent, radius: 3pt)[
          #set text(fill: white, weight: "bold", size: 10.5pt)
          #grid(columns: (1fr, auto), align: (left, right), d.libelle_total, d.totaux.ttc)
        ]
      ]
    ]
  ]

  v(0.4cm)
  // Mentions légales de TVA (autoliquidation, taux de 6 %, franchise) : textes figés, imprimés en entier.
  for m in d.mentions {
    v(2pt)
    block(width: 100%, breakable: false, inset: 6pt, stroke: 0.4pt + trait, radius: 2pt)[
      #text(size: 7.5pt, m)
    ]
  }
  if d.paiement != none {
    block(width: 100%, inset: 10pt, stroke: 0.6pt + trait, radius: 3pt)[
      #text(weight: "bold", fill: accent)[Paiement]
      #v(2pt)
      #grid(
        columns: (auto, 1fr),
        column-gutter: 10pt,
        row-gutter: 4pt,
        text(fill: gris)[Montant], d.totaux.ttc,
        ..if d.paiement.echeance != "" { (text(fill: gris)[À payer avant le], d.paiement.echeance) } else { () },
        ..if d.paiement.iban != "" { (text(fill: gris)[Compte], d.paiement.iban) } else { () },
        ..if d.paiement.vcs != "" { (text(fill: gris)[Communication], text(font: "Inter", weight: "bold", d.paiement.vcs)) } else { () },
      )
    ]
  }
  if d.conditions != "" {
    v(2pt)
    [#text(fill: gris)[Conditions de paiement :] #d.conditions]
  }
  if d.remarque != "" {
    v(2pt)
    par(multi(d.remarque))
  }
}
