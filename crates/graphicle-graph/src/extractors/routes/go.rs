//! Gin registrations. Receiver bindings are local to each function/block;
//! unknown groups and reassigned receivers do not produce guessed paths.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
struct Group {
    app: String,
    path: String,
    middleware: Vec<String>,
}

fn string(n: Node, src: &[u8]) -> Option<String> {
    match n.kind() {
        "interpreted_string_literal" => serde_json::from_str(text(n, src)).ok(),
        "raw_string_literal" => Some(text(n, src).trim_matches('`').to_string()),
        _ => None,
    }
}

fn call<'a>(n: Node<'a>, src: &'a [u8]) -> Option<(&'a str, &'a str, Vec<Node<'a>>)> {
    if n.kind() != "call_expression" {
        return None;
    }
    let f = n.child_by_field_name("function")?;
    if f.kind() != "selector_expression" {
        return None;
    }
    let receiver = text(f.child_by_field_name("operand")?, src);
    let name = text(f.child_by_field_name("field")?, src);
    let args = n.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    Some((receiver, name, args.named_children(&mut cursor).collect()))
}

fn handler(n: Node, src: &[u8]) -> Option<String> {
    let n = if n.kind() == "call_expression" {
        n.child_by_field_name("function")?
    } else {
        n
    };
    match n.kind() {
        "identifier" => Some(text(n, src).to_string()),
        "selector_expression" => Some(text(n.child_by_field_name("field")?, src).to_string()),
        _ => None,
    }
}

fn walk(
    n: Node,
    src: &[u8],
    file: &str,
    aliases: &BTreeSet<String>,
    groups: &mut BTreeMap<String, Group>,
    routes: &mut Vec<Route>,
) {
    // A declaration starts its own receiver scope. Do not carry receivers
    // between functions with the same local variable names.
    if matches!(
        n.kind(),
        "function_declaration" | "method_declaration" | "func_literal"
    ) {
        if let Some(body) = n.child_by_field_name("body") {
            walk(body, src, file, aliases, &mut BTreeMap::new(), routes);
        }
        return;
    }
    if matches!(n.kind(), "short_var_declaration" | "assignment_statement") {
        if let (Some(left), Some(right)) = (
            n.child_by_field_name("left"),
            n.child_by_field_name("right"),
        ) {
            if left.named_child_count() == 1 && right.named_child_count() == 1 {
                let name = text(left.named_child(0).unwrap(), src).to_string();
                let value = right.named_child(0).unwrap();
                let group = call(value, src).and_then(|(receiver, method, args)| {
                    if aliases.contains(receiver) && matches!(method, "New" | "Default") {
                        Some(Group {
                            app: format!("gin:{}", value.start_byte()),
                            ..Default::default()
                        })
                    } else if method == "Group" {
                        let mut group = groups.get(receiver)?.clone();
                        let suffix = string(*args.first()?, src)?;
                        group.path = express_join(&group.path, &suffix);
                        group
                            .middleware
                            .extend(args[1..].iter().filter_map(|n| handler(*n, src)));
                        Some(group)
                    } else {
                        None
                    }
                });
                groups.remove(&name);
                if let Some(group) = group {
                    groups.insert(name, group);
                }
            } else {
                // Tuple assignment or short declaration may replace a known
                // receiver. Unsupported values must not retain its old route.
                let mut cursor = left.walk();
                for name in left.named_children(&mut cursor) {
                    groups.remove(text(name, src));
                }
            }
        }
        return;
    }
    if let Some((receiver, method, args)) = call(n, src) {
        if let Some(group) = groups.get_mut(receiver) {
            if method == "Use" {
                group
                    .middleware
                    .extend(args.iter().filter_map(|n| handler(*n, src)));
            } else if VERBS.iter().any(|v| v.to_uppercase() == method)
                && let Some(path) = args.first().and_then(|n| string(*n, src))
            {
                let Some(last) = args
                    .last()
                    .filter(|_| args.len() > 1)
                    .and_then(|n| handler(*n, src))
                else {
                    return;
                };
                let mut middleware = group.middleware.clone();
                middleware.extend(
                    args[1..args.len() - 1]
                        .iter()
                        .filter_map(|n| handler(*n, src)),
                );
                routes.push(Route {
                    file: file.into(),
                    line: n.start_position().row + 1,
                    end_line: n.end_position().row + 1,
                    method: method.into(),
                    path: express_join(&group.path, &path),
                    router: group.app.clone(),
                    kind: Kind::Express,
                    handler: Some((last, 0)),
                    middleware,
                });
            }
        }
        return;
    }
    let mut cursor = n.walk();
    for child in n.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "block"
                | "if_statement"
                | "for_statement"
                | "expression_switch_statement"
                | "type_switch_statement"
                | "select_statement"
        ) {
            // Child block bindings cannot shadow receivers in their parent.
            let mut scoped = groups.clone();
            walk(child, src, file, aliases, &mut scoped, routes);
            // An assignment (as opposed to :=) can change an outer binding.
            // Withhold subsequent uses instead of guessing a branch's value.
            invalidate_assignments(child, src, groups);
        } else {
            walk(child, src, file, aliases, groups, routes);
        }
    }
}

