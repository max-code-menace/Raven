use std::net::SocketAddr;
use std::time::Duration;

use ubifacture::{AppState, application, connexion, db, emails, securite};

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
    // `cargo run -- admin …` : les administrateurs du back office de Raven (/admin).
    if arguments.first().map(String::as_str) == Some("admin") {
        std::process::exit(administrateurs(&pool, &arguments[1..]).await);
    }
    // `cargo run -- organisation creer "<nom>"` · `organisation liste`.
    if arguments.first().map(String::as_str) == Some("organisation") {
        let resultat = match arguments[1..].iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["creer", nom @ ..] if !nom.is_empty() => ubifacture::organisations::creer(&pool, &nom.join(" "))
                .await
                .map(|id| format!("Organisation n° {id} créée. Ajoute son premier compte : cargo run -- utilisateur ajouter <e-mail> --organisation {id}")),
            ["liste"] => ubifacture::organisations::liste(&pool)
                .await
                .map(|l| l.iter().map(|(id, nom)| format!("{id}  {nom}")).collect::<Vec<_>>().join("\n"))
                .map_err(|e| e.to_string()),
            _ => Err("Commandes : organisation creer \"<nom>\" · organisation liste".to_string()),
        };
        match resultat {
            Ok(message) => println!("{message}"),
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
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
    emails::configurer(
        std::env::var("BREVO_CLE_API").ok(),
        std::env::var("ADRESSE_PUBLIQUE").ok(),
        std::env::var("EMAIL_EXPEDITEUR").ok(),
    );
    if emails::envoi_actif() {
        println!("E-mails : envoi par Brevo activé.");
        if std::env::var("EMAIL_EXPEDITEUR").map(|e| !ubifacture::commun::email_valide(e.trim())).unwrap_or(true) {
            println!("ATTENTION : EMAIL_EXPEDITEUR manque : les e-mails d'inscription, de mot de passe et d'invitation ne partiront pas.");
        }
    } else {
        println!("E-mails : mode test (pas de BREVO_CLE_API), les e-mails sont seulement enregistrés.");
    }

    // Peppol : sans RECOMMAND_CLE_API et RECOMMAND_SECRET, rien ne part par Peppol.
    ubifacture::peppol::configurer(
        std::env::var("RECOMMAND_CLE_API").ok(),
        std::env::var("RECOMMAND_SECRET").ok(),
        std::env::var("RECOMMAND_ADRESSE").ok(),
    );
    if ubifacture::peppol::envoi_actif() {
        println!("Peppol : envoi par Recommand activé (la clé dit s'il s'agit du bac à sable ou du vrai réseau).");
    } else {
        println!("Peppol : non configuré (pas de RECOMMAND_CLE_API), rien ne part par Peppol.");
    }
    // Réception : sans RECOMMAND_WEBHOOK_SECRET, l'adresse /peppol/webhook refuse tout ; les factures fournisseurs
    // arrivent alors par la relève (bouton « Relever » et ouverture de la boîte de réception).
    ubifacture::peppol::configurer_webhook(std::env::var("RECOMMAND_WEBHOOK_SECRET").ok());

    // Mise en ligne : PRODUCTION=1 sur le serveur de l'hébergeur. Raven refuse alors de démarrer tant qu'il manque un
    // réglage indispensable (https, e-mails, serveurs frontaux, utilisateur de la base soumis aux règles par organisation).
    let production = matches!(std::env::var("PRODUCTION").as_deref().map(str::trim), Ok("1" | "oui" | "true"));
    let proxies = match std::env::var("PROXIES_DE_CONFIANCE") {
        Err(_) => 0,
        Ok(v) => match v.trim().parse::<usize>() {
            Ok(n) if n <= 5 => n,
            _ => {
                eprintln!("PROXIES_DE_CONFIANCE doit être un nombre de 0 à 5.");
                std::process::exit(1);
            }
        },
    };
    if production {
        match securite::problemes_de_production(&pool, |nom| std::env::var(nom).ok()).await {
            Ok(problemes) if problemes.is_empty() => println!("Production : tous les réglages indispensables sont là."),
            Ok(problemes) => {
                eprintln!("Raven ne démarre pas en production, il manque :");
                for probleme in problemes {
                    eprintln!("  - {probleme}");
                }
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Vérification des réglages de production impossible : {e}");
                std::process::exit(1);
            }
        }
    }
    securite::configurer(production, proxies);

    // L'adresse d'écoute : ADRESSE_ECOUTE (« 0.0.0.0:8080 »), sinon PORT (donné par l'hébergeur : toutes les adresses
    // de la machine), sinon 127.0.0.1:3001 (ce PC seulement).
    let ecoute = match (std::env::var("ADRESSE_ECOUTE"), std::env::var("PORT")) {
        (Ok(adresse), _) if !adresse.trim().is_empty() => adresse.trim().to_string(),
        (_, Ok(port)) if !port.trim().is_empty() => format!("0.0.0.0:{}", port.trim()),
        _ => "127.0.0.1:3001".to_string(),
    };
    let listener = match tokio::net::TcpListener::bind(&ecoute).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("Impossible d'écouter sur {ecoute} : {e}");
            std::process::exit(1);
        }
    };
    println!("Raven tourne sur http://{ecoute}");
    // L'adresse de la connexion accompagne chaque demande : c'est l'adresse IP du client quand aucun serveur frontal
    // n'est devant Raven (`securite::Ip`).
    let site = application(AppState { db: pool }).into_make_service_with_connect_info::<SocketAddr>();
    if let Err(e) = axum::serve(listener, site).with_graceful_shutdown(arret_demande()).await {
        eprintln!("Le serveur s'est arrêté : {e}");
    }
    ubifacture::finir_les_enregistrements(Duration::from_secs(30)).await;
    println!("Raven est arrêté.");
}

