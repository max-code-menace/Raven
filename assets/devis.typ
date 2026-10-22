// Mise en page du devis (PDF), au design de Raven (refonte, étape 5). Typst : https://typst.app/docs
// Ce fichier est suivi, à la fin, d'un appel `#devis((...))` écrit par le serveur avec les données
// du devis. Les textes arrivent comme valeurs (jamais comme balisage) : rien de ce que la personne
// tape ne peut changer la mise en page.
//
// Les couleurs et les polices sont celles du design system (templates/_raven.html) : encre sur papier blanc,
// traits gris clairs, le jaune de Raven pour le total à payer ; Archivo pour le texte, IBM Plex Mono pour les
// chiffres et les petites étiquettes.

#let encre = rgb("#111110")
#let gris = rgb("#6a6a64")
#let trait = rgb("#e3e3de")
#let doux = rgb("#f6f6f2")
#let jaune = rgb("#fef6cc")
#let jaune-trait = rgb("#cd9100")
#let sans = "Archivo"
#let mono = "IBM Plex Mono"

// Un texte sur plusieurs lignes : chaque saut de ligne devient un retour à la ligne.
#let multi(s) = s.split("\n").join(linebreak())

// Petite étiquette en capitales (comme « rv-eyebrow » à l'écran).
#let etiquette(t) = text(font: mono, size: 6.5pt, weight: "medium", fill: gris, tracking: 0.06em, upper(t))

// Un chiffre : police à chasse fixe, pour que les colonnes s'alignent (un peu plus petite : elle est plus large).
#let chiffre(t) = text(font: mono, size: 0.92em, t)

#let bloc-adresse(b) = {
  text(weight: "semibold", b.nom)
  for l in b.adresse { linebreak(); l }
  if b.tva != "" { linebreak(); text(fill: gris)[TVA ] + chiffre(b.tva) }
}

