//! Inventory is extracted from Rust router syntax and the API tables, never from a copied snapshot.
use super::REGISTRIES;
use crate::cos::operations::{match_operation, path_matches};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use syn::visit::{self, Visit};
use syn::{Expr, ItemConst, Lit};

type Route = (String, String);

fn normalized(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') && segment.ends_with('}') {
                "{}"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn route(method: &str, path: &str) -> Route {
    (method.to_string(), normalized(path))
}

fn mutation(name: &str) -> Option<&'static str> {
    match name {
        "post" => Some("POST"),
        "put" | "put_route" => Some("PUT"),
        "patch" => Some("PATCH"),
        "delete" => Some("DELETE"),
        _ => None,
    }
}

#[derive(Default)]
struct Constants(BTreeMap<String, String>);
impl<'ast> Visit<'ast> for Constants {
    fn visit_item_const(&mut self, item: &'ast ItemConst) {
        if let Expr::Lit(value) = item.expr.as_ref()
            && let Lit::Str(value) = &value.lit
        {
            self.0.insert(item.ident.to_string(), value.value());
        }
        visit::visit_item_const(self, item);
    }
}

#[derive(Default)]
struct Methods(BTreeSet<&'static str>);
impl<'ast> Visit<'ast> for Methods {
    fn visit_expr_call(&mut self, expr: &'ast syn::ExprCall) {
        if let Expr::Path(path) = expr.func.as_ref()
            && let Some(last) = path.path.segments.last()
            && let Some(method) = mutation(&last.ident.to_string())
        {
            self.0.insert(method);
        }
        visit::visit_expr_call(self, expr);
    }
    fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
        if let Some(method) = mutation(&expr.method.to_string()) {
            self.0.insert(method);
        }
        visit::visit_expr_method_call(self, expr);
    }
}

struct Routers {
    constants: Constants,
    routes: BTreeSet<Route>,
}
impl Routers {
    fn path(&self, expr: &Expr) -> String {
        match expr {
            Expr::Lit(value) => match &value.lit {
                Lit::Str(value) => value.value(),
                _ => panic!("route path is not a string"),
            },
            Expr::Reference(value) => self.path(&value.expr),
            Expr::Path(value) => self.constants.0[&value
                .path
                .segments
                .last()
                .expect("constant")
                .ident
                .to_string()]
                .clone(),
            Expr::Macro(value) if value.mac.path.is_ident("format") => {
                let format = syn::parse2::<syn::LitStr>(value.mac.tokens.clone())
                    .expect("route format literal")
                    .value();
                let mut expanded = format.clone();
                for (name, value) in &self.constants.0 {
                    expanded = expanded.replace(&format!("{{{name}}}"), value);
                }
                assert_ne!(expanded, format, "unsupported dynamic route: {format}");
                expanded
            }
            _ => panic!("unsupported route syntax: update the inventory extractor"),
        }
    }
}
impl<'ast> Visit<'ast> for Routers {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        // Inline test routers are fixtures, not API endpoints.
        if item.ident != "tests"
            && !item.attrs.iter().any(|attr| {
                attr.path().is_ident("cfg")
                    && attr
                        .meta
                        .require_list()
                        .is_ok_and(|list| list.tokens.to_string() == "test")
            })
        {
            visit::visit_item_mod(self, item);
        }
    }
    fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
        if expr.method == "route" {
            let path = self.path(expr.args.first().expect("route path"));
            let mut methods = Methods::default();
            methods.visit_expr(expr.args.iter().nth(1).expect("method router"));
            if path.starts_with("/api/v1/") {
                for method in methods.0 {
                    self.routes.insert(route(method, &path));
                }
            }
        }
        visit::visit_expr_method_call(self, expr);
    }
}

fn router_inventory(dir: &Path, routes: &mut BTreeSet<Route>) {
    for entry in std::fs::read_dir(dir).expect("source directory") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            if path.file_name().expect("name") != "tests" {
                router_inventory(&path, routes);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs")
            && !path
                .file_name()
                .expect("name")
                .to_string_lossy()
                .ends_with("_tests.rs")
        {
            let source = std::fs::read_to_string(&path).expect("Rust source");
            let ast = syn::parse_file(&source)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let mut constants = Constants::default();
            constants.visit_file(&ast);
            let mut visitor = Routers {
                constants,
                routes: BTreeSet::new(),
            };
            visitor.visit_file(&ast);
            routes.extend(visitor.routes);
        }
    }
}

fn api_table_inventory(text: &str) -> BTreeSet<Route> {
    let mut routes = BTreeSet::new();
    // Endpoint summary and its supplemental browser control table precede the endpoint details.
    for line in text
        .split("## 3. 各エンドポイント")
        .next()
        .expect("API tables")
        .lines()
    {
        let cells: Vec<_> = line.split('|').map(str::trim).collect();
        for (index, cell) in cells.iter().enumerate() {
            if matches!(*cell, "POST" | "PUT" | "PATCH" | "DELETE")
                && let Some(path) = cells
                    .get(index + 1)
                    .and_then(|cell| cell.strip_prefix('`'))
                    .and_then(|cell| cell.strip_suffix('`'))
                && path.starts_with('/')
            {
                routes.insert(route(cell, &format!("/api/v1{path}")));
            }
        }
    }
    routes
}

