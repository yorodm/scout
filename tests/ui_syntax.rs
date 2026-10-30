use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;

#[test]
fn app_js_parses_without_syntax_errors() {
    let source = include_str!("../web/app.js");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::mjs()).parse();
    assert!(
        parsed.diagnostics.is_empty(),
        "app.js has syntax errors: {:?}",
        parsed.diagnostics
    );
}
