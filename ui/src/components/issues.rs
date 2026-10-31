use crate::api::{self, AttachmentMeta, Issue, IssueDetailResponse, StackFrame};
use crate::components::common::{empty_state, level_badge, section_title, skeleton, state_badge};
use crate::state::{self, use_store, AppState};
use crate::util;
use serde_json::Value;
use sycamore::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

#[component]
pub fn IssuesView() -> View {
    let store = use_store();

    let issues = create_selector(move || store.with(|state| state.issues.clone()));
    let can_send = create_selector(move || store.with(state::can_send));
    let list = View::from_dynamic(move || -> View {
        let issues = issues.get_clone();
        if issues.is_empty() {
            let action = if can_send.get() {
                view! {
                    button(
                        class="btn btn-primary",
                        r#type="button",
                        on:click=move |_| state::open_modal(store, state::ModalKind::SendEvent),
                    ) {
                        "Send a test error"
                    }
                }
            } else {
                View::new()
            };
            empty_state(
                "No issues match",
                "Try a different state, level, or search term. New errors appear here as soon as ingest is processed.",
                action,
            )
        } else {
            let rows: Vec<View> = issues.iter().cloned().map(issue_row).collect();
            view! { div(class="panel issue-list") { (rows) } }
        }
    });

    view! {
        div {
            div(class="toolbar") {
                input(
                    class="input",
                    placeholder="Search title, culprit, fingerprint…",
                    value=move || store.with(|state| state.filters.q.clone()),
                    on:input=move |event: web_sys::Event| {
                        if let Some(target) = event.target() {
                            let input = target.unchecked_into::<web_sys::HtmlInputElement>();
                            let value = input.value();
                            store.update(|state| state.filters.q = value);
                            spawn_local(state::load_issues(store));
                        }
                    },
                )
                select(
                    class="select",
                    value=move || store.with(|state| state.filters.state.clone()),
                    on:change=move |event: web_sys::Event| {
                        if let Some(target) = event.target() {
                            let select = target.unchecked_into::<web_sys::HtmlSelectElement>();
                            let value = select.value();
                            store.update(|state| state.filters.state = value);
                            spawn_local(state::load_issues(store));
                        }
                    },
                ) {
                    option(value="open") { "Open" }
                    option(value="resolved") { "Resolved" }
                    option(value="ignored") { "Ignored" }
                    option(value="") { "All states" }
                }
                select(
                    class="select",
                    value=move || store.with(|state| state.filters.level.clone()),
                    on:change=move |event: web_sys::Event| {
                        if let Some(target) = event.target() {
                            let select = target.unchecked_into::<web_sys::HtmlSelectElement>();
                            let value = select.value();
                            store.update(|state| state.filters.level = value);
                            spawn_local(state::load_issues(store));
                        }
                    },
                ) {
                    option(value="") { "All levels" }
                    option(value="error") { "Error" }
                    option(value="fatal") { "Fatal" }
                    option(value="warning") { "Warning" }
                    option(value="info") { "Info" }
                }
                select(
                    class="select",
                    value=move || store.with(|state| state.filters.sort.clone()),
                    on:change=move |event: web_sys::Event| {
                        if let Some(target) = event.target() {
                            let select = target.unchecked_into::<web_sys::HtmlSelectElement>();
                            let value = select.value();
                            store.update(|state| state.filters.sort = value);
                            spawn_local(state::load_issues(store));
                        }
                    },
                ) {
                    option(value="last_seen") { "Last seen" }
                    option(value="count") { "Occurrences" }
                    option(value="first_seen") { "First seen" }
                }
                span(class="spacer") {}
                span(class="muted small") {
                    (move || {
                        let count = store.with(|state| state.issues.len());
                        format!("{} issue{}", count, if count == 1 { "" } else { "s" })
                    })
                }
            }
            (list)
        }
    }
}

