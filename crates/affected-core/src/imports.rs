use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, BinaryOperator, CallExpression, Expression, ImportExpression, NewExpression,
    TSImportEqualsDeclaration, TSImportType, TSModuleReference, TemplateLiteral,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

/// How a request names its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A module specifier, resolved like `import` resolves it.
    Module,
    /// A path relative to the importing file's directory: a `/// <reference path>`, an asset
    /// read through `join(__dirname, …)` or `new URL(…, import.meta.url)`. A directory stands
    /// for everything below it.
    File,
    /// A glob relative to the importing file's directory: `import.meta.glob`,
    /// `require.context`, or a template-literal `import()` with substitutions.
    Glob,
}

/// One module request found in a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Specifier {
    pub(crate) request: Box<str>,
    pub(crate) type_only: bool,
    pub(crate) kind: Kind,
}

/// Single-file components whose `<script>` blocks are parsed as TypeScript.
const COMPONENT_EXTENSIONS: [&str; 3] = ["vue", "svelte", "astro"];

/// Test-runner calls whose first argument names a module.
const MOCK_OBJECTS: [&str; 2] = ["jest", "vi"];
const MOCK_METHODS: [&str; 8] =
    ["mock", "doMock", "unmock", "requireActual", "requireMock", "importActual", "importMock", "createMockFromModule"];

fn is_component(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| COMPONENT_EXTENSIONS.contains(&extension))
}

/// Whether a path is parsed for imports or kept as a leaf of the graph.
pub(crate) fn is_parseable(path: &Path) -> bool {
    SourceType::from_path(path).is_ok() || is_component(path)
}

/// Every module request in `source`: static imports and re-exports, `import()`,
/// `require()`, `import x = require()`, `import('…')` types, triple-slash references,
/// test-runner mocks, globs and asset paths.
pub(crate) fn extract(path: &Path, source: &str) -> Vec<Specifier> {
    if is_component(path) {
        return component_scripts(source)
            .flat_map(|(script, source_type)| extract_script(script, source_type))
            .collect();
    }
    match SourceType::from_path(path) {
        Ok(source_type) => extract_script(source, source_type),
        Err(_) => Vec::new(),
    }
}

/// The `<script>` blocks of a component, and an Astro component's `---` frontmatter.
fn component_scripts(source: &str) -> impl Iterator<Item = (&str, SourceType)> {
    let typescript = SourceType::ts();
    let frontmatter = source
        .strip_prefix("---")
        .and_then(|rest| rest.find("\n---").and_then(|end| rest.get(..end)))
        .map(|block| (block, typescript));
    let mut rest = source;
    let blocks = std::iter::from_fn(move || {
        let open = rest.find("<script")?;
        let after_tag = rest.get(open..)?;
        let body_start = after_tag.find('>')? + 1;
        let tag = after_tag.get(..body_start)?;
        let body = after_tag.get(body_start..)?;
        let body_end = body.find("</script>")?;
        rest = body.get(body_end..)?;
        let source_type = if tag.contains("lang=\"js\"") || tag.contains("lang='js'") {
            SourceType::mjs()
        } else if tag.contains("tsx") {
            SourceType::tsx()
        } else {
            typescript
        };
        Some((body.get(..body_end)?, source_type))
    });
    frontmatter.into_iter().chain(blocks)
}

fn extract_script(source: &str, source_type: SourceType) -> Vec<Specifier> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type)
        .with_options(ParseOptions { parse_regular_expression: false, ..ParseOptions::default() })
        .parse();

    let mut found = Vec::with_capacity(parsed.module_record.requested_modules.len());
    for (request, occurrences) in &parsed.module_record.requested_modules {
        found.push(Specifier {
            request: request.as_str().into(),
            type_only: occurrences.iter().all(|occurrence| occurrence.is_type),
            kind: Kind::Module,
        });
    }

    for comment in parsed.program.comments.iter().filter(|comment| comment.is_line()) {
        let span = comment.content_span();
        if let Some(specifier) = source.get(span.start as usize..span.end as usize).and_then(triple_slash) {
            found.push(specifier);
        }
    }

    if needs_ast_walk(source) {
        let mut visitor = Collector { found: &mut found };
        visitor.visit_program(&parsed.program);
    }
    found
}

