//! Bounded same-file request URL propagation. Only immutable strings and
//! single-return request helpers are evaluated; this is not a JS interpreter.
use super::{
    ScopeNames, VERBS, arguments, binding_pattern, client_receiver, literal, request_method,
    request_option, text,
};
use rustc_hash::{FxHashMap, FxHashSet};
use tree_sitter::Node;

const MAX_DEPTH: usize = 16;
const MAX_STEPS: usize = 64;
const MAX_BYTES: usize = 4096;

#[derive(Default)]
pub(super) struct Resolver<'t> {
    bindings: FxHashMap<String, Vec<Node<'t>>>,
    written: FxHashSet<String>,
}

fn immutable(declaration: Node) -> bool {
    declaration.kind() == "variable_declarator"
        && declaration
            .parent()
            .and_then(|p| p.child(0))
            .is_some_and(|n| n.kind() == "const")
}

impl<'t> Resolver<'t> {
    pub(super) fn new(root: Node<'t>, names: &ScopeNames, src: &[u8]) -> Self {
        let mut result = Self::default();
        result.collect(root, names, src);
        result
    }

    fn collect(&mut self, node: Node<'t>, names: &ScopeNames, src: &[u8]) {
        if matches!(node.kind(), "variable_declarator" | "function_declaration")
            && let Some(name) = node
                .child_by_field_name("name")
                .filter(|n| n.kind() == "identifier")
        {
            let scope = if node.kind() == "function_declaration" {
                node.parent().unwrap_or(node)
            } else {
                name
            };
            self.bindings
                .entry(names.name(scope, text(name, src), false))
                .or_default()
                .push(node);
        }
        let target = match node.kind() {
            "assignment_expression" | "augmented_assignment_expression" | "for_in_statement" => {
                node.child_by_field_name("left")
            }
            "update_expression" => node.child_by_field_name("argument"),
            _ => None,
        };
        if let Some(target) = target {
            let mut bound = FxHashSet::default();
            binding_pattern(target, src, &mut bound);
            for name in bound {
                self.written.insert(names.name(target, &name, false));
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.collect(child, names, src);
        }
    }

    fn binding(&self, reference: Node, names: &ScopeNames, src: &[u8]) -> Option<Node<'t>> {
        // Named function/class expressions bind their own name internally.
        // The router scope inventory doesn't model that expression-only scope.
        let mut parent = reference.parent();
        while let Some(node) = parent {
            if matches!(
                node.kind(),
                "function_expression" | "class" | "class_expression"
            ) && node
                .child_by_field_name("name")
                .is_some_and(|name| text(name, src) == text(reference, src))
            {
                return None;
            }
            parent = node.parent();
        }
        let key = names.name(reference, text(reference, src), false);
        if self.written.contains(&key) {
            return None;
        }
        let entries = self.bindings.get(&key)?;
        (entries.len() == 1).then(|| entries[0])
    }