pub(crate) fn issue_row(issue: Issue) -> View {
    let href = format!("#/issues/{}", issue.id);
    let title = issue.title.clone();
    let culprit = issue
        .culprit
        .clone()
        .unwrap_or_else(|| issue.fingerprint.chars().take(20).collect());
    let count = util::num(issue.count);
    let last_seen = util::relative(&issue.last_seen);
    let dot_class = util::level_dot_class(&issue.level);
    let mut flags: Vec<View> = Vec::new();
    if issue.state != "open" {
        flags.push(state_badge(&issue.state));
    }
    if let Some(decision) = &issue.decision {
        let text = format!("categorised: {}", util::cat_label(&decision.category));
        flags.push(view! { span(class="badge cat") { (text) } });
    } else if let Some(suggestion) = &issue.suggestion {
        let text = format!(
            "suggested: {} · {}",
            util::cat_label(&suggestion.category),
            util::pct(suggestion.confidence)
        );
        flags.push(view! { span(class="badge suggest") { (text) } });
    }

    view! {
        a(class="issue-row", href=href.clone()) {
            span(class=dot_class) {}
            span(class="issue-main") {
                span(class="issue-title") { (title.clone()) }
                span(class="issue-culprit") { (culprit.clone()) }
            }
            span(class="issue-meta") {
                span(class="count") { (count.clone()) }
                span { (last_seen.clone()) }
            }
            span(class="issue-flags") { (flags) }
        }
    }
}

#[component]
pub fn IssueDetail() -> View {
    let store = use_store();
    let raw_tab = create_signal("event".to_string());
    let raw_text = create_signal(String::new());
    let override_category = create_signal("issue".to_string());
    let last_event = create_signal(String::new());

    create_effect(move || {
        let (event_id, json) = store.with(|state| {
            state
                .issue
                .as_ref()
                .map(|detail| {
                    (
                        detail
                            .event
                            .as_ref()
                            .map(|event| event.id.clone())
                            .unwrap_or_default(),
                        detail.event_json.clone(),
                    )
                })
                .unwrap_or((String::new(), None))
        });
        if !event_id.is_empty() && event_id != last_event.get_clone() {
            raw_tab.set("event".into());
            raw_text.set(
                json.map(|value| serde_json::to_string_pretty(&value).unwrap_or_default())
                    .unwrap_or_default(),
            );
            last_event.set(event_id);
        }
    });

    let detail = create_selector(move || store.with(|state| state.issue.clone()));
    let body = View::from_dynamic(move || -> View {
        match detail.get_clone() {
            Some(detail) => detail_view(store, detail, raw_tab, raw_text, override_category),
            None => view! {
                div(class="panel panel-pad") {
                    (skeleton("150px"))
                    (skeleton("120px"))
                    (skeleton("320px"))
                }
            },
        }
    });

    view! { (body) }
}

