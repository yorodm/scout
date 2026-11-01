use crate::state::{self, use_store, ModalKind, TestEventForm};
use crate::util;
use sycamore::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;

#[component]
pub fn ModalHost() -> View {
    let store = use_store();
    let modal = create_selector(move || store.with(|state| state.modal));
    let body = View::from_dynamic(move || -> View {
        match modal.get() {
            Some(ModalKind::SendEvent) => view! { SendEventModal() },
            Some(ModalKind::SendLog) => view! { SendLogModal() },
            Some(ModalKind::FirstSignInCodes) => view! { FirstSignInCodesModal() },
            None => View::new(),
        }
    });
    view! { (body) }
}

#[component]
pub fn ToastHost() -> View {
    let store = use_store();
    let toast = create_selector(move || store.with(|state| state.toast.clone()));
    let body = View::from_dynamic(move || -> View {
        match toast.get_clone() {
            Some(toast) => {
                let class = format!("toast {}", toast.kind);
                let message = toast.message;
                view! {
                    div(
                        class=class,
                        on:click=move |_| store.update(|state| state.toast = None),
                    ) {
                        span(class="dot") {}
                        span { (message) }
                    }
                }
            }
            None => View::new(),
        }
    });
    view! { div(class="toasts") { (body) } }
}

#[component]
fn FirstSignInCodesModal() -> View {
    let store = use_store();
    let codes = store.with(|state| state.pending_recovery_codes.clone());
    let items: Vec<View> = codes
        .into_iter()
        .map(|code| view! { div(class="recovery-code") { (code) } })
        .collect();
    view! {
        // Deliberately not dismissible by clicking the backdrop: the codes are
        // shown once and must be acknowledged before the operator continues.
        div(class="modal-backdrop") {
            div(
                class="modal",
                on:click=move |event: web_sys::MouseEvent| event.stop_propagation(),
            ) {
                div(class="modal-head") {
                    div {
                        h2 { "Save your recovery codes" }
                        p {
                            "These are shown once and are the only way back in without database access. Each code resets your password a single time."
                        }
                    }
                }
                div(class="recovery-codes") { (items) }
                div(class="form-actions") {
                    button(
                        class="btn btn-primary",
                        r#type="button",
                        on:click=move |_| state::acknowledge_recovery_codes(store),
                    ) {
                        "I saved these codes"
                    }
                }
            }
        }
    }
}