    fn evaluate(
        &self,
        node: Node,
        names: &ScopeNames,
        src: &[u8],
        parameters: &FxHashMap<String, String>,
        depth: usize,
        steps: &mut usize,
    ) -> Option<String> {
        if depth >= MAX_DEPTH || *steps == 0 || node.has_error() {
            return None;
        }
        *steps -= 1;
        let mut recurse = |n| self.evaluate(n, names, src, parameters, depth + 1, steps);
        let value = match node.kind() {
            "string" => {
                let value = literal(node, src)?;
                // The existing literal reader preserves escapes. Reject them
                // rather than assigning the raw spelling a different URL.
                if value.contains('\\') {
                    return None;
                }
                value
            }
            "identifier" => {
                let key = names.name(node, text(node, src), false);
                if self.written.contains(&key) {
                    return None;
                }
                if let Some(value) = parameters.get(&key) {
                    value.clone()
                } else {
                    let declaration = self.binding(node, names, src)?;
                    if !immutable(declaration) || declaration.end_byte() > node.start_byte() {
                        return None;
                    }
                    recurse(declaration.child_by_field_name("value")?)?
                }
            }
            "parenthesized_expression" => recurse(node.named_child(0)?)?,
            "binary_expression" => {
                if text(node.child_by_field_name("operator")?, src) != "+" {
                    return None;
                }
                let mut left = recurse(node.child_by_field_name("left")?)?;
                left.push_str(&recurse(node.child_by_field_name("right")?)?);
                left
            }
            "template_string" => {
                let mut result = String::new();
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    match child.kind() {
                        "string_fragment" => result.push_str(text(child, src)),
                        "template_substitution" => {
                            result.push_str(&recurse(child.named_child(0)?)?)
                        }
                        _ => return None,
                    }
                    if result.len() > MAX_BYTES {
                        return None;
                    }
                }
                result
            }
            _ => return None,
        };
        (value.len() <= MAX_BYTES).then_some(value)
    }

    pub(super) fn url(&self, expression: Node, names: &ScopeNames, src: &[u8]) -> Option<String> {
        let mut steps = MAX_STEPS;
        self.evaluate(expression, names, src, &FxHashMap::default(), 0, &mut steps)
            // Preserve existing partial-path handling for direct literals.
            .or_else(|| literal(expression, src))
    }

    pub(super) fn request(
        &self,
        call: Node,
        names: &ScopeNames,
        src: &[u8],
    ) -> Option<(String, String)> {
        if let Some((method, url)) = direct_request(call, names, src) {
            return Some((method, self.url(url, names, src)?));
        }
        let function = call
            .child_by_field_name("function")
            .filter(|n| n.kind() == "identifier")?;
        let declaration = self.binding(function, names, src)?;
        let helper = if declaration.kind() == "function_declaration" {
            declaration
        } else {
            if !immutable(declaration) || declaration.end_byte() > call.start_byte() {
                return None;
            }
            declaration.child_by_field_name("value")?
        };
        if !matches!(
            helper.kind(),
            "function_declaration" | "function_expression" | "arrow_function"
        ) || helper.has_error()
        {
            return None;
        }
        let body = helper.child_by_field_name("body")?;
        let mut sink = if body.kind() == "statement_block" {
            let mut cursor = body.walk();
            let statements: Vec<_> = body
                .named_children(&mut cursor)
                .filter(|n| n.kind() != "comment")
                .collect();
            if statements.len() != 1 || statements[0].kind() != "return_statement" {
                return None;
            }
            statements[0].named_child(0)?
        } else {
            body
        };
        if sink.kind() == "await_expression" {
            sink = sink.named_child(0)?;
        }
        let (method, url) = direct_request(sink, names, src)?;
        let params = if let Some(parameters) = helper.child_by_field_name("parameters") {
            let mut cursor = parameters.walk();
            parameters
                .named_children(&mut cursor)
                .filter(|n| n.kind() != "comment")
                .collect::<Vec<_>>()
        } else {
            helper
                .child_by_field_name("parameter")
                .into_iter()
                .collect()
        };
        let (args, _) = arguments(call.child_by_field_name("arguments")?, src);
        if params.len() > 1 || args.len() != params.len() {
            return None;
        }
        let mut substitutions = FxHashMap::default();
        let mut steps = MAX_STEPS;
        for (parameter, argument) in params.into_iter().zip(args) {
            let parameter = if parameter.kind() == "required_parameter" {
                parameter.child_by_field_name("pattern")?
            } else {
                parameter
            };
            if parameter.kind() != "identifier" {
                return None;
            }
            let value =
                self.evaluate(argument, names, src, &FxHashMap::default(), 0, &mut steps)?;
            substitutions.insert(names.name(parameter, text(parameter, src), false), value);
        }
        Some((
            method,
            self.evaluate(url, names, src, &substitutions, 0, &mut steps)?,
        ))
    }
}

