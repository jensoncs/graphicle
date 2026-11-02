use super::*;
use rustc_hash::FxHashSet;

struct Module<'t> {
    imports: FxHashMap<String, String>,
    declarations: FxHashMap<String, Node<'t>>,
    exports: Vec<(String, String)>,
    names: ScopeNames,
    written: FxHashSet<String>,
}

impl<'t> Module<'t> {
    fn new(root: Node<'t>, src: &[u8]) -> Self {
        let mut module = Self {
            imports: FxHashMap::default(),
            declarations: FxHashMap::default(),
            exports: vec![],
            names: ScopeNames::collect(root, src, false),
            written: FxHashSet::default(),
        };
        for statement in children(root) {
            if statement.kind() == "import_statement" {
                let Some(source) = statement
                    .child_by_field_name("source")
                    .and_then(|n| string(n, src))
                else {
                    continue;
                };
                // Type-only imports have no runtime identity.
                if text(statement, src)
                    .trim_start()
                    .starts_with("import type ")
                {
                    continue;
                }
                walk(statement, &mut |n| {
                    if n.kind() == "import_specifier" && !text(n, src).starts_with("type ") {
                        if let Some(name) = n.child_by_field_name("name") {
                            let alias = n.child_by_field_name("alias").unwrap_or(name);
                            module.imports.insert(
                                text(alias, src).into(),
                                format!("{source}.{}", text(name, src)),
                            );
                        }
                    } else if n.kind() == "namespace_import"
                        && let Some(name) = children(n).last()
                    {
                        module
                            .imports
                            .insert(text(name, src).into(), source.clone());
                    }
                });
                continue;
            }
            let exported = statement.kind() == "export_statement";
            if exported
                && text(statement, src)
                    .trim_start()
                    .starts_with("export type ")
            {
                continue;
            }
            if exported && statement.child_by_field_name("source").is_some() {
                continue;
            }
            let declaration = if exported {
                statement.child_by_field_name("declaration")
            } else {
                Some(statement)
            };
            let default = exported
                && (0..statement.child_count())
                    .any(|i| statement.child(i).is_some_and(|n| n.kind() == "default"));
            if let Some(decl) = declaration {
                if decl.kind() == "function_declaration"
                    && let Some(name) = decl.child_by_field_name("name")
                {
                    let name = text(name, src).to_owned();
                    module.declarations.insert(name.clone(), decl);
                    if exported {
                        module.exports.push((
                            if default {
                                "default".into()
                            } else {
                                name.clone()
                            },
                            name,
                        ));
                    }
                } else if decl.kind() == "lexical_declaration"
                    && decl.child(0).is_some_and(|n| n.kind() == "const")
                {
                    for item in children(decl).filter(|n| n.kind() == "variable_declarator") {
                        if let Some(name) = item
                            .child_by_field_name("name")
                            .filter(|n| n.kind() == "identifier")
                        {
                            let name = text(name, src).to_owned();
                            module.declarations.insert(name.clone(), item);
                            if exported {
                                module.exports.push((name.clone(), name));
                            }
                        }
                    }
                }
            }
            if exported {
                if default
                    && let Some(value) = statement
                        .child_by_field_name("value")
                        .filter(|n| callable(*n))
                {
                    module.declarations.insert("$default".into(), value);
                    module.exports.push(("default".into(), "$default".into()));
                }
                if default
                    && let Some(value) = statement
                        .child_by_field_name("value")
                        .filter(|n| n.kind() == "identifier")
                {
                    module
                        .exports
                        .push(("default".into(), text(value, src).into()));
                }
                for clause in children(statement).filter(|n| n.kind() == "export_clause") {
                    for specifier in children(clause) {
                        if let Some(name) = specifier.child_by_field_name("name") {
                            let alias = specifier.child_by_field_name("alias").unwrap_or(name);
                            module
                                .exports
                                .push((text(alias, src).into(), text(name, src).into()));
                        }
                    }
                }
            }
        }
        walk(root, &mut |n| {
            let target = match n.kind() {
                "assignment_expression"
                | "augmented_assignment_expression"
                | "for_in_statement" => n.child_by_field_name("left"),
                "update_expression" => n.child_by_field_name("argument"),
                _ => None,
            };
            if let Some(target) = target {
                // Conservatively also invalidate writes through member expressions.
                walk(target, &mut |part| {
                    if part.kind() == "identifier" {
                        module
                            .written
                            .insert(module.names.name(part, text(part, src), false));
                    }
                });
            }
        });
        module
    }

