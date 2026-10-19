//! Reprise des données de Vertuoza (transition, étapes 1 et 2) : les factures, notes de crédit et paiements (étape 1),
//! puis les devis, avenants et chantiers (étape 2, voir `reprise_devis.rs`).
//!
//! Les données sont relevées dans Vertuoza (en lecture seule) et écrites dans un fichier JSON (format plus bas),
//! puis chargées par `cargo run -- reprise <fichier.json>`. Sans `--enregistrer`, c'est un **essai** : tout est fait
//! dans une transaction annulée à la fin, et seul le rapport s'affiche (factures, totaux, alertes). Avec
//! `--enregistrer`, la même chose est gardée.
//!
//! Une facture reprise garde le numéro, les dates, les montants et la communication structurée de Vertuoza (le PDF
//! d'origine fait foi) ; elle est validée et verrouillée (`reprise = true`). Les clients sont retrouvés par leur nom
//! (entreprise, puis contact), sinon créés. Relancer la reprise ne crée pas de doublon : une facture déjà reprise
//! (même série, même année, même numéro) est passée. Une facture créée dans Ubifacture avec le même numéro bloque tout.
//! Les compteurs de numéros sont remontés au plus grand numéro repris de l'année.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use sqlx::{PgPool, Postgres, Transaction};

use crate::commun::format_euros;

