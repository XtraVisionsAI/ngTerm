use clap::Parser;
use ngterm::{audit, auth, config, db, web};

#[derive(Parser)]
#[command(name = "ngterm", about = "Web-based multi-server SSH terminal manager")]
struct Args {
    #[arg(short, long, default_value = "8080")]
    port: u16,

    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    #[arg(long, default_value = ".")]
    data_dir: String,

    #[arg(long, default_value = "120")]
    default_cols: u16,

    #[arg(long, default_value = "36")]
    default_rows: u16,

    /// Specify master key for first-run initialization (ignored if already initialized)
    #[arg(long)]
    master_key: Option<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ngterm=debug,tower_http=debug".parse().unwrap()),
        )
        .init();

    let args = Args::parse();

    let data_dir = ngterm::utils::shellexpand(&args.data_dir);
    std::fs::create_dir_all(&data_dir).expect("Failed to create data directory");

    let database = db::Database::open(&data_dir).expect("Failed to open database");

    audit::close_stale_sessions(&database);

    if let Some(master_key) = auth::init_admin(&data_dir, args.master_key.as_deref()) {
        tracing::info!("========================================");
        tracing::info!("  First run! Admin master key:");
        tracing::info!("  {}", master_key);
        tracing::info!("  Save this key! It will NOT be shown again.");
        tracing::info!("========================================");
    }

    let pepper = auth::read_pepper(&data_dir);
    let jwt_secret = auth::read_or_generate_jwt_secret(&data_dir);

    let app_config = config::AppConfig {
        default_cols: args.default_cols,
        default_rows: args.default_rows,
        data_dir: data_dir.clone(),
        pepper,
        jwt_secret,
    };

    let (state, mut session_ended_rx) = ngterm::build_app_state(app_config, database).await;

    let audit_state = state.clone();
    tokio::spawn(async move {
        while let Some(ended) = session_ended_rx.recv().await {
            audit_state.helpers.remove(&ended.session_id).await;
            if let Err(e) = audit::log_disconnect(&audit_state.db, &ended.session_id, "ssh_closed")
            {
                tracing::error!("Failed to write disconnect audit log: {}", e);
            }
        }
    });

    let app = web::build_router(state.clone());

    let addr = format!("{}:{}", args.host, args.port);
    tracing::info!("NGTerm listening on http://{}", addr);
    tracing::info!("Data directory: {}", data_dir);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .unwrap();
}