    fn identity(&self, n: Node, src: &[u8]) -> Option<String> {
        match n.kind() {
            "identifier" => {
                let mut parent = n.parent();
                while let Some(scope) = parent {
                    if matches!(
                        scope.kind(),
                        "function_expression" | "class" | "class_expression"
                    ) && scope
                        .child_by_field_name("name")
                        .is_some_and(|name| text(name, src) == text(n, src))
                    {
                        return None;
                    }
                    parent = scope.parent();
                }
                let key = self.names.name(n, text(n, src), false);
                if self.written.contains(&key) {
                    return None;
                }
                self.imports.get(&key).cloned()
            }
            "member_expression" => Some(format!(
                "{}.{}",
                self.identity(n.child_by_field_name("object")?, src)?,
                text(
                    n.child_by_field_name("property")
                        .filter(|p| p.kind() == "property_identifier")?,
                    src
                )
            )),
            _ => None,
        }
    }
}

/// App and Pages routes are relative to an admitted Next package, never an
/// arbitrary directory named `app`. Catchalls retain the matcher's semantics.
fn next_paths(path: &Path, project: &Path) -> Option<(bool, Vec<String>)> {
    let relative = path.strip_prefix(project).ok()?;
    let parts = relative
        .iter()
        .map(|s| s.to_str())
        .collect::<Option<Vec<_>>>()?;
    let parts = if parts.first() == Some(&"src") {
        &parts[1..]
    } else {
        &parts[..]
    };
    let app = parts.first() == Some(&"app");
    if !(app || parts.first() == Some(&"pages") && parts.get(1) == Some(&"api")) {
        return None;
    }
    let filename = Path::new(parts.last()?).file_stem()?.to_str()?;
    if app && filename != "route" {
        return None;
    }
    let mut segments = vec![];
    let parts = parts[1..parts.len() - 1]
        .iter()
        .copied()
        .chain((!app && filename != "index").then_some(filename))
        .collect::<Vec<_>>();
    for (index, segment) in parts.iter().copied().enumerate() {
        if segment.starts_with(['_', '@']) || segment.contains('%') || segment.starts_with("(.") {
            return None;
        }
        if app && segment.starts_with('(') && segment.ends_with(')') {
            continue;
        }
        if segment.starts_with("[[...") && segment.ends_with("]]") {
            if index + 1 != parts.len() || segment.len() <= 7 {
                return None;
            }
            // Only a terminal optional catchall has a meaningful empty variant.
            segments.push(format!("{{{}:path}}", &segment[5..segment.len() - 2]));
            let full = format!("/{}", segments.join("/"));
            segments.pop();
            return Some((app, vec![format!("/{}", segments.join("/")), full]));
        } else if segment.starts_with("[...") && segment.ends_with(']') {
            if index + 1 != parts.len() || segment.len() <= 5 {
                return None;
            }
            segments.push(format!("{{{}:path}}", &segment[4..segment.len() - 1]));
        } else if segment.starts_with('[') && segment.ends_with(']') {
            if segment.len() <= 2 || segment[1..segment.len() - 1].contains(['[', ']']) {
                return None;
            }
            segments.push(format!("{{{}}}", &segment[1..segment.len() - 1]));
        } else if segment.contains(['[', ']', '(', ')']) {
            return None;
        } else {
            segments.push(segment.to_owned());
        }
    }
    Some((app, vec![format!("/{}", segments.join("/"))]))
}

fn callable(n: Node) -> bool {
    matches!(
        n.kind(),
        "function_declaration" | "function_expression" | "arrow_function"
    )
}

fn firebase_kind(identity: &str) -> Option<(&'static str, &'static str)> {
    Some(match identity {
        "firebase-functions/v2/https.onCall"
        | "firebase-functions.https.onCall"
        | "firebase-functions/v1.https.onCall" => ("cloud_function", "callable"),
        "firebase-functions/v2/https.onRequest"
        | "firebase-functions.https.onRequest"
        | "firebase-functions/v1.https.onRequest" => ("cloud_function", "http"),
        "firebase-functions/v2/scheduler.onSchedule" => ("cloud_trigger", "schedule"),
        "firebase-functions/v2/firestore.onDocumentCreated"
        | "firebase-functions/v2/firestore.onDocumentUpdated"
        | "firebase-functions/v2/firestore.onDocumentDeleted"
        | "firebase-functions/v2/firestore.onDocumentWritten" => ("cloud_trigger", "firestore"),
        "firebase-functions/v2/pubsub.onMessagePublished" => ("cloud_trigger", "pubsub"),
        "firebase-functions/v2/database.onValueCreated"
        | "firebase-functions/v2/database.onValueUpdated"
        | "firebase-functions/v2/database.onValueDeleted"
        | "firebase-functions/v2/database.onValueWritten" => ("cloud_trigger", "database"),
        "firebase-functions/v2/storage.onObjectFinalized"
        | "firebase-functions/v2/storage.onObjectDeleted"
        | "firebase-functions/v2/storage.onObjectArchived"
        | "firebase-functions/v2/storage.onObjectMetadataUpdated" => ("cloud_trigger", "storage"),
        _ => return None,
    })
}

