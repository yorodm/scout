use actix_web::{web, HttpResponse};

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLES: &str = include_str!("../web/styles.css");

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.route("/", web::get().to(index))
        .route("/app.js", web::get().to(app_js))
        .route("/styles.css", web::get().to(styles));
}

async fn index() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .insert_header(("Cache-Control", "no-store"))
        .body(INDEX)
}

async fn app_js() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("application/javascript; charset=utf-8")
        .insert_header(("Cache-Control", "no-store"))
        .body(APP_JS)
}

async fn styles() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/css; charset=utf-8")
        .insert_header(("Cache-Control", "no-store"))
        .body(STYLES)
}