fn invalidate_assignments(n: Node, src: &[u8], groups: &mut BTreeMap<String, Group>) {
    if n.kind() == "assignment_statement"
        && let Some(left) = n.child_by_field_name("left")
    {
        let mut cursor = left.walk();
        for name in left.named_children(&mut cursor) {
            groups.remove(text(name, src));
        }
    }
    let mut cursor = n.walk();
    for child in n.named_children(&mut cursor) {
        invalidate_assignments(child, src, groups);
    }
}

pub(super) fn scan(path: &Path, parsed: &super::super::engine_parser::Parsed) -> Scanned {
    let src = parsed.source.as_slice();
    let root = parsed.tree.root_node();
    let mut aliases = BTreeSet::new();
    fn imports(n: Node, src: &[u8], aliases: &mut BTreeSet<String>) {
        if n.kind() == "import_spec"
            && n.child_by_field_name("path")
                .and_then(|p| string(p, src))
                .as_deref()
                == Some("github.com/gin-gonic/gin")
        {
            let name = n
                .child_by_field_name("name")
                .map(|n| text(n, src))
                .unwrap_or("gin");
            if name != "." && name != "_" {
                aliases.insert(name.to_string());
            }
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            imports(child, src, aliases);
        }
    }
    // Go imports are top-level declarations. Do not traverse every function
    // body in large generated files merely to discover that Gin is absent.
    let mut cursor = root.walk();
    for declaration in root.named_children(&mut cursor) {
        if declaration.kind() == "import_declaration" {
            imports(declaration, src, &mut aliases);
        }
    }
    let mut routes = Vec::new();
    if !aliases.is_empty() {
        walk(
            root,
            src,
            &path.to_string_lossy(),
            &aliases,
            &mut BTreeMap::new(),
            &mut routes,
        );
    }
    Scanned {
        facts: Facts::default(),
        routes,
        requests: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gin_factories_with_the_same_local_name_keep_distinct_applications() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("router.go");
        std::fs::write(&p, "package api\nimport \"github.com/gin-gonic/gin\"\nfunc first() { r := gin.New(); r.GET(\"/same\", Handle) }\nfunc second() { r := gin.New(); r.GET(\"/same\", Handle) }\n").unwrap();
        let parsed = parse_cache::parse(&p, "go", tree_sitter_go::LANGUAGE.into()).unwrap();
        let result = scan(&p, &parsed);
        assert_eq!(result.routes.len(), 2);
        assert_ne!(result.routes[0].router, result.routes[1].router);
    }
    #[test]
    fn gin_groups_methods_middleware_shadowing_and_unknown_receivers() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("router.go");
        std::fs::write(
            &p,
            r#"package api
import g "github.com/gin-gonic/gin"
func build() {
 r := g.New()
 v1 := r.Group("/nexus/v1")
 v1.Use(Forward())
 { auth := v1.Group("/", Auth()); auth.DELETE("/serviceconfig", h.Delete) }
 { v1 := r.Group("/other"); v1.GET("/items/:id", h.Get) }
 v1.GET("/list", h.List)
 v1 = unknown()
 v1.GET("/wrong", h.Wrong)
 client.GET("/not-a-route", h.Wrong)
 r, other = replacement(), 1
 r.GET("/old-tuple-receiver", h.Wrong)
 next := g.New()
 next, ok := replacementPair()
 next.GET("/old-short-receiver", h.Wrong)
}
"#,
        )
        .unwrap();
        let parsed = parse_cache::parse(&p, "go", tree_sitter_go::LANGUAGE.into()).unwrap();
        let result = scan(&p, &parsed);
        assert_eq!(result.routes.len(), 3);
        assert_eq!(result.routes[0].path, "/nexus/v1/serviceconfig");
        assert_eq!(result.routes[0].middleware, ["Forward", "Auth"]);
        assert_eq!(result.routes[1].path, "/other/items/:id");
        assert_eq!(result.routes[2].path, "/nexus/v1/list");
    }
}
