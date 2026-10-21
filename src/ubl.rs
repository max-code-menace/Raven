//! Factures et notes de crédit au format **UBL 2.1** (étape 8-4), selon la norme européenne EN 16931 dans sa variante
//! **Peppol BIS Billing 3.0** : c'est ce que lisent les logiciels comptables belges, et ce qui part par Peppol
//! (étape 5). L'adresse Peppol des parties (`EndpointID`) vient de `peppol` : le numéro d'entreprise belge (0208).
//!
//! Les lignes sont écrites « 1 × montant » : une ligne d'avancement vaut le montant avancé, pas forcément
//! quantité × prix ; la quantité et l'unité réelles sont reprises dans la description de l'article.
//! Montants en euros avec deux décimales et un point (« 1234.56 »).

use std::fmt::Write as _;

use chrono::NaiveDate;
use sqlx::PgConnection;

use crate::calcul;
use crate::factures::{self, LigneBase};

/// Une partie (vendeur ou acheteur).
#[derive(Debug, Clone, Default)]
pub struct Partie {
    pub nom: String,
    pub rue: String,
    pub code_postal: String,
    pub ville: String,
    pub pays: String,
    /// Numéro de TVA (« BE0123456789 »), vide pour un particulier.
    pub tva: String,
    pub email: String,
    pub telephone: String,
    /// L'adresse Peppol (« 0208:0123456789 ») ; aucune pour un particulier.
    pub peppol: Option<crate::peppol::Identifiant>,
}

#[derive(Clone)]
pub(crate) struct Document {
    pub note_credit: bool,
    /// Le numéro tel qu'il est imprimé (« 2026-125 ») : la numérotation repart à 1 chaque année, le numéro seul ne
    /// serait pas unique.
    pub numero: String,
    pub date: NaiveDate,
    pub echeance: Option<NaiveDate>,
    pub reference: String,
    pub remarque: String,
    /// La facture corrigée (note de crédit), numéro imprimé.
    pub facture_corrigee: Option<String>,
    pub vendeur: Partie,
    pub acheteur: Partie,
    pub iban: String,
    pub vcs: String,
    pub conditions: String,
    pub(crate) lignes: Vec<LigneBase>,
}

fn echapper(texte: &str) -> String {
    let mut sortie = String::with_capacity(texte.len());
    for c in texte.chars() {
        match c {
            '&' => sortie.push_str("&amp;"),
            '<' => sortie.push_str("&lt;"),
            '>' => sortie.push_str("&gt;"),
            '"' => sortie.push_str("&quot;"),
            '\'' => sortie.push_str("&apos;"),
            // Les caractères de contrôle sont interdits en XML (sauf tabulation et retours à la ligne).
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => {}
            c => sortie.push(c),
        }
    }
    sortie
}

/// 123456 → « 1234.56 » ; -5 → « -0.05 ».
pub fn montant(centimes: i64) -> String {
    let signe = if centimes < 0 { "-" } else { "" };
    let a = centimes.unsigned_abs();
    format!("{signe}{}.{:02}", a / 100, a % 100)
}

/// 600 → « 6 » ; 550 → « 5.5 ».
fn pourcentage(centiemes: i32) -> String {
    let (e, d) = (centiemes / 100, centiemes % 100);
    match d {
        0 => e.to_string(),
        d if d % 10 == 0 => format!("{e}.{}", d / 10),
        d => format!("{e}.{d:02}"),
    }
}

/// La catégorie de TVA de la norme : S (taux normal ou réduit), AE (autoliquidation), E (exonéré : franchise des
/// petites entreprises, à confirmer avec le point d'accès Peppol), Z (taux zéro).
fn categorie(taux_centiemes: i32, autoliquidation: bool, franchise: bool) -> &'static str {
    match (autoliquidation, franchise, taux_centiemes) {
        (true, _, _) => "AE",
        (false, true, _) => "E",
        (false, false, 0) => "Z",
        _ => "S",
    }
}

/// « BE 0123.456.789 » → « BE0123456789 ».
fn compact(texte: &str) -> String {
    texte.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase()
}

