mod api;
mod audit;
mod auth;
mod classifier;
mod compat;
mod config;
mod error;
mod grouping;
mod ingest;
mod metrics;
mod middleware;
mod migrate;
mod models;
mod objects;
mod schema;
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

/// The mode selected by the command line. Kept free of server types so the
/// dispatch can be tested without wiring Actix, workers, or retention.
pub(crate) fn selected_mode<I, S>(args: I) -> Option<&'static str>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .nth(1)
        .filter(|arg| arg.as_ref() == crate::migrate::MODE)
        .map(|_| crate::migrate::MODE)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    if selected_mode(std::env::args()).is_some() {
        return match crate::migrate::run_from_env().await {
            Ok(0) => Ok(()),
            Ok(_) => std::process::exit(1),
            Err(error) => {
                eprintln!("migrate: {error}");
                std::process::exit(1);
            }
        };
    }

    let config = Config::from_env();
    let port = config.port;
    let self_url = config.self_url.clone();
    let demo_mode = config.demo_mode;

    if let Err(error) = config.validate_storage() {
        tracing::error!("{error}");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, error));
    }

    let objects = match crate::objects::Objects::from_config(&config) {
        Ok(objects) => objects,
        Err(error) => {
            tracing::error!("{error}");
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, error));
        }
    };

    // FR-005: prove the bucket is writable before any telemetry is accepted.
    // `verify_writable` names the endpoint, bucket, and likely cause on failure.
    if let Err(error) = objects.verify_writable().await {
        let message = format!("object storage startup check failed: {error}");
        tracing::error!("{message}");
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            message,
        ));
    }
    let objects = Arc::new(objects);

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
    let metrics = Arc::new(crate::metrics::Metrics::new());
    worker::spawn(store.clone(), rx, http.clone(), metrics.clone());
    worker::spawn_retention(
        store.clone(),
        objects.clone(),
        Duration::from_secs(config.retention_interval_secs),
        metrics.clone(),
    );

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
        metrics,
        store_kind: "memory",
    });

    // Log the counters periodically so a log-only deployment still sees them.
    {
        let metrics = state.metrics.clone();
        actix_web::rt::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                tracing::info!("{}", metrics.log_line("memory"));
            }
        });
    }

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

#[cfg(test)]
mod mode_tests {
    use super::selected_mode;
    use crate::migrate::MODE;

    #[test]
    fn only_the_migration_argument_selects_the_migration_mode() {
        assert_eq!(selected_mode(["scout", "migrate-objects"]), Some(MODE));
        assert_eq!(selected_mode(["scout"]), None);
        assert_eq!(selected_mode(["scout", "serve"]), None);
    }
}