/// `/// <reference path="…" />` and `/// <reference types="…" />`; `text` follows the `//`.
fn triple_slash(text: &str) -> Option<Specifier> {
    let reference = text.strip_prefix('/')?.trim_start().strip_prefix("<reference")?;
    let attribute = |name: &str| {
        let value = reference.split_once(&format!("{name}="))?.1;
        let quote = value.chars().next().filter(|quote| *quote == '"' || *quote == '\'')?;
        value.get(1..)?.split(quote).next()
    };
    let (request, kind) = match (attribute("path"), attribute("types")) {
        (Some(path), _) => (path, Kind::File),
        (None, Some(types)) => (types, Kind::Module),
        (None, None) => return None,
    };
    Some(Specifier { request: request.into(), type_only: true, kind })
}

fn needs_ast_walk(source: &str) -> bool {
    ["require", "import(", "import (", "import.meta", "__dirname", "mock", "Mock", "Actual"]
        .iter()
        .any(|needle| source.contains(needle))
}

struct Collector<'s> {
    found: &'s mut Vec<Specifier>,
}

/// The text of a string literal, or of a template literal without substitutions.
/// `None` also covers literals with lone surrogates, which no file path can match.
fn literal<'e>(expression: &'e Expression<'_>) -> Option<&'e str> {
    match expression.get_inner_expression() {
        Expression::StringLiteral(literal) => literal.value.as_str(),
        Expression::TemplateLiteral(template) => template.single_quasi().and_then(|text| text.as_str()),
        _ => None,
    }
}

fn argument_literal<'e>(argument: Option<&'e Argument<'_>>) -> Option<&'e str> {
    literal(argument?.as_expression()?)
}

/// A dynamic path as a glob: `` `./locales/${l}.json` `` → `./locales/*.json`,
/// `'./pages/' + name` → `./pages/*`. Only relative paths with a static directory qualify,
/// since anything else could name any module.
fn dynamic_glob(expression: &Expression<'_>) -> Option<String> {
    let pattern = match expression.get_inner_expression() {
        Expression::TemplateLiteral(template) => template_glob(template)?,
        Expression::BinaryExpression(binary) if binary.operator == BinaryOperator::Addition => {
            let mut pattern = literal(&binary.left).map(str::to_owned).or_else(|| dynamic_glob(&binary.left))?;
            pattern.push('*');
            if let Some(suffix) = literal(&binary.right) {
                pattern.push_str(suffix);
            }
            pattern
        }
        _ => return None,
    };
    let relative = pattern.starts_with("./") || pattern.starts_with("../");
    let static_dir = pattern.split('*').next().is_some_and(|prefix| prefix.trim_start_matches("./").contains('/'));
    (relative && static_dir).then(|| collapse_stars(&pattern))
}

fn template_glob(template: &TemplateLiteral<'_>) -> Option<String> {
    let mut pattern = String::new();
    for (index, quasi) in template.quasis.iter().enumerate() {
        if index > 0 {
            pattern.push('*');
        }
        pattern.push_str(quasi.value.cooked?.as_str()?);
    }
    Some(pattern)
}

/// `**` stays a recursive wildcard; runs of single stars from adjacent substitutions fold.
fn collapse_stars(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut stars = 0;
    for character in pattern.chars().chain(std::iter::once('\0')) {
        if character == '*' {
            stars += 1;
            continue;
        }
        out.push_str(match stars {
            0 => "",
            1 => "*",
            _ => "**",
        });
        stars = 0;
        if character != '\0' {
            out.push(character);
        }
    }
    out
}

fn is_dirname(expression: &Expression<'_>) -> bool {
    expression.is_specific_id("__dirname") || is_import_meta(expression, "dirname")
}

fn is_import_meta(expression: &Expression<'_>, property: &str) -> bool {
    match expression.get_inner_expression() {
        Expression::StaticMemberExpression(member) => {
            matches!(member.object.get_inner_expression(), Expression::ImportMeta(_))
                && member.property.name == property
        }
        _ => false,
    }
}