#[test]
fn cos_ops_registry_classifies_every_mutation_as_allowed_or_excluded() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut expected = BTreeSet::new();
    router_inventory(&root.join("crates/task-api/src"), &mut expected);
    assert!(
        !expected.is_empty(),
        "router extractor returned no mutations"
    );
    let documented = api_table_inventory(
        &std::fs::read_to_string(root.join("docs/api/v1/gui-api.md")).expect("API docs"),
    );
    assert!(
        !documented.is_empty(),
        "API table extractor returned no mutations"
    );
    expected.extend(documented);
    let mut registered = BTreeMap::new();
    for registry in REGISTRIES {
        for (kind, rows) in [
            (
                "ALLOWED",
                registry
                    .allowed
                    .iter()
                    .map(|(m, p, _)| (*m, *p))
                    .collect::<Vec<_>>(),
            ),
            (
                "EXCLUDED",
                registry.excluded.iter().map(|(m, p, _)| (*m, *p)).collect(),
            ),
        ] {
            for (method, path) in rows {
                let key = route(method, path);
                let owner = format!("{}::{kind}", registry.name);
                assert!(
                    registered.insert(key.clone(), owner.clone()).is_none(),
                    "duplicate classification {key:?} at {owner}"
                );
            }
        }
    }
    assert_eq!(
        expected,
        registered.keys().cloned().collect(),
        "every mutation must be ALLOWED or EXCLUDED (no pending), and every row must name an endpoint"
    );
}

#[test]
fn cos_ops_every_allowed_row_is_reachable_through_match_operation() {
    for registry in REGISTRIES {
        for (method, pattern, _) in registry.allowed {
            let path = pattern
                .split('/')
                .map(|segment| {
                    if segment.starts_with('{') {
                        "example"
                    } else {
                        segment
                    }
                })
                .collect::<Vec<_>>()
                .join("/");
            assert!(
                match_operation(method, &path).is_ok(),
                "{}: {method} {pattern} is ALLOWED but not reachable",
                registry.name
            );
        }
    }
}

#[test]
fn cos_ops_registry_matches_adr_domain_assignment_and_exclusions() {
    let text =
        include_str!("../../../../../agent-docs/adr/2026-10-09-cos-operations-all-mutations.md");
    let mut assignments = BTreeMap::new();
    let mut reasons = BTreeMap::new();
    let mut domain = None;
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("### ") {
            domain = REGISTRIES
                .iter()
                .find(|registry| registry.name == heading)
                .map(|registry| registry.name);
        }
        let cells: Vec<_> = line.split('|').map(str::trim).collect();
        if cells.len() > 4 && matches!(cells[1], "POST" | "PUT" | "PATCH" | "DELETE") {
            let key = route(cells[1], &format!("/api/v1{}", cells[2].trim_matches('`')));
            if let Some(domain) = domain {
                assert!(
                    assignments.insert(key, domain).is_none(),
                    "duplicate ADR assignment"
                );
            } else if cells.len() == 6 {
                reasons.insert(key, format!("{}: {}", cells[3].trim_matches('`'), cells[4]));
            }
        }
    }
    assert!(!assignments.is_empty());
    let mut actual = BTreeMap::new();
    let mut exclusions = BTreeMap::new();
    for registry in REGISTRIES {
        for (method, path) in registry
            .allowed
            .iter()
            .map(|(m, p, _)| (*m, *p))
            .chain(registry.excluded.iter().map(|(m, p, _)| (*m, *p)))
        {
            actual.insert(route(method, path), registry.name);
        }
        for (method, path, reason) in registry.excluded {
            exclusions.insert(route(method, path), reason.to_string());
        }
    }
    assert_eq!(actual, assignments);
    assert_eq!(exclusions, reasons);
}

#[test]
fn cos_ops_exclusions_match_all_named_parameters_and_report_adr_reason() {
    for registry in REGISTRIES {
        for (method, pattern, reason) in registry.excluded {
            let path = pattern
                .split('/')
                .map(|segment| {
                    if segment.starts_with('{') {
                        "example"
                    } else {
                        segment
                    }
                })
                .collect::<Vec<_>>()
                .join("/");
            assert_eq!(
                match_operation(method, &path).expect_err("excluded"),
                *reason
            );
        }
    }
    assert!(path_matches("/a/{one}/{two}", "/a/id-1/id_2"));
    assert!(!path_matches("/a/{one}", "/a/one/two"));
    assert!(!path_matches("/a/{one}", "/a/"));
    assert!(!path_matches("/a/{one}", "/a/%2F"));
    assert!(match_operation("GET", "/api/v1/tasks").is_err());
    assert_eq!(
        match_operation("post", "/api/v1/tasks")
            .expect("case insensitive")
            .action,
        "task.create"
    );
}