fn balise(xml: &mut String, nom: &str, valeur: &str) {
    if !valeur.trim().is_empty() {
        let _ = write!(xml, "<{nom}>{}</{nom}>", echapper(valeur.trim()));
    }
}

fn partie(xml: &mut String, role: &str, p: &Partie) {
    let _ = write!(xml, "<cac:{role}><cac:Party>");
    let tva = compact(&p.tva);
    if let Some(id) = &p.peppol {
        let _ = write!(xml, "<cbc:EndpointID schemeID=\"{}\">{}</cbc:EndpointID>", echapper(&id.schema), echapper(&id.numero));
    }
    let _ = write!(xml, "<cac:PartyName><cbc:Name>{}</cbc:Name></cac:PartyName>", echapper(&p.nom));
    xml.push_str("<cac:PostalAddress>");
    balise(xml, "cbc:StreetName", &p.rue);
    balise(xml, "cbc:CityName", &p.ville);
    balise(xml, "cbc:PostalZone", &p.code_postal);
    let pays = if p.pays.trim().is_empty() { "BE" } else { p.pays.trim() };
    let _ = write!(xml, "<cac:Country><cbc:IdentificationCode>{}</cbc:IdentificationCode></cac:Country></cac:PostalAddress>", echapper(pays));
    if !tva.is_empty() {
        let _ = write!(
            xml,
            "<cac:PartyTaxScheme><cbc:CompanyID>{}</cbc:CompanyID><cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:PartyTaxScheme>",
            echapper(&tva)
        );
    }
    let _ = write!(xml, "<cac:PartyLegalEntity><cbc:RegistrationName>{}</cbc:RegistrationName>", echapper(&p.nom));
    // Le numéro d'entreprise belge, identifiant légal de la partie (BT-30 / BT-47).
    if let Some(id) = p.peppol.as_ref().filter(|i| i.schema == crate::peppol::SCHEMA_BCE) {
        let _ = write!(xml, "<cbc:CompanyID schemeID=\"{}\">{}</cbc:CompanyID>", id.schema, echapper(&id.numero));
    }
    xml.push_str("</cac:PartyLegalEntity>");
    if !p.email.trim().is_empty() || !p.telephone.trim().is_empty() {
        xml.push_str("<cac:Contact>");
        balise(xml, "cbc:Telephone", &p.telephone);
        balise(xml, "cbc:ElectronicMail", &p.email);
        xml.push_str("</cac:Contact>");
    }
    let _ = write!(xml, "</cac:Party></cac:{role}>");
}

