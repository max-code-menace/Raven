use ubifacture::{AppState, app, db, emails};

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
    axum::serve(listener, app(AppState { db: pool }))
        .await
        .expect("le serveur s'est arrêté");
}