fn detail_view(
    store: Signal<AppState>,
    detail: IssueDetailResponse,
    raw_tab: Signal<String>,
    raw_text: Signal<String>,
    override_category: Signal<String>,
) -> View {
    let issue = detail.issue.clone();
    let state_action = if issue.state == "resolved" {
        ("Reopen issue", "open")
    } else {
        ("Resolve issue", "resolved")
    };
    let issue_id_for_raw = issue.id.clone();
    let issue_id_for_state = issue.id.clone();
    let issue_id_for_confirm = issue.id.clone();
    let issue_id_for_override = issue.id.clone();
    let actionable = store.with(state::can_triage);
    let level_view = level_badge(&issue.level);
    let state_view = state_badge(&issue.state);
    let subline = format!(
        "{} · fingerprint {}",
        issue.culprit.clone().unwrap_or_else(|| "no culprit".into()),
        issue.fingerprint.chars().take(16).collect::<String>()
    );
    let first_seen_title = util::full_time(&issue.first_seen);
    let first_seen_relative = util::relative(&issue.first_seen);
    let last_seen_title = util::full_time(&issue.last_seen);
    let last_seen_relative = util::relative(&issue.last_seen);
    let occurrence_count = util::num(issue.count);
    let frame_count_text = format!(
        "{} frame{} · crash site first",
        issue.frames.len(),
        if issue.frames.len() == 1 { "" } else { "s" }
    );
    let crumb_count_text = issue.breadcrumbs.len().to_string();
    let attachment_count = detail.attachments.len();
    let has_attachments = !detail.attachments.is_empty();
    let raw_meta_text = if detail.raw_available {
        format!(
            "{} bytes received · {} attachment(s)",
            util::num(detail.event.as_ref().map(|event| event.size).unwrap_or(0) as u64),
            detail.attachments.len()
        )
    } else {
        "expired under retention".to_string()
    };

    let stack: Vec<View> = issue
        .frames
        .iter()
        .cloned()
        .map(frame_view)
        .collect::<Vec<_>>();
    let stack_view = if stack.is_empty() {
        view! { div(class="muted small") { "No stack frames in this payload." } }
    } else {
        view! { div(class="stack") { (stack) } }
    };

    let crumbs: Vec<View> = issue.breadcrumbs.iter().cloned().map(crumb_view).collect();
    let crumbs_view = if crumbs.is_empty() {
        view! { div(class="muted small") { "No breadcrumbs in this payload." } }
    } else {
        view! { div(class="crumbs") { (crumbs) } }
    };

    let tag_view = tags_view(issue.tags.clone());
    let attachments_text = attachments_text(&detail.attachments);

    let suggestion = suggestion_block(
        store,
        issue.clone(),
        actionable,
        override_category,
        issue_id_for_confirm,
        issue_id_for_override,
    );

    let state_button = if actionable {
        let (label, next) = state_action;
        let issue_id = issue_id_for_state;
        view! {
            button(
                class="btn btn-sm",
                r#type="button",
                on:click=move |_| {
                    let issue_id = issue_id.clone();
                    spawn_local(state::change_issue_state(store, issue_id, next));
                },
            ) {
                (label)
            }
        }
    } else {
        View::new()
    };

    let tabs = {
        let attachments_text = attachments_text.clone();
        view! {
            div(class="tabs") {
                button(
                    class=move || {
                        if raw_tab.get_clone() == "event" {
                            "tab active".to_string()
                        } else {
                            "tab".to_string()
                        }
                    },
                    r#type="button",
                    on:click=move |_| {
                        raw_tab.set("event".into());
                        let text = store.with(|state| {
                            state
                                .issue
                                .as_ref()
                                .and_then(|detail| detail.event_json.clone())
                                .map(|value| serde_json::to_string_pretty(&value).unwrap_or_default())
                                .unwrap_or_default()
                        });
                        raw_text.set(text);
                    },
                ) {
                    "Event JSON"
                }
                button(
                    class=move || {
                        if raw_tab.get_clone() == "envelope" {
                            "tab active".to_string()
                        } else {
                            "tab".to_string()
                        }
                    },
                    r#type="button",
                    on:click=move |_| {
                        raw_tab.set("envelope".into());
                        raw_text.set("Loading…".into());
                        let issue_id = issue_id_for_raw.clone();
                        let token = store.with(|state| state.token.clone()).unwrap_or_default();
                        spawn_local(async move {
                            let text = api::get_text(
                                &format!("/api/issues/{issue_id}/raw"),
                                Some(&token),
                            )
                            .await
                            .unwrap_or_else(|error| error);
                            raw_text.set(text);
                        });
                    },
                ) {
                    "Raw envelope"
                }
                (if !has_attachments {
                    View::new()
                } else {
                    let attachments_text = attachments_text.clone();
                    view! {
                        button(
                            class=move || {
                                if raw_tab.get_clone() == "attachments" {
                                    "tab active".to_string()
                                } else {
                                    "tab".to_string()
                                }
                            },
                            r#type="button",
                            on:click=move |_| {
                                raw_tab.set("attachments".into());
                                raw_text.set(attachments_text.clone());
                            },
                        ) {
                            (format!("Attachments ({attachment_count})"))
                        }
                    }
                })
            }
        }
    };

    let raw_panel = View::from_dynamic(move || {
        let _ = raw_tab.get_clone();
        let text = raw_text.get_clone();
        view! { pre(class="raw") { (text) } }
    });

    let stack_header = section_title(
        "Stack trace",
        view! { span(class="muted small") { (frame_count_text.clone()) } },
    );
    let crumb_header = section_title(
        "Breadcrumbs",
        view! { span(class="muted small") { (crumb_count_text.clone()) } },
    );
    let raw_header = section_title(
        "Raw event",
        view! { span(class="muted small") { (raw_meta_text.clone()) } },
    );

    view! {
        section(class="panel issue-hero") {
            div(class="title-line") {
                (level_view)
                h1 { (issue.title.clone()) }
                (state_view)
                div(class="hero-actions") { (state_button) }
            }
            div(class="subline") { (subline) }
            div(class="stat-row") {
                div(class="stat") {
                    span(class="k") { "Occurrences" }
                    span(class="v") { (occurrence_count) }
                }
                div(class="stat") {
                    span(class="k") { "First seen" }
                    span(class="v", title=first_seen_title) {
                        (first_seen_relative)
                    }
                }
                div(class="stat") {
                    span(class="k") { "Last seen" }
                    span(class="v", title=last_seen_title) {
                        (last_seen_relative)
                    }
                }
                div(class="stat") {
                    span(class="k") { "Release" }
                    span(class="v") { (issue.release.clone().unwrap_or_else(|| "—".into())) }
                }
                div(class="stat") {
                    span(class="k") { "Environment" }
                    span(class="v") {
                        (issue.environment.clone().unwrap_or_else(|| "—".into()))
                    }
                }
            }
        }

        (suggestion)

        div(class="detail-grid split") {
            section(class="panel panel-pad") {
                (stack_header)
                (stack_view)
            }
            section(class="panel panel-pad") {
                (crumb_header)
                (crumbs_view)
            }
        }

        section(class="panel panel-pad") {
            (section_title("Tags", View::new()))
            (tag_view)
        }

        section(class="panel panel-pad") {
            (raw_header)
            (if detail.raw_available {
                view! {
                    (tabs)
                    (raw_panel)
                }
            } else {
                view! {
                    div(class="empty") {
                        strong { "Raw payload expired" }
                        span { "The issue metadata is retained, but the original payload has passed this project's retention window." }
                    }
                }
            })
        }
    }
}

