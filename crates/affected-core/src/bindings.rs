//! Binding information used only for runtime selection across two revisions.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::{ClassElement, Declaration, Expression, ImportDeclarationSpecifier, Program, Statement};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use rustc_hash::FxHashMap;

use crate::source::Source;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Binding {
    Names(Vec<String>),
    Star,
    Aliases(Vec<(String, String)>),
}

/// Only describe a request with one static declaration. Mixed or repeated requests
/// (including an additional side-effect import) keep their original file-wide edge.
pub(crate) fn requests(program: &Program<'_>) -> FxHashMap<String, Binding> {
    let barrel = program.directives.is_empty()
        && program
            .body
            .iter()
            .all(|s| matches!(s, Statement::ExportAllDeclaration(_) | Statement::ExportFromDeclaration(_)));
    let mut bindings = FxHashMap::default();
    let mut seen = BTreeSet::new();
    for stmt in &program.body {
        let (request, binding) = match stmt {
            Statement::ImportDeclaration(d) => {
                if d.phase.is_some() || d.with_clause.is_some() {
                    return FxHashMap::default();
                }
                let names = d.specifiers.as_ref().and_then(|specs| {
                    specs
                        .iter()
                        .map(|s| match s {
                            ImportDeclarationSpecifier::ImportSpecifier(i) => Some(i.imported.name().to_string()),
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => Some("default".to_owned()),
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => None,
                        })
                        .collect::<Option<Vec<_>>>()
                });
                (d.source.value.as_str(), names.filter(|n| !n.is_empty()).map(Binding::Names))
            }
            Statement::ExportAllDeclaration(d) => (
                d.source.value.as_str(),
                (barrel && d.exported.is_none() && d.with_clause.is_none()).then_some(Binding::Star),
            ),
            Statement::ExportFromDeclaration(d) => (
                d.source.value.as_str(),
                (barrel && !d.specifiers.is_empty() && d.with_clause.is_none()).then(|| {
                    Binding::Aliases(
                        d.specifiers
                            .iter()
                            .map(|s| (s.local.name().to_string(), s.exported.name().to_string()))
                            .collect(),
                    )
                }),
            ),
            _ => continue,
        };
        let Some(request) = request else { continue };
        if !seen.insert(request) {
            bindings.remove(request);
        } else if let Some(binding) = binding {
            bindings.insert(request.to_owned(), binding);
        }
    }
    bindings
}

#[derive(Default)]
struct Leaf {
    imports: Vec<String>,
    names: BTreeSet<String>,
    emission: Vec<String>,
}

#[derive(Default)]
struct LiteralSpans(Vec<Span>);

impl<'a> Visit<'a> for LiteralSpans {
    fn visit_expression(&mut self, expr: &Expression<'a>) {
        match expr {
            Expression::StringLiteral(_) | Expression::NumericLiteral(_) => self.0.push(expr.span()),
            _ => walk::walk_expression(self, expr),
        }
    }
}

// TypeScript can elide an unused value import. Prove its binding usage stayed
// unchanged by comparing declarations modulo literal values. An edit that may
// start or stop executing an imported module retains the file-wide result.
fn emission_shape(stmt: &Statement<'_>, source: &str) -> String {
    let mut literals = LiteralSpans::default();
    literals.visit_statement(stmt);
    let mut start = stmt.span().start as usize;
    let mut shape = String::new();
    for literal in literals.0 {
        shape.push_str(source.get(start..literal.start as usize).unwrap_or_default());
        shape.push('\0');
        start = literal.end as usize;
    }
    shape.push_str(source.get(start..stmt.span().end as usize).unwrap_or_default());
    shape
}

/// Whitelist declarations whose values can only be observed by a consumer.
/// Compare import declarations verbatim: changing one can execute new module code.
fn leaf(path: &Path, source: &str) -> Option<Leaf> {
    let allocator = Allocator::default();
    let kind = SourceType::from_path(path).ok()?;
    if kind.is_jsx() || kind.is_script() {
        return None;
    }
    let parsed = Parser::new(&allocator, source, kind).parse();
    if !parsed.diagnostics.is_empty() || !parsed.program.directives.is_empty() {
        return None;
    }
    let mut leaf = Leaf::default();
    let may_elide = kind.is_typescript()
        && parsed.program.body.iter().any(
            |s| matches!(s, Statement::ImportDeclaration(d) if !d.import_kind.is_type() && d.specifiers.is_some()),
        );
    for stmt in &parsed.program.body {
        if may_elide && matches!(stmt, Statement::ExportDeclaration(_)) {
            leaf.emission.push(emission_shape(stmt, source));
        }
        match stmt {
            Statement::ImportDeclaration(_) => leaf.imports.push(stmt.span().source_text(source).to_owned()),
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::ClassDeclaration(c)
                    if !c.declare
                        && c.decorators.is_empty()
                        && c.heritage.is_none()
                        && c.body.body.iter().all(|m| match m {
                            ClassElement::MethodDefinition(m) => {
                                !m.key.is_computed()
                                    && m.decorators.is_empty()
                                    && m.value.params.items.iter().all(|p| p.decorators.is_empty())
                            }
                            ClassElement::PropertyDefinition(p) => {
                                !p.key.is_computed() && p.decorators.is_empty() && !p.r#static
                            }
                            _ => false,
                        }) =>
                {
                    leaf.names.insert(c.id.as_ref()?.name.to_string());
                }
                Declaration::FunctionDeclaration(f) if f.body.is_some() && !f.declare => {
                    leaf.names.insert(f.id.as_ref()?.name.to_string());
                }
                Declaration::VariableDeclaration(d) if d.kind.is_const() && !d.declare => {
                    for var in &d.declarations {
                        if !matches!(
                            var.init,
                            Some(
                                Expression::NumericLiteral(_)
                                    | Expression::StringLiteral(_)
                                    | Expression::BooleanLiteral(_)
                                    | Expression::NullLiteral(_)
                                    | Expression::ArrowFunctionExpression(_)
                                    | Expression::FunctionExpression(_)
                            )
                        ) {
                            return None;
                        }
                        let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &var.id else {
                            return None;
                        };
                        leaf.names.insert(id.name.to_string());
                    }
                }
                _ => return None,
            },
            _ => return None,
        }
    }
    (!leaf.names.is_empty()).then_some(leaf)
}

pub(crate) type Changes = FxHashMap<PathBuf, Vec<String>>;

pub(crate) fn compare(head: &impl Source, base: &impl Source, paths: &[PathBuf]) -> Changes {
    paths
        .iter()
        .filter_map(|path| {
            if SourceType::from_path(path).is_err() {
                return None;
            }
            let before = leaf(path, &base.read_to_string(path).ok()?)?;
            let after = leaf(path, &head.read_to_string(path).ok()?)?;
            // Export shape changes can cause linking errors (including ambiguous star exports).
            (before.imports == after.imports && before.names == after.names && before.emission == after.emission)
                .then(|| (path.clone(), after.names.into_iter().collect()))
        })
        .collect()
}
