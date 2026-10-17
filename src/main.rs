use ubifacture::{AppState, application, connexion, db, emails};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let pool = match db::connecter().await {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("Connexion à la base impossible : {e}");
            std::process::exit(1);
        }
    };
    println!("Base de données connectée, migrations appliquées.");

    // `cargo run -- utilisateur …` : gérer les comptes, sans lancer le site.
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("utilisateur") {
        std::process::exit(utilisateurs(&pool, &arguments[1..]).await);
    }

    let comptes = connexion::liste_utilisateurs(&pool).await.unwrap_or_default();
    if comptes.is_empty() {
        println!("Aucun utilisateur : crée le premier avec  cargo run -- utilisateur ajouter ton@email.be");
    }

    // E-mails : sans BREVO_CLE_API, rien ne part (les e-mails sont seulement enregistrés dans l'application).
    emails::configurer(std::env::var("BREVO_CLE_API").ok(), std::env::var("ADRESSE_PUBLIQUE").ok());
    if emails::envoi_actif() {
        println!("E-mails : envoi par Brevo activé.");
    } else {
        println!("E-mails : mode test (pas de BREVO_CLE_API), les e-mails sont seulement enregistrés.");
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3001")
        .await
        .expect("impossible d'ouvrir le port 3001");
    println!("Ubifacture tourne sur http://127.0.0.1:3001");
    axum::serve(listener, application(AppState { db: pool }))
        .await
        .expect("le serveur s'est arrêté");
}

/// Demande un mot de passe deux fois, sans l'afficher à l'écran.
fn demander_mot_de_passe() -> Option<String> {
    let premier = rpassword::prompt_password("Mot de passe (10 caractères au moins) : ").ok()?;
    let second = rpassword::prompt_password("Encore une fois : ").ok()?;
    if premier != second {
        eprintln!("Les deux mots de passe ne sont pas identiques.");
        return None;
    }
    Some(premier)
}

/// `utilisateur ajouter <e-mail> [nom]`, `utilisateur mot-de-passe <e-mail>`, `utilisateur liste`.
async fn utilisateurs(pool: &sqlx::PgPool, arguments: &[String]) -> i32 {
    let resultat = match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["ajouter", email, nom @ ..] => match demander_mot_de_passe() {
            Some(mdp) => connexion::ajouter_utilisateur(pool, email, &nom.join(" "), &mdp).await.map(|()| format!("Utilisateur {email} créé.")),
            None => Err("Rien n'a été créé.".to_string()),
        },
        ["mot-de-passe", email] => match demander_mot_de_passe() {
            Some(mdp) => connexion::changer_mot_de_passe(pool, email, &mdp)
                .await
                .map(|()| format!("Mot de passe de {email} changé (ses sessions sont fermées).")),
            None => Err("Rien n'a été changé.".to_string()),
        },
        ["liste"] => connexion::liste_utilisateurs(pool)
            .await
            .map(|l| if l.is_empty() { "Aucun utilisateur.".to_string() } else { l.iter().map(|(e, n)| format!("{e}  {n}")).collect::<Vec<_>>().join("\n") })
            .map_err(|e| e.to_string()),
        _ => Err("Commandes : utilisateur ajouter <e-mail> [nom] · utilisateur mot-de-passe <e-mail> · utilisateur liste".to_string()),
    };
    match resultat {
        Ok(message) => {
            println!("{message}");
            0
        }
        Err(message) => {
            eprintln!("{message}");
            1
        }
    }
}
