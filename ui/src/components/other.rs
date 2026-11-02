use crate::api::{self, IngestKey, LogLine};
use crate::components::common::{
    avatar, empty_state, level_badge, role_badge, section_title, skeleton,
};
use crate::components::issues::issue_row;
use crate::state::{self, use_store, AppState};
use crate::util;
use sycamore::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{HtmlInputElement, HtmlSelectElement};

// ------------------------------------------------------------------ logs

#[component]
pub fn LogsView() -> View {
    let store = use_store();
    let logs = create_selector(move || store.with(|state| state.logs.clone()));
    let can_send = create_selector(move || store.with(state::can_send));

    let list = View::from_dynamic(move || -> View {
        let logs = logs.get_clone();
        if logs.is_empty() {
            let filtered = store.with(|state| {
                !state.log_filters.q.is_empty() || !state.log_filters.level.is_empty()
            });
            if filtered {
                return empty_state(
                    "No log lines match",
                    "Try a different level or search term; ingest is unaffected.",
                    View::new(),
                );
            }
            let action = if can_send.get() {
                view! {
                    button(
                        class="btn btn-primary",
                        r#type="button",
                        on:click=move |_| state::open_modal(store, state::ModalKind::SendLog),
                    ) {
                        "Send a test log"
                    }
                }
            } else {
                View::new()
            };
            empty_state(
                "No log lines yet",
                "Ingest structured logs through /api/{project}/logs/ with the project key, or send a test log.",
                action,
            )
        } else {
            let table = log_table(logs);
            view! { div(class="panel") { (table) } }
        }
    });

    view! {
        div {
            div(class="toolbar") {
                input(
                    class="input",
                    placeholder="Search messages, sources, attributes…",
                    value=move || store.with(|state| state.log_filters.q.clone()),
                    on:input=move |event: web_sys::Event| {
                        if let Some(target) = event.target() {
                            let input = target.unchecked_into::<HtmlInputElement>();
                            let value = input.value();
                            store.update(|state| state.log_filters.q = value);
                            spawn_local(state::load_logs(store));
                        }
                    },
                )
                select(
                    class="select",
                    value=move || store.with(|state| state.log_filters.level.clone()),
                    on:change=move |event: web_sys::Event| {
                        if let Some(target) = event.target() {
                            let select = target.unchecked_into::<HtmlSelectElement>();
                            let value = select.value();
                            store.update(|state| state.log_filters.level = value);
                            spawn_local(state::load_logs(store));
                        }
                    },
                ) {
                    option(value="") { "All levels" }
                    option(value="error") { "Error" }
                    option(value="warning") { "Warning" }
                    option(value="info") { "Info" }
                    option(value="debug") { "Debug" }
                }
                span(class="spacer") {}
                span(class="muted small") {
                    (move || {
                        let count = store.with(|state| state.logs.len());
                        format!("{} line{}", count, if count == 1 { "" } else { "s" })
                    })
                }
            }
            (list)
        }
    }
}

fn log_table(logs: Vec<LogLine>) -> View {
    let rows: Vec<View> = logs
        .into_iter()
        .map(|log| {
            let time = util::short_time(&log.timestamp);
            let time_title = util::full_time(&log.timestamp);
            let level = log.level.clone();
            let message = log.message.clone();
            let source = log.source.clone();
            let attributes = if log.attributes.is_null() {
                String::new()
            } else {
                serde_json::to_string(&log.attributes).unwrap_or_default()
            };
            view! {
                tr {
                    td(class="time", title=time_title) {
                        (time.clone())
                    }
                    td { (level_badge(&level)) }
                    td(class="msg") { (message.clone()) }
                    td(class="src") { (source.clone()) }
                    td(class="attrs") { (attributes.clone()) }
                }
            }
        })
        .collect();
    view! {
        table(class="table") {
            thead {
                tr {
                    th(style="width:110px") { "Time" }
                    th(style="width:90px") { "Level" }
                    th { "Message" }
                    th(style="width:150px") { "Source" }
                    th(style="width:220px") { "Attributes" }
                }
            }
            tbody { (rows) }
        }
    }
}

// ------------------------------------------------------------------ search

