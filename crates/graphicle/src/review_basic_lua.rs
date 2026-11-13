//! Lua: top-level functions are declarations, a call by a declared name links
//! its caller to it, and `require` names a module file. Blocks are matched by
//! keyword over the source with its strings and comments blanked.
use super::basic_refs::{Decl, Facts, Ref, To, masked};

fn word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Identifiers and dotted names (`a.b:c`) with their offsets.
fn names(code: &str) -> Vec<(usize, &str)> {
    let bytes = code.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < bytes.len() {
        if word(bytes[i]) && (i == 0 || !word(bytes[i - 1]) && bytes[i - 1] != b'.') {
            let start = i;
            while i < bytes.len()
                && (word(bytes[i])
                    || matches!(bytes[i], b'.' | b':')
                        && bytes.get(i + 1).is_some_and(|c| word(*c)))
            {
                i += 1;
            }
            out.push((start, &code[start..i]));
        } else {
            i += 1;
        }
    }
    out
}

pub(super) fn lua(content: &str) -> Facts {
    let mut f = Facts::default();
    let code = masked(content, "--", Some(("--[[", "]]")));
    let tokens = names(&code);
    let next_char = |at: usize| code[at..].trim_start().chars().next();
    // Open blocks: Some(declaration index) for a top-level function.
    let mut open: Vec<Option<usize>> = vec![];
    for (i, (at, name)) in tokens.iter().enumerate() {
        match *name {
            "function" => {
                let declared = match (i.checked_sub(1).map(|p| tokens[p].1), tokens.get(i + 1)) {
                    // `name = function(` or `local name = function(`
                    _ if code[..*at].trim_end().ends_with('=') => {
                        i.checked_sub(1).map(|p| tokens[p].1)
                    }
                    // `function name(` or `local function name(`, not `function(`
                    (_, Some((_, n))) if next_char(at + name.len()) != Some('(') => Some(*n),
                    _ => None,
                };
                let top = open.is_empty();
                open.push(declared.filter(|_| top).map(|name| {
                    let start = match i.checked_sub(1).map(|p| tokens[p]) {
                        Some((s, "local")) => s,
                        Some((s, n)) if code[..*at].trim_end().ends_with('=') && n == name => i
                            .checked_sub(2)
                            .map(|p| tokens[p])
                            .filter(|(_, l)| *l == "local")
                            .map_or(s, |(s, _)| s),
                        _ => *at,
                    };
                    f.decls.push(Decl {
                        name: name.replace(':', "."),
                        kind: "function",
                        start,
                        body_start: code[*at..].find(')').map_or(*at, |p| at + p + 1),
                        end: content.len(),
                    });
                    f.decls.len() - 1
                }));
            }
            "if" | "do" | "repeat" => open.push(None),
            "end" | "until" => {
                if let Some(Some(decl)) = open.pop() {
                    f.decls[decl].end = at + name.len();
                }
            }
            "require" | "dofile" | "loadfile" => {
                let rest = &content[at + name.len()..];
                let quoted = rest.trim_start().trim_start_matches('(').trim_start();
                let Some(quote) = quoted.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
                    continue;
                };
                let Some((module, _)) = quoted[1..].split_once(quote) else {
                    continue;
                };
                let start = module.as_ptr() as usize - content.as_ptr() as usize;
                let to = if *name == "require" {
                    let file = module.replace('.', "/");
                    To::Suffix(vec![format!("{file}.lua"), format!("{file}/init.lua")])
                } else {
                    To::Suffix(vec![module.into()])
                };
                f.refs.push(Ref {
                    to,
                    start,
                    end: start + module.len(),
                    relation: "potential_import",
                    evidence: "lua_module_candidate",
                });
            }
            _ => {}
        }
    }
    let declared: Vec<String> = f.decls.iter().map(|d| d.name.clone()).collect();
    for (i, (at, name)) in tokens.iter().enumerate() {
        let name = name.replace(':', ".");
        let after = next_char(at + tokens[i].1.len());
        let defining = i.checked_sub(1).is_some_and(|p| tokens[p].1 == "function");
        if !defining && matches!(after, Some('(' | '{')) && declared.contains(&name) {
            f.decl_ref(
                &name,
                false,
                *at,
                at + tokens[i].1.len(),
                "may_call",
                "lua_function_name",
            );
        }
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_functions_span_their_blocks_and_calls_bind_by_name() {
        let text = "local M = {}\nlocal json = require(\"lib.json\")\n\nlocal function helper(x)\n  for i = 1, 2 do\n    if x then return \"end\" end -- end\n  end\n  repeat x = x until x\n  return x\nend\n\nfunction M.run(h)\n  local inner = function() return helper(1) end\n  return helper(2)\nend\n\nM.stop = function(h)\n  h:log(\"x\")\nend\n";
        let f = lua(text);
        let decls: Vec<_> = f
            .decls
            .iter()
            .map(|d| (d.name.as_str(), &text[d.start..d.end]))
            .collect();
        assert_eq!(decls.len(), 3, "{decls:?}");
        assert!(decls[0].1.starts_with("local function helper(x)"));
        assert!(decls[0].1.ends_with("return x\nend"), "{:?}", decls[0]);
        assert_eq!(decls[1].0, "M.run");
        assert!(decls[1].1.ends_with("helper(2)\nend"));
        assert_eq!(decls[2].0, "M.stop");
        assert!(
            decls[2].1.starts_with("M.stop = function(h)"),
            "{:?}",
            decls[2]
        );
        let calls: Vec<_> = f
            .refs
            .iter()
            .filter_map(|r| match &r.to {
                To::Decl { name, .. } => Some((name.as_str(), r.start)),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|(n, _)| *n == "helper"));
        assert!(matches!(&f.refs[0].to, To::Suffix(s) if s[0] == "lib/json.lua"));
    }
}
