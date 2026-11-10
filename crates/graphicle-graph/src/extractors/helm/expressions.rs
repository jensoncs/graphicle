//! Bounded symbolic evaluation of reference expressions. No Helm/Sprig code is
//! executed and no value-file contents become constants: overrides stay unknown.
use super::{Token, actions, identifier};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Value {
    Unknown,
    Root,
    Path(String),
    Strings(BTreeSet<String>),
}
#[derive(Clone)]
pub(super) struct Context {
    pub variables: BTreeMap<String, Value>,
    pub dot: Value,
}
impl Default for Context {
    fn default() -> Self {
        Self {
            variables: BTreeMap::from([("$".into(), Value::Root)]),
            dot: Value::Root,
        }
    }
}
pub(super) struct Evaluation {
    pub value: Value,
    pub references: BTreeSet<(String, String)>,
    pub issues: BTreeSet<String>,
}
impl Evaluation {
    fn new(value: Value) -> Self {
        Self {
            value,
            references: BTreeSet::new(),
            issues: BTreeSet::new(),
        }
    }
    pub fn read(&mut self) {
        if let Value::Path(path) = &self.value {
            if path.is_empty() {
                self.issues.insert("whole_values_reference".into());
            } else {
                self.references
                    .insert(("value".into(), format!("Values.{path}")));
            }
        }
    }
    fn absorb(&mut self, other: &Self) {
        self.references.extend(other.references.iter().cloned());
        self.issues.extend(other.issues.iter().cloned());
    }
}
#[derive(Clone)]
enum Expr {
    Atom(Token),
    Call(Vec<Expr>),
    Pipe(Vec<Expr>),
}

