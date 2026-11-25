use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    CallExpression, Expression, ImportExpression, TSImportEqualsDeclaration, TSImportType, TSModuleReference,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

/// One module request found in a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Specifier {
    pub(crate) request: Box<str>,
    pub(crate) type_only: bool,
}

/// Whether a path is parsed for imports or kept as a leaf of the graph.
pub(crate) fn is_parseable(path: &Path) -> bool {
    SourceType::from_path(path).is_ok()
}

/// Every module request in `source`: static imports and re-exports, `import()`,
/// `require()`, `import x = require()` and `import('…')` types.
pub(crate) fn extract(path: &Path, source: &str) -> Vec<Specifier> {
    let Ok(source_type) = SourceType::from_path(path) else {
        return Vec::new();
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type)
        .with_options(ParseOptions { parse_regular_expression: false, ..ParseOptions::default() })
        .parse();

    let mut found = Vec::with_capacity(parsed.module_record.requested_modules.len());
    for (request, occurrences) in &parsed.module_record.requested_modules {
        found.push(Specifier {
            request: request.as_str().into(),
            type_only: occurrences.iter().all(|occurrence| occurrence.is_type),
        });
    }

    if needs_ast_walk(source) {
        let mut visitor = Collector { found: &mut found };
        visitor.visit_program(&parsed.program);
    }
    found
}

fn needs_ast_walk(source: &str) -> bool {
    source.contains("require") || source.contains("import(") || source.contains("import (")
}

struct Collector<'s> {
    found: &'s mut Vec<Specifier>,
}

impl Collector<'_> {
    /// `None` is a literal with lone surrogates, which no file path can match.
    fn push(&mut self, request: Option<&str>, type_only: bool) {
        if let Some(request) = request {
            self.found.push(Specifier { request: request.into(), type_only });
        }
    }
}

impl<'a> Visit<'a> for Collector<'_> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Some(literal) = it.common_js_require() {
            self.push(literal.value.as_str(), false);
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        match &it.source {
            Expression::StringLiteral(literal) => self.push(literal.value.as_str(), false),
            Expression::TemplateLiteral(template) => {
                if let Some(text) = template.single_quasi() {
                    self.push(text.as_str(), false);
                }
            }
            _ => {}
        }
        walk::walk_import_expression(self, it);
    }

    fn visit_ts_import_equals_declaration(&mut self, it: &TSImportEqualsDeclaration<'a>) {
        if let TSModuleReference::ExternalModuleReference(reference) = &it.module_reference {
            self.push(reference.expression.value.as_str(), it.import_kind.is_type());
        }
        walk::walk_ts_import_equals_declaration(self, it);
    }

    fn visit_ts_import_type(&mut self, it: &TSImportType<'a>) {
        self.push(it.source.value.as_str(), true);
        walk::walk_ts_import_type(self, it);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requests(path: &str, source: &str) -> Vec<(String, bool)> {
        let mut found: Vec<_> =
            extract(Path::new(path), source).into_iter().map(|s| (s.request.into_string(), s.type_only)).collect();
        found.sort();
        found
    }

    #[test]
    fn collects_every_module_request_form() {
        let source = r#"
            import a from './a'
            import type { B } from './b'
            import { type C } from './c'
            export * from './d'
            export { e } from './e'
            export type { F } from './f'
            const g = await import('./g')
            const h = import(`./h`)
            const i = require('./i')
            import j = require('./j')
            type K = import('./k').K
            const dynamic = import(name)
        "#;
        assert_eq!(
            requests("x.ts", source),
            [
                ("./a", false),
                ("./b", true),
                ("./c", false),
                ("./d", false),
                ("./e", false),
                ("./f", true),
                ("./g", false),
                ("./h", false),
                ("./i", false),
                ("./j", false),
                ("./k", true),
            ]
            .map(|(request, type_only)| (request.to_owned(), type_only))
        );
    }

    #[test]
    fn a_value_import_wins_over_a_type_import_of_the_same_module() {
        assert_eq!(
            requests("x.ts", "import type { A } from './a'\nimport { b } from './a'"),
            [("./a".to_owned(), false)]
        );
    }

    #[test]
    fn parses_jsx_and_skips_unknown_extensions() {
        assert_eq!(
            requests("x.tsx", "import a from './a'\nexport const X = () => <div />"),
            [("./a".to_owned(), false)]
        );
        assert!(requests("x.json", "{}").is_empty());
        assert!(!is_parseable(Path::new("styles.css")));
    }

    #[test]
    fn survives_syntax_errors() {
        assert_eq!(requests("x.ts", "import a from './a'\nconst = ;"), [("./a".to_owned(), false)]);
    }
}