// ─── Le fichier ─────────────────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
pub struct Fichier {
    #[serde(default)]
    pub factures: Vec<FactureReprise>,
    #[serde(default)]
    pub devis: Vec<crate::reprise_devis::DevisReprise>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Client {
    pub nom: String,
    /// « entreprise » ou « contact ».
    #[serde(default = "entreprise")]
    pub r#type: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub tva: String,
}

fn entreprise() -> String {
    "entreprise".to_string()
}

#[derive(Deserialize, Debug, Clone)]
pub struct LigneReprise {
    /// « titre », « texte » ou « poste ».
    #[serde(default = "poste")]
    pub genre: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub quantite: String,
    #[serde(default)]
    pub unite: String,
    #[serde(default)]
    pub prix_unitaire: String,
    /// Taux de TVA en centièmes (600 = 6 %).
    #[serde(default)]
    pub tva: Option<i32>,
    /// Total HT de la ligne (« 1295,92 »).
    #[serde(default)]
    pub total: String,
}

fn poste() -> String {
    "poste".to_string()
}

#[derive(Deserialize, Debug, Clone)]
pub struct Totaux {
    pub ht: String,
    pub tva: String,
    pub ttc: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PaiementReprise {
    pub date: NaiveDate,
    pub montant: String,
    #[serde(default)]
    pub methode: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct FactureReprise {
    /// simple, acompte, avancement, finale, recurrente, note_credit.
    pub genre: String,
    pub numero: i64,
    pub date: NaiveDate,
    #[serde(default)]
    pub echeance: Option<NaiveDate>,
    /// « a_envoyer » ou « envoyee ».
    #[serde(default = "envoyee")]
    pub statut: String,
    pub client: Client,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub responsable: String,
    /// La référence du chantier dans Vertuoza (« PL45 »).
    #[serde(default)]
    pub chantier: String,
    /// Nombre de jours des conditions de paiement (7 = « 7 jour(s) après date de facturation »).
    #[serde(default)]
    pub conditions_jours: Option<i32>,
    #[serde(default)]
    pub vcs: String,
    /// Pour une note de crédit : le numéro de la facture corrigée (même année).
    #[serde(default)]
    pub facture_origine: Option<i64>,
    #[serde(default)]
    pub lignes: Vec<LigneReprise>,
    pub totaux: Totaux,
    #[serde(default)]
    pub paiements: Vec<PaiementReprise>,
}

fn envoyee() -> String {
    "envoyee".to_string()
}

const GENRES: [&str; 6] = ["simple", "acompte", "avancement", "finale", "recurrente", "note_credit"];

/// « 1 295,92 », « -50 », « 3852.54 » → centimes.
pub fn centimes(texte: &str) -> Option<i64> {
    let t = texte.trim().replace(['€', '\u{a0}'], "");
    let (signe, t) = match t.trim().strip_prefix('-') {
        Some(r) => (-1, r.to_string()),
        None => (1, t.trim().to_string()),
    };
    crate::fiches::euros_vers_centimes(&t).map(|c| signe * c)
}

// ─── Le rapport ─────────────────────────────────────────────────────────────

#[derive(Default, Debug)]
pub struct Rapport {
    pub enregistre: bool,
    pub factures: usize,
    pub notes_credit: usize,
    pub deja: usize,
    pub paiements: usize,
    pub clients_crees: Vec<String>,
    /// (année, série) → (nombre, total HT, total TTC)
    pub par_annee: BTreeMap<(i32, &'static str), (usize, i64, i64)>,
    pub a_recevoir_cents: i64,
    pub alertes: Vec<String>,
    // Étape 2 : devis, avenants, chantiers.
    pub devis: usize,
    pub avenants: usize,
    pub chantiers: usize,
    pub devis_deja: usize,
    pub factures_rattachees: usize,
    /// Total HT des devis et avenants acceptés repris.
    pub accepte_ht_cents: i64,
    /// statut → (nombre, total HT)
    pub devis_par_statut: BTreeMap<String, (usize, i64)>,
}

impl std::fmt::Display for Rapport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", if self.enregistre { "REPRISE ENREGISTRÉE" } else { "ESSAI (rien n'est enregistré ; ajoute --enregistrer pour garder)" })?;
        writeln!(f, "Factures reprises : {} · notes de crédit : {} · déjà présentes : {} · paiements : {}", self.factures, self.notes_credit, self.deja, self.paiements)?;
        if self.devis + self.avenants + self.devis_deja > 0 {
            writeln!(
                f,
                "Devis repris : {} · avenants : {} · chantiers créés : {} · déjà présents : {} · factures rattachées à un chantier : {}",
                self.devis, self.avenants, self.chantiers, self.devis_deja, self.factures_rattachees
            )?;
            for (statut, (n, ht)) in &self.devis_par_statut {
                writeln!(f, "  {statut} : {n} pièce(s), {} HT", format_euros(*ht))?;
            }
            writeln!(f, "  Devis et avenants acceptés repris : {} HT", format_euros(self.accepte_ht_cents))?;
        }
        for ((annee, serie), (n, ht, ttc)) in &self.par_annee {
            writeln!(f, "  {annee} · {serie} : {n} pièce(s), {} HT, {} TTC", format_euros(*ht), format_euros(*ttc))?;
        }
        writeln!(f, "Reste à recevoir sur les factures reprises : {}", format_euros(self.a_recevoir_cents))?;
        if !self.clients_crees.is_empty() {
            writeln!(f, "Clients créés ({}) : {}", self.clients_crees.len(), self.clients_crees.join(", "))?;
        }
        if self.alertes.is_empty() {
            writeln!(f, "Aucune alerte.")
        } else {
            writeln!(f, "Alertes ({}) :", self.alertes.len())?;
            for a in &self.alertes {
                writeln!(f, "  - {a}")?;
            }
            Ok(())
        }
    }
}

// ─── L'import ───────────────────────────────────────────────────────────────

pub(crate) async fn client(tx: &mut Transaction<'_, Postgres>, c: &Client, rapport: &mut Rapport) -> Result<(Option<i64>, Option<i64>), sqlx::Error> {
    let nom = c.nom.trim();
    let entreprise: Option<i64> =
        sqlx::query_scalar("SELECT id FROM entreprises WHERE lower(nom) = lower($1) ORDER BY id LIMIT 1").bind(nom).fetch_optional(&mut **tx).await?;
    if entreprise.is_some() {
        return Ok((entreprise, None));
    }
    let contact: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM contacts WHERE lower(trim(concat_ws(' ', prenom, nom))) = lower($1) OR lower(nom) = lower($1) ORDER BY id LIMIT 1",
    )
    .bind(nom)
    .fetch_optional(&mut **tx)
    .await?;
    if contact.is_some() {
        return Ok((None, contact));
    }
    let email = Some(c.email.trim()).filter(|e| crate::commun::email_valide(e));
    let tva = Some(c.tva.trim()).filter(|t| !t.is_empty());
    rapport.clients_crees.push(nom.to_string());
    if c.r#type == "contact" {
        let id: i64 = sqlx::query_scalar("INSERT INTO contacts (nom, profils, email, numero_tva) VALUES ($1, '{client}', $2, $3) RETURNING id")
            .bind(nom)
            .bind(email)
            .bind(tva)
            .fetch_one(&mut **tx)
            .await?;
        Ok((None, Some(id)))
    } else {
        let id: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, profils, email, numero_tva) VALUES ($1, '{client}', $2, $3) RETURNING id")
            .bind(nom)
            .bind(email)
            .bind(tva)
            .fetch_one(&mut **tx)
            .await?;
        Ok((Some(id), None))
    }
}