fn parse(tokens: &[Token], at: &mut usize, nested: bool, depth: usize) -> Option<Expr> {
    if depth > 64 {
        return None;
    }
    let mut commands = vec![];
    let mut items = vec![];
    let mut closed = false;
    while let Some(token) = tokens.get(*at) {
        *at += 1;
        match token.value.as_str() {
            "(" if !token.quoted => items.push(parse(tokens, at, true, depth + 1)?),
            ")" if !token.quoted => {
                if !nested {
                    return None;
                }
                closed = true;
                break;
            }
            "|" if !token.quoted => {
                if items.is_empty() {
                    return None;
                }
                commands.push(Expr::Call(std::mem::take(&mut items)));
            }
            _ => items.push(Expr::Atom(token.clone())),
        }
    }
    if nested && !closed || items.is_empty() {
        return None;
    }
    commands.push(Expr::Call(items));
    Some(if commands.len() == 1 {
        commands.remove(0)
    } else {
        Expr::Pipe(commands)
    })
}
fn field(value: Value, tail: &str) -> Option<Value> {
    if !tail.split('.').all(identifier) {
        return None;
    }
    match value {
        Value::Root if tail == "Values" => Some(Value::Path(String::new())),
        Value::Root if tail.starts_with("Values.") => Some(Value::Path(tail[7..].into())),
        Value::Path(path) => Some(Value::Path(if path.is_empty() {
            tail.into()
        } else {
            format!("{path}.{tail}")
        })),
        _ => Some(Value::Unknown),
    }
}
fn atom(token: &Token, context: &Context) -> Evaluation {
    if token.quoted {
        return Evaluation::new(Value::Strings(BTreeSet::from([token.value.clone()])));
    }
    let text = token.value.as_str();
    if text == "." {
        return Evaluation::new(context.dot.clone());
    }
    let mut out = Evaluation::new(Value::Unknown);
    if let Some(tail) = text.strip_prefix('.') {
        out.value = field(context.dot.clone(), tail).unwrap_or_else(|| {
            out.issues.insert("dynamic_value_path".into());
            Value::Unknown
        });
        if context.dot == Value::Unknown {
            out.issues.insert("unknown_dot_scope".into());
        }
    } else if text.starts_with('$') {
        let (name, tail) = text
            .split_once('.')
            .map_or((text, None), |(n, t)| (n, Some(t)));
        if let Some(value) = context.variables.get(name) {
            out.value = tail
                .map_or_else(|| Some(value.clone()), |tail| field(value.clone(), tail))
                .unwrap_or(Value::Unknown);
            if out.value == Value::Unknown {
                out.issues.insert("unknown_variable_scope".into());
            }
        } else {
            out.issues.insert("unknown_variable_scope".into());
        }
    }
    out
}
fn string(value: &Value) -> Option<&str> {
    if let Value::Strings(values) = value
        && values.len() == 1
    {
        return values.first().map(String::as_str);
    }
    None
}
fn append_path(base: &Value, keys: &[Evaluation]) -> Option<Value> {
    let Value::Path(mut path) = base.clone() else {
        return None;
    };
    for key in keys {
        let key = string(&key.value)?;
        if !identifier(key) {
            return None;
        }
        if !path.is_empty() {
            path.push('.');
        }
        path.push_str(key);
    }
    Some(Value::Path(path))
}
fn format_strings(args: &[Evaluation], printf: bool) -> Option<Value> {
    let (format, values) = if printf {
        (Some(string(&args.first()?.value)?), &args[1..])
    } else {
        (None, args)
    };
    let mut combinations = vec![Vec::<String>::new()];
    for arg in values {
        let Value::Strings(choices) = &arg.value else {
            return None;
        };
        if combinations.len().checked_mul(choices.len())? > 16 {
            return None;
        }
        combinations = combinations
            .iter()
            .flat_map(|prefix| {
                choices.iter().map(move |choice| {
                    let mut next = prefix.clone();
                    next.push(choice.clone());
                    next
                })
            })
            .collect();
    }
    let mut results = BTreeSet::new();
    for parts in combinations {
        let mut out = String::new();
        if let Some(format) = format {
            let mut chars = format.chars();
            let mut parts = parts.iter();
            while let Some(c) = chars.next() {
                if c != '%' {
                    out.push(c);
                } else {
                    match chars.next()? {
                        '%' => out.push('%'),
                        's' => out.push_str(parts.next()?),
                        _ => return None,
                    }
                }
                if out.len() > 4096 {
                    return None;
                }
            }
            if parts.next().is_some() {
                return None;
            }
        } else {
            out = parts.concat();
        }
        if out.len() > 4096 {
            return None;
        }
        results.insert(out);
    }
    Some(Value::Strings(results))
}
fn evaluate(expr: &Expr, context: &Context, piped: Option<Evaluation>, depth: usize) -> Evaluation {
    let mut out = Evaluation::new(Value::Unknown);
    if depth > 32 {
        out.issues.insert("expression_depth_limit".into());
        return out;
    }
    match expr {
        Expr::Atom(token) => atom(token, context),
        Expr::Pipe(commands) => {
            let mut result = None;
            for command in commands {
                result = Some(evaluate(command, context, result, depth + 1));
            }
            result.unwrap_or(out)
        }
        Expr::Call(items) => {
            if items.len() == 1 && piped.is_none() {
                return evaluate(&items[0], context, None, depth + 1);
            }
            let name = match items.first() {
                Some(Expr::Atom(t)) if !t.quoted => t.value.as_str(),
                _ => "",
            };
            let mut args: Vec<_> = items
                .iter()
                .skip(1)
                .map(|e| evaluate(e, context, None, depth + 1))
                .collect();
            if let Some(piped) = piped {
                args.push(piped);
            }
            for arg in &args {
                out.absorb(arg);
            }
            let path = match name {
                "index" if args.len() >= 2 => append_path(&args[0].value, &args[1..]),
                "get" if args.len() == 2 => append_path(&args[0].value, &args[1..]),
                "dig" if args.len() >= 3 => {
                    append_path(&args.last().unwrap().value, &args[..args.len() - 2])
                }
                _ => None,
            };
            if let Some(path) = path {
                out.value = path;
                if name == "dig" {
                    let i = args.len() - 2;
                    args[i].read();
                    out.absorb(&args[i]);
                }
                return out;
            }
            for arg in &mut args {
                arg.read();
                out.absorb(arg);
            }
            match name {
                "include" | "template" => {
                    if let Some(Value::Strings(names)) = args.first().map(|a| &a.value).filter(
                        |v| matches!(v, Value::Strings(n) if !n.is_empty() && n.len() <= 16),
                    ) {
                        for name in names {
                            out.references.insert(("helper".into(), name.clone()));
                        }
                    } else {
                        out.issues.insert("dynamic_helper".into());
                    }
                    if args.len() != 2 || args[1].value != Value::Root {
                        out.issues.insert("custom_helper_context".into());
                    }
                }
                "printf" | "print" => {
                    out.value = format_strings(&args, name == "printf").unwrap_or(Value::Unknown);
                }
                "ternary" if args.len() == 3 => {
                    if let (Value::Strings(a), Value::Strings(b)) = (&args[0].value, &args[1].value)
                    {
                        let names: BTreeSet<_> = a.union(b).cloned().collect();
                        if names.len() <= 16 {
                            out.value = Value::Strings(names);
                        }
                    }
                }
                "index" | "get" | "dig" => {
                    out.issues.insert("dynamic_value_path".into());
                }
                "tpl" => {
                    if args.len() == 2
                        && args[1].value == Value::Root
                        && let Value::Strings(texts) = &args[0].value
                    {
                        for text in texts {
                            if let Ok(actions) = actions(text) {
                                for (_, tokens) in actions {
                                    if tokens.iter().any(|t| {
                                        !t.quoted
                                            && matches!(
                                                t.value.as_str(),
                                                "define"
                                                    | "block"
                                                    | "with"
                                                    | "range"
                                                    | "if"
                                                    | "else"
                                                    | "end"
                                                    | "="
                                            )
                                    }) {
                                        out.issues.insert("dynamic_tpl_structure".into());
                                        continue;
                                    }
                                    let mut at = 0;
                                    if let Some(expr) = parse(&tokens, &mut at, false, 0) {
                                        let mut nested =
                                            evaluate(&expr, &Context::default(), None, depth + 1);
                                        nested.read();
                                        out.absorb(&nested);
                                    } else {
                                        out.issues.insert("dynamic_template".into());
                                    }
                                }
                            } else {
                                out.issues.insert("dynamic_template".into());
                            }
                        }
                    } else {
                        out.issues.insert("dynamic_template".into());
                    }
                }
                "lookup" => {
                    out.issues.insert("runtime_lookup".into());
                }
                name if name.starts_with(".Files.") => {
                    out.issues.insert("runtime_file_access".into());
                }
                _ => {}
            }
            out
        }
    }
}
pub(super) fn analyze(tokens: &[Token], context: &Context) -> Evaluation {
    let mut at = 0;
    if let Some(expr) = parse(tokens, &mut at, false, 0) {
        evaluate(&expr, context, None, 0)
    } else {
        let mut out = Evaluation::new(Value::Unknown);
        out.issues.insert("unsupported_expression".into());
        out
    }
}
