use crate::api;
use crate::components::common::{avatar, brand};
use crate::components::{issues, other, tokens};
use crate::state::{self, use_store, AppState, ModalKind, ViewKind};
use crate::util;
use sycamore::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlSelectElement;

#[component]
pub fn App() -> View {
    let store = create_signal(AppState::default());
    provide_context(store);

    on_mount(move || {
        spawn_local(async move {
            state::load_accounts(store).await;
            // An identity-provider landing carries the session in the fragment;
            // consume it before looking at a stored session.
            if state::apply_oidc_landing(store).await {
                return;
            }
            if let Some(token) = util::storage_get("scout_token") {
                store.update(|state| state.token = Some(token.clone()));
                match api::me(&token).await {
                    Ok(data) => {
                        store.update(|state| state.me = Some(data.user));
                        state::load_projects(store).await;
                        state::sync_from_hash(store).await;
                    }
                    Err(_) => {
                        util::storage_remove("scout_token");
                        store.update(|state| state.token = None);
                    }
                }
            }
        });

        let callback = Closure::<dyn FnMut()>::new(move || {
            spawn_local(state::sync_from_hash(store));
        });
        if let Some(window) = web_sys::window() {
            let _ = window
                .add_event_listener_with_callback("hashchange", callback.as_ref().unchecked_ref());
        }
        callback.forget();
    });

    let logged_in = create_selector(move || store.with(|state| state.me.is_some()));
    let content = View::from_dynamic(move || -> View {
        if logged_in.get() {
            view! { Shell() }
        } else {
            view! { Login() }
        }
    });

    view! {
        (content)
        super::overlays::ModalHost()
        super::overlays::ToastHost()
    }
}