#[component]
pub fn SearchView() -> View {
    let store = use_store();
    let results = create_selector(move || {
        store.with(|state| {
            (
                state.search_query.clone(),
                state.search_issues.clone(),
                state.search_logs.clone(),
                state.search_has_more,
            )
        })
    });

    let run_search = move || {
        let query = store.with(|state| state.search_query.clone());
        spawn_local(state::search(store, query));
    };

    let results_view = View::from_dynamic(move || -> View {
        let (query, issues, logs, has_more) = results.get_clone();
        if query.is_empty() {
            return empty_state(
                "Search your telemetry",
                "Results include grouped issues and log lines from this project only.",
                View::new(),
            );
        }
        let issue_rows: Vec<View> = issues.iter().cloned().map(issue_row).collect();
        let issue_count_text = format!(
            "{} match{}",
            issues.len(),
            if issues.len() == 1 { "" } else { "es" }
        );
        let log_count_text = format!(
            "{} match{}",
            logs.len(),
            if logs.len() == 1 { "" } else { "es" }
        );
        let issue_body = if issues.is_empty() {
            view! { div(class="muted small") { "No matching issues." } }
        } else {
            view! { div(class="issue-list") { (issue_rows) } }
        };
        let log_body = if logs.is_empty() {
            view! { div(class="empty small") { "No matching log lines." } }
        } else {
            log_table(logs.clone())
        };
        let issue_header = section_title(
            "Issues",
            view! { span(class="muted small") { (issue_count_text.clone()) } },
        );
        let log_header = section_title(
            "Logs",
            view! { span(class="muted small") { (log_count_text.clone()) } },
        );
        view! {
            div {
                (if has_more {
                    view! { div(class="hint") { "Showing the newest 200 matches; narrow the query to see more." } }
                } else {
                    View::new()
                })
                section(class="panel panel-pad", style="margin-bottom:16px") {
                    (issue_header)
                    (issue_body)
                }
                section(class="panel") {
                    div(style="padding:16px 16px 0") {
                        (log_header)
                    }
                    (log_body)
                }
            }
        }
    });

    view! {
        div {
            div(class="panel panel-pad", style="margin-bottom:16px") {
                div(class="toolbar", style="margin:0") {
                    input(
                        class="input",
                        style="max-width:520px",
                        placeholder="Search issues and logs…",
                        value=move || store.with(|state| state.search_query.clone()),
                        on:input=move |event: web_sys::Event| {
                            if let Some(target) = event.target() {
                                let input = target.unchecked_into::<HtmlInputElement>();
                                let value = input.value();
                                store.update(|state| state.search_query = value);
                            }
                        },
                        on:keydown=move |event: web_sys::KeyboardEvent| {
                            if event.key() == "Enter" {
                                let query = store.with(|state| state.search_query.clone());
                                spawn_local(state::search(store, query));
                            }
                        },
                    )
                    button(class="btn btn-primary", r#type="button", on:click=move |_| run_search()) {
                        "Search"
                    }
                }
                div(class="hint") {
                    "One query across grouped errors and log lines for this project."
                }
            }
            (results_view)
        }
    }
}

// ------------------------------------------------------------------ members

#[component]
pub fn MembersView() -> View {
    let store = use_store();
    let members = create_selector(move || store.with(|state| state.members.clone()));
    let can_manage = create_selector(move || store.with(state::can_manage));
    let new_email = create_signal(String::new());
    let new_role = create_signal("viewer".to_string());

    let rows = View::from_dynamic(move || -> View {
        let members = members.get_clone();
        let manage = can_manage.get();
        if members.is_empty() {
            return empty_state(
                "No members",
                "This project has no members yet.",
                View::new(),
            );
        }
        let rows: Vec<View> = members
            .into_iter()
            .map(|member| {
                let role = member.role.clone();
                let controls = if manage {
                    let uid_for_select = member.user_id.clone();
                    let uid_for_remove = member.user_id.clone();
                    view! {
                        select(
                            class="select",
                            style="width:auto",
                            value=role.clone(),
                            on:change=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let select = target.unchecked_into::<HtmlSelectElement>();
                                    spawn_local(state::update_member(
                                        store,
                                        uid_for_select.clone(),
                                        select.value(),
                                    ));
                                }
                            },
                        ) {
                            option(value="owner") { "owner" }
                            option(value="admin") { "admin" }
                            option(value="triager") { "triager" }
                            option(value="viewer") { "viewer" }
                        }
                        button(
                            class="btn btn-danger btn-sm",
                            r#type="button",
                            on:click=move |_| {
                                if util::window()
                                    .confirm_with_message("Remove this member from the project?")
                                    .unwrap_or(false)
                                {
                                    spawn_local(state::remove_member(store, uid_for_remove.clone()));
                                }
                            },
                        ) {
                            "Remove"
                        }
                    }
                } else {
                    role_badge(&role)
                };
                let name = member.name.clone();
                let email = member.email.clone();
                let avatar_view = avatar(&name);
                view! {
                    div(class="member-row") {
                        div(class="member-id") {
                            (avatar_view)
                            div(class="who") {
                                strong { (name.clone()) }
                                small { (email.clone()) }
                            }
                        }
                        (controls)
                    }
                }
            })
            .collect();
        view! { div { (rows) } }
    });

    let add_form = View::from_dynamic(move || -> View {
        if !can_manage.get() {
            return view! { div(class="hint") { "Only admins and owners can change access." } };
        }
        let accounts = store.with(|state| state.accounts.clone());
        let emails: Vec<View> = accounts
            .into_iter()
            .map(|account| view! { option(value=account.email) {} })
            .collect();
        view! {
            div {
                div(class="inline-form") {
                    label(class="field") {
                        "Email"
                        input(
                            class="input",
                            list="account-emails",
                            placeholder="person@example.com",
                            value=move || new_email.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    new_email.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field", style="max-width:150px") {
                        "Role"
                        select(
                            class="select",
                            value=move || new_role.get_clone(),
                            on:change=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let select = target.unchecked_into::<HtmlSelectElement>();
                                    new_role.set(select.value());
                                }
                            },
                        ) {
                            option(value="viewer") { "viewer" }
                            option(value="triager") { "triager" }
                            option(value="admin") { "admin" }
                            option(value="owner") { "owner" }
                        }
                    }
                    button(
                        class="btn btn-primary",
                        r#type="button",
                        on:click=move |_| {
                            spawn_local(state::add_member(
                                store,
                                new_email.get_clone(),
                                new_role.get_clone(),
                            ));
                        },
                    ) {
                        "Add member"
                    }
                }
                datalist(id="account-emails") { (emails) }
            }
        }
    });

    let project_label = View::from_dynamic(move || {
        let name = store.with(state::project_name);
        view! { span(class="muted small") { (format!("Roles are scoped to {name}")) } }
    });

    let header = section_title("Project access", project_label);
    view! {
        div(class="panel panel-pad") {
            (header)
            (rows)
            (add_form)
        }
    }
}