impl Collector<'_> {
    fn push(&mut self, request: impl Into<Box<str>>, kind: Kind) {
        self.found.push(Specifier { request: request.into(), type_only: false, kind });
    }

    fn push_module(&mut self, request: Option<&str>) {
        if let Some(request) = request {
            self.push(request, Kind::Module);
        }
    }

    /// `join(__dirname, 'a', 'b')` / `path.resolve(import.meta.dirname, 'a')` with literal segments.
    fn asset_path(&mut self, call: &CallExpression<'_>) {
        let callee = call.callee.get_inner_expression();
        let is_path_call = ["join", "resolve"]
            .iter()
            .any(|name| callee.is_specific_id(*name) || callee.is_specific_member_access("path", name));
        let Some((first, rest)) = call.arguments.split_first() else { return };
        if !is_path_call || rest.is_empty() || !first.as_expression().is_some_and(is_dirname) {
            return;
        }
        let segments: Option<Vec<&str>> = rest.iter().map(|argument| argument_literal(Some(argument))).collect();
        if let Some(segments) = segments {
            let mut request = String::from(".");
            for segment in segments {
                request.push('/');
                request.push_str(segment.trim_start_matches("./"));
            }
            self.push(request, Kind::File);
        }
    }

    /// `import.meta.glob('./a/*.ts')`, `import.meta.glob(['./a/*', '!./a/skip.ts'])`.
    /// Negated patterns are ignored, which can only widen the match.
    fn import_meta_glob(&mut self, call: &CallExpression<'_>) {
        let Some(argument) = call.arguments.first().and_then(Argument::as_expression) else { return };
        let patterns: Vec<&str> = match argument.get_inner_expression() {
            Expression::ArrayExpression(array) => {
                array.elements.iter().filter_map(ArrayExpressionElement::as_expression).filter_map(literal).collect()
            }
            other => literal(other).into_iter().collect(),
        };
        for pattern in patterns.into_iter().filter(|pattern| !pattern.starts_with('!')) {
            self.push(pattern, Kind::Glob);
        }
    }

    /// `require.context('./dir', recursive = true, filter)`; the filter is ignored, which can only widen the match.
    fn require_context(&mut self, call: &CallExpression<'_>) {
        let Some(dir) = argument_literal(call.arguments.first()) else { return };
        let recursive = !matches!(
            call.arguments.get(1).and_then(Argument::as_expression).map(Expression::get_inner_expression),
            Some(Expression::BooleanLiteral(flag)) if !flag.value
        );
        let dir = dir.trim_end_matches('/');
        self.push(format!("{dir}/{}", if recursive { "**/*" } else { "*" }), Kind::Glob);
    }
}