#[component]
fn SendEventModal() -> View {
    let store = use_store();
    let event_type = create_signal("TypeError".to_string());
    let message =
        create_signal("Cannot read properties of undefined (reading 'currency')".to_string());
    let level = create_signal("error".to_string());
    let release = create_signal("web@2.4.2".to_string());
    let count = create_signal("1".to_string());
    let attachment = create_signal(true);
    let busy = create_signal(false);
    let progress = create_signal(0.0f64);
    let error = create_signal(None::<String>);

    let (dsn_id, key) = store.with(|state| {
        state::project(state)
            .map(|project| {
                (
                    project.dsn_id.clone(),
                    project.key.clone().unwrap_or_default(),
                )
            })
            .unwrap_or_default()
    });
    let host = util::window()
        .location()
        .host()
        .unwrap_or_else(|_| "localhost:8080".into());
    let dsn = format!("http://{key}@{host}/{dsn_id}");

    let send = move || {
        if busy.get() {
            return;
        }
        let form = TestEventForm {
            event_type: event_type.get_clone(),
            message: message.get_clone(),
            level: level.get_clone(),
            release: release.get_clone(),
            count: count.get_clone().parse::<u32>().unwrap_or(1),
            attachment: attachment.get(),
        };
        let total = form.count;
        busy.set(true);
        error.set(None);
        progress.set(0.0);
        spawn_local(async move {
            match state::send_test_events(store, form, progress).await {
                Ok(()) => {
                    state::close_modal(store);
                    state::navigate(store, "#/issues");
                    state::poll_issues(store);
                    state::toast(
                        store,
                        format!(
                            "Sent {} event{}",
                            util::num(total as u64),
                            if total == 1 { "" } else { "s" }
                        ),
                        "success",
                    );
                }
                Err(message) => {
                    error.set(Some(message));
                    busy.set(false);
                }
            }
        });
    };

    view! {
        div(class="modal-backdrop", on:click=move |_| state::close_modal(store)) {
            div(
                class="modal",
                on:click=move |event: web_sys::MouseEvent| event.stop_propagation(),
            ) {
                div(class="modal-head") {
                    div {
                        h2 { "Send a test error" }
                        p {
                            (format!("Posts a real Sentry envelope to /api/{dsn_id}/envelope/ with the project key."))
                        }
                    }
                    button(
                        class="close",
                        r#type="button",
                        on:click=move |_| state::close_modal(store),
                    ) {
                        "×"
                    }
                }
                div(class="form-grid") {
                    label(class="field full") {
                        "Exception type"
                        input(
                            class="input",
                            value=move || event_type.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    event_type.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field full") {
                        "Message"
                        input(
                            class="input",
                            value=move || message.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    message.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field") {
                        "Level"
                        select(
                            class="select",
                            value=move || level.get_clone(),
                            on:change=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let select = target.unchecked_into::<web_sys::HtmlSelectElement>();
                                    level.set(select.value());
                                }
                            },
                        ) {
                            option(value="error") { "error" }
                            option(value="fatal") { "fatal" }
                            option(value="warning") { "warning" }
                            option(value="info") { "info" }
                        }
                    }
                    label(class="field") {
                        "Release"
                        input(
                            class="input",
                            value=move || release.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    release.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field") {
                        "Occurrences"
                        input(
                            class="input",
                            r#type="number",
                            min="1",
                            max="1000",
                            value=move || count.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    count.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field", style="justify-content:flex-end") {
                        span(style="display:flex;gap:7px;align-items:center;font-size:12.5px") {
                            input(
                                r#type="checkbox",
                                checked=move || attachment.get(),
                                on:change=move |event: web_sys::Event| {
                                    if let Some(target) = event.target() {
                                        let input = target.unchecked_into::<HtmlInputElement>();
                                        attachment.set(input.checked());
                                    }
                                },
                            )
                            "include attachment"
                        }
                    }
                }
                div(class="hint") {
                    "Sending 1,000 identical events demonstrates grouping: exactly one issue with count 1,000. Events carry an explicit fingerprint, breadcrumbs, a release tag, and an attachment."
                }
                div(class="hint") {
                    "DSN "
                    span(class="mono") { (dsn) }
                }
                div(
                    class=move || {
                        if error.with(|value| value.is_some()) {
                            "modal-error".to_string()
                        } else {
                            "modal-error hidden".to_string()
                        }
                    },
                ) {
                    (move || error.get_clone().unwrap_or_default())
                }
                div(
                    class=move || {
                        if busy.get() {
                            "progress".to_string()
                        } else {
                            "progress hidden".to_string()
                        }
                    },
                ) {
                    div(style=move || format!("width:{}%", (progress.get() * 100.0) as i32)) {}
                }
                div(class="form-actions") {
                    button(
                        class="btn btn-ghost",
                        r#type="button",
                        on:click=move |_| state::close_modal(store),
                    ) {
                        "Cancel"
                    }
                    button(
                        class="btn btn-primary",
                        r#type="button",
                        disabled=move || busy.get(),
                        on:click=move |_| send(),
                    ) {
                        "Send events"
                    }
                }
            }
        }
    }
}

#[component]
fn SendLogModal() -> View {
    let store = use_store();
    let message =
        create_signal("checkout failed for cart 8f21: TypeError: missing currency".to_string());
    let level = create_signal("error".to_string());
    let source = create_signal("checkout-web".to_string());
    let busy = create_signal(false);
    let error = create_signal(None::<String>);

    let (dsn_id, _key) = store.with(|state| {
        state::project(state)
            .map(|project| {
                (
                    project.dsn_id.clone(),
                    project.key.clone().unwrap_or_default(),
                )
            })
            .unwrap_or_default()
    });

    let send = move || {
        if busy.get() {
            return;
        }
        busy.set(true);
        error.set(None);
        let message_value = message.get_clone();
        let level_value = level.get_clone();
        let source_value = source.get_clone();
        spawn_local(async move {
            match state::send_test_log(store, message_value, level_value, source_value).await {
                Ok(()) => {
                    state::close_modal(store);
                    state::navigate(store, "#/logs");
                    state::poll_logs(store);
                    state::toast(store, "Log line ingested", "success");
                }
                Err(message) => {
                    error.set(Some(message));
                    busy.set(false);
                }
            }
        });
    };

    view! {
        div(class="modal-backdrop", on:click=move |_| state::close_modal(store)) {
            div(
                class="modal",
                on:click=move |event: web_sys::MouseEvent| event.stop_propagation(),
            ) {
                div(class="modal-head") {
                    div {
                        h2 { "Send a test log" }
                        p {
                            (format!("Posts a structured log line to /api/{dsn_id}/logs/ with the project key."))
                        }
                    }
                    button(
                        class="close",
                        r#type="button",
                        on:click=move |_| state::close_modal(store),
                    ) {
                        "×"
                    }
                }
                div(class="form-grid") {
                    label(class="field full") {
                        "Message"
                        input(
                            class="input",
                            value=move || message.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    message.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field") {
                        "Level"
                        select(
                            class="select",
                            value=move || level.get_clone(),
                            on:change=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let select = target.unchecked_into::<web_sys::HtmlSelectElement>();
                                    level.set(select.value());
                                }
                            },
                        ) {
                            option(value="info") { "info" }
                            option(value="warning") { "warning" }
                            option(value="error") { "error" }
                            option(value="debug") { "debug" }
                        }
                    }
                    label(class="field") {
                        "Source"
                        input(
                            class="input",
                            value=move || source.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<HtmlInputElement>();
                                    source.set(input.value());
                                }
                            },
                        )
                    }
                }
                div(class="hint") {
                    "The line is searchable alongside issues immediately. Attributes are attached to show structured metadata."
                }
                div(
                    class=move || {
                        if error.with(|value| value.is_some()) {
                            "modal-error".to_string()
                        } else {
                            "modal-error hidden".to_string()
                        }
                    },
                ) {
                    (move || error.get_clone().unwrap_or_default())
                }
                div(class="form-actions") {
                    button(
                        class="btn btn-ghost",
                        r#type="button",
                        on:click=move |_| state::close_modal(store),
                    ) {
                        "Cancel"
                    }
                    button(
                        class="btn btn-primary",
                        r#type="button",
                        disabled=move || busy.get(),
                        on:click=move |_| send(),
                    ) {
                        "Send log"
                    }
                }
            }
        }
    }
}