fn frame_view(frame: StackFrame) -> View {
    let in_app = frame.in_app == Some(true);
    let class = if in_app { "frame inapp" } else { "frame" };
    let function = frame
        .function
        .clone()
        .unwrap_or_else(|| "<anonymous>".into());
    let location = format!(
        "{}{}",
        frame.filename.clone().unwrap_or_default(),
        frame
            .lineno
            .map(|line| format!(":{line}"))
            .unwrap_or_default()
    );
    let line = frame
        .lineno
        .map(|line| line.to_string())
        .unwrap_or_default();
    let context_view = if let Some(context) = frame.context_line.clone() {
        view! {
            div(class="ctx") {
                span(class="ln") { (line.clone()) }
                (context)
            }
        }
    } else {
        View::new()
    };
    view! {
        div(class=class) {
            div(class="frame-head") {
                span(class="fn") { (function.clone()) }
                span(class="loc") { (location.clone()) }
                (if in_app {
                    view! { span(class="tag-inapp") { "in app" } }
                } else {
                    View::new()
                })
            }
            (context_view)
        }
    }
}

fn crumb_view(crumb: api::Breadcrumb) -> View {
    let time = crumb
        .timestamp
        .as_deref()
        .map(util::short_time)
        .unwrap_or_else(|| "—".into());
    let category = crumb.category.clone().unwrap_or_else(|| "event".into());
    let message = crumb.message.clone().unwrap_or_default();
    let data_view = if let Some(data) = crumb
        .data
        .as_ref()
        .map(|value| serde_json::to_string(value).unwrap_or_default())
    {
        view! { div(class="data") { (data) } }
    } else {
        View::new()
    };
    view! {
        div(class="crumb") {
            span(class="time") { (time.clone()) }
            span(class="what") {
                span(class="cat") { (category.clone()) }
                span(class="msg") { (message.clone()) }
                (data_view)
            }
        }
    }
}

fn tags_view(tags: std::collections::HashMap<String, Value>) -> View {
    if tags.is_empty() {
        return view! { span(class="muted small") { "No tags." } };
    }
    let mut keys: Vec<String> = tags.keys().cloned().collect();
    keys.sort();
    let chips: Vec<View> = keys
        .into_iter()
        .map(|key| {
            let value = tags
                .get(&key)
                .map(util::value_to_string)
                .unwrap_or_default();
            view! {
                span(class="tag") {
                    span(class="k") { (key.clone()) }
                    span(class="v") { (value.clone()) }
                }
            }
        })
        .collect();
    view! { div(class="tags") { (chips) } }
}

