use std::net::SocketAddr;

use mangako_api::{api, config::Config, db, operations};
use tokio::net::TcpListener;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), mangako_api::error::ApiError> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mangako_api=info,tower_http=info,axum::rejection=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env()?;
    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    mangako_api::cover_storage::enqueue_existing_mangadex_assets(&pool).await?;
    config.cover_storage.ensure_root().await?;
    let worker_pool = pool.clone();
    let worker_storage = config.cover_storage.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("valid cover worker client");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            while let Ok(true) =
                mangako_api::cover_storage::process_next(&worker_pool, &worker_storage, &client)
                    .await
            {}
        }
    });
    let app = operations::with_observability(api::router_with_cover_storage(
        pool,
        config.auth,
        config.operations,
        config.cover_storage,
    ));

    let listener = TcpListener::bind(config.http_addr).await?;
    tracing::info!(addr = %config.http_addr, "listening");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
