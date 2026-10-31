mod api;
mod audit;
mod auth;
mod classifier;
mod config;
mod error;
mod grouping;
mod ingest;
mod middleware;
mod models;
mod objects;
mod state;
mod store;
#[cfg(test)]
mod tests;
mod ui;
mod worker;

use crate::config::Config;
use crate::state::AppState;
use crate::worker::Job;
use actix_web::{web, App, HttpServer};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock, Semaphore};
use tracing_subscriber::EnvFilter;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env();
    let port = config.port;
    let self_url = config.self_url.clone();

    let objects = match crate::objects::Objects::from_config(&config) {
        Ok(objects) => Arc::new(objects),
        Err(error) => {
            tracing::error!("{error}");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, error));
        }
    };

    let (mut seeded_store, seeded_raw) = store::seed(&self_url);
    for (key, data) in seeded_raw {
        if let Err(error) = objects.put_create(&key, data).await {
            tracing::warn!("could not seed raw payload {key}: {error}");
        }
    }
    // FR-030: the operator exists only when seeded from the environment, is
    // never re-seeded, and a missing bootstrap value is a startup error.
    match crate::auth::bootstrap_operator(&mut seeded_store, &config) {
        Ok(crate::auth::BootstrapOutcome::Created(email)) => {
            tracing::info!(operator = %email, "deployment operator created")
        }
        Ok(crate::auth::BootstrapOutcome::Existing) => {}
        Err(error) => {
            tracing::error!("{error}");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, error));
        }
    }
    let store = Arc::new(RwLock::new(seeded_store));
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
        ingest_gate: Arc::new(Semaphore::new(config.ingest_max_in_flight)),
        session_ttl_days: config.session_ttl_days,
        objects,
        storage_ready: Arc::new(AtomicBool::new(true)),
    });

    tracing::info!("Scout listening on http://127.0.0.1:{port}");
    tracing::info!("demo accounts: demo@scout.local · triager@scout.local · viewer@scout.local");
    tracing::info!("demo password: {}", store::DEMO_PASSWORD);

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .app_data(web::PayloadConfig::new(10 * 1024 * 1024))
            .app_data(web::JsonConfig::default().limit(10 * 1024 * 1024))
            .wrap(actix_web::middleware::Logger::default())
            .wrap(actix_web::middleware::from_fn(
                crate::middleware::request_id,
            ))
            .configure(api::config)
            .configure(ingest::config)
            .route(
                "/mock-jev/v1/systemone",
                web::post().to(classifier::mock_systemone),
            )
            .route("/mock-jev/outage", web::get().to(classifier::get_outage))
            .route("/mock-jev/outage", web::post().to(classifier::set_outage))
            // Keep the static UI last: Files("/") is a catch-all and would
            // otherwise shadow /api and /mock-jev.
            .configure(ui::config)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
