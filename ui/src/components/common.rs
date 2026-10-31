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

/// A failed request is a normal state: explain what failed and offer a retry
/// action instead of leaving the view blank.
#[allow(dead_code)] // consumed by the issue/log views as they gain request state
pub fn error_state(title: &str, body: &str, action: View) -> View {
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

/// Placeholder rows shown while a list query is in flight.
#[allow(dead_code)] // consumed by the issue/log views as they gain request state
pub fn loading_rows(count: usize, height: &str) -> View {
    let rows: Vec<View> = (0..count).map(|_| skeleton(height)).collect();
    view! {
        div(class="panel") { (rows) }
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
