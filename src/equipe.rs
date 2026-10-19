//! L'équipe d'une organisation (étape 2) : Réglages › Équipe et la page d'invitation.
//!
//! - **Propriétaire** : invite (par e-mail, lien valable 7 jours), annule une invitation, change un rôle, retire un
//!   membre. **Membre** : voit l'équipe. Toujours au moins un propriétaire.
//! - **8 personnes au plus** par organisation (membres et invitations en attente) ; la limite de chaque formule viendra
//!   avec les abonnements (étape 6).
//! - Pour l'instant, **un compte = une organisation** : on n'invite pas une adresse qui a déjà un compte, et un membre
//!   retiré qui n'appartient à aucune autre organisation est supprimé (ses données restent dans l'organisation).
//! - Les tables `membres` et `invitations` sont communes au site : chaque requête de ces écrans filtre explicitement
//!   sur l'organisation de la session.

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::{Connection as _, PgConnection};

use crate::base::{Base, Utilisateur};
use crate::comptes::PageMessage;
use crate::connexion::{self, LONGUEUR_MIN, assez_long, empreinte};
use crate::emails_systeme::{self, Genre};
use crate::{AppState, Erreur, commun::email_valide};

pub const MEMBRES_MAX: i64 = 8;
const VALIDITE_JOURS: i32 = 7;
const LONGUEUR_MAX_NOM: usize = 120;

pub const ROLES: [(&str, &str); 2] = [("membre", "Membre"), ("proprietaire", "Propriétaire")];

fn libelle_role(role: &str) -> &'static str {
    ROLES.iter().find(|(v, _)| *v == role).map(|(_, l)| *l).unwrap_or("Membre")
}

async fn role(db: &mut PgConnection, utilisateur: i64, organisation: i64) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT role::text FROM membres WHERE utilisateur_id = $1 AND organisation_id = $2")
        .bind(utilisateur)
        .bind(organisation)
        .fetch_optional(db)
        .await
}

/// Les membres et les invitations encore valables : ce qui compte pour la limite de 8.
async fn places_prises(db: &mut PgConnection, organisation: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM membres WHERE organisation_id = $1)
              + (SELECT count(*) FROM invitations WHERE organisation_id = $1 AND acceptee_le IS NULL AND expire_le > now())",
    )
    .bind(organisation)
    .fetch_one(db)
    .await
}

fn vers(code: &str, valeur: &str) -> Response {
    Redirect::to(&format!("/parametres/equipe?{code}={valeur}")).into_response()
}

// ─── Réglages › Équipe ──────────────────────────────────────────────────────

struct Membre {
    id: i64,
    email: String,
    nom: String,
    role: String,
    libelle: &'static str,
    depuis: String,
    moi: bool,
}

struct InvitationVue {
    id: i64,
    email: String,
    libelle: &'static str,
    expire: String,
}

#[derive(Template)]
#[template(path = "equipe.html")]
struct Page {
    membres: Vec<Membre>,
    invitations: Vec<InvitationVue>,
    proprietaire: bool,
    places: i64,
    max: i64,
    roles: &'static [(&'static str, &'static str)],
    message_ok: &'static str,
    message_erreur: &'static str,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
    erreur: String,
}