/// Reuse the route pass's supported client forms and method rules. The URL
/// stays an AST expression so direct calls and helpers share one evaluator.
fn direct_request<'t>(
    call: Node<'t>,
    names: &ScopeNames,
    src: &[u8],
) -> Option<(String, Node<'t>)> {
    if call.kind() != "call_expression" {
        return None;
    }
    let function = call.child_by_field_name("function")?;
    let (args, _) = arguments(call.child_by_field_name("arguments")?, src);
    if function.kind() == "member_expression" {
        let object = function.child_by_field_name("object")?;
        let receiver = text(object, src);
        let method = text(function.child_by_field_name("property")?, src);
        if client_receiver(receiver)
            && VERBS.contains(&method)
            && names.name(object, receiver, false) == receiver
        {
            return Some((method.to_uppercase(), *args.first()?));
        }
        return None;
    }
    let name = text(function, src);
    if !matches!(name, "fetch" | "axios") || names.name(function, name, false) != name {
        return None;
    }
    let config = (name == "axios")
        .then(|| args.first().copied().filter(|p| p.kind() == "object"))
        .flatten();
    let url = match config {
        Some(config) => request_option(config, "url", src)??,
        None => *args.first()?,
    };
    let method = request_method(config.or_else(|| args.get(1).copied()), src, name)?;
    Some((method, url))
}

#[cfg(test)]
mod tests {
    use super::super::scan;
    use super::*;

    fn requests(source: &str) -> Vec<super::super::Request> {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("requests.ts");
        std::fs::write(&file, source).unwrap();
        scan(&file).unwrap().requests
    }

    #[test]
    fn helper_arguments_remain_specific_to_each_call_site() {
        let source = "function load(path) { return fetch('/inventory'+path); }\nfunction first() { return load('/orders'); }\nfunction second() { return load('/items'); }";
        let found: Vec<_> = requests(source)
            .into_iter()
            .map(|r| (r.caller.unwrap().0, r.line, r.path))
            .collect();
        assert_eq!(
            found,
            vec![
                ("first".into(), 2, "/inventory/orders".into()),
                ("second".into(), 3, "/inventory/items".into())
            ]
        );
    }

    #[test]
    fn writes_shadowing_and_unsupported_helpers_do_not_guess() {
        let cases = [
            "function load(path) { return fetch('/inventory'+path); } [load]=others; function caller(){return load('/orders');}",
            "function load(path) { return fetch('/inventory'+path); } ({load}=others); function caller(){return load('/orders');}",
            "function load(path) { return fetch('/inventory'+path); } load++; function caller(){return load('/orders');}",
            "function load(path) { return fetch('/inventory'+path); } function replace(){load=other;} function caller(){return load('/orders');}",
            "function load(axios) { return axios.get('/inventory/orders'); } function caller(){return load('/orders');}",
            "const url='/inventory/orders'; const f=function url(){return fetch(url);};",
            "const url='/inventory/orders'; function caller(){const {url}=options; return fetch(url);}",
            "const load=(path)=>fetch('/inventory'+path); function caller(){return load(...parts);}",
            "function load(path, method){return fetch('/inventory'+path,{method});} function caller(){return load('/orders','GET');}",
            "function load(path){return fetch('/inventory'+path);} function caller(){return load('/orders',sideEffect());}",
        ];
        for source in cases {
            let found: Vec<_> = requests(source)
                .into_iter()
                .filter(|r| {
                    r.caller
                        .as_ref()
                        .is_some_and(|(name, _)| name == "caller" || name == "url")
                })
                .collect();
            assert!(found.is_empty(), "{source}: {found:?}");
        }
    }

    #[test]
    fn propagation_bounds_abstain() {
        let mut aliases = String::from("function caller(){const p0='/inventory/orders';");
        for i in 1..=MAX_DEPTH {
            aliases.push_str(&format!("const p{i}=p{};", i - 1));
        }
        aliases.push_str(&format!("return fetch(p{MAX_DEPTH});}}"));
        assert!(requests(&aliases).is_empty());
        let large = format!(
            "function caller(){{const url='/{}';return fetch(url);}}",
            "a".repeat(MAX_BYTES)
        );
        assert!(requests(&large).is_empty());
        // A balanced expression stays below the depth and byte bounds but
        // exceeds the independent expression-step budget.
        let mut expression = "'/x'".to_string();
        for _ in 0..6 {
            expression = format!("({expression}+{expression})");
        }
        let wide = format!("function caller(){{const url={expression};return fetch(url);}}");
        assert!(requests(&wide).is_empty());
        assert!(requests("function caller(){const a=b;const b=a;return fetch(a);}").is_empty());
    }
}
