use super::*;
use rustc_hash::FxHashSet;

const PACKAGE: &str = "org.springframework.web.bind.annotation";
struct Imports {
    exact: FxHashMap<String, String>,
    wildcards: Vec<String>,
    types: FxHashSet<String>,
}
impl Imports {
    fn identity(&self, name: &str) -> Option<String> {
        if name.contains('.') {
            return Some(name.into());
        }
        if self.types.contains(name) {
            return None;
        }
        if let Some(import) = self.exact.get(name) {
            return Some(import.clone());
        }
        (self.wildcards.len() == 1).then(|| format!("{}.{name}", self.wildcards[0]))
    }
    fn annotation(&self, n: Node, src: &[u8]) -> Option<String> {
        self.identity(text(n.child_by_field_name("name")?, src))
    }
}

fn annotations(n: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    children(n)
        .filter(|n| n.kind() == "modifiers")
        .flat_map(children)
        .filter(|n| matches!(n.kind(), "annotation" | "marker_annotation"))
}

fn values(n: Node<'_>) -> Vec<Node<'_>> {
    if n.kind() == "element_value_array_initializer" {
        children(n).collect()
    } else {
        vec![n]
    }
}

/// An explicit mapping; conditions we cannot represent (headers, params,
/// consumes/produces, custom annotations) do not become unconstrained routes.
fn mapping(n: Node, src: &[u8], imports: &Imports) -> Option<(Vec<String>, Vec<String>)> {
    let identity = imports.annotation(n, src)?;
    let name = identity.strip_prefix(&format!("{PACKAGE}."))?;
    let method = match name {
        "RequestMapping" => "ANY",
        "GetMapping" => "GET",
        "PostMapping" => "POST",
        "PutMapping" => "PUT",
        "PatchMapping" => "PATCH",
        "DeleteMapping" => "DELETE",
        _ => return None,
    };
    let mut paths = None;
    let mut methods = None;
    if let Some(args) = n.child_by_field_name("arguments") {
        for arg in children(args) {
            let (key, value) = if arg.kind() == "element_value_pair" {
                (
                    text(arg.child_by_field_name("key")?, src),
                    arg.child_by_field_name("value")?,
                )
            } else {
                ("value", arg)
            };
            let items = values(value);
            if items.is_empty() || items.len() > 16 {
                return None;
            }
            match key {
                "value" | "path" if paths.is_none() => {
                    paths = Some(
                        items
                            .into_iter()
                            .map(|n| string(n, src))
                            .collect::<Option<Vec<_>>>()?,
                    );
                }
                "method" if method == "ANY" && methods.is_none() => {
                    methods = Some(
                        items
                            .into_iter()
                            .map(|n| {
                                let raw = text(n, src);
                                let (owner, verb) = raw.rsplit_once('.')?;
                                if imports.identity(owner)? != format!("{PACKAGE}.RequestMethod")
                                    || ![
                                        "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS",
                                        "TRACE",
                                    ]
                                    .contains(&verb)
                                {
                                    return None;
                                }
                                Some(verb.to_owned())
                            })
                            .collect::<Option<Vec<_>>>()?,
                    );
                }
                // A name does not constrain request matching.
                "name" if string(value, src).is_some() => {}
                _ => return None,
            }
        }
    }
    Some((
        paths.unwrap_or_else(|| vec![String::new()]),
        methods.unwrap_or_else(|| vec![method.into()]),
    ))
}

fn is_mapping(n: Node, src: &[u8], imports: &Imports) -> bool {
    imports
        .annotation(n, src)
        .is_some_and(|n| n.starts_with(&format!("{PACKAGE}.")) && n.ends_with("Mapping"))
}

pub(super) fn scan(path: &Path, root: Node, src: &[u8]) -> Vec<Route> {
    let mut imports = Imports {
        exact: FxHashMap::default(),
        wildcards: vec![],
        types: FxHashSet::default(),
    };
    for n in children(root).filter(|n| n.kind() == "import_declaration") {
        let raw = text(n, src)
            .trim()
            .trim_start_matches("import ")
            .trim_end_matches(';')
            .trim();
        if raw.starts_with("static ") {
            continue;
        }
        if let Some(package) = raw.strip_suffix(".*") {
            imports.wildcards.push(package.into());
        } else if let Some((_, name)) = raw.rsplit_once('.') {
            imports.exact.insert(name.into(), raw.into());
        }
    }
    walk(root, &mut |n| {
        if matches!(
            n.kind(),
            "class_declaration"
                | "interface_declaration"
                | "annotation_type_declaration"
                | "enum_declaration"
        ) && let Some(name) = n.child_by_field_name("name")
        {
            imports.types.insert(text(name, src).into());
        }
    });
    let mut out = vec![];
    walk(root, &mut |class| {
        if class.kind() != "class_declaration" {
            return;
        }
        if !annotations(class).any(|n| {
            imports.annotation(n, src).is_some_and(|name| {
                name == format!("{PACKAGE}.RestController")
                    || name == "org.springframework.stereotype.Controller"
            })
        }) {
            return;
        }
        let class_mappings = annotations(class)
            .filter(|n| is_mapping(*n, src, &imports))
            .collect::<Vec<_>>();
        let base = match class_mappings.as_slice() {
            [] => (vec![String::new()], vec!["ANY".into()]),
            [annotation] => {
                let Some(base) = mapping(*annotation, src, &imports) else {
                    return;
                };
                base
            }
            _ => return,
        };
        let Some(body) = class.child_by_field_name("body") else {
            return;
        };
        for method in children(body).filter(|n| n.kind() == "method_declaration") {
            let mappings = annotations(method)
                .filter(|n| is_mapping(*n, src, &imports))
                .collect::<Vec<_>>();
            let [annotation] = mappings.as_slice() else {
                continue;
            };
            let Some((paths, verbs)) = mapping(*annotation, src, &imports) else {
                continue;
            };
            let Some(name) = method.child_by_field_name("name") else {
                continue;
            };
            if base.0.len() * paths.len() * verbs.len() > 64 {
                continue;
            }
            // Spring method-level methods override the type-level condition.
            let verbs = if verbs == ["ANY"] { &base.1 } else { &verbs };
            for prefix in &base.0 {
                for url in &paths {
                    for verb in verbs {
                        let url = super::super::express_join(prefix, url);
                        let url = format!("/{}", url.trim_start_matches('/'));
                        out.push(route(
                            path,
                            method,
                            "spring",
                            verb,
                            url,
                            Some((text(name, src).into(), method.start_position().row + 1)),
                        ));
                    }
                }
            }
        }
    });
    out
}
