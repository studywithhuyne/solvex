// Engine, api, and db modules are declared in lib.rs (matching_engine crate).

use matching_engine::api::{router, state::AppState};
use matching_engine::db::{pool, worker};
use matching_engine::observability::metrics as observability_metrics;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    tracing::info!("Matching Engine starting...");

    let policy = matching_engine::api::policy::RuntimePolicy::from_env()?;
    let metrics_handle = observability_metrics::install_prometheus_recorder()?;
    tracing::info!("Prometheus metrics recorder ready");

    // ── DB layer ──────────────────────────────────────────────────────────────
    let db_pool = pool::create_pool().await?;
    tracing::info!("Database pool ready (max_connections=5, migrations applied)");

    let (events_tx, _worker_handle) =
        worker::spawn_persistence_worker(db_pool.clone(), worker::WORKER_BUFFER);
    tracing::info!(
        "Persistence worker spawned (buffer={})",
        worker::WORKER_BUFFER
    );

    // ── Axum server ───────────────────────────────────────────────────────────
    let app_state = AppState::new(db_pool, events_tx, metrics_handle, policy).await?;
    if app_state.policy.mode == matching_engine::api::policy::RuntimeMode::Demo {
        let _simulator_handle =
            matching_engine::api::simulator::spawn_simulator_worker(app_state.clone());
        tracing::info!("Background simulator available; explicit start required");
    }
    let app = router::build_router(app_state);

    let bind_address =
        std::env::var("CEX_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&bind_address).await?;
    tracing::info!(%bind_address, "HTTP listener ready");

    axum::serve(listener, app).await?;
    Ok(())
}
