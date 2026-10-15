use ubifacture::{AppState, app, db};

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

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3001")
        .await
        .expect("impossible d'ouvrir le port 3001");
    println!("Ubifacture tourne sur http://127.0.0.1:3001");
    axum::serve(listener, app(AppState { db: pool }))
        .await
        .expect("le serveur s'est arrêté");
}
