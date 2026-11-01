mod api;
mod audit;
mod auth;
mod classifier;
mod compat;
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
    let demo_mode = config.demo_mode;

    if let Err(error) = config.validate_storage() {
        tracing::error!("{error}");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, error));
    }

    let objects = match crate::objects::Objects::from_config(&config) {
        Ok(objects) => Arc::new(objects),
        Err(error) => {
            tracing::error!("{error}");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, error));
        }
    };

    // Demo fixtures carry known credentials, so they are opt-in only.
    let (mut seeded_store, seeded_raw) = if demo_mode {
        store::seed(&self_url)
    } else {
        tracing::info!("SCOUT_DEMO_MODE is off: starting with an empty store");
        (store::Store::empty(), Vec::new())
    };
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
    let backfilled = seeded_store.ensure_org_slugs();
    if backfilled > 0 {
        tracing::info!(count = backfilled, "backfilled organisation slugs");
    }
    let store = Arc::new(RwLock::new(seeded_store));
    let (jobs, rx) = mpsc::unbounded_channel::<Job>();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("build http client");
    worker::spawn(store.clone(), rx, http.clone());
    worker::spawn_retention(store.clone(), objects.clone(), Duration::from_secs(3600));

    let state = web::Data::new(AppState {
        store,
        jobs,
        self_url: self_url.clone(),
        ingest_gate: Arc::new(Semaphore::new(config.ingest_max_in_flight)),
        session_ttl_days: config.session_ttl_days,
        retention_days: config.retention_days.clamp(1, 3650),
        confidence_threshold: config.confidence_threshold,
        objects,
        storage_ready: Arc::new(AtomicBool::new(true)),
        compat_enabled: config.sentry_compat_enabled,
    });

    tracing::info!("Scout listening on http://127.0.0.1:{port}");
    if demo_mode {
        tracing::warn!(
            "SCOUT_DEMO_MODE is on: demo data, demo endpoints, and the shared demo password are active"
        );
    }

    HttpServer::new(move || {
        let app = App::new()
            .app_data(state.clone())
            .app_data(web::PayloadConfig::new(10 * 1024 * 1024))
            .app_data(web::JsonConfig::default().limit(10 * 1024 * 1024))
            .wrap(actix_web::middleware::Logger::default())
            .wrap(actix_web::middleware::from_fn(
                crate::middleware::request_id,
            ))
            .configure(api::config)
            .configure(compat::config)
            .configure(ingest::config)
            .route(
                "/mock-jev/v1/systemone",
                web::post().to(classifier::mock_systemone),
            )
            .route("/mock-jev/outage", web::get().to(classifier::get_outage))
            .route("/mock-jev/outage", web::post().to(classifier::set_outage));
        // Demo routes are opt-in: they list seeded accounts and reset the store
        // without authentication.
        let app = if demo_mode {
            app.configure(api::demo_config)
        } else {
            app
        };
        // Keep the static UI last: Files("/") is a catch-all and would
        // otherwise shadow /api and /mock-jev.
        app.configure(ui::config)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