#[component]
fn Login() -> View {
    let store = use_store();
    let email_input = create_signal(String::new());
    let password_input = create_signal(String::new());
    let show_recovery = create_signal(false);
    let recovery_code = create_signal(String::new());
    let recovery_password = create_signal(String::new());

    let invite_token = util::current_hash()
        .strip_prefix("#/invite/")
        .map(str::to_string);
    let invite_preview = create_signal(None::<api::InvitationPreview>);
    let invite_error = create_signal(None::<String>);
    let invite_name = create_signal(String::new());
    let invite_password = create_signal(String::new());

    let token_for_load = invite_token.clone();
    on_mount(move || {
        if let Some(token) = token_for_load.clone() {
            // Remember the invitation so an existing account can claim it right
            // after signing in.
            util::storage_set("scout_pending_invite", &token);
            spawn_local(async move {
                match api::invitation_preview(&token).await {
                    Ok(preview) => invite_preview.set(Some(preview)),
                    Err(error) => invite_error.set(Some(error)),
                }
            });
        }
    });

    let demo_section = View::from_dynamic(move || -> View {
        let accounts = store.with(|state| state.accounts.clone());
        if accounts.is_empty() {
            return View::new();
        }
        let cards: Vec<View> = accounts
            .into_iter()
            .map(|account| {
                let memberships: Vec<View> = account
                    .memberships
                    .iter()
                    .map(|membership| {
                        let role = membership.role.clone();
                        let project = membership.project_name.clone();
                        let badge_class = util::role_badge_class(&role);
                        let text = format!("{role} · {project}");
                        view! {
                            span(class=badge_class) {
                                (text)
                            }
                        }
                    })
                    .collect();
                let name = account.name.clone();
                let email = account.email.clone();
                let email_for_click = email.clone();
                let avatar_view = avatar(&name);
                view! {
                    button(
                        class="account-card",
                        r#type="button",
                        on:click=move |_| {
                            let email = email_for_click.clone();
                            // Seeded demo accounts share one known password.
                            spawn_local(state::login(store, email, "scout-demo".into()));
                        },
                    ) {
                        (avatar_view)
                        strong { (name.clone()) }
                        span(class="muted small") { (email.clone()) }
                        span(class="roles") { (memberships) }
                        span(class="btn btn-primary btn-sm", style="margin-top:auto") { "Sign in" }
                    }
                }
            })
            .collect();
        view! {
            h2(style="font-size:14px;margin:20px 0 12px") { "Or sign in with a demo account" }
            div(class="accounts") { (cards) }
            div(class="login-note") {
                "Demo accounts are seeded and share the password "
                code { "scout-demo" }
                ". Signing in scopes every request to the projects and role shown above. Demo shortcuts exist only when demo mode is enabled."
            }
        }
    });

    let invitation_card = View::from_dynamic(move || -> View {
        if invite_token.is_none() {
            return View::new();
        }
        match invite_preview.get_clone() {
            Some(preview) => {
                let target = preview
                    .project
                    .clone()
                    .unwrap_or_else(|| preview.organisation.clone());
                let role = preview.role.clone().unwrap_or_else(|| "member".into());
                let summary = format!(
                    "{} invited {} to {} as {}.",
                    preview.organisation, preview.email, target, role
                );
                let token = invite_token.clone().unwrap_or_default();
                view! {
                    div(class="panel panel-pad", style="margin-bottom:20px") {
                        h2(style="font-size:14px;margin-bottom:8px") { "You are invited" }
                        p(class="muted small") { (summary) }
                        div(class="form-grid", style="margin-top:12px") {
                            label(class="field") {
                                "Your name"
                                input(
                                    class="input",
                                    value=move || invite_name.get_clone(),
                                    on:input=move |event: web_sys::Event| {
                                        if let Some(target) = event.target() {
                                            let input = target.unchecked_into::<web_sys::HtmlInputElement>();
                                            invite_name.set(input.value());
                                        }
                                    },
                                )
                            }
                            label(class="field") {
                                "Choose a password"
                                input(
                                    class="input",
                                    r#type="password",
                                    autocomplete="new-password",
                                    value=move || invite_password.get_clone(),
                                    on:input=move |event: web_sys::Event| {
                                        if let Some(target) = event.target() {
                                            let input = target.unchecked_into::<web_sys::HtmlInputElement>();
                                            invite_password.set(input.value());
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
                                    let name = invite_name.get_clone();
                                    let password = invite_password.get_clone();
                                    if password.chars().count() < 12 {
                                        state::toast(
                                            store,
                                            "Choose a password of at least 12 characters",
                                            "error",
                                        );
                                        return;
                                    }
                                    spawn_local(state::accept_invitation_as_new(
                                        store,
                                        token.clone(),
                                        name,
                                        password,
                                    ));
                                },
                            ) {
                                "Accept invitation"
                            }
                        }
                        p(class="muted small", style="margin-top:10px") {
                            "Already have an account? Sign in below and the invitation is accepted automatically."
                        }
                    }
                }
            }
            None => match invite_error.get_clone() {
                Some(error) => view! { div(class="login-note") { (error) } },
                None => view! { div(class="login-note") { "Loading your invitation…" } },
            },
        }
    });

    let auth_form = View::from_dynamic(move || -> View {
        if show_recovery.get() {
            view! {
                h2(style="font-size:14px;margin-bottom:12px") { "Reset with a recovery code" }
                form(
                    class="form-grid",
                    on:submit=move |event: web_sys::SubmitEvent| {
                        event.prevent_default();
                        let email = email_input.get_clone();
                        let code = recovery_code.get_clone();
                        let password = recovery_password.get_clone();
                        if email.is_empty() || code.is_empty() || password.is_empty() {
                            state::toast(
                                store,
                                "Email, recovery code, and new password are required",
                                "error",
                            );
                            return;
                        }
                        spawn_local(async move {
                            match api::recover(&email, &code, &password).await {
                                Ok(_) => {
                                    state::toast(
                                        store,
                                        "Password reset. Sign in with the new password.",
                                        "success",
                                    );
                                    email_input.set(email);
                                    show_recovery.set(false);
                                }
                                Err(error) => state::toast(store, error, "error"),
                            }
                        });
                    },
                ) {
                    label(class="field full") {
                        "Email"
                        input(
                            class="input",
                            r#type="email",
                            autocomplete="username",
                            value=move || email_input.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<web_sys::HtmlInputElement>();
                                    email_input.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field full") {
                        "Recovery code"
                        input(
                            class="input mono",
                            placeholder="XXXXX-XXXXX",
                            value=move || recovery_code.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<web_sys::HtmlInputElement>();
                                    recovery_code.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field full") {
                        "New password"
                        input(
                            class="input",
                            r#type="password",
                            autocomplete="new-password",
                            value=move || recovery_password.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<web_sys::HtmlInputElement>();
                                    recovery_password.set(input.value());
                                }
                            },
                        )
                    }
                    div(class="form-actions full") {
                        button(
                            class="btn btn-ghost",
                            r#type="button",
                            on:click=move |_| show_recovery.set(false),
                        ) {
                            "Back"
                        }
                        button(class="btn btn-primary", r#type="submit") { "Reset password" }
                    }
                }
            }
        } else {
            view! {
                h2(style="font-size:14px;margin-bottom:12px") { "Sign in" }
                form(
                    class="form-grid",
                    on:submit=move |event: web_sys::SubmitEvent| {
                        event.prevent_default();
                        let email = email_input.get_clone();
                        let password = password_input.get_clone();
                        if email.is_empty() || password.is_empty() {
                            state::toast(store, "Enter your email and password", "error");
                            return;
                        }
                        spawn_local(state::login(store, email, password));
                    },
                ) {
                    label(class="field") {
                        "Email"
                        input(
                            class="input",
                            r#type="email",
                            autocomplete="username",
                            placeholder="you@example.com",
                            value=move || email_input.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<web_sys::HtmlInputElement>();
                                    email_input.set(input.value());
                                }
                            },
                        )
                    }
                    label(class="field") {
                        "Password"
                        input(
                            class="input",
                            r#type="password",
                            autocomplete="current-password",
                            value=move || password_input.get_clone(),
                            on:input=move |event: web_sys::Event| {
                                if let Some(target) = event.target() {
                                    let input = target.unchecked_into::<web_sys::HtmlInputElement>();
                                    password_input.set(input.value());
                                }
                            },
                        )
                    }
                    div(class="form-actions full") {
                        button(class="btn btn-primary", r#type="submit") { "Sign in" }
                    }
                }
                div(class="form-actions", style="justify-content:flex-start;margin-top:8px") {
                    button(
                        class="btn btn-ghost btn-sm",
                        r#type="button",
                        on:click=move |_| show_recovery.set(true),
                    ) {
                        "Use a recovery code"
                    }
                }
            }
        }
    });

    view! {
        div(class="login") {
            div(class="login-card") {
                div(class="login-brand") {
                    (brand())
                    div {
                        h1 { "Scout" }
                        p(class="muted") { "Self-hosted, Sentry-compatible error tracking & logs" }
                    }
                }
                p(class="lede") {
                    "Point an existing Sentry SDK at Scout — change only the DSN. Errors group into issues, logs sit beside them, and the decision model only ever suggests; a human confirms."
                }
                (invitation_card)
                (auth_form)
                (demo_section)
            }
        }
    }
}

#[component]
fn Shell() -> View {
    let store = use_store();

    let project_select = View::from_dynamic(move || {
        let projects = store.with(|state| state.projects.clone());
        let selected = store.with(|state| state.project_id.clone());
        let options: Vec<View> = projects
            .into_iter()
            .map(|project| {
                let id = project.id.clone();
                view! { option(value=id) { (project.name.clone()) } }
            })
            .collect();
        view! {
            select(
                class="select",
                value=selected,
                on:change=move |event: web_sys::Event| {
                    if let Some(target) = event.target() {
                        let select = target.unchecked_into::<HtmlSelectElement>();
                        state::change_project(store, select.value());
                    }
                },
            ) {
                (options)
            }
        }
    });

    let role_line = View::from_dynamic(move || -> View {
        let (role, org) = store.with(|state| {
            state::project(state)
                .map(|project| (project.role.clone(), project.org_name.clone()))
                .unwrap_or((String::new(), String::new()))
        });
        if role.is_empty() {
            view! { span(class="muted") { "no project access" } }
        } else {
            let badge = util::role_badge_class(&role);
            let role_text = role.clone();
            let org_text = org.clone();
            view! {
                span(class=badge) { (role_text) }
                span { (org_text) }
            }
        }
    });

    let user_pill = View::from_dynamic(move || -> View {
        match store.with(|state| state.me.clone()) {
            Some(user) => {
                let name = user.name.clone();
                let email = user.email.clone();
                let avatar_view = avatar(&name);
                view! {
                    div(class="user-pill") {
                        (avatar_view)
                        div(class="user-meta") {
                            strong { (name.clone()) }
                            small { (email.clone()) }
                        }
                    }
                }
            }
            None => View::new(),
        }
    });

    let nav_class = move |target: &'static str| {
        move || {
            let active = store.with(|state| util::active_nav(&state.view) == target);
            if active {
                "nav-link active".to_string()
            } else {
                "nav-link".to_string()
            }
        }
    };

    let settings_class = move || {
        let (active, manage) = store.with(|state| {
            (
                util::active_nav(&state.view) == "settings",
                state::can_manage(state),
            )
        });
        let mut class = String::from("nav-link");
        if active {
            class.push_str(" active");
        }
        if !manage {
            class.push_str(" hidden");
        }
        class
    };

    let orgs_class = move || {
        let (active, operator) = store.with(|state| {
            (
                util::active_nav(&state.view) == "orgs",
                state::is_operator(state),
            )
        });
        let mut class = String::from("nav-link");
        if active {
            class.push_str(" active");
        }
        if !operator {
            class.push_str(" hidden");
        }
        class
    };

    let title = View::from_dynamic(move || {
        let (heading, subtitle) = store.with(|state| (title_for(state), subtitle_for(state)));
        view! {
            div {
                h2 { (heading) }
                p { (subtitle) }
            }
        }
    });

    let back = View::from_dynamic(move || -> View {
        if store.with(|state| state.view == ViewKind::Issue) {
            view! {
                button(
                    class="icon-btn",
                    r#type="button",
                    aria-label="Back to issues",
                    on:click=move |_| state::navigate(store, "#/issues"),
                ) {
                    "←"
                }
            }
        } else {
            View::new()
        }
    });

    let actions = View::from_dynamic(move || -> View {
        let (view, can_send) = store.with(|state| (state.view, state::can_send(state)));
        match view {
            ViewKind::Issue => view! {
                a(href="#/issues", class="btn btn-ghost btn-sm") { "← All issues" }
            },
            ViewKind::Issues if can_send => view! {
                button(
                    class="btn btn-primary",
                    r#type="button",
                    on:click=move |_| state::open_modal(store, ModalKind::SendEvent),
                ) {
                    "＋ Send test event"
                }
            },
            ViewKind::Logs if can_send => view! {
                button(
                    class="btn",
                    r#type="button",
                    on:click=move |_| state::open_modal(store, ModalKind::SendLog),
                ) {
                    "＋ Send test log"
                }
            },
            _ => View::new(),
        }
    });

    let view = create_selector(move || store.with(|state| state.view));
    let main_view = View::from_dynamic(move || -> View {
        match view.get() {
            ViewKind::Issues => view! { issues::IssuesView() },
            ViewKind::Issue => view! { issues::IssueDetail() },
            ViewKind::Logs => view! { other::LogsView() },
            ViewKind::Search => view! { other::SearchView() },
            ViewKind::Members => view! { other::MembersView() },
            ViewKind::Settings => view! { other::SettingsView() },
            ViewKind::Tokens => view! { tokens::TokensView() },
            ViewKind::Account => view! { other::AccountView() },
            ViewKind::Orgs => view! { other::OrgsView() },
        }
    });

    view! {
        div(class="shell") {
            aside(class="sidebar") {
                a(href="#/issues", class="brand") {
                    (brand())
                    div {
                        strong { "Scout" }
                        small { "self-hosted telemetry" }
                    }
                }
                div(class="switch-block") {
                    label { "Project" }
                    (project_select)
                    div(class="role-line") { (role_line) }
                }
                nav(class="nav") {
                    a(href="#/issues", class=nav_class("issues")) {
                        span(class="dot") {}
                        "Issues"
                    }
                    a(href="#/logs", class=nav_class("logs")) {
                        span(class="dot") {}
                        "Logs"
                    }
                    a(href="#/search", class=nav_class("search")) {
                        span(class="dot") {}
                        "Search"
                    }
                    a(href="#/members", class=nav_class("members")) {
                        span(class="dot") {}
                        "Members"
                    }
                    a(href="#/settings", class=settings_class) {
                        span(class="dot") {}
                        "Settings"
                    }
                    a(href="#/tokens", class=nav_class("tokens")) {
                        span(class="dot") {}
                        "API tokens"
                    }
                    a(href="#/account", class=nav_class("account")) {
                        span(class="dot") {}
                        "Account"
                    }
                    a(href="#/orgs", class=orgs_class) {
                        span(class="dot") {}
                        "Organisations"
                    }
                }
                div(class="sidebar-foot") {
                    (user_pill)
                    button(
                        class="btn btn-ghost btn-sm",
                        r#type="button",
                        on:click=move |_| spawn_local(state::logout(store)),
                    ) {
                        "Sign out"
                    }
                    div(class="build-note") { "prototype · in-memory store" }
                }
            }
            div(class="main") {
                header(class="topbar") {
                    div(class="topbar-title") {
                        (back)
                        (title)
                    }
                    div(class="topbar-actions") { (actions) }
                }
                main(id="view") { (main_view) }
            }
        }
    }
}

fn title_for(state: &AppState) -> String {
    match state.view {
        ViewKind::Issues => "Issues".into(),
        ViewKind::Issue => state
            .issue
            .as_ref()
            .map(|detail| util::truncate(&detail.issue.title, 70))
            .unwrap_or_else(|| "Issue".into()),
        ViewKind::Logs => "Logs".into(),
        ViewKind::Search => "Search".into(),
        ViewKind::Members => "Members".into(),
        ViewKind::Settings => "Settings".into(),
        ViewKind::Tokens => "API tokens".into(),
        ViewKind::Account => "Account".into(),
        ViewKind::Orgs => "Organisations".into(),
    }
}

fn subtitle_for(state: &AppState) -> String {
    let name = state::project_name(state);
    match state.view {
        ViewKind::Issues => format!(
            "{} · {} open",
            name,
            state
                .projects
                .iter()
                .find(|project| project.id == state.project_id)
                .map(|project| project.open_issues)
                .unwrap_or(0)
        ),
        ViewKind::Issue => state
            .issue
            .as_ref()
            .map(|detail| {
                format!(
                    "{} · {} · {}",
                    name,
                    detail.issue.environment.as_deref().unwrap_or("unknown env"),
                    detail.issue.release.as_deref().unwrap_or("no release")
                )
            })
            .unwrap_or_else(|| name.clone()),
        ViewKind::Logs => format!("{name} · searchable beside issues"),
        ViewKind::Search => format!("{name} · issues and logs in one query"),
        ViewKind::Members => format!("{name} · project-scoped roles"),
        ViewKind::Settings => format!("{name} · ingest, retention, decision model"),
        ViewKind::Tokens => "Long-lived bearer tokens for /api/0/".into(),
        ViewKind::Account => "Password, recovery codes, and locked accounts".into(),
        ViewKind::Orgs => "Deployment-level provisioning".into(),
    }
}