/// Charge le fichier. `enregistrer` faux : essai, tout est annulé à la fin.
pub async fn importer(db: &PgPool, texte: &str, enregistrer: bool) -> Result<Rapport, String> {
    let fichier: Fichier = serde_json::from_str(texte).map_err(|e| format!("Le fichier n'est pas lisible : {e}"))?;
    let mut rapport = Rapport { enregistre: enregistrer, ..Default::default() };
    let erreur_base = |e: sqlx::Error| format!("Erreur de la base : {e}");
    let mut tx = db.begin().await.map_err(erreur_base)?;

    // Les devis, avenants et chantiers d'abord : les factures y sont rattachées à la fin.
    crate::reprise_devis::importer(&mut tx, &fichier.devis, &mut rapport).await?;

    // Les factures d'abord, les notes de crédit ensuite (elles pointent vers leur facture).
    let mut pieces = fichier.factures.clone();
    pieces.sort_by_key(|p| (p.genre == "note_credit", p.date, p.numero));
    for p in &pieces {
        let nom = format!("{} n° {}-{}", if p.genre == "note_credit" { "Note de crédit" } else { "Facture" }, p.date.year(), p.numero);
        if !GENRES.contains(&p.genre.as_str()) {
            return Err(format!("{nom} : type inconnu « {} »", p.genre));
        }
        let note = p.genre == "note_credit";
        let serie = if note { "notes de crédit" } else { "factures" };
        // Déjà là ?
        let existante: Option<bool> = sqlx::query_scalar(
            "SELECT reprise FROM factures WHERE numero = $1 AND EXTRACT(YEAR FROM date_facture) = $2 AND (genre = 'note_credit') = $3",
        )
        .bind(p.numero)
        .bind(f64::from(p.date.year()))
        .bind(note)
        .fetch_optional(&mut *tx)
        .await
        .map_err(erreur_base)?;
        match existante {
            Some(true) => {
                rapport.deja += 1;
                continue;
            }
            Some(false) => return Err(format!("{nom} existe déjà dans Ubifacture (créée ici, pas reprise) : rien n'est importé.")),
            None => {}
        }
        let (Some(ht), Some(tva), Some(ttc)) = (centimes(&p.totaux.ht), centimes(&p.totaux.tva), centimes(&p.totaux.ttc)) else {
            return Err(format!("{nom} : les totaux ne sont pas des montants."));
        };
        if ht + tva != ttc {
            rapport.alertes.push(format!("{nom} : HT + TVA ({}) ≠ TTC ({}).", format_euros(ht + tva), format_euros(ttc)));
        }
        let origine: Option<i64> = match (note, p.facture_origine) {
            (true, Some(n)) => {
                let id: Option<i64> = sqlx::query_scalar(
                    "SELECT id FROM factures WHERE numero = $1 AND EXTRACT(YEAR FROM date_facture) = $2 AND genre <> 'note_credit'",
                )
                .bind(n)
                .bind(f64::from(p.date.year()))
                .fetch_optional(&mut *tx)
                .await
                .map_err(erreur_base)?;
                Some(id.ok_or_else(|| format!("{nom} : la facture corrigée n° {n} n'est pas dans la reprise.")))
                    .transpose()?
            }
            (true, None) => return Err(format!("{nom} : il manque la facture corrigée (facture_origine).")),
            _ => None,
        };
        let (entreprise, contact) = client(&mut tx, &p.client, &mut rapport).await.map_err(erreur_base)?;
        let conditions: Option<i64> = match p.conditions_jours {
            Some(0) => sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'comptant'").fetch_optional(&mut *tx).await.map_err(erreur_base)?,
            Some(j) => sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = $1")
                .bind(j)
                .fetch_optional(&mut *tx)
                .await
                .map_err(erreur_base)?,
            None => None,
        };
        // Brouillon d'abord (les lignes d'une pièce numérotée ne changent plus), puis numéroté.
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO factures (genre, date_facture, reference, responsable, client_entreprise_id, client_contact_id, conditions_paiement_id,
                                   facture_origine_id, reprise, chantier_vertuoza, montant_ht_cents, montant_tva_cents, montant_ttc_cents)
             VALUES ($1::text::genre_facture, $2, $3, $4, $5, $6, $7, $8, true, $9, $10, $11, $12) RETURNING id",
        )
        .bind(&p.genre)
        .bind(p.date)
        .bind(p.reference.trim())
        .bind(p.responsable.trim())
        .bind(entreprise)
        .bind(contact)
        .bind(conditions)
        .bind(origine)
        .bind(p.chantier.trim())
        .bind(ht)
        .bind(tva)
        .bind(ttc)
        .fetch_one(&mut *tx)
        .await
        .map_err(erreur_base)?;
        let mut somme_lignes = 0;
        for (i, l) in p.lignes.iter().enumerate() {
            let genre = match l.genre.as_str() {
                "titre" | "lot" => "titre",
                "texte" | "sous_titre" => "texte",
                _ => "poste",
            };
            if genre != "poste" {
                sqlx::query("INSERT INTO facture_lignes (facture_id, position, genre, description) VALUES ($1, $2, $3, $4)")
                    .bind(id)
                    .bind(i as i32 + 1)
                    .bind(genre)
                    .bind(l.description.trim())
                    .execute(&mut *tx)
                    .await
                    .map_err(erreur_base)?;
                continue;
            }
            let total = centimes(&l.total).ok_or_else(|| format!("{nom}, ligne {} : le total n'est pas un montant.", i + 1))?;
            somme_lignes += total;
            let taux = l.tva.unwrap_or(2100);
            let taux_id: Option<i64> = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = $1 ORDER BY actif DESC, id LIMIT 1")
                .bind(taux)
                .fetch_optional(&mut *tx)
                .await
                .map_err(erreur_base)?;
            if taux_id.is_none() {
                rapport.alertes.push(format!("{nom}, ligne {} : taux de TVA {} inconnu.", i + 1, crate::calcul::taux_en_texte(taux)));
            }
            sqlx::query(
                "INSERT INTO facture_lignes (facture_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, taux_tva_id, taux_centiemes, total_cents)
                 VALUES ($1, $2, 'poste', $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(id)
            .bind(i as i32 + 1)
            .bind(l.description.trim())
            .bind(crate::calcul::lire_decimal_e5(&l.quantite))
            .bind(l.unite.trim().chars().take(20).collect::<String>())
            .bind(crate::calcul::lire_decimal_e5(&l.prix_unitaire))
            .bind(taux_id)
            .bind(taux_id.map(|_| taux))
            .bind(total)
            .execute(&mut *tx)
            .await
            .map_err(erreur_base)?;
        }
        if !p.lignes.is_empty() && somme_lignes != ht {
            rapport.alertes.push(format!("{nom} : la somme des lignes ({}) ≠ total HT ({}). Les totaux de Vertuoza sont gardés.", format_euros(somme_lignes), format_euros(ht)));
        }
        let vcs = Some(p.vcs.trim().to_string()).filter(|v| !v.is_empty() && !note);
        if let Some(v) = &vcs
            && !crate::facture_calcul::vcs_valide(v)
        {
            rapport.alertes.push(format!("{nom} : communication structurée « {v} » non valable."));
        }
        let statut = if p.statut == "a_envoyer" { "a_envoyer" } else { "envoyee" };
        sqlx::query(
            "UPDATE factures SET numero = $2, statut = $3::text::statut_facture, date_echeance = $4, vcs = $5, validee_le = now(),
                                 envoyee_le = CASE WHEN $3 = 'envoyee' THEN now() END
             WHERE id = $1",
        )
        .bind(id)
        .bind(p.numero)
        .bind(statut)
        .bind(if note { None } else { p.echeance })
        .bind(vcs)
        .execute(&mut *tx)
        .await
        .map_err(erreur_base)?;
        crate::paiements::noter(&mut tx, id, "validation", &format!("Reprise de Vertuoza : {nom}")).await.map_err(erreur_base)?;
        // Paiements.
        for pa in &p.paiements {
            if note {
                rapport.alertes.push(format!("{nom} : un paiement sur une note de crédit est ignoré."));
                continue;
            }
            let montant = centimes(&pa.montant).filter(|m| *m > 0).ok_or_else(|| format!("{nom} : paiement « {} » invalide.", pa.montant))?;
            let methode: i64 = sqlx::query_scalar(
                "SELECT id FROM methodes_paiement WHERE lower(libelle) = lower($1) UNION ALL SELECT id FROM methodes_paiement WHERE libelle = 'Virement' LIMIT 1",
            )
            .bind(pa.methode.trim())
            .fetch_one(&mut *tx)
            .await
            .map_err(erreur_base)?;
            sqlx::query("INSERT INTO paiements (facture_id, date_paiement, montant_cents, methode_id, remarque) VALUES ($1, $2, $3, $4, 'Reprise de Vertuoza')")
                .bind(id)
                .bind(pa.date)
                .bind(montant)
                .bind(methode)
                .execute(&mut *tx)
                .await
                .map_err(erreur_base)?;
            rapport.paiements += 1;
        }
        if note {
            rapport.notes_credit += 1;
        } else {
            rapport.factures += 1;
        }
        let entree = rapport.par_annee.entry((p.date.year(), serie)).or_insert((0, 0, 0));
        entree.0 += 1;
        entree.1 += ht;
        entree.2 += ttc;
    }

    // Les compteurs ne redescendent jamais sous le plus grand numéro repris de l'année en cours du compteur.
    for (compteur, note) in [("facture", false), ("note_credit", true)] {
        let (valeur, annee): (i64, Option<i32>) =
            sqlx::query_as("SELECT valeur, annee FROM compteurs WHERE nom = $1 FOR UPDATE").bind(compteur).fetch_one(&mut *tx).await.map_err(erreur_base)?;
        let max: Option<(i32, i64)> = sqlx::query_as(
            "SELECT EXTRACT(YEAR FROM date_facture)::int AS a, max(numero) FROM factures
             WHERE numero IS NOT NULL AND (genre = 'note_credit') = $1 GROUP BY a ORDER BY a DESC LIMIT 1",
        )
        .bind(note)
        .fetch_optional(&mut *tx)
        .await
        .map_err(erreur_base)?;
        if let Some((a, n)) = max {
            let (nouvelle_valeur, nouvelle_annee) = match annee {
                Some(x) if x > a => (valeur, x),
                Some(x) if x == a => (valeur.max(n), x),
                _ => (n, a),
            };
            if (nouvelle_valeur, Some(nouvelle_annee)) != (valeur, annee) {
                rapport.alertes.push(format!("Compteur des {compteur}s : {valeur} ({}) → {nouvelle_valeur} ({nouvelle_annee}).", annee.map(|a| a.to_string()).unwrap_or_default()));
                sqlx::query("UPDATE compteurs SET valeur = $2, annee = $3 WHERE nom = $1")
                    .bind(compteur)
                    .bind(nouvelle_valeur)
                    .bind(nouvelle_annee)
                    .execute(&mut *tx)
                    .await
                    .map_err(erreur_base)?;
            }
        }
    }
    crate::reprise_devis::remonter_compteur(&mut tx, &mut rapport).await.map_err(erreur_base)?;
    crate::reprise_devis::rattacher_factures(&mut tx, &mut rapport).await.map_err(erreur_base)?;
    rapport.a_recevoir_cents = sqlx::query_scalar(
        "SELECT COALESCE(sum(GREATEST(et.reste_cents, 0)), 0)::bigint FROM factures f JOIN factures_etats et ON et.facture_id = f.id WHERE f.reprise",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(erreur_base)?;
    if enregistrer {
        tx.commit().await.map_err(erreur_base)?;
    } else {
        tx.rollback().await.map_err(erreur_base)?;
    }
    Ok(rapport)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_montants() {
        assert_eq!(centimes("1 295,92"), Some(129_592));
        assert_eq!(centimes("-3 780,36 €"), Some(-378_036));
        assert_eq!(centimes("3852.54"), Some(385_254));
        assert_eq!(centimes("abc"), None);
    }
}
