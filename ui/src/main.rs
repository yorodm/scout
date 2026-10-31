mod api;
mod components;
mod state;
mod util;

use components::App;
use sycamore::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    sycamore::render(|| {
        view! { App() }
    });
}
