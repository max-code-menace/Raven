// Mise en page d'une facture ou d'une note de crédit (PDF), au design de Raven (comme le devis, assets/devis.typ).
// Typst : https://typst.app/docs
// Ce fichier est suivi, à la fin, d'un appel `#facture((...))` écrit par le serveur avec les données de la pièce.
// Les textes arrivent comme valeurs (jamais comme balisage) : rien de ce que la personne tape ne peut changer
// la mise en page. Mentions légales à faire valider par le comptable.

#let encre = rgb("#111110")
#let gris = rgb("#6a6a64")
#let trait = rgb("#e3e3de")
#let doux = rgb("#f6f6f2")
#let jaune = rgb("#fef6cc")
#let jaune-trait = rgb("#cd9100")
#let sans = "Archivo"
#let mono = "IBM Plex Mono"

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
    if l.type == "titre" {
      cellules.push(table.cell(fill: doux, stroke: (top: 0.8pt + encre, bottom: 0.5pt + trait), text(font: mono, weight: "semibold", l.numero)))
      cellules.push(table.cell(fill: doux, stroke: (top: 0.8pt + encre, bottom: 0.5pt + trait), colspan: 6, text(weight: "semibold", multi(l.description))))
    } else if l.type == "texte" {
      cellules.push(text(font: mono, fill: gris, l.numero))
      cellules.push(table.cell(colspan: 6, text(fill: gris, multi(l.description))))
    } else {
      cellules.push(chiffre(l.numero))
      cellules.push(multi(l.description))
      cellules.push(chiffre(l.quantite))
      cellules.push(l.unite)
      cellules.push(chiffre(l.prix_unitaire))
      cellules.push(chiffre(l.tva))
      cellules.push(chiffre(l.total))
    }
  }
  table(
    columns: (auto, 1fr, auto, auto, auto, auto, auto),
    stroke: (x: none, y: none, bottom: 0.5pt + trait),
    inset: (x: 5pt, y: 5.5pt),
    align: (left + top, left + top, right + top, left + top, right + top, right + top, right + top),
    table.header(
      ..([N°], [Désignation], [Qté], [Unité], [P.U. HT], [TVA], [Total HT]).map(t => table.cell(
        stroke: (bottom: 0.8pt + encre),
        etiquette(t),
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
    background: if d.brouillon { rotate(-35deg, text(font: sans, size: 70pt, fill: luma(235), weight: "bold")[BROUILLON]) },
  )
  set text(font: sans, size: 8.5pt, lang: "fr", region: "BE", fill: encre)
  set par(leading: 0.62em, spacing: 0.9em)

  // En-tête : la société à gauche ; le type de pièce (« FACTURE FINALE », comme dans Vertuoza), le numéro et les
  // renseignements à droite (référence, date, échéance, devis et chantier).
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
      etiquette(d.titre)
      v(1pt)
      if d.numero != "" {
        text(size: 20pt, weight: "bold", tracking: -0.01em)[N° #d.numero]
      } else {
        text(size: 20pt, weight: "bold", fill: gris)[Brouillon]
      }
      v(8pt)
      grid(
        columns: (auto, 1fr),
        column-gutter: 10pt,
        row-gutter: 5pt,
        ..if d.corrige != "" { (etiquette("Corrige"), d.corrige) } else { () },
        etiquette("Date"), chiffre(d.date),
        ..if d.echeance != "" { (etiquette("Échéance"), text(font: mono, size: 0.92em, weight: "semibold", d.echeance)) } else { () },
        ..if d.reference != "" { (etiquette("Référence"), d.reference) } else { () },
        ..if d.responsable != "" { (etiquette("Responsable"), d.responsable) } else { () },
        ..if d.devis != "" { (etiquette("Devis N°"), chiffre(d.devis)) } else { () },
        ..if d.chantier != "" { (etiquette(d.mot_chantier), d.chantier) } else { () },
        ..if d.adresse_chantier != "" { (etiquette(d.mot_adresse_chantier), d.adresse_chantier) } else { () },
      )
    },
  )

  v(0.8cm)
  etiquette("Client")
  v(3pt)
  if d.client != none {
    block(width: 50%, inset: (left: 9pt, y: 2pt), stroke: (left: 1.5pt + encre), bloc-adresse(d.client))
  } else {
    text(fill: gris, style: "italic")[Client à compléter]
  }
  v(0.7cm)

  if d.lignes.len() > 0 { tableau-lignes(d.lignes) } else {
    align(center, text(fill: gris, style: "italic")[Aucune ligne.])
  }

  v(0.5cm)
  block(breakable: false, width: 100%)[
    #align(right)[
      #block(width: 8.4cm)[
        #set text(size: 8.5pt)
        #block(width: 100%, inset: (x: 9pt))[#grid(
          columns: (1fr, auto),
          align: (left, right),
          row-gutter: 6pt,
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
            text(weight: "bold", size: 10pt, d.libelle_total),
            text(font: mono, weight: "semibold", size: 13pt, d.totaux.ttc),
          )
        ]
      ]
    ]
  ]

  v(0.4cm)
  // Mentions légales de TVA (autoliquidation, taux de 6 %, franchise) : textes figés, imprimés en entier.
  for m in d.mentions {
    v(2pt)
    block(width: 100%, breakable: false, inset: 7pt, stroke: (left: 1.5pt + encre))[
      #text(size: 7.5pt, m)
    ]
  }
  // Le paiement : montant, échéance, compte et communication ; le QR code à scanner avec l'application bancaire.
  if d.paiement != none {
    v(4pt)
    block(width: 100%, breakable: false, inset: 11pt, stroke: 0.6pt + encre)[
      #grid(
        columns: if d.paiement.qr != none { (1fr, auto) } else { (1fr,) },
        column-gutter: 14pt,
        {
          text(weight: "bold", size: 9.5pt)[Paiement]
          v(6pt)
          grid(
            columns: (auto, 1fr),
            column-gutter: 12pt,
            row-gutter: 6pt,
            etiquette("Montant"), text(font: mono, weight: "semibold", d.totaux.ttc),
            ..if d.paiement.echeance != "" { (etiquette("À payer avant le"), text(font: mono, weight: "semibold", d.paiement.echeance)) } else { () },
            ..if d.paiement.iban != "" { (etiquette("Compte"), chiffre(d.paiement.iban)) } else { () },
            ..if d.paiement.vcs != "" { (etiquette("Communication"), text(font: mono, weight: "semibold", size: 10pt, d.paiement.vcs)) } else { () },
          )
          if d.paiement.vcs != "" {
            v(6pt)
            text(size: 7pt, fill: gris)[Indiquez bien cette communication structurée : elle permet de reconnaître votre paiement.]
          }
        },
        ..if d.paiement.qr != none {
          (
            align(center, {
              image(d.paiement.qr, width: 2.9cm)
              v(1pt)
              block(width: 3.1cm, text(size: 6.5pt, fill: gris)[Scannez avec votre application bancaire : le virement se remplit seul.])
            }),
          )
        } else { () },
      )
    ]
  }
  if d.conditions != "" {
    v(4pt)
    [#etiquette("Conditions de paiement") #h(4pt) #d.conditions]
  }
  if d.remarque != "" {
    v(2pt)
    par(multi(d.remarque))
  }
}