/// L'arrêt demandé : Ctrl+C, ou le signal d'arrêt de l'hébergeur (SIGTERM, à chaque mise à jour du site). Le serveur
/// n'accepte plus de nouvelles demandes et finit celles en cours.
async fn arret_demande() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminer = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminer = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminer => {}
    }
    println!("Arrêt demandé : Raven finit les demandes en cours.");
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
        None => match ubifacture::organisations::liste(pool).await.map_err(|e| e.to_string())?.as_slice() {
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

/// `admin ajouter <e-mail> [nom]`, `admin mot-de-passe <e-mail>`, `admin retirer <e-mail>`, `admin liste`.
async fn administrateurs(pool: &sqlx::PgPool, arguments: &[String]) -> i32 {
    use ubifacture::administration;
    let resultat = match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["ajouter", email, nom @ ..] => match demander_mot_de_passe() {
            Some(mdp) => administration::ajouter_administrateur(pool, email, &nom.join(" "), &mdp)
                .await
                .map(|()| format!("Administrateur {email} créé : back office sur http://127.0.0.1:3001/admin")),
            None => Err("Rien n'a été créé.".to_string()),
        },
        ["mot-de-passe", email] => match demander_mot_de_passe() {
            Some(mdp) => administration::changer_mot_de_passe(pool, email, &mdp)
                .await
                .map(|()| format!("Mot de passe de l'administrateur {email} changé (ses sessions sont fermées).")),
            None => Err("Rien n'a été changé.".to_string()),
        },
        ["retirer", email] => administration::retirer_administrateur(pool, email).await.map(|()| format!("Administrateur {email} retiré.")),
        ["liste"] => administration::liste_administrateurs(pool)
            .await
            .map(|l| {
                if l.is_empty() {
                    "Aucun administrateur.".to_string()
                } else {
                    l.iter().map(|(e, n)| format!("{e}  {n}")).collect::<Vec<_>>().join("\n")
                }
            })
            .map_err(|e| e.to_string()),
        _ => Err("Commandes : admin ajouter <e-mail> [nom] · admin mot-de-passe <e-mail> · admin retirer <e-mail> · admin liste".to_string()),
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