/// Le document UBL complet.
pub(crate) fn document(d: &Document) -> String {
    let (racine, code_type, nom_code, ligne_nom, quantite_nom) = if d.note_credit {
        ("CreditNote", "381", "CreditNoteTypeCode", "CreditNoteLine", "CreditedQuantity")
    } else {
        ("Invoice", "380", "InvoiceTypeCode", "InvoiceLine", "InvoicedQuantity")
    };
    let t = factures::totaux(&d.lignes);
    let autoliq = |taux: i32| d.lignes.iter().any(|l| l.genre == "poste" && l.taux_centiemes == Some(taux) && l.autoliquidation);
    let franchise = |taux: i32| d.lignes.iter().any(|l| l.genre == "poste" && l.taux_centiemes == Some(taux) && l.franchise);
    let eur = "currencyID=\"EUR\"";

    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = write!(
        xml,
        "<{racine} xmlns=\"urn:oasis:names:specification:ubl:schema:xsd:{racine}-2\" \
         xmlns:cac=\"urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2\" \
         xmlns:cbc=\"urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2\">"
    );
    xml.push_str("<cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0</cbc:CustomizationID>");
    xml.push_str("<cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID>");
    let _ = write!(xml, "<cbc:ID>{}</cbc:ID><cbc:IssueDate>{}</cbc:IssueDate>", echapper(&d.numero), d.date);
    if let (Some(e), false) = (d.echeance, d.note_credit) {
        let _ = write!(xml, "<cbc:DueDate>{e}</cbc:DueDate>");
    }
    let _ = write!(xml, "<cbc:{nom_code}>{code_type}</cbc:{nom_code}>");
    balise(&mut xml, "cbc:Note", &d.remarque);
    // Les mentions légales de TVA font partie de la facture : elles voyagent aussi dans le XML (BT-22).
    for mention in crate::mentions_tva::a_imprimer(&d.lignes, d.lignes.iter().any(|l| l.genre == "poste" && l.franchise)) {
        balise(&mut xml, "cbc:Note", mention);
    }
    xml.push_str("<cbc:DocumentCurrencyCode>EUR</cbc:DocumentCurrencyCode>");
    // La référence de l'acheteur est obligatoire chez Peppol : la référence de la pièce, sinon son numéro.
    let reference = if d.reference.trim().is_empty() { d.numero.clone() } else { d.reference.clone() };
    balise(&mut xml, "cbc:BuyerReference", &reference);
    if let Some(n) = &d.facture_corrigee {
        let _ = write!(xml, "<cac:BillingReference><cac:InvoiceDocumentReference><cbc:ID>{}</cbc:ID></cac:InvoiceDocumentReference></cac:BillingReference>", echapper(n));
    }
    partie(&mut xml, "AccountingSupplierParty", &d.vendeur);
    partie(&mut xml, "AccountingCustomerParty", &d.acheteur);
    if !d.note_credit && !d.iban.trim().is_empty() {
        // 30 : virement.
        xml.push_str("<cac:PaymentMeans><cbc:PaymentMeansCode>30</cbc:PaymentMeansCode>");
        // La communication structurée en chiffres seuls (« +++000/0000/12629+++ » → « 000000012629 »).
        balise(&mut xml, "cbc:PaymentID", &d.vcs.chars().filter(char::is_ascii_digit).collect::<String>());
        let _ = write!(xml, "<cac:PayeeFinancialAccount><cbc:ID>{}</cbc:ID></cac:PayeeFinancialAccount></cac:PaymentMeans>", echapper(&compact(&d.iban)));
    }
    if !d.conditions.trim().is_empty() {
        let _ = write!(xml, "<cac:PaymentTerms><cbc:Note>{}</cbc:Note></cac:PaymentTerms>", echapper(&d.conditions));
    }

    let _ = write!(xml, "<cac:TaxTotal><cbc:TaxAmount {eur}>{}</cbc:TaxAmount>", montant(t.ttc_cents - t.ht_cents));
    for v in &t.tva {
        let cat = categorie(v.taux_centiemes, autoliq(v.taux_centiemes), franchise(v.taux_centiemes));
        let _ = write!(
            xml,
            "<cac:TaxSubtotal><cbc:TaxableAmount {eur}>{}</cbc:TaxableAmount><cbc:TaxAmount {eur}>{}</cbc:TaxAmount>\
             <cac:TaxCategory><cbc:ID>{cat}</cbc:ID><cbc:Percent>{}</cbc:Percent>",
            montant(v.base_cents),
            montant(v.tva_cents),
            pourcentage(v.taux_centiemes)
        );
        if cat == "AE" {
            xml.push_str("<cbc:TaxExemptionReasonCode>VATEX-EU-AE</cbc:TaxExemptionReasonCode><cbc:TaxExemptionReason>Autoliquidation</cbc:TaxExemptionReason>");
        }
        if cat == "E" {
            let _ = write!(xml, "<cbc:TaxExemptionReason>{}</cbc:TaxExemptionReason>", echapper(crate::societe::MENTION_FRANCHISE));
        }
        xml.push_str("<cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:TaxCategory></cac:TaxSubtotal>");
    }
    xml.push_str("</cac:TaxTotal>");
    let _ = write!(
        xml,
        "<cac:LegalMonetaryTotal><cbc:LineExtensionAmount {eur}>{ht}</cbc:LineExtensionAmount><cbc:TaxExclusiveAmount {eur}>{ht}</cbc:TaxExclusiveAmount>\
         <cbc:TaxInclusiveAmount {eur}>{ttc}</cbc:TaxInclusiveAmount><cbc:PayableAmount {eur}>{ttc}</cbc:PayableAmount></cac:LegalMonetaryTotal>",
        ht = montant(t.ht_cents),
        ttc = montant(t.ttc_cents)
    );

    let mut rang = 0;
    for l in d.lignes.iter().filter(|l| l.genre == "poste") {
        let Some(taux) = l.taux_centiemes else { continue };
        rang += 1;
        // Le prix ne peut pas être négatif : une déduction s'écrit « −1 × montant ».
        let quantite = if l.total_cents < 0 { "-1" } else { "1" };
        let mut nom: String = l.description.lines().next().unwrap_or("").trim().chars().take(100).collect();
        if nom.is_empty() {
            nom = format!("Ligne {rang}");
        }
        let mut detail = l.description.trim().to_string();
        if let (Some(q), Some(p)) = (l.quantite_e5, l.prix_unitaire_e5) {
            let quantite = [calcul::ecrire_decimal_e5(q), l.unite.trim().to_string()].join(" ");
            let _ = write!(detail, " ({} × {})", quantite.trim(), calcul::prix_pour_affichage(p));
        }
        let _ = write!(
            xml,
            "<cac:{ligne_nom}><cbc:ID>{rang}</cbc:ID><cbc:{quantite_nom} unitCode=\"C62\">{quantite}</cbc:{quantite_nom}>\
             <cbc:LineExtensionAmount {eur}>{}</cbc:LineExtensionAmount><cac:Item>",
            montant(l.total_cents)
        );
        balise(&mut xml, "cbc:Description", &detail);
        let _ = write!(
            xml,
            "<cbc:Name>{}</cbc:Name><cac:ClassifiedTaxCategory><cbc:ID>{}</cbc:ID><cbc:Percent>{}</cbc:Percent>\
             <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:ClassifiedTaxCategory></cac:Item>\
             <cac:Price><cbc:PriceAmount {eur}>{}</cbc:PriceAmount></cac:Price></cac:{ligne_nom}>",
            echapper(&nom),
            categorie(taux, l.autoliquidation, l.franchise),
            pourcentage(taux),
            montant(l.total_cents.abs())
        );
    }
    let _ = write!(xml, "</{racine}>\n");
    xml
}