// ------------------------------------------------------------------ settings

#[component]
pub fn SettingsView() -> View {
    let store = use_store();
    let can_manage = create_selector(move || store.with(state::can_manage));
    let settings = create_selector(move || store.with(|state| state.settings.clone()));
    let outage = create_selector(move || store.with(|state| state.outage));

    let retention = create_signal(String::new());
    let endpoint = create_signal(String::new());
    let model_id = create_signal("jev-latest".to_string());
    let model_key = create_signal(String::new());
    let last_loaded = create_signal(None::<api::SettingsResponse>);

    create_effect(move || {
        let current = store.with(|state| state.settings.clone());
        if current != last_loaded.get_clone() {
            if let Some(settings) = &current {
                retention.set(settings.project.retention_days.to_string());
                if let Some(model) = &settings.project.model {
                    endpoint.set(model.endpoint.clone());
                    model_id.set(model.model.clone());
                    model_key.set(model.api_key.clone().unwrap_or_default());
                } else {
                    endpoint.set(String::new());
                    model_id.set("jev-latest".into());
                    model_key.set(String::new());
                }
            }
            last_loaded.set(current);
        }
    });

    let body = View::from_dynamic(move || -> View {
        if !can_manage.get() {
            return empty_state(
                "Admin access required",
                "Settings manage ingest keys, retention, and the decision model. Sign in as an owner or admin.",
                View::new(),
            );
        }
        match settings.get_clone() {
            Some(settings) => settings_view(
                store, settings, retention, endpoint, model_id, model_key, outage,
            ),
            None => view! {
                div(class="panel panel-pad") {
                    (skeleton("120px"))
                    (skeleton("160px"))
                }
            },
        }
    });

    view! { (body) }
}

