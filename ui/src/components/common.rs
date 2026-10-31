use crate::util;
use sycamore::prelude::*;

pub fn level_badge(level: &str) -> View {
    let class = util::level_badge_class(level);
    let label = level.to_string();
    view! { span(class=class) { (label) } }
}

pub fn state_badge(state: &str) -> View {
    let class = util::state_badge_class(state);
    let label = state.to_string();
    view! { span(class=class) { (label) } }
}

pub fn role_badge(role: &str) -> View {
    let class = util::role_badge_class(role);
    let label = role.to_string();
    view! { span(class=class) { (label) } }
}

pub fn empty_state(title: &str, body: &str, action: View) -> View {
    let title = title.to_string();
    let body = body.to_string();
    view! {
        div(class="panel empty") {
            strong { (title) }
            span { (body) }
            (action)
        }
    }
}

pub fn section_title(title: &str, right: View) -> View {
    let title = title.to_string();
    view! {
        div(class="section-title") {
            h3 { (title) }
            (right)
        }
    }
}

pub fn brand() -> View {
    view! {
        span(class="brand-mark") { "∿" }
    }
}

pub fn avatar(name: &str) -> View {
    let initials = name
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .take(2)
        .map(|part| {
            part.chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string()
        })
        .collect::<String>();
    view! { span(class="avatar") { (initials) } }
}

pub fn skeleton(height: &str) -> View {
    let style = format!("height: {height}");
    view! { div(class="skeleton", style=style) {} }
}
