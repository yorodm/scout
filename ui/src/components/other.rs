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
                "No log lines match",
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
                        button(
                            class="btn btn-sm",
                            r#type="button",
                            on:click=move |_| spawn_local(state::create_key(store)),
                        ) {
                            "Create key"
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
                div(class="form-actions") {
                    button(
                        class="btn btn-ghost",
                        r#type="button",
                        on:click=move |_| {
                            let retention_days = retention_value.get_clone().parse::<u32>().unwrap_or(30);
                            spawn_local(state::save_settings(store, retention_days, None));
                        },
                    ) {
                        "Disable model"
                    }
                    button(
                        class="btn btn-ghost",
                        r#type="button",
                        on:click=move |_| {
                            let retention_days = retention_value.get_clone().parse::<u32>().unwrap_or(30);
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
                            let retention_days = retention_value.get_clone().parse::<u32>().unwrap_or(30);
                            let endpoint = endpoint_value.get_clone().trim().to_string();
                            let model = if endpoint.is_empty() {
                                None
                            } else {
                                let api_key = model_key_value.get_clone().trim().to_string();
                                Some(api::ModelConfig {
                                    endpoint,
                                    api_key: if api_key.is_empty() { None } else { Some(api_key) },
                                    model: model_id_value.get_clone().trim().to_string(),
                                })
                            };
                            spawn_local(state::save_settings(store, retention_days, model));
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

            section(class="panel panel-pad") {
                (section_title(
                    "Demo data",
                    view! { span(class="muted small") { "in-memory" } },
                ))
                p(class="muted small") {
                    "Reset the store to its seeded state. This signs you out and discards everything sent during this session."
                }
                div(class="form-actions", style="justify-content:flex-start;margin-top:18px") {
                    button(
                        class="btn btn-danger",
                        r#type="button",
                        on:click=move |_| {
                            if util::window()
                                .confirm_with_message("Reset all demo data and sign out?")
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
