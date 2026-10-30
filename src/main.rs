mod api;
mod classifier;
mod error;
mod grouping;
mod ingest;
mod models;
mod state;
mod store;
mod ui;
mod worker;

use crate::state::AppState;
use crate::worker::Job;
use actix_web::{web, App, HttpServer};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tracing_subscriber::EnvFilter;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8080);
    let self_url =
        std::env::var("SCOUT_SELF_URL").unwrap_or_else(|_| format!("http://127.0.0.1:{port}"));

    let store = Arc::new(RwLock::new(store::seed(&self_url)));
    let (jobs, rx) = mpsc::unbounded_channel::<Job>();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("build http client");
    worker::spawn(store.clone(), rx, http.clone());

    let state = web::Data::new(AppState {
        store,
        jobs,
        self_url: self_url.clone(),
    });

    tracing::info!("Scout prototype listening on http://127.0.0.1:{port}");
    tracing::info!("demo accounts: demo@scout.local · triager@scout.local · viewer@scout.local");

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .app_data(web::PayloadConfig::new(10 * 1024 * 1024))
            .app_data(web::JsonConfig::default().limit(10 * 1024 * 1024))
            .wrap(actix_web::middleware::Logger::default())
            .configure(api::config)
            .configure(ingest::config)
            .configure(ui::config)
            .route(
                "/mock-jev/v1/systemone",
                web::post().to(classifier::mock_systemone),
            )
            .route("/mock-jev/outage", web::get().to(classifier::get_outage))
            .route("/mock-jev/outage", web::post().to(classifier::set_outage))
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
