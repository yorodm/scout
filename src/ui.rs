use actix_files::Files;
use actix_web::{web, HttpResponse};
use std::path::PathBuf;

/// Serve the Trunk-built Sycamore bundle. During development, run `trunk serve`
/// in `ui/` and let it proxy `/api` to this binary; in a single-binary run, this
/// module serves the contents of `ui/dist` directly.
pub fn config(cfg: &mut web::ServiceConfig) {
    let dist = std::env::var("SCOUT_UI_DIR").unwrap_or_else(|_| "ui/dist".to_string());
    let index = PathBuf::from(&dist).join("index.html");
    if index.exists() {
        cfg.service(Files::new("/", dist).index_file("index.html"));
    } else {
        cfg.route("/", web::get().to(missing_ui));
    }
}

async fn missing_ui() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(
            r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Scout — UI not built</title>
<style>body{font-family:system-ui,sans-serif;background:#080c18;color:#e8edff;padding:48px;line-height:1.6}code{background:#151d3b;padding:2px 6px;border-radius:4px}pre{background:#101731;border:1px solid #263156;border-radius:10px;padding:18px;overflow:auto}</style>
</head><body>
<h1>Scout UI is not built</h1>
<p>The Actix API is running, but <code>ui/dist/index.html</code> is missing.</p>
<pre>cd ui
trunk build --release
# then restart the server</pre>
<p>For live development, run <code>trunk serve</code> in <code>ui/</code> and proxy <code>/api</code> to this server.</p>
</body></html>"#,
        )
}
