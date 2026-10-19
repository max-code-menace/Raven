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
    // `cargo run -- reprise <fichier.json> [--enregistrer] [--organisation <n°>]` : reprise des factures de Vertuoza
    // (essai par défaut).
    if arguments.first().map(String::as_str) == Some("reprise") {
        let Some(chemin) = arguments.get(1).filter(|a| !a.starts_with("--")) else {
            eprintln!("Commande : reprise <fichier.json> [--enregistrer] [--organisation <n°>]");
            std::process::exit(1);
        };
        let enregistrer = arguments.iter().any(|a| a == "--enregistrer");
        let options: Vec<&str> = arguments[2..].iter().map(String::as_str).filter(|a| *a != "--enregistrer").collect();
        let organisation = match organisation_choisie(&pool, &options).await {
            Ok((organisation, reste)) if reste.is_empty() => organisation,
            Ok((_, reste)) => {
                eprintln!("Argument inconnu : {reste}");
                std::process::exit(1);
            }
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(1);
            }
        };
        let code = match std::fs::read_to_string(chemin) {
            Err(e) => {
                eprintln!("Lecture de {chemin} impossible : {e}");
                1
            }
            Ok(texte) => match ubifacture::reprise::importer(&pool, organisation, &texte, enregistrer).await {
                Ok(rapport) => {
                    println!("Organisation n° {organisation}.\n{rapport}");
                    0
                }
                Err(message) => {
                    eprintln!("Reprise refusée, rien n'est enregistré : {message}");
                    1
                }
            },
        };
        std::process::exit(code);
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

/// L'organisation choisie (`--organisation <n°>`, obligatoire s'il y en a plusieurs) et les autres mots (le nom d'un
/// nouvel utilisateur).
async fn organisation_choisie(pool: &sqlx::PgPool, reste: &[&str]) -> Result<(i64, String), String> {
    let mut organisation = None;
    let mut nom = Vec::new();
    let mut i = 0;
    while i < reste.len() {
        if reste[i] == "--organisation" {
            let valeur = reste.get(i + 1).and_then(|v| v.parse::<i64>().ok());
            organisation = Some(valeur.ok_or("Après --organisation, indique le numéro de l'organisation.")?);
            i += 2;
        } else {
            nom.push(reste[i]);
            i += 1;
        }
    }
    let organisation = match organisation {
        Some(o) => o,
        None => match connexion::liste_organisations(pool).await.map_err(|e| e.to_string())?.as_slice() {
            [(seule, _)] => *seule,
            liste => {
                let noms: Vec<String> = liste.iter().map(|(id, nom)| format!("  {id}  {nom}")).collect();
                return Err(format!("Plusieurs organisations : ajoute --organisation <n°>.\n{}", noms.join("\n")));
            }
        },
    };
    Ok((organisation, nom.join(" ")))
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
        ["ajouter", email, reste @ ..] => match organisation_choisie(pool, reste).await {
            Err(message) => Err(message),
            Ok((organisation, nom)) => match demander_mot_de_passe() {
                Some(mdp) => connexion::ajouter_utilisateur(pool, organisation, email, &nom, &mdp)
                    .await
                    .map(|()| format!("Utilisateur {email} créé dans l'organisation n° {organisation}.")),
                None => Err("Rien n'a été créé.".to_string()),
            },
        },
        ["mot-de-passe", email] => match demander_mot_de_passe() {
            Some(mdp) => connexion::changer_mot_de_passe(pool, email, &mdp)
                .await
                .map(|()| format!("Mot de passe de {email} changé (ses sessions sont fermées).")),
            None => Err("Rien n'a été changé.".to_string()),
        },
        ["liste"] => connexion::liste_utilisateurs(pool)
            .await
            .map(|l| {
                if l.is_empty() {
                    "Aucun utilisateur.".to_string()
                } else {
                    l.iter().map(|(e, n, o)| format!("{e}  {n}  (organisation : {o})")).collect::<Vec<_>>().join("\n")
                }
            })
            .map_err(|e| e.to_string()),
        _ => Err("Commandes : utilisateur ajouter <e-mail> [nom] [--organisation <n°>] · utilisateur mot-de-passe <e-mail> · utilisateur liste"
            .to_string()),
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