async fn afficher(mut db: Base, Utilisateur(moi): Utilisateur, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let organisation = db.organisation;
    let lignes: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT u.id, u.email, u.nom, m.role::text, to_char(m.created_at AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY')
         FROM membres m JOIN utilisateurs u ON u.id = m.utilisateur_id WHERE m.organisation_id = $1 ORDER BY m.created_at, u.id",
    )
    .bind(organisation)
    .fetch_all(&mut *db)
    .await?;
    let invitations: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT id, email, role::text, to_char(expire_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY')
         FROM invitations WHERE organisation_id = $1 AND acceptee_le IS NULL AND expire_le > now() ORDER BY created_at",
    )
    .bind(organisation)
    .fetch_all(&mut *db)
    .await?;
    let proprietaire = role(&mut db, moi, organisation).await?.as_deref() == Some("proprietaire");
    let page = Page {
        places: places_prises(&mut db, organisation).await?,
        membres: lignes
            .into_iter()
            .map(|(id, email, nom, role, depuis)| Membre { id, email, nom, libelle: libelle_role(&role), role, depuis, moi: id == moi })
            .collect(),
        invitations: invitations
            .into_iter()
            .map(|(id, email, role, expire)| InvitationVue { id, email, libelle: libelle_role(&role), expire })
            .collect(),
        proprietaire,
        max: MEMBRES_MAX,
        roles: &ROLES,
        message_ok: match r.ok.as_str() {
            "invitation" => "Invitation envoyée : elle est valable 7 jours.",
            "annulee" => "Invitation annulée.",
            "role" => "Rôle changé.",
            "retire" => "Membre retiré de l'équipe.",
            _ => "",
        },
        message_erreur: match r.erreur.as_str() {
            "droits" => "Seul un propriétaire peut gérer l'équipe.",
            "email" => "Cette adresse e-mail n'est pas valable.",
            "membre" => "Cette personne fait déjà partie de l'équipe.",
            "compte" => "Cette adresse a déjà un compte : pour l'instant, un compte ne peut appartenir qu'à une seule organisation.",
            "limite" => "L'équipe est complète (8 personnes au plus, invitations en attente comprises).",
            "toi" => "Tu ne peux pas te retirer toi-même de l'équipe.",
            "dernier" => "Il faut garder au moins un propriétaire.",
            _ => "",
        },
    };
    Ok(Html(page.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Invitation {
    email: String,
    role: String,
}

async fn inviter(
    State(state): State<AppState>,
    mut db: Base,
    Utilisateur(moi): Utilisateur,
    Form(i): Form<Invitation>,
) -> Result<Response, Erreur> {
    let organisation = db.organisation;
    if role(&mut db, moi, organisation).await?.as_deref() != Some("proprietaire") {
        return Ok(vers("erreur", "droits"));
    }
    let email = i.email.trim().to_string();
    if !email_valide(&email) {
        return Ok(vers("erreur", "email"));
    }
    let role_invite = if i.role == "proprietaire" { "proprietaire" } else { "membre" };
    let compte: Option<bool> = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM membres WHERE utilisateur_id = u.id AND organisation_id = $2) FROM utilisateurs u WHERE lower(u.email) = lower($1)",
    )
    .bind(&email)
    .bind(organisation)
    .fetch_optional(&mut *db)
    .await?;
    match compte {
        Some(true) => return Ok(vers("erreur", "membre")),
        Some(false) => return Ok(vers("erreur", "compte")),
        None => {}
    }
    let mut tx = db.begin().await?;
    // Une nouvelle invitation remplace celle qui attendait encore pour cette adresse.
    sqlx::query("DELETE FROM invitations WHERE organisation_id = $1 AND lower(email) = lower($2) AND acceptee_le IS NULL")
        .bind(organisation)
        .bind(&email)
        .execute(&mut *tx)
        .await?;
    if places_prises(&mut tx, organisation).await? >= MEMBRES_MAX {
        return Ok(vers("erreur", "limite"));
    }
    let jeton = connexion::jeton_aleatoire();
    sqlx::query(
        "INSERT INTO invitations (organisation_id, email, role, empreinte, invite_par, expire_le)
         VALUES ($1, $2, $3::text::role_membre, $4, $5, now() + make_interval(days => $6))",
    )
    .bind(organisation)
    .bind(&email)
    .bind(role_invite)
    .bind(empreinte(&jeton))
    .bind(moi)
    .bind(VALIDITE_JOURS)
    .execute(&mut *tx)
    .await?;
    let (nom_organisation, nom_invitant): (String, String) = sqlx::query_as(
        "SELECT o.nom, coalesce(nullif(u.nom, ''), u.email) FROM organisations o, utilisateurs u WHERE o.id = $1 AND u.id = $2",
    )
    .bind(organisation)
    .bind(moi)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    drop(db);
    let texte = format!(
        "Bonjour,\n\n{nom_invitant} t'invite à rejoindre {nom_organisation} sur {site}, pour préparer les devis et les factures \
         ensemble.\nPour accepter et choisir ton mot de passe, ouvre ce lien (valable 7 jours) :\n{lien}\n\n{site}",
        site = emails_systeme::NOM_DU_SITE,
        lien = format!("{}/invitation/{jeton}", crate::emails::adresse_publique()),
    );
    emails_systeme::envoyer(&state.db, Genre::Invitation, &email, &format!("Invitation à rejoindre {nom_organisation}"), &texte).await?;
    Ok(vers("ok", "invitation"))
}

async fn annuler(mut db: Base, Utilisateur(moi): Utilisateur, Path(id): Path<i64>) -> Result<Response, Erreur> {
    let organisation = db.organisation;
    if role(&mut db, moi, organisation).await?.as_deref() != Some("proprietaire") {
        return Ok(vers("erreur", "droits"));
    }
    let fait = sqlx::query("DELETE FROM invitations WHERE id = $1 AND organisation_id = $2 AND acceptee_le IS NULL")
        .bind(id)
        .bind(organisation)
        .execute(&mut *db)
        .await?;
    if fait.rows_affected() == 0 {
        return Err(Erreur::introuvable());
    }
    Ok(vers("ok", "annulee"))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NouveauRole {
    role: String,
}

/// Les autres propriétaires que cet utilisateur, dans l'organisation.
async fn autres_proprietaires(db: &mut PgConnection, utilisateur: i64, organisation: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM membres WHERE organisation_id = $1 AND role = 'proprietaire' AND utilisateur_id <> $2")
        .bind(organisation)
        .bind(utilisateur)
        .fetch_one(db)
        .await
}

async fn changer_role(
    mut db: Base,
    Utilisateur(moi): Utilisateur,
    Path(id): Path<i64>,
    Form(n): Form<NouveauRole>,
) -> Result<Response, Erreur> {
    let organisation = db.organisation;
    if role(&mut db, moi, organisation).await?.as_deref() != Some("proprietaire") {
        return Ok(vers("erreur", "droits"));
    }
    let nouveau = if n.role == "proprietaire" { "proprietaire" } else { "membre" };
    let mut tx = db.begin().await?;
    let actuel: Option<String> =
        sqlx::query_scalar("SELECT role::text FROM membres WHERE utilisateur_id = $1 AND organisation_id = $2 FOR UPDATE")
            .bind(id)
            .bind(organisation)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(actuel) = actuel else { return Err(Erreur::introuvable()) };
    if actuel == "proprietaire" && nouveau == "membre" && autres_proprietaires(&mut tx, id, organisation).await? == 0 {
        return Ok(vers("erreur", "dernier"));
    }
    sqlx::query("UPDATE membres SET role = $3::text::role_membre WHERE utilisateur_id = $1 AND organisation_id = $2")
        .bind(id)
        .bind(organisation)
        .bind(nouveau)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(vers("ok", "role"))
}

async fn retirer(mut db: Base, Utilisateur(moi): Utilisateur, Path(id): Path<i64>) -> Result<Response, Erreur> {
    let organisation = db.organisation;
    if role(&mut db, moi, organisation).await?.as_deref() != Some("proprietaire") {
        return Ok(vers("erreur", "droits"));
    }
    if id == moi {
        return Ok(vers("erreur", "toi"));
    }
    let mut tx = db.begin().await?;
    let retire = sqlx::query("DELETE FROM membres WHERE utilisateur_id = $1 AND organisation_id = $2").bind(id).bind(organisation).execute(&mut *tx).await?;
    if retire.rows_affected() == 0 {
        return Err(Erreur::introuvable());
    }
    // Un compte = une organisation (pour l'instant) : sans autre organisation, le compte disparaît (ses sessions aussi).
    sqlx::query("DELETE FROM utilisateurs u WHERE u.id = $1 AND NOT EXISTS (SELECT 1 FROM membres m WHERE m.utilisateur_id = u.id)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(vers("ok", "retire"))
}

// ─── Page d'invitation (sans connexion) ─────────────────────────────────────

#[derive(Template)]
#[template(path = "invitation.html")]
struct PageInvitation {
    jeton: String,
    organisation: String,
    email: String,
    nom: String,
    erreur: String,
}

fn message(statut: StatusCode, titre: &'static str, texte: &str) -> Result<Response, Erreur> {
    Ok((statut, Html(PageMessage::texte(titre, texte.to_string()).render()?)).into_response())
}

/// L'invitation encore valable de ce lien : (id, organisation, nom de l'organisation, e-mail, rôle).
async fn invitation_valable(db: &mut PgConnection, jeton: &str, verrou: bool) -> Result<Option<(i64, i64, String, String, String)>, sqlx::Error> {
    let sql = format!(
        "SELECT i.id, i.organisation_id, o.nom, i.email, i.role::text FROM invitations i JOIN organisations o ON o.id = i.organisation_id
         WHERE i.empreinte = $1 AND i.acceptee_le IS NULL AND i.expire_le > now(){}",
        if verrou { " FOR UPDATE OF i" } else { "" }
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(empreinte(jeton)).fetch_optional(db).await
}

const PERIMEE: &str = "Cette invitation n'est plus valable : elle a déjà servi, elle a été annulée ou elle a plus de 7 jours. \
                       Demande une nouvelle invitation à la personne qui t'a invité.";
const COMPTE_EXISTANT: &str = "Cette adresse a déjà un compte. Pour l'instant, un compte ne peut appartenir qu'à une seule organisation.";

async fn page_invitation(State(state): State<AppState>, Path(jeton): Path<String>) -> Result<Response, Erreur> {
    let mut db = state.db.acquire().await?;
    let Some((_, _, organisation, email, _)) = invitation_valable(&mut db, &jeton, false).await? else {
        return message(StatusCode::GONE, "Invitation", PERIMEE);
    };
    let existe: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM utilisateurs WHERE lower(email) = lower($1))").bind(&email).fetch_one(&mut *db).await?;
    if existe {
        return message(StatusCode::CONFLICT, "Invitation", COMPTE_EXISTANT);
    }
    Ok(Html(PageInvitation { jeton, organisation, email, nom: String::new(), erreur: String::new() }.render()?).into_response())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Acceptation {
    nom: String,
    mot_de_passe: String,
    confirmation: String,
}

async fn accepter(
    State(state): State<AppState>,
    cookies: CookieJar,
    Path(jeton): Path<String>,
    Form(a): Form<Acceptation>,
) -> Result<Response, Erreur> {
    let mut conn = state.db.acquire().await?;
    let mut tx = conn.begin().await?;
    let Some((invitation, organisation, nom_organisation, email, role)) = invitation_valable(&mut tx, &jeton, true).await? else {
        return message(StatusCode::GONE, "Invitation", PERIMEE);
    };
    let nom = a.nom.trim().to_string();
    let erreur = if nom.is_empty() || nom.chars().count() > LONGUEUR_MAX_NOM {
        Some("Indique ton nom (120 caractères au plus).".to_string())
    } else if !assez_long(&a.mot_de_passe) {
        Some(format!("Le mot de passe doit faire au moins {LONGUEUR_MIN} caractères."))
    } else if a.mot_de_passe != a.confirmation {
        Some("Les deux mots de passe ne sont pas identiques.".to_string())
    } else {
        None
    };
    if let Some(erreur) = erreur {
        let page = PageInvitation { jeton, organisation: nom_organisation, email, nom, erreur };
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page.render()?)).into_response());
    }
    let membres: i64 = sqlx::query_scalar("SELECT count(*) FROM membres WHERE organisation_id = $1").bind(organisation).fetch_one(&mut *tx).await?;
    if membres >= MEMBRES_MAX {
        return message(StatusCode::CONFLICT, "Invitation", "L'équipe est déjà complète (8 personnes au plus).");
    }
    // L'invitation a été reçue à cette adresse : elle est vérifiée.
    let utilisateur = match connexion::creer_compte(&mut tx, organisation, &email, &nom, &a.mot_de_passe, &role, true).await {
        Ok(id) => id,
        Err(_) => return message(StatusCode::CONFLICT, "Invitation", COMPTE_EXISTANT),
    };
    sqlx::query("UPDATE invitations SET acceptee_le = now() WHERE id = $1").bind(invitation).execute(&mut *tx).await?;
    tx.commit().await?;
    drop(conn);
    let cookies = connexion::ouvrir(&state.db, utilisateur, organisation, cookies).await?;
    Ok((cookies, Redirect::to("/")).into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/parametres/equipe", get(afficher))
        .route("/parametres/equipe/inviter", post(inviter))
        .route("/parametres/equipe/invitations/{id}/annuler", post(annuler))
        .route("/parametres/equipe/membres/{id}/role", post(changer_role))
        .route("/parametres/equipe/membres/{id}/retirer", post(retirer))
        .route("/invitation/{jeton}", get(page_invitation).post(accepter))
}