/// Charge une facture (ou note de crédit) validée et écrit son UBL. `None` pour un brouillon.
pub async fn de_la_facture(db: &mut PgConnection, id: i64) -> Result<Option<String>, sqlx::Error> {
    let Some(f) = factures::charger(&mut *db, id).await? else { return Ok(None) };
    let Some(numero) = f.numero else { return Ok(None) };
    let lignes = factures::charger_lignes(&mut *db, id).await?;
    let societe: Option<(String, String, String, String, String, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT nom, trim(concat_ws(' ', rue, numero)), code_postal, ville, pays, '', numero_tva, email, telephone FROM societe WHERE organisation_id = app_organisation()",
    )
    .fetch_optional(&mut *db)
    .await?;
    let vendeur = societe
        .map(|(nom, rue, code_postal, ville, pays, _, tva, email, telephone)| Partie {
            nom,
            rue,
            code_postal,
            ville,
            pays,
            peppol: crate::peppol::depuis_tva(tva.as_deref().unwrap_or("")),
            tva: tva.unwrap_or_default(),
            email,
            telephone,
        })
        .unwrap_or_default();
    let colonnes = "nom_affiche, trim(concat_ws(' ', COALESCE(facturation_rue, rue), COALESCE(facturation_numero, numero))),
                    COALESCE(facturation_code_postal, code_postal), COALESCE(facturation_ville, ville), COALESCE(facturation_pays, pays),
                    COALESCE(numero_tva, ''), COALESCE(email, ''), COALESCE(telephone, ''), peppol_id";
    let acheteur: Option<(String, String, String, String, String, String, String, String, Option<String>)> = if let Some(e) = f.client_entreprise_id {
        sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {colonnes} FROM (SELECT *, nom AS nom_affiche FROM entreprises) x WHERE id = $1")))
            .bind(e)
            .fetch_optional(&mut *db)
            .await?
    } else if let Some(c) = f.client_contact_id {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {colonnes} FROM (SELECT *, trim(concat_ws(' ', prenom, nom)) AS nom_affiche FROM contacts) x WHERE id = $1"
        )))
        .bind(c)
        .fetch_optional(&mut *db)
        .await?
    } else {
        None
    };
    let acheteur = acheteur
        .map(|(nom, rue, code_postal, ville, pays, tva, email, telephone, peppol_id)| Partie {
            peppol: crate::peppol::du_client(peppol_id.as_deref(), &tva),
            nom,
            rue,
            code_postal,
            ville,
            pays,
            tva,
            email,
            telephone,
        })
        .unwrap_or_default();
    let iban: Option<String> = sqlx::query_scalar("SELECT iban FROM societe_comptes_bancaires WHERE organisation_id = app_organisation() AND par_defaut").fetch_optional(&mut *db).await?;
    let conditions = match f.conditions_paiement_id {
        Some(c) => crate::fiches::Options::charger(&mut *db, crate::fiches::Genre::Entreprise)
            .await?
            .conditions
            .into_iter()
            .find(|(i, _)| *i == c)
            .map(|(_, t)| t)
            .unwrap_or_default(),
        None => String::new(),
    };
    let facture_corrigee: Option<String> = match f.facture_origine_id {
        Some(o) => {
            let origine: Option<(Option<i64>, NaiveDate)> =
                sqlx::query_as("SELECT numero, date_facture FROM factures WHERE id = $1").bind(o).fetch_optional(&mut *db).await?;
            origine.and_then(|(n, date)| n.map(|n| factures::numero_affiche(date, n)))
        }
        None => None,
    };
    Ok(Some(document(&Document {
        note_credit: f.genre == "note_credit",
        numero: factures::numero_affiche(f.date_facture, numero),
        date: f.date_facture,
        echeance: f.date_echeance,
        reference: f.reference.clone(),
        remarque: f.remarque.clone(),
        facture_corrigee,
        vendeur,
        acheteur,
        iban: iban.unwrap_or_default(),
        vcs: f.vcs.clone().unwrap_or_default(),
        conditions,
        lignes,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poste(description: &str, total: i64, taux: i32, autoliquidation: bool) -> LigneBase {
        LigneBase {
            id: 0,
            genre: "poste".into(),
            description: description.into(),
            quantite_e5: Some(100_000),
            unite: "forfait".into(),
            prix_unitaire_e5: Some(total * 1_000),
            taux_tva_id: Some(1),
            taux_centiemes: Some(taux),
            total_cents: total,
            autoliquidation,
            franchise: false,
        }
    }

    fn doc(lignes: Vec<LigneBase>, note_credit: bool) -> Document {
        Document {
            note_credit,
            numero: "2026-126".into(),
            date: NaiveDate::from_ymd_opt(2026, 10, 16).unwrap(),
            echeance: NaiveDate::from_ymd_opt(2026, 10, 23),
            reference: "L63 Toiture & co".into(),
            remarque: String::new(),
            facture_corrigee: note_credit.then(|| "2026-125".to_string()),
            vendeur: Partie {
                nom: "Ubique SRL".into(),
                tva: "BE 0123.456.749".into(),
                pays: "BE".into(),
                peppol: crate::peppol::depuis_tva("BE 0123.456.749"),
                ..Default::default()
            },
            acheteur: Partie { nom: "Lambert <Marc>".into(), ..Default::default() },
            iban: "BE68 5390 0754 7034".into(),
            vcs: "+++000/0000/12629+++".into(),
            conditions: "7 jour(s) après date de facturation".into(),
            lignes,
        }
    }

    #[test]
    fn montants_et_pourcentages() {
        assert_eq!(montant(296_930), "2969.30");
        assert_eq!(montant(-5), "-0.05");
        assert_eq!(montant(0), "0.00");
        assert_eq!(pourcentage(600), "6");
        assert_eq!(pourcentage(2100), "21");
        assert_eq!(pourcentage(550), "5.5");
        assert_eq!(pourcentage(1225), "12.25");
    }

    #[test]
    fn la_facture_finale_de_l_exemple() {
        // 2 969,30 € HT à 6 % : TVA 178,16 €, TTC 3 147,46 €.
        let xml = document(&doc(vec![poste("Solde du chantier", 296_930, 600, false)], false));
        for attendu in [
            "<Invoice xmlns=\"urn:oasis:names:specification:ubl:schema:xsd:Invoice-2\"",
            "<cbc:ID>2026-126</cbc:ID><cbc:IssueDate>2026-10-16</cbc:IssueDate><cbc:DueDate>2026-10-23</cbc:DueDate>",
            "<cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>",
            "<cbc:BuyerReference>L63 Toiture &amp; co</cbc:BuyerReference>",
            "<cbc:EndpointID schemeID=\"0208\">0123456749</cbc:EndpointID>",
            "<cbc:CompanyID>BE0123456749</cbc:CompanyID>",
            "<cbc:RegistrationName>Ubique SRL</cbc:RegistrationName><cbc:CompanyID schemeID=\"0208\">0123456749</cbc:CompanyID>",
            "<cbc:Description>Solde du chantier (1 forfait × 2969,30\u{a0}€)</cbc:Description>",
            "<cbc:Name>Lambert &lt;Marc&gt;</cbc:Name>",
            "<cbc:PaymentID>000000012629</cbc:PaymentID><cac:PayeeFinancialAccount><cbc:ID>BE68539007547034</cbc:ID>",
            "<cbc:TaxAmount currencyID=\"EUR\">178.16</cbc:TaxAmount>",
            "<cbc:TaxableAmount currencyID=\"EUR\">2969.30</cbc:TaxableAmount>",
            "<cbc:ID>S</cbc:ID><cbc:Percent>6</cbc:Percent>",
            "<cbc:PayableAmount currencyID=\"EUR\">3147.46</cbc:PayableAmount>",
            "<cbc:InvoicedQuantity unitCode=\"C62\">1</cbc:InvoicedQuantity><cbc:LineExtensionAmount currencyID=\"EUR\">2969.30</cbc:LineExtensionAmount>",
        ] {
            assert!(xml.contains(attendu), "{attendu}\n{xml}");
        }
        assert!(xml.ends_with("</Invoice>\n"));
        // Un particulier sans TVA n'a pas d'adresse Peppol.
        assert_eq!(xml.matches("<cbc:EndpointID").count(), 1);
    }

    #[test]
    fn deduction_autoliquidation_et_note_de_credit() {
        let xml = document(&doc(vec![poste("Travaux", 100_000, 0, true), poste("Déduction de l'acompte", -30_000, 0, true)], false));
        assert!(xml.contains("<cbc:InvoicedQuantity unitCode=\"C62\">-1</cbc:InvoicedQuantity><cbc:LineExtensionAmount currencyID=\"EUR\">-300.00</cbc:LineExtensionAmount>"));
        assert!(xml.contains("<cbc:PriceAmount currencyID=\"EUR\">300.00</cbc:PriceAmount>"), "le prix reste positif");
        assert!(xml.contains("<cbc:ID>AE</cbc:ID><cbc:Percent>0</cbc:Percent><cbc:TaxExemptionReasonCode>VATEX-EU-AE</cbc:TaxExemptionReasonCode>"));
        assert!(xml.contains("<cbc:PayableAmount currencyID=\"EUR\">700.00</cbc:PayableAmount>"));

        let nc = document(&doc(vec![poste("Travaux", 100_000, 2100, false)], true));
        for attendu in ["<CreditNote xmlns", "<cbc:CreditNoteTypeCode>381</cbc:CreditNoteTypeCode>", "<cac:InvoiceDocumentReference><cbc:ID>2026-125</cbc:ID>", "<cac:CreditNoteLine>", "<cbc:CreditedQuantity unitCode=\"C62\">1</cbc:CreditedQuantity>", "<cbc:PayableAmount currencyID=\"EUR\">1210.00</cbc:PayableAmount>"] {
            assert!(nc.contains(attendu), "{attendu}");
        }
        assert!(!nc.contains("DueDate") && !nc.contains("PaymentMeans"), "une note de crédit ne se paie pas");
    }

    #[test]
    fn les_caracteres_speciaux_sont_proteges() {
        assert_eq!(echapper("a & b < c > \"d\" 'e'\u{1}"), "a &amp; b &lt; c &gt; &quot;d&quot; &apos;e&apos;");
        assert_eq!(compact("BE 0123.456.789"), "BE0123456789");
    }
}
