use crate::components::common::{empty_state, section_title};
use crate::state::{self, use_store};
use crate::util;
use sycamore::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::HtmlInputElement;

/// API tokens for the Sentry-compatible surface. The plaintext is shown once,
/// immediately after creation, and never rendered again.
#[component]
pub fn TokensView() -> View {
    let store = use_store();
    let tokens = create_selector(move || store.with(|state| state.tokens.clone()));
    let new_token = create_selector(move || store.with(|state| state.new_token.clone()));
    let label = create_signal(String::new());

    let create = move |_| {
        let value = label.get_clone();
        if value.trim().is_empty() {
            state::toast(store, "Give the token a label first", "error");
            return;
        }
        spawn_local(async move {
            state::create_token(store, value).await;
            label.set(String::new());
        });
    };

    let reveal = View::from_dynamic(move || -> View {
        let Some(created) = new_token.get_clone() else {
            return View::new();
        };
        let plaintext = created.token.clone();
        let copied = plaintext.clone();
        view! {
            div(class="panel") {
                div(class="section-title") {
                    h3 { "Copy this token now" }
                    button(
                        class="btn btn-ghost btn-sm",
                        r#type="button",
                        on:click=move |_| state::dismiss_new_token(store),
                    ) {
                        "Done"
                    }
                }
                p(class="hint") {
                    "This is the only time the token is shown. Any Sentry client can use it as a bearer token."
                }
                div(class="inline-form") {
                    input(class="input", readonly=true, value=plaintext.clone())
                    button(
                        class="btn",
                        r#type="button",
                        on:click=move |_| {
                            let value = copied.clone();
                            spawn_local(async move {
                                let clipboard = util::window().navigator().clipboard();
                                match JsFuture::from(clipboard.write_text(&value)).await {
                                    Ok(_) => state::toast(store, "Token copied", "success"),
                                    Err(_) => state::toast(
                                        store,
                                        "Copy failed — select the field manually",
                                        "error",
                                    ),
                                }
                            });
                        },
                    ) {
                        "Copy"
                    }
                }
            }
        }
    });

    let rows = View::from_dynamic(move || -> View {
        let tokens = tokens.get_clone();
        if tokens.is_empty() {
            return empty_state(
                "No API tokens",
                "Create a token to let a Sentry client read this deployment's projects.",
                View::new(),
            );
        }
        let rows: Vec<View> = tokens
            .into_iter()
            .map(|token| {
                let revoked = token.revoked_at.is_some();
                let id = token.id.clone();
                let status = if revoked { "revoked" } else { "active" };
                let status_class = if revoked { "badge badge-muted" } else { "badge badge-open" };
                let last_used = token
                    .last_used_at
                    .as_deref()
                    .map(util::relative)
                    .unwrap_or_else(|| "never".into());
                let revoke = if revoked {
                    View::new()
                } else {
                    view! {
                        button(
                            class="btn btn-danger btn-sm",
                            r#type="button",
                            on:click=move |_| {
                                if util::window()
                                    .confirm_with_message("Revoke this token? Clients using it will be denied immediately.")
                                    .unwrap_or(false)
                                {
                                    spawn_local(state::revoke_token(store, id.clone()));
                                }
                            },
                        ) {
                            "Revoke"
                        }
                    }
                };
                view! {
                    div(class="member-row") {
                        div(class="member-id") {
                            div(class="who") {
                                strong { (token.label.clone()) }
                                small { (format!("{} · created {}", token.token_prefix, util::relative(&token.created_at))) }
                            }
                        }
                        div(class="row-actions") {
                            span(class=status_class) { (status) }
                            span(class="muted small") { (format!("last used {last_used}")) }
                            (revoke)
                        }
                    }
                }
            })
            .collect();
        view! { div { (rows) } }
    });

    let form = view! {
        div(class="inline-form") {
            label(class="field") {
                "Label"
                input(
                    class="input",
                    placeholder="release-dashboard",
                    value=move || label.get_clone(),
                    on:input=move |event: web_sys::Event| {
                        if let Some(target) = event.target() {
                            let input = target.unchecked_into::<HtmlInputElement>();
                            label.set(input.value());
                        }
                    },
                )
            }
            button(class="btn btn-primary", r#type="button", on:click=create) {
                "Create token"
            }
        }
    };

    view! {
        div(class="page") {
            (section_title("API tokens", View::new()))
            p(class="hint") {
                "Tokens authenticate Sentry clients against /api/0/. They inherit your current project roles, never expire on their own, and stop working the moment you revoke them."
            }
            (reveal)
            (form)
            (rows)
        }
    }
}