#[allow(clippy::too_many_arguments)]
fn settings_view(
    store: Signal<AppState>,
    settings: api::SettingsResponse,
    retention: Signal<String>,
    endpoint: Signal<String>,
    model_id: Signal<String>,
    model_key: Signal<String>,
    outage: ReadSignal<bool>,
) -> View {
    let key_hint = settings
        .key_prefix
        .clone()
        .map(|prefix| format!("{prefix}… (active key)"))
        .unwrap_or_else(|| "No active key — create one below and copy it once.".to_string());
    let settings_error = create_signal(None::<String>);
    let validate = move |days: Option<u32>, endpoint: &str| -> Result<(), String> {
        match days {
            Some(days) if (1..=3650).contains(&days) => {}
            _ => return Err("Retention must be between 1 and 3650 days.".into()),
        }
        if !endpoint.is_empty()
            && !(endpoint.starts_with("http://") || endpoint.starts_with("https://"))
        {
            return Err("The model endpoint must be an http(s) URL.".into());
        }
        Ok(())
    };
    let settings_error_view = View::from_dynamic(move || match settings_error.get_clone() {
        Some(message) => view! {
            div(class="hint", style="color:var(--color-warn);margin-top:10px") { (message) }
        },
        None => View::new(),
    });
    let dsn_for_copy = format!("/api/{}/envelope/", settings.project.dsn_id);
    let mock_endpoint = settings.mock_model_endpoint.clone();
    let retention_value = retention;
    let endpoint_value = endpoint;
    let model_id_value = model_id;
    let model_key_value = model_key;

    let key_rows: Vec<View> = settings
        .keys
        .iter()
        .map(|key| key_row(store, key))
        .collect();
    let key_rows = if key_rows.is_empty() {
        view! { div(class="muted small") { "No keys." } }
    } else {
        view! { div { (key_rows) } }
    };

    let dsn_input = view! {
        input(
            class="input",
            readonly=true,
            value=key_hint.clone(),
        )
    };
    let copy_button = view! {
        button(
            class="btn",
            r#type="button",
            on:click=move |_| {
                let dsn = dsn_for_copy.clone();
                spawn_local(async move {
                    let clipboard = util::window().navigator().clipboard();
                    match JsFuture::from(clipboard.write_text(&dsn)).await {
                        Ok(_) => state::toast(store, "Ingest path copied", "success"),
                        Err(_) => state::toast(store, "Copy failed — select the field manually", "error"),
                    }
                });
            },
        ) {
            "Copy"
        }
    };

    let model_status = if settings.project.model.is_some() {
        "model connected"
    } else {
        "no model"
    };
    let model_header = section_title(
        "Retention & decision model",
        view! { span(class="muted small") { (model_status) } },
    );

    let outage_panel = View::from_dynamic(move || {
        let down = outage.get();
        let (badge_class, label) = if down {
            ("badge warning", "outage simulated")
        } else {
            ("badge resolved", "reachable")
        };
        view! {
            section(class="panel panel-pad") {
                (section_title(
                    "Classifier resilience",
                    view! { span(class=badge_class) { (label) } },
                ))
                p(class="muted small") {
                    "Toggle a simulated outage of the built-in demo endpoint. Ingest keeps accepting and storing; new items simply carry no suggestion."
                }
                div(class="outage-strip", style="margin-top:13px") {
                    span(
                        style=format!(
                            "width:8px;height:8px;border-radius:50%;background:{}",
                            if down { "var(--color-warn)" } else { "var(--color-ok)" }
                        ),
                    ) {}
                    span {
                        (if down {
                            "Decision model is returning 503. Send an event to verify ingest is unaffected."
                        } else {
                            "Decision model responds to suggestions. Send an event to see one attached."
                        })
                    }
                }
                button(
                    class=if down { "btn btn-primary" } else { "btn" },
                    r#type="button",
                    on:click=move |_| spawn_local(state::toggle_outage(store)),
                ) {
                    (if down { "Restore classifier" } else { "Simulate classifier outage" })
                }
            }
        }
    });

    view! {
        div(class="settings-grid") {
            section(class="panel panel-pad") {
                (section_title(
                    "Ingest",
                    view! { span(class="badge") { "Sentry-compatible" } },
                ))
                label(class="field") {
                    "Ingest key"
                    div(class="dsn-box") {
                        (dsn_input)
                        (copy_button)
                    }
                }
                div(class="hint") {
                    (format!(
                        "Keys are hashed at rest and the plaintext is shown once when created or rotated. Use http://<key>@<host>/{}/ as the DSN. Ingest path: POST /api/{}/envelope/",
                        settings.project.dsn_id, settings.project.dsn_id
                    ))
                }
                div(class="divider") {}
                (section_title(
                    "Ingest keys",
                    view! {
                        div(style="display:flex;gap:8px") {
                            button(
                                class="btn btn-sm",
                                r#type="button",
                                on:click=move |_| {
                                    if util::window()
                                        .confirm_with_message(
                                            "Rotate the ingest key? The current key stops working immediately.",
                                        )
                                        .unwrap_or(false)
                                    {
                                        spawn_local(state::rotate_key(store));
                                    }
                                },
                            ) {
                                "Rotate"
                            }
                            button(
                                class="btn btn-sm",
                                r#type="button",
                                on:click=move |_| spawn_local(state::create_key(store)),
                            ) {
                                "Create key"
                            }
                        }
                    },
                ))
                (key_rows)
            }

            section(class="panel panel-pad") {
                (model_header)
                div(class="form-grid") {
                    label(class="field") {
                        "Retention (days)"
                        input(
                            class="input",
                            r#type="number",
                            min="1",
                            max="3650",
                            value=move || retention_value.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    retention_value.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field") {
                        "Model id"
                        input(
                            class="input",
                            value=move || model_id_value.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    model_id_value.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field full") {
                        "Decision endpoint"
                        input(
                            class="input",
                            placeholder="https://model.internal",
                            value=move || endpoint_value.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    endpoint_value.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field full") {
                        "Bearer key (optional)"
                        input(
                            class="input",
                            value=move || model_key_value.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    model_key_value.set(input.value());
                                }
                            },
                        )
                    }
                }
                (settings_error_view)
                div(class="form-actions") {
                    button(
                        class="btn btn-ghost",
                        r#type="button",
                        on:click=move |_| {
                            let retention_days = retention_value
                                .get_clone()
                                .trim()
                                .parse::<u32>()
                                .ok();
                            match validate(retention_days, "") {
                                Ok(()) => {
                                    settings_error.set(None);
                                    spawn_local(state::save_settings(
                                        store,
                                        retention_days.unwrap_or(30),
                                        None,
                                    ));
                                }
                                Err(message) => settings_error.set(Some(message)),
                            }
                        },
                    ) {
                        "Disable model"
                    }
                    button(
                        class="btn btn-ghost",
                        r#type="button",
                        on:click=move |_| {
                            let retention_days = retention_value
                                .get_clone()
                                .trim()
                                .parse::<u32>()
                                .unwrap_or(30);
                            let model = api::ModelConfig {
                                endpoint: mock_endpoint.clone(),
                                api_key: None,
                                model: "jev-latest".into(),
                            };
                            spawn_local(state::save_settings(store, retention_days, Some(model)));
                        },
                    ) {
                        "Use demo model"
                    }
                    button(
                        class="btn btn-primary",
                        r#type="button",
                        on:click=move |_| {
                            let retention_days = retention_value
                                .get_clone()
                                .trim()
                                .parse::<u32>()
                                .ok();
                            let endpoint = endpoint_value.get_clone().trim().to_string();
                            match validate(retention_days, &endpoint) {
                                Ok(()) => {
                                    settings_error.set(None);
                                    let model = if endpoint.is_empty() {
                                        None
                                    } else {
                                        let api_key = model_key_value.get_clone().trim().to_string();
                                        Some(api::ModelConfig {
                                            endpoint,
                                            api_key: if api_key.is_empty() {
                                                None
                                            } else {
                                                Some(api_key)
                                            },
                                            model: model_id_value.get_clone().trim().to_string(),
                                        })
                                    };
                                    spawn_local(state::save_settings(
                                        store,
                                        retention_days.unwrap_or(30),
                                        model,
                                    ));
                                }
                                Err(message) => settings_error.set(Some(message)),
                            }
                        },
                    ) {
                        "Save settings"
                    }
                }
                div(class="hint") {
                    "Scout sends only a 2 KB summary. A slow or unreachable model leaves the item with no suggestion. Ingest never waits for it."
                }
            }

            (outage_panel)

            (oidc_panel(store))

            (invitation_panel(store))

            section(class="panel panel-pad") {
                (section_title(
                    "Demo data",
                    view! { span(class="muted small") { "in-memory" } },
                ))
                p(class="muted small") {
                    "Reset the demo organisation to its seeded state. Real organisations, their projects, and the operator are untouched. Demo accounts are signed out."
                }
                div(class="form-actions", style="justify-content:flex-start;margin-top:18px") {
                    button(
                        class="btn btn-danger",
                        r#type="button",
                        on:click=move |_| {
                            if util::window()
                                .confirm_with_message("Reset the demo organisation's data?")
                                .unwrap_or(false)
                            {
                                spawn_local(state::reset_demo(store));
                            }
                        },
                    ) {
                        "Reset demo data"
                    }
                }
            }
        }
    }
}

/// Organisation-level single sign-on. Local sign-in stays available whether or
/// not a provider is configured (FR-018, US3/AC4).
fn oidc_panel(store: Signal<AppState>) -> View {
    let config = create_selector(move || store.with(|state| state.oidc.clone()));
    let loaded = create_selector(move || store.with(|state| state.oidc_loaded));
    let issuer = create_signal(String::new());
    let client_id = create_signal(String::new());
    let client_secret = create_signal(String::new());
    let scopes = create_signal("openid email profile".to_string());

    // Prefill from the stored connection. The secret is never returned, so the
    // field stays empty and is write-only.
    create_effect(move || {
        if let Some(config) = config.get_clone() {
            issuer.set(config.issuer);
            client_id.set(config.client_id);
            scopes.set(config.scopes);
        }
    });

    let body = View::from_dynamic(move || -> View {
        if !loaded.get() {
            return view! { (skeleton("90px")) };
        }
        let configured = config.get_clone().is_some();
        let badge_class = if configured {
            "badge resolved"
        } else {
            "badge"
        };
        let badge_label = if configured { "configured" } else { "optional" };
        view! {
            section(class="panel panel-pad") {
                div(class="section-title") {
                    h3 { "Single sign-on" }
                    span(class=badge_class) { (badge_label) }
                }
                p(class="muted small") {
                    "Optional per-organisation OpenID Connect. Only a verified email that matches an existing membership can sign in; local email-and-password sign-in always keeps working."
                }
                div(class="form-grid", style="margin-top:12px") {
                    label(class="field full") {
                        "Issuer"
                        input(
                            class="input",
                            placeholder="https://idp.example.com",
                            value=move || issuer.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    issuer.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field") {
                        "Client id"
                        input(
                            class="input",
                            value=move || client_id.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    client_id.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field") {
                        "Client secret"
                        input(
                            class="input",
                            r#type="password",
                            autocomplete="new-password",
                            placeholder="write-only; never shown again",
                            value=move || client_secret.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    client_secret.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field full") {
                        "Scopes"
                        input(
                            class="input",
                            placeholder="openid email profile",
                            value=move || scopes.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    scopes.set(input.value());
                                }
                            },
                        )
                    }
                }
                div(class="form-actions", style="justify-content:space-between") {
                    (if configured {
                        view! {
                            button(
                                class="btn btn-danger btn-sm",
                                r#type="button",
                                on:click=move |_| {
                                    if util::window()
                                        .confirm_with_message(
                                            "Remove the identity provider? Local sign-in keeps working.",
                                        )
                                        .unwrap_or(false)
                                    {
                                        spawn_local(state::remove_oidc(store));
                                    }
                                },
                            ) {
                                "Remove"
                            }
                        }
                    } else {
                        View::new()
                    })
                    button(
                        class="btn btn-primary",
                        r#type="button",
                        on:click=move |_| {
                            let issuer = issuer.get_clone().trim().to_string();
                            let client_id = client_id.get_clone().trim().to_string();
                            let secret = client_secret.get_clone();
                            let scopes = scopes.get_clone().trim().to_string();
                            if issuer.is_empty() || client_id.is_empty() || secret.is_empty() {
                                state::toast(
                                    store,
                                    "Issuer, client id, and client secret are required",
                                    "error",
                                );
                                return;
                            }
                            spawn_local(state::save_oidc(
                                store,
                                issuer,
                                client_id,
                                secret,
                                scopes,
                            ));
                        },
                    ) {
                        "Save provider"
                    }
                }
                div(class="hint") {
                    "The issuer is validated with a discovery request before it is saved. Redeeming a sign-in consumes the pending invitation exactly like a local acceptance."
                }
            }
        }
    });

    view! { (body) }
}

fn key_row(store: Signal<AppState>, key: &IngestKey) -> View {
    let key_text = format!("{}…", key.key_prefix);
    let label = key.label.clone();
    let created = util::relative(&key.created_at);
    let revoked = key.revoked;
    let key_id = key.id.clone();
    let revoke = if revoked {
        View::new()
    } else {
        view! {
            button(
                class="btn btn-danger btn-sm",
                r#type="button",
                on:click=move |_| {
                    if util::window()
                        .confirm_with_message("Revoke this ingest key? Senders using it will be rejected.")
                        .unwrap_or(false)
                    {
                        spawn_local(state::revoke_key(store, key_id.clone()));
                    }
                },
            ) {
                "Revoke"
            }
        }
    };
    let status = if revoked {
        view! { span(class="badge ignored") { "revoked" } }
    } else {
        view! { span(class="badge resolved") { "active" } }
    };
    view! {
        div(class="kv") {
            div {
                div(class="mono", style="font-size:12px") { (key_text) }
                div(class="muted small") { (format!("{label} · created {created}")) }
            }
            div(style="display:flex;gap:8px;align-items:center") {
                (status)
                (revoke)
            }
        }
    }
}

// ------------------------------------------------------------------ account

/// The signed-in account's own credentials. Every account can change its own
/// password and reissue recovery codes; the operator additionally sees locked
/// accounts and can unlock them (FR-012, FR-032, SC-017, SC-023).
#[component]
pub fn AccountView() -> View {
    let store = use_store();
    let email = create_selector(move || {
        store.with(|state| {
            state
                .me
                .as_ref()
                .map(|user| user.email.clone())
                .unwrap_or_default()
        })
    });
    let operator = create_selector(move || store.with(state::is_operator));
    let current = create_signal(String::new());
    let fresh = create_signal(String::new());
    let busy = create_signal(false);

    let submit = move || {
        if busy.get() {
            return;
        }
        let current_password = current.get_clone();
        let new_password = fresh.get_clone();
        if current_password.is_empty() || new_password.is_empty() {
            state::toast(
                store,
                "Enter your current password and the new one",
                "error",
            );
            return;
        }
        busy.set(true);
        spawn_local(async move {
            state::change_password(store, current_password, new_password).await;
            current.set(String::new());
            fresh.set(String::new());
            busy.set(false);
        });
    };

    let locked_users = create_selector(move || store.with(|state| state.locked_users.clone()));

    let operator_panel = View::from_dynamic(move || -> View {
        if !operator.get() {
            return View::new();
        }
        let users = locked_users.get_clone();
        let body = if users.is_empty() {
            empty_state(
                "No locked accounts",
                "Accounts lock after five consecutive failed sign-ins and appear here for unlocking.",
                View::new(),
            )
        } else {
            let rows: Vec<View> = users
                .into_iter()
                .map(|user| {
                    let id = user.id.clone();
                    let email = user.email.clone();
                    view! {
                        div(class="member-row") {
                            div(class="member-id") {
                                div(class="who") {
                                    strong { (email.clone()) }
                                    small { "locked by failed sign-ins" }
                                }
                            }
                            button(
                                class="btn btn-sm",
                                r#type="button",
                                on:click=move |_| {
                                    spawn_local(state::unlock_account(store, id.clone()));
                                },
                            ) {
                                "Unlock"
                            }
                        }
                    }
                })
                .collect();
            view! { div { (rows) } }
        };
        view! {
            section(class="panel panel-pad", style="margin-top:16px") {
                (section_title(
                    "Locked accounts",
                    view! { span(class="badge warning") { "operator only" } },
                ))
                p(class="muted small") {
                    "Five consecutive failed sign-ins lock an account regardless of elapsed time. Unlocking resets the counter; existing sessions are never ended by a lock."
                }
                div(style="margin-top:12px") { (body) }
            }
        }
    });

    view! {
        div(class="settings-grid") {
            section(class="panel panel-pad") {
                (section_title(
                    "Password",
                    view! { span(class="muted small") { "at least 12 characters" } },
                ))
                label(class="field") {
                    "Signed in as"
                    input(class="input", readonly=true, value=move || email.get_clone())
                }
                div(class="form-grid", style="margin-top:12px") {
                    label(class="field") {
                        "Current password"
                        input(
                            class="input",
                            r#type="password",
                            autocomplete="current-password",
                            value=move || current.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    current.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field") {
                        "New password"
                        input(
                            class="input",
                            r#type="password",
                            autocomplete="new-password",
                            value=move || fresh.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    fresh.set(input.value());
                                }
                            },
                        )
                    }
                }
                div(class="hint") {
                    "Changing the password ends every other session for this account. The session making the change stays signed in."
                }
                div(class="form-actions") {
                    button(
                        class="btn btn-primary",
                        r#type="button",
                        disabled=move || busy.get(),
                        on:click=move |_| submit(),
                    ) {
                        "Change password"
                    }
                }
            }

            section(class="panel panel-pad") {
                (section_title(
                    "Recovery codes",
                    view! { span(class="badge") { "shown once" } },
                ))
                p(class="muted small") {
                    "Recovery codes reset your password without database access. Reissuing revokes every unused code; only the newest set works, and it is shown once."
                }
                div(class="form-actions") {
                    button(
                        class="btn",
                        r#type="button",
                        on:click=move |_| spawn_local(state::reissue_recovery_codes(store)),
                    ) {
                        "Reissue recovery codes"
                    }
                }
            }
        }
        (operator_panel)
    }
}

/// Invite an email to the current project. The link is shown once, like an
/// ingest key, and can be revoked until it is accepted.
fn invitation_panel(store: Signal<AppState>) -> View {
    let email = create_signal(String::new());
    let role = create_signal("viewer".to_string());
    let last = create_selector(move || store.with(|state| state.last_invitation.clone()));

    let last_block = View::from_dynamic(move || -> View {
        let Some((invited, token, _)) = last.get_clone() else {
            return View::new();
        };
        let origin = util::window()
            .location()
            .origin()
            .unwrap_or_else(|_| "http://localhost:8080".into());
        let link = format!("{origin}/#/invite/{token}");
        let link_for_copy = link.clone();
        view! {
            div(class="panel panel-pad", style="margin-top:12px") {
                (section_title(
                    "Invitation link",
                    view! { span(class="badge warning") { "shown once" } },
                ))
                p(class="muted small") {
                    (format!("For {invited}. Deliver it out of band; it expires in seven days."))
                }
                div(class="dsn-box") {
                    input(class="input", readonly=true, value=link)
                    button(
                        class="btn",
                        r#type="button",
                        on:click=move |_| {
                            let text = link_for_copy.clone();
                            spawn_local(async move {
                                let clipboard = util::window().navigator().clipboard();
                                match JsFuture::from(clipboard.write_text(&text)).await {
                                    Ok(_) => state::toast(store, "Invitation link copied", "success"),
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
                div(class="form-actions", style="justify-content:flex-start") {
                    button(
                        class="btn btn-danger btn-sm",
                        r#type="button",
                        on:click=move |_| {
                            if util::window()
                                .confirm_with_message("Revoke this invitation?")
                                .unwrap_or(false)
                            {
                                spawn_local(state::revoke_invitation(store));
                            }
                        },
                    ) {
                        "Revoke"
                    }
                }
            }
        }
    });

    view! {
        section(class="panel panel-pad") {
            (section_title(
                "Invitations",
                view! { span(class="muted small") { "organisation admin" } },
            ))
            p(class="muted small") {
                "An invitation grants the role below on this project. It expires seven days after issue, is shown once, and can be revoked until it is redeemed."
            }
            div(class="form-grid", style="margin-top:12px") {
                label(class="field full") {
                    "Email"
                    input(
                        class="input",
                        r#type="email",
                        placeholder="teammate@example.com",
                        value=move || email.get_clone(),
                        on:input=move |event: web_sys::Event| {
                            if let Some(target) = event.target() {
                                let input = target.unchecked_into::<HtmlInputElement>();
                                email.set(input.value());
                            }
                        },
                    )
                }
                label(class="field") {
                    "Role"
                    select(
                        class="select",
                        value=move || role.get_clone(),
                        on:change=move |event: web_sys::Event| {
                            if let Some(target) = event.target() {
                                let select = target.unchecked_into::<HtmlSelectElement>();
                                role.set(select.value());
                            }
                        },
                    ) {
                        option(value="viewer") { "viewer" }
                        option(value="triager") { "triager" }
                        option(value="admin") { "admin" }
                        option(value="owner") { "owner" }
                    }
                }
            }
            div(class="form-actions", style="justify-content:flex-start") {
                button(
                    class="btn",
                    r#type="button",
                    on:click=move |_| {
                        let target = email.get_clone().trim().to_string();
                        if target.is_empty() {
                            state::toast(store, "Enter an email address to invite", "error");
                            return;
                        }
                        spawn_local(state::create_invitation(
                            store,
                            target,
                            role.get_clone(),
                        ));
                    },
                ) {
                    "Create invitation"
                }
            }
            (last_block)
        }
    }
}

/// The deployment operator's organisation console (FR-012).
#[component]
pub fn OrgsView() -> View {
    let store = use_store();
    let operator = create_selector(move || store.with(state::is_operator));
    let orgs = create_selector(move || store.with(|state| state.orgs.clone()));
    let last = create_selector(move || store.with(|state| state.last_setup_token.clone()));
    let name = create_signal(String::new());
    let admin_email = create_signal(String::new());

    let body = View::from_dynamic(move || -> View {
        if !operator.get() {
            return empty_state(
                "Operator access required",
                "Organisations and their initial administrators are created by the deployment operator.",
                View::new(),
            );
        }
        let rows: Vec<View> = orgs
            .get_clone()
            .into_iter()
            .map(|org| {
                let demo = if org.is_demo {
                    view! { span(class="badge warning") { "demo" } }
                } else {
                    View::new()
                };
                view! {
                    div(class="member-row") {
                        div(class="member-id") {
                            div(class="who") {
                                strong { (org.name.clone()) }
                                small { (format!("{} · {}", org.slug, util::relative(&org.created_at))) }
                            }
                        }
                        (demo)
                    }
                }
            })
            .collect();
        let list = if rows.is_empty() {
            empty_state(
                "No organisations",
                "Create the first organisation and name its initial administrator.",
                View::new(),
            )
        } else {
            view! { div { (rows) } }
        };

        view! {
            div(class="settings-grid") {
                section(class="panel panel-pad") {
                    (section_title(
                        "Organisations",
                        view! { span(class="muted small") { "operator only" } },
                    ))
                    div(style="margin-top:12px") { (list) }
                }
                section(class="panel panel-pad") {
                    (section_title(
                        "Create an organisation",
                        view! { span(class="badge") { "one-time token" } },
                    ))
                    p(class="muted small") {
                        "The initial administrator redeems a single-use token, sets their own password, and receives recovery codes once. Deliver the token out of band."
                    }
                    div(class="form-grid", style="margin-top:12px") {
                        label(class="field full") {
                            "Name"
                            input(
                                class="input",
                                placeholder="Acme",
                                value=move || name.get_clone(),
                                on:input=move |event: web_sys::Event| {
                                    if let Some(target) = event.target() {
                                        let input = target.unchecked_into::<HtmlInputElement>();
                                        name.set(input.value());
                                    }
                                },
                            )
                        }
                        label(class="field full") {
                            "Initial administrator email"
                            input(
                                class="input",
                                r#type="email",
                                placeholder="admin@acme.test",
                                value=move || admin_email.get_clone(),
                                on:input=move |event: web_sys::Event| {
                                    if let Some(target) = event.target() {
                                        let input = target.unchecked_into::<HtmlInputElement>();
                                        admin_email.set(input.value());
                                    }
                                },
                            )
                        }
                    }
                    div(class="form-actions", style="justify-content:flex-start") {
                        button(
                            class="btn btn-primary",
                            r#type="button",
                            on:click=move |_| {
                                let org_name = name.get_clone().trim().to_string();
                                let email = admin_email.get_clone().trim().to_string();
                                if org_name.is_empty() || email.is_empty() {
                                    state::toast(store, "Name and administrator email are required", "error");
                                    return;
                                }
                                spawn_local(state::create_org(store, org_name, email));
                            },
                        ) {
                            "Create organisation"
                        }
                    }
                    (if let Some((org, setup_token)) = last.get_clone() {
                        let token_for_copy = setup_token.clone();
                        view! {
                            div(class="panel panel-pad", style="margin-top:14px") {
                                (section_title(
                                    "Setup token",
                                    view! { span(class="badge warning") { "shown once" } },
                                ))
                                p(class="muted small") {
                                    (format!("For the initial administrator of {org}. It expires in seven days."))
                                }
                                div(class="dsn-box") {
                                    input(class="input mono", readonly=true, value=setup_token)
                                    button(
                                        class="btn",
                                        r#type="button",
                                        on:click=move |_| {
                                            let text = token_for_copy.clone();
                                            spawn_local(async move {
                                                let clipboard = util::window().navigator().clipboard();
                                                match JsFuture::from(clipboard.write_text(&text)).await {
                                                    Ok(_) => state::toast(store, "Setup token copied", "success"),
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
                    } else {
                        View::new()
                    })
                }
            }
        }
    });

    view! { (body) }
}