fn firebase(
    value: Node,
    module: &Module<'_>,
    src: &[u8],
) -> Option<(&'static str, &'static str, Option<String>)> {
    let function = value.child_by_field_name("function")?;
    if let Some((node_kind, kind)) = module
        .identity(function, src)
        .as_deref()
        .and_then(firebase_kind)
    {
        let resource = value
            .child_by_field_name("arguments")
            .and_then(|a| children(a).next())
            .and_then(|n| {
                string(n, src).or_else(|| {
                    let key = match kind {
                        "schedule" => "schedule",
                        "firestore" => "document",
                        "pubsub" => "topic",
                        "database" => "ref",
                        "storage" => "bucket",
                        _ => return None,
                    };
                    super::super::request_option(n, key, src)
                        .flatten()
                        .and_then(|n| string(n, src))
                })
            });
        return Some((node_kind, kind, resource));
    }
    // V1 builders preserve a resource declaration but still carry no proven
    // deployment project, region or event-to-consumer relationship.
    if function.kind() != "member_expression" {
        return None;
    }
    let builder = function.child_by_field_name("object")?;
    if builder.kind() != "call_expression" {
        return None;
    }
    let identity = module.identity(builder.child_by_field_name("function")?, src)?;
    let identity = identity
        .strip_prefix("firebase-functions.")
        .or_else(|| identity.strip_prefix("firebase-functions/v1."))?;
    let action = text(function.child_by_field_name("property")?, src);
    let kind = match (identity, action) {
        ("pubsub.schedule", "onRun") => "schedule",
        ("pubsub.topic", "onPublish") => "pubsub",
        ("firestore.document", "onCreate" | "onUpdate" | "onDelete" | "onWrite") => "firestore",
        ("database.ref", "onCreate" | "onUpdate" | "onDelete" | "onWrite") => "database",
        ("storage.object", "onFinalize" | "onDelete" | "onArchive" | "onMetadataUpdate") => {
            "storage"
        }
        ("auth.user", "onCreate" | "onDelete") => "auth",
        _ => return None,
    };
    let resource = builder
        .child_by_field_name("arguments")
        .and_then(|a| children(a).next())
        .and_then(|n| string(n, src));
    Some(("cloud_trigger", kind, resource))
}

pub(super) fn scan(
    path: &Path,
    root: Node,
    src: &[u8],
    projects: &Projects,
) -> (Vec<Route>, Vec<Value>) {
    let next = projects.next_root(path).and_then(|p| next_paths(path, p));
    let has_firebase = children(root)
        .filter(|n| n.kind() == "import_statement")
        .any(|n| {
            n.child_by_field_name("source")
                .and_then(|n| string(n, src))
                .is_some_and(|s| {
                    s == "firebase/functions"
                        || s == "firebase-functions"
                        || s.starts_with("firebase-functions/")
                })
        });
    if next.is_none() && !has_firebase {
        return Default::default();
    }
    let module = Module::new(root, src);
    let mut routes = vec![];
    let mut boundaries = vec![];
    for (export, local) in &module.exports {
        if module.written.contains(local) {
            continue;
        }
        let Some(decl) = module.declarations.get(local).copied() else {
            continue;
        };
        let value = if callable(decl) {
            Some(decl)
        } else {
            decl.child_by_field_name("value")
        };
        let Some(value) = value else {
            continue;
        };
        if let Some((app, urls)) = &next {
            let method = if *app
                && super::super::VERBS
                    .iter()
                    .any(|v| v.to_uppercase() == *export)
            {
                Some(export.as_str())
            } else if !app && export == "default" {
                Some("ANY")
            } else {
                None
            };
            if let Some(method) = method
                && callable(value)
            {
                for url in urls {
                    routes.push(route(
                        path,
                        decl,
                        "next",
                        method,
                        url.clone(),
                        (local != "$default")
                            .then(|| (local.clone(), decl.start_position().row + 1)),
                    ));
                }
            }
        }
        if value.kind() == "call_expression"
            && let Some((node_kind, kind, resource)) = firebase(value, &module, src)
        {
            let callback = value
                .child_by_field_name("arguments")
                .and_then(|a| children(a).last());
            if !callback.is_some_and(|n| {
                callable(n)
                    || (n.kind() == "identifier"
                        && !module.written.contains(text(n, src))
                        && module.declarations.get(text(n, src)).is_some_and(|d| {
                            callable(*d) || d.child_by_field_name("value").is_some_and(callable)
                        }))
            }) {
                continue;
            }
            boundaries.push(boundary(path, decl, node_kind, kind, export, resource));
        }
    }
    walk(root, &mut |n| {
        if n.kind() != "call_expression" {
            return;
        }
        if n.child_by_field_name("function")
            .and_then(|f| module.identity(f, src))
            .as_deref()
            != Some("firebase/functions.httpsCallable")
        {
            return;
        }
        if let Some(name) = n
            .child_by_field_name("arguments")
            .and_then(|a| children(a).nth(1))
            .and_then(|n| string(n, src))
        {
            boundaries.push(boundary(path, n, "cloud_call", "callable", &name, None));
        }
    });
    (routes, boundaries)
}