fn attachments_text(attachments: &[AttachmentMeta]) -> String {
    if attachments.is_empty() {
        return "No attachments.".into();
    }
    attachments
        .iter()
        .map(|attachment| {
            format!(
                "{}  ({}, {} bytes)",
                attachment.filename,
                attachment.content_type,
                util::num(attachment.size as u64)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn suggestion_block(
    store: Signal<AppState>,
    issue: Issue,
    actionable: bool,
    override_category: Signal<String>,
    issue_id_for_confirm: String,
    issue_id_for_override: String,
) -> View {
    if let Some(decision) = issue.decision.clone() {
        let mut actions: Vec<View> = Vec::new();
        if actionable {
            actions.push(override_controls(
                store,
                override_category,
                issue_id_for_override,
            ));
        }
        let category_label = util::cat_label(&decision.category);
        let by = decision.by.clone();
        let at = util::full_time(&decision.at);
        let verb = if decision.overridden {
            "Override"
        } else {
            "Confirmed"
        };
        let model_note = issue
            .suggestion
            .as_ref()
            .map(|suggestion| {
                format!(
                    " · the model suggested {} ({})",
                    util::cat_label(&suggestion.category),
                    util::pct(suggestion.confidence)
                )
            })
            .unwrap_or_default();
        let detail = format!("{verb} by {by} · {at}{model_note}");
        return view! {
            div(class="suggestion decided") {
                span(class="icon") { "✓" }
                div(class="body") {
                    strong { (format!("Categorised as {category_label}")) }
                    div(class="muted small") { (detail) }
                }
                div(class="actions") { (actions) }
            }
        };
    }

    if let Some(suggestion) = issue.suggestion.clone() {
        let category_label = util::cat_label(&suggestion.category);
        let confidence = util::pct(suggestion.confidence);
        let source = suggestion.source.clone();
        let confirm_label = format!("Confirm {category_label}");
        let confirm = if actionable {
            let issue_id = issue_id_for_confirm;
            view! {
                button(
                    class="btn btn-primary btn-sm",
                    r#type="button",
                    on:click=move |_| {
                        let issue_id = issue_id.clone();
                        spawn_local(state::categorise(
                            store,
                            issue_id,
                            "confirm",
                            None,
                        ));
                    },
                ) {
                    (confirm_label.clone())
                }
            }
        } else {
            view! { span(class="badge") { "read only" } }
        };
        let override_view = if actionable {
            override_controls(store, override_category, issue_id_for_override)
        } else {
            View::new()
        };
        return view! {
            div(class="suggestion") {
                span(class="icon") { "✦" }
                div(class="body") {
                    strong { (format!("Model suggests {category_label}")) }
                    span(class="confidence") {
                        (format!("{confidence} confidence · {source}"))
                    }
                    div(class="muted small") {
                        "The item is already stored. Confirm or override — the model never decides silently."
                    }
                }
                div(class="actions") {
                    (confirm)
                    (override_view)
                }
            }
        };
    }

    view! {
        div(class="suggestion none") {
            span(class="icon") { "—" }
            div(class="body") {
                strong { "No suggestion" }
                div(class="muted small") {
                    "The decision model is not configured, unreachable, or returned no usable answer. Ingest and storage were unaffected."
                }
            }
        }
    }
}

fn override_controls(
    store: Signal<AppState>,
    override_category: Signal<String>,
    issue_id: String,
) -> View {
    view! {
        select(
            class="select",
            style="width:auto",
            value=move || override_category.get_clone(),
            on:change=move |event: web_sys::Event| {
                if let Some(target) = event.target() {
                    let select = target.unchecked_into::<web_sys::HtmlSelectElement>();
                    override_category.set(select.value());
                }
            },
        ) {
            option(value="issue") { "Issue" }
            option(value="log") { "Log" }
            option(value="raw_event") { "Raw event" }
        }
        button(
            class="btn btn-ghost btn-sm",
            r#type="button",
            on:click=move |_| {
                let category = override_category.get_clone();
                let issue_id = issue_id.clone();
                spawn_local(state::categorise(
                    store,
                    issue_id,
                    "override",
                    Some(category),
                ));
            },
        ) {
            "Override"
        }
    }
}