#let pied(d) = {
  set text(font: mono, size: 6.5pt, fill: gris)
  line(length: 100%, stroke: 0.5pt + trait)
  v(3pt)
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
      cellules.push(table.cell(fill: doux, stroke: (top: 0.8pt + encre, bottom: 0.5pt + trait), text(font: mono, weight: "semibold", l.numero)))
      cellules.push(table.cell(fill: doux, stroke: (top: 0.8pt + encre, bottom: 0.5pt + trait), colspan: 6, text(weight: "semibold", corps)))
      cellules.push(table.cell(fill: doux, stroke: (top: 0.8pt + encre, bottom: 0.5pt + trait), align: right, text(font: mono, weight: "semibold", l.sous_total)))
    } else if l.type == "sous_titre" {
      cellules.push(chiffre(l.numero))
      cellules.push(table.cell(colspan: 7, text(weight: "semibold", corps)))
    } else if l.type == "texte" {
      cellules.push(text(font: mono, fill: gris, l.numero))
      cellules.push(table.cell(colspan: 7, text(fill: gris, corps)))
    } else {
      cellules.push(chiffre(l.numero))
      cellules.push(corps)
      cellules.push(text(font: mono, size: 7pt, fill: gris, l.type_quantite))
      cellules.push(chiffre(l.quantite))
      cellules.push(l.unite)
      cellules.push(chiffre(l.prix_unitaire))
      cellules.push(chiffre(l.tva))
      cellules.push(chiffre(l.total))
    }
  }
  table(
    columns: (auto, 1fr, auto, auto, auto, auto, auto, auto),
    stroke: (x: none, y: none, bottom: 0.5pt + trait),
    inset: (x: 5pt, y: 5.5pt),
    align: (left + top, left + top, left + top, right + top, left + top, right + top, right + top, right + top),
    table.header(
      ..([N°], [Désignation], [Type], [Qté], [Unité], [P.U. HT], [TVA], [Total HT]).map(t => table.cell(
        stroke: (bottom: 0.8pt + encre),
        etiquette(t),
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
  set text(font: sans, size: 8.5pt, lang: "fr", region: "BE", fill: encre)
  set par(leading: 0.62em, spacing: 0.9em)

  // En-tête : la société à gauche ; le titre, le numéro et les renseignements du devis à droite.
  grid(
    columns: (1fr, 1fr),
    column-gutter: 1.2cm,
    {
      if d.societe != none {
        if d.societe.logo != none {
          image(d.societe.logo, height: 1.7cm, fit: "contain")
          v(6pt)
        }
        text(weight: "semibold", size: 10.5pt, d.societe.nom)
        set text(fill: gris)
        for l in d.societe.adresse { linebreak(); l }
        if d.societe.telephone != "" { linebreak(); d.societe.telephone }
        if d.societe.email != "" { linebreak(); d.societe.email }
        if d.societe.tva != "" { linebreak(); [TVA ] + chiffre(d.societe.tva) }
      }
    },
    {
      etiquette(d.genre)
      v(1pt)
      text(size: 20pt, weight: "bold", tracking: -0.01em)[N° #d.numero]
      v(8pt)
      grid(
        columns: (auto, 1fr),
        column-gutter: 10pt,
        row-gutter: 5pt,
        ..if d.corrige != "" { (etiquette("Corrige"), d.corrige) } else { () },
        etiquette("Date"), chiffre(d.date),
        etiquette("Référence"), d.reference,
        ..if d.responsable != "" { (etiquette("Responsable"), d.responsable) } else { () },
        ..if d.chantier != "" { (etiquette(d.mot_chantier), d.chantier) } else { () },
      )
    },
  )

  v(0.8cm)
  // Le client, et la personne facturée quand ce n'est pas la même.
  grid(
    columns: (1fr, 1fr),
    column-gutter: 1.2cm,
    {
      etiquette("Client")
      v(3pt)
      if d.client != none {
        block(width: 100%, inset: (left: 9pt, y: 2pt), stroke: (left: 1.5pt + encre), bloc-adresse(d.client))
      } else {
        text(fill: gris, style: "italic")[Client à compléter]
      }
    },
    {
      if d.facturation != none {
        etiquette("Facturé à")
        v(3pt)
        block(width: 100%, inset: (left: 9pt, y: 2pt), stroke: (left: 1.5pt + trait), bloc-adresse(d.facturation))
      }
    },
  )
  v(0.7cm)

  // Les lignes. Un « saut de page » du devis coupe le tableau et passe à la page suivante.
  for (i, segment) in d.segments.enumerate() {
    if i > 0 { pagebreak() }
    if segment.len() > 0 { tableau-lignes(segment) }
  }
  if d.segments.flatten().len() == 0 {
    align(center, text(fill: gris, style: "italic")[Aucune ligne.])
  }
  // Ce que veulent dire QF, QP et Fft : seulement les types employés dans ce devis.
  if d.legende.len() > 0 {
    v(4pt)
    text(size: 7pt, fill: gris, d.legende.map(t => text(font: mono, fill: encre, t.sigle) + [ ] + t.texte).join([ · ]))
  }

  v(0.5cm)
  // Les totaux et ce qui les suit restent groupés.
  block(breakable: false, width: 100%)[
    #align(right)[
      #block(width: 8.4cm)[
        #set text(size: 8.5pt)
        #block(width: 100%, inset: (x: 9pt))[#grid(
          columns: (1fr, auto),
          align: (left, right),
          row-gutter: 6pt,
          ..if d.totaux.a_ajustement {
            (
              [Sous-total HT], chiffre(d.totaux.sous_total),
              [#d.totaux.libelle_ajustement], chiffre(d.totaux.ajustement),
            )
          } else { () },
          text(weight: "semibold")[Total HT], text(font: mono, size: 0.92em, weight: "semibold", d.totaux.ht),
          ..d.totaux.tva.map(t => (
            text(fill: gris)[TVA #t.taux (base #t.base)], text(font: mono, size: 0.92em, fill: gris, t.montant),
          )).flatten(),
        )]
        #v(6pt)
        #block(width: 100%, inset: (x: 9pt, y: 8pt), fill: jaune, stroke: (top: 1.2pt + encre, bottom: 0.6pt + jaune-trait))[
          #grid(
            columns: (1fr, auto),
            align: (left + horizon, right + horizon),
            text(weight: "bold", size: 10pt)[Total TTC à payer],
            text(font: mono, weight: "semibold", size: 13pt, d.totaux.ttc),
          )
        ]
      ]
    ]
  ]

  v(0.4cm)
  if d.franchise {
    par(text(weight: "semibold")[#d.mention_franchise])
  }
  if d.conditions != "" {
    [#etiquette("Conditions de paiement") #h(4pt) #d.conditions]
    v(2pt)
  }
  for r in (d.remarque_1, d.remarque_2) {
    if r != "" { par(multi(r)) }
  }

  v(0.7cm)
  // Le bon pour accord du client.
  block(breakable: false, width: 100%, inset: 11pt, stroke: 0.6pt + encre)[
    #text(weight: "bold", size: 9.5pt)[Bon pour accord]
    #v(2pt)
    #text(size: 7.5pt, fill: gris)[Mention manuscrite : « Lu et approuvé, bon pour accord ». Date et signature du client.]
    #v(1.7cm)
    #grid(
      columns: (1fr, 1fr),
      column-gutter: 1cm,
      row-gutter: 4pt,
      align: left,
      line(length: 100%, stroke: 0.5pt + gris),
      line(length: 100%, stroke: 0.5pt + gris),
      etiquette("Date"),
      etiquette("Signature"),
    )
  ]
}