impl<'a> Visit<'a> for Collector<'_> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let callee = it.callee.get_inner_expression();
        if let Some(literal) = it.common_js_require() {
            self.push_module(literal.value.as_str());
        } else if callee.is_specific_id("require") {
            if let Some(pattern) = it.arguments.first().and_then(Argument::as_expression).and_then(dynamic_glob) {
                self.push(pattern, Kind::Glob);
            }
        } else if callee.is_specific_member_access("require", "resolve") {
            self.push_module(argument_literal(it.arguments.first()));
        } else if callee.is_specific_member_access("require", "context") {
            self.require_context(it);
        } else if is_import_meta(callee, "glob") {
            self.import_meta_glob(it);
        } else if MOCK_OBJECTS
            .iter()
            .any(|object| MOCK_METHODS.iter().any(|method| callee.is_specific_member_access(object, method)))
        {
            self.push_module(argument_literal(it.arguments.first()));
        } else {
            self.asset_path(it);
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        let base_is_module_url =
            it.arguments.get(1).and_then(Argument::as_expression).is_some_and(|base| is_import_meta(base, "url"));
        if it.callee.is_specific_id("URL")
            && base_is_module_url
            && let Some(request) = argument_literal(it.arguments.first()).filter(|request| request.starts_with('.'))
        {
            self.push(request, Kind::File);
        }
        walk::walk_new_expression(self, it);
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        if let Some(request) = literal(&it.source) {
            self.push(request, Kind::Module);
        } else if let Some(pattern) = dynamic_glob(&it.source) {
            self.push(pattern, Kind::Glob);
        }
        walk::walk_import_expression(self, it);
    }

    fn visit_ts_import_equals_declaration(&mut self, it: &TSImportEqualsDeclaration<'a>) {
        if let TSModuleReference::ExternalModuleReference(reference) = &it.module_reference
            && let Some(request) = reference.expression.value.as_str()
        {
            self.found.push(Specifier {
                request: request.into(),
                type_only: it.import_kind.is_type(),
                kind: Kind::Module,
            });
        }
        walk::walk_ts_import_equals_declaration(self, it);
    }

    fn visit_ts_import_type(&mut self, it: &TSImportType<'a>) {
        if let Some(request) = it.source.value.as_str() {
            self.found.push(Specifier { request: request.into(), type_only: true, kind: Kind::Module });
        }
        walk::walk_ts_import_type(self, it);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requests(path: &str, source: &str) -> Vec<(String, bool)> {
        let mut found: Vec<_> = extract(Path::new(path), source)
            .into_iter()
            .filter(|s| s.kind == Kind::Module)
            .map(|s| (s.request.into_string(), s.type_only))
            .collect();
        found.sort();
        found
    }

    fn of_kind(path: &str, source: &str, kind: Kind) -> Vec<String> {
        let mut found: Vec<_> = extract(Path::new(path), source)
            .into_iter()
            .filter(|s| s.kind == kind)
            .map(|s| s.request.into_string())
            .collect();
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

    #[test]
    fn test_runner_mocks_and_require_resolve_name_modules() {
        let source =
            "jest.mock('./a')\nvi.mock('./b', () => ({}))\nconst c = jest.requireActual('./c')\nrequire.resolve('./d')";
        let names: Vec<String> = requests("x.test.ts", source).into_iter().map(|(request, _)| request).collect();
        assert_eq!(names, ["./a", "./b", "./c", "./d"]);
    }

    #[test]
    fn triple_slash_references() {
        let source =
            "/// <reference path=\"./globals.d.ts\" />\n/// <reference types='node' />\n// <reference path=\"no\" />";
        assert_eq!(of_kind("x.ts", source, Kind::File), ["./globals.d.ts"]);
        assert_eq!(requests("x.ts", source), [("node".to_owned(), true)]);
    }

    #[test]
    fn dynamic_paths_become_globs() {
        let source = r#"
            import(`./locales/${lang}.json`)
            import('./pages/' + name)
            require(`../a/${x}/${y}.ts`)
            import.meta.glob(['./routes/**/*.tsx', '!./routes/skip.tsx'])
            import.meta.glob('./one/*.ts', { eager: true })
            require.context('./icons', false, /\.svg$/)
            require.context('./deep')
            import(`${anything}`)
            import(`./${x}`)
        "#;
        assert_eq!(
            of_kind("x.ts", source, Kind::Glob),
            [
                "../a/*/*.ts",
                "./deep/**/*",
                "./icons/*",
                "./locales/*.json",
                "./one/*.ts",
                "./pages/*",
                "./routes/**/*.tsx"
            ]
        );
    }

    #[test]
    fn asset_paths_relative_to_the_module() {
        let source = r#"
            readFileSync(join(__dirname, 'prompt.md'))
            fs.readFileSync(path.resolve(import.meta.dirname, './templates', 'a.hbs'))
            new Worker(new URL('./worker.ts', import.meta.url))
            join(__dirname, name)
            join(root, 'x')
            new URL('https://example.com', import.meta.url)
        "#;
        assert_eq!(of_kind("x.ts", source, Kind::File), ["./prompt.md", "./templates/a.hbs", "./worker.ts"]);
    }

    #[test]
    fn component_script_blocks() {
        let vue = "<template><div/></template>\n<script setup lang=\"ts\">import { x } from './x'</script>\n<script>import y from './y.js'</script>";
        assert_eq!(requests("App.vue", vue), [("./x".to_owned(), false), ("./y.js".to_owned(), false)]);
        let astro = "---\nimport Layout from '../layouts/Base.astro'\n---\n<Layout />";
        assert_eq!(requests("page.astro", astro), [("../layouts/Base.astro".to_owned(), false)]);
        assert!(is_parseable(Path::new("Comp.svelte")));
    }
}
