//! Protocol Buffers for basic review. Messages, enums, services, rpcs and
//! extensions are declarations. A field's, rpc's or extension's message or
//! enum type, and a custom option (`[(buf.validate.field).required = true]`),
//! name the declaration they use; basic review resolves that name by
//! protobuf's scoping in the file and the files it imports. Import roots come
//! from no build flags: an import names the one snapshot `.proto` at that
//! path or ending in it.
use std::collections::BTreeSet;

/// A message, enum, service, rpc (`Service.Method`) or extension, its name
/// nested as written (`Outer.Inner`).
pub(super) struct Decl {
    pub name: String,
    pub kind: &'static str,
    pub start: usize,
    pub body_start: usize,
    pub end: usize,
}

/// A type or custom option name as written, the declaration it is in (`None`
/// at file level) and the nested scope it resolves from.
pub(super) struct Reference {
    pub owner: Option<usize>,
    pub name: String,
    pub scope: String,
    pub start: usize,
    pub end: usize,
    pub option: bool,
}

#[derive(Default)]
pub(super) struct Proto {
    pub package: String,
    pub imports: Vec<String>,
    pub go_package: Option<String>,
    pub decls: Vec<Decl>,
    pub refs: Vec<Reference>,
    /// Braces did not balance: declarations left open are dropped.
    pub incomplete: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Tok<'a> {
    Word(&'a str),
    Str(&'a str),
    Punct(u8),
}

fn tokens(content: &str) -> Vec<(Tok<'_>, usize, usize)> {
    let b = content.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'.';
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if b[i..].starts_with(b"//") {
            i += b[i..]
                .iter()
                .position(|&c| c == b'\n')
                .unwrap_or(b.len() - i);
        } else if b[i..].starts_with(b"/*") {
            i += b[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(b.len() - i, |n| n + 4);
        } else if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            while j < b.len() && b[j] != c && b[j] != b'\n' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            let j = j.min(b.len());
            out.push((Tok::Str(&content[i + 1..j]), i, (j + 1).min(b.len())));
            i = j + 1;
        } else if word(c) {
            let j = i + b[i..].iter().position(|&c| !word(c)).unwrap_or(b.len() - i);
            out.push((Tok::Word(&content[i..j]), i, j));
            i = j;
        } else {
            // Multi-byte characters are only ever inside comments or strings
            // in valid input; step over the whole character.
            let n = content[i..].chars().next().map_or(1, char::len_utf8);
            if n == 1 {
                out.push((Tok::Punct(c), i, i + 1));
            }
            i += n;
        }
    }
    out
}

const SCALARS: &[&str] = &[
    "double", "float", "int32", "int64", "uint32", "uint64", "sint32", "sint64", "fixed32",
    "fixed64", "sfixed32", "sfixed64", "bool", "string", "bytes",
];

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Message,
    Enum,
    Service,
    Rpc,
    Oneof,
    Extend,
    Other,
}

struct Scope {
    kind: Kind,
    decl: Option<usize>,
    name: String,
}

struct Parser<'a> {
    toks: Vec<(Tok<'a>, usize, usize)>,
    i: usize,
    out: Proto,
    stack: Vec<Scope>,
}

impl<'a> Parser<'a> {
    fn at(&self, n: usize) -> Option<Tok<'a>> {
        self.toks.get(self.i + n).map(|t| t.0)
    }
    /// The nested name of the innermost message, enum or service.
    fn scope(&self) -> String {
        self.stack
            .iter()
            .rev()
            .find(|s| matches!(s.kind, Kind::Message | Kind::Enum | Kind::Service))
            .map_or(String::new(), |s| s.name.clone())
    }
    /// The declaration a field or option belongs to.
    fn owner(&self) -> Option<usize> {
        self.stack.iter().rev().find_map(|s| s.decl)
    }
    fn reference(&mut self, owner: Option<usize>, at: usize, option: bool) {
        let Some(&(Tok::Word(name), start, end)) = self.toks.get(at) else {
            return;
        };
        if option || !SCALARS.contains(&name) {
            let scope = self.scope();
            self.out.refs.push(Reference {
                owner,
                name: name.into(),
                scope,
                start,
                end,
                option,
            });
        }
    }
    /// Skips to the end of a statement (`;`, or a `{...}` body), recording
    /// each custom option `(name)` that opens an item of a `[...]` list.
    fn skip(&mut self, owner: Option<usize>) {
        let mut depth = 0usize;
        while let Some(tok) = self.at(0) {
            match tok {
                Tok::Punct(b'[') if depth == 0 => {
                    let mut inner = 0usize;
                    let mut item = true;
                    self.i += 1;
                    while let Some(tok) = self.at(0) {
                        match tok {
                            Tok::Punct(b'(') if item && inner == 0 => {
                                if let (Some(Tok::Word(_)), Some(Tok::Punct(b')'))) =
                                    (self.at(1), self.at(2))
                                {
                                    self.reference(owner, self.i + 1, true);
                                }
                                inner += 1;
                            }
                            Tok::Punct(b'[' | b'{' | b'(') => inner += 1,
                            Tok::Punct(b']') if inner == 0 => break,
                            Tok::Punct(b']' | b'}' | b')') => inner = inner.saturating_sub(1),
                            _ => {}
                        }
                        item = matches!(tok, Tok::Punct(b',')) && inner == 0;
                        self.i += 1;
                    }
                }
                Tok::Punct(b'{' | b'(') => depth += 1,
                Tok::Punct(b'}') if depth == 1 => {
                    self.i += 1;
                    return;
                }
                Tok::Punct(b'}') if depth == 0 => return,
                Tok::Punct(b'}' | b')') => depth = depth.saturating_sub(1),
                Tok::Punct(b';') if depth == 0 => {
                    self.i += 1;
                    return;
                }
                _ => {}
            }
            self.i += 1;
        }
    }
    fn open(&mut self, kind: Kind, decl: Option<(&str, &'static str, usize)>, name: &str) {
        let decl = decl.map(|(name, kind, start)| {
            self.out.decls.push(Decl {
                name: name.into(),
                kind,
                start,
                body_start: self.toks[self.i].2,
                end: 0,
            });
            self.out.decls.len() - 1
        });
        self.stack.push(Scope {
            kind,
            decl,
            name: name.into(),
        });
        self.i += 1;
    }
    fn nested(&self, name: &str) -> String {
        match self.scope() {
            s if s.is_empty() => name.into(),
            s => format!("{s}.{name}"),
        }
    }
    fn statement(&mut self) {
        let (tok, start, _) = self.toks[self.i];
        let inside = self.stack.last().map_or(Kind::Other, |s| s.kind);
        match (tok, self.at(1), self.at(2)) {
            (Tok::Punct(b'}'), ..) => {
                if let Some(d) = self.stack.pop().and_then(|scope| scope.decl) {
                    self.out.decls[d].end = self.toks[self.i].2;
                }
                self.i += 1;
            }
            (Tok::Word("package"), Some(Tok::Word(name)), _) if self.stack.is_empty() => {
                self.out.package = name.into();
                self.skip(None);
            }
            (Tok::Word("import"), ..) => {
                if let Some(Tok::Str(path)) = (1..3).find_map(|n| match self.at(n) {
                    Some(t @ Tok::Str(_)) => Some(t),
                    _ => None,
                }) {
                    self.out.imports.push(path.into());
                }
                self.skip(None);
            }
            (Tok::Word("option"), ..) => {
                let owner = self.owner();
                match (self.at(1), self.at(2), self.at(3), self.at(4)) {
                    (
                        Some(Tok::Word("go_package")),
                        Some(Tok::Punct(b'=')),
                        Some(Tok::Str(v)),
                        _,
                    ) if self.stack.is_empty() => {
                        self.out.go_package = Some(v.into());
                    }
                    (Some(Tok::Punct(b'(')), Some(Tok::Word(_)), Some(Tok::Punct(b')')), _) => {
                        self.reference(owner, self.i + 2, true);
                    }
                    _ => {}
                }
                self.skip(owner);
            }
            (
                Tok::Word(word @ ("message" | "enum" | "service")),
                Some(Tok::Word(name)),
                Some(Tok::Punct(b'{')),
            ) if inside != Kind::Enum => {
                let (kind, label) = match word {
                    "message" => (Kind::Message, "proto_message"),
                    "enum" => (Kind::Enum, "proto_enum"),
                    _ => (Kind::Service, "proto_service"),
                };
                let full = self.nested(name);
                self.i += 2;
                self.open(kind, Some((&full, label, start)), &full);
            }
            (Tok::Word("oneof"), Some(Tok::Word(_)), Some(Tok::Punct(b'{'))) => {
                self.i += 2;
                self.open(Kind::Oneof, None, "");
            }
            (Tok::Word("extend"), Some(Tok::Word(_)), Some(Tok::Punct(b'{'))) => {
                self.i += 2;
                self.open(Kind::Extend, None, "");
            }
            (Tok::Word("rpc"), Some(Tok::Word(name)), _) if inside == Kind::Service => {
                let full = self.nested(name);
                self.out.decls.push(Decl {
                    name: full.clone(),
                    kind: "proto_rpc",
                    start,
                    body_start: start,
                    end: 0,
                });
                let decl = self.out.decls.len() - 1;
                self.i += 2;
                // `(stream? Request) returns (stream? Response)`
                while let Some(tok) = self.at(0) {
                    match tok {
                        Tok::Punct(b'(') => {
                            let at =
                                self.i + 1 + usize::from(self.at(1) == Some(Tok::Word("stream")));
                            self.reference(Some(decl), at, false);
                        }
                        Tok::Punct(b';') => {
                            self.out.decls[decl].end = self.toks[self.i].2;
                            self.i += 1;
                            return;
                        }
                        Tok::Punct(b'{') => {
                            self.out.decls[decl].body_start = self.toks[self.i].2;
                            self.stack.push(Scope {
                                kind: Kind::Rpc,
                                decl: Some(decl),
                                name: String::new(),
                            });
                            self.i += 1;
                            return;
                        }
                        Tok::Punct(b'}') => return,
                        _ => {}
                    }
                    self.i += 1;
                }
            }
            (Tok::Word(_), ..) if matches!(inside, Kind::Message | Kind::Oneof | Kind::Extend) => {
                self.field(inside, start);
            }
            _ => {
                let owner = self.owner();
                self.skip(owner);
            }
        }
    }
    /// `[label] Type name = N [options];` or `map<K, V> name = N;`.
    fn field(&mut self, inside: Kind, start: usize) {
        let mut at = self.i;
        if let Tok::Word("optional" | "repeated" | "required") = self.toks[at].0 {
            at += 1;
        }
        let (ty, name) = match self
            .toks
            .get(at..at + 7)
            .map(|t| t.iter().map(|t| t.0).collect::<Vec<_>>())
        {
            Some(t) if t[0] == Tok::Word("map") && t[1] == Tok::Punct(b'<') => (at + 4, at + 6),
            _ => (at, at + 1),
        };
        let (Some((Tok::Word(_), ..)), Some((Tok::Word(field), ..))) =
            (self.toks.get(ty), self.toks.get(name))
        else {
            let owner = self.owner();
            return self.skip(owner);
        };
        let owner = if inside == Kind::Extend {
            let full = self.nested(field);
            self.out.decls.push(Decl {
                name: full,
                kind: "proto_extension",
                start,
                body_start: start,
                end: 0,
            });
            Some(self.out.decls.len() - 1)
        } else {
            self.owner()
        };
        self.reference(owner, ty, false);
        self.skip(owner);
        if inside == Kind::Extend {
            let d = owner.unwrap();
            self.out.decls[d].end = self.toks[self.i - 1].2;
        }
    }
}

pub(super) fn parse(content: &str) -> Proto {
    let mut p = Parser {
        toks: tokens(content),
        i: 0,
        out: Proto::default(),
        stack: vec![],
    };
    while p.i < p.toks.len() {
        let before = p.i;
        p.statement();
        if p.i == before {
            p.i += 1;
        }
    }
    let mut out = p.out;
    out.incomplete = !p.stack.is_empty() || out.decls.iter().any(|d| d.end == 0);
    // ponytail: a declaration left open is dropped with every reference it owns.
    let open: BTreeSet<usize> = (0..out.decls.len())
        .filter(|&d| out.decls[d].end == 0)
        .collect();
    out.refs
        .retain(|r| r.owner.is_none_or(|o| !open.contains(&o)));
    if !open.is_empty() {
        let mut renumber = vec![None; out.decls.len()];
        let mut kept = 0;
        for (d, slot) in renumber.iter_mut().enumerate() {
            if !open.contains(&d) {
                *slot = Some(kept);
                kept += 1;
            }
        }
        for r in &mut out.refs {
            r.owner = r.owner.and_then(|o| renumber[o]);
        }
        let mut d = 0;
        out.decls.retain(|_| {
            d += 1;
            !open.contains(&(d - 1))
        });
    }
    out
}

/// The line of a byte offset, from a table of the file's line starts.
pub(super) fn lines(content: &str) -> impl Fn(usize) -> usize {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(content.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    move |at| starts.partition_point(|&s| s <= at)
}

/// The full names a reference may mean, innermost scope first, as protobuf
/// resolves a relative name: `.a.B` is absolute.
pub(super) fn candidates(package: &str, scope: &str, name: &str) -> Vec<String> {
    if let Some(absolute) = name.strip_prefix('.') {
        return vec![absolute.into()];
    }
    let mut prefix: String = [package, scope]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(".");
    let mut out = vec![];
    loop {
        out.push(if prefix.is_empty() {
            name.into()
        } else {
            format!("{prefix}.{name}")
        });
        match prefix.rsplit_once('.') {
            Some((parent, _)) => prefix = parent.into(),
            None if !prefix.is_empty() => prefix.clear(),
            None => return out,
        }
    }
}

/// protoc-gen-go's Go name for a nested message or enum (`Outer.Inner`,
/// relative to the package): `strs.GoCamelCase` over the whole dotted name.
/// A dot becomes `_` unless a lowercase letter follows it, where it is
/// dropped (`Comment.postType` is `CommentPostType`).
pub(super) fn go_name(nested: &str) -> String {
    let s = nested.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        let lower_next = s.get(i + 1).is_some_and(u8::is_ascii_lowercase);
        match c {
            b'.' if lower_next => {}
            b'.' => out.push('_'),
            b'_' if i == 0 || s[i - 1] == b'.' => out.push('X'),
            b'_' if lower_next => {}
            c if c.is_ascii_digit() => out.push(c as char),
            c => {
                out.push(c.to_ascii_uppercase() as char);
                while s.get(i + 1).is_some_and(u8::is_ascii_lowercase) {
                    i += 1;
                    out.push(s[i] as char);
                }
            }
        }
        i += 1;
    }
    out
}

/// The type names a generated `.pb.go` declares.
pub(super) fn go_types(text: &str) -> BTreeSet<&str> {
    text.lines()
        .filter_map(|l| l.strip_prefix("type "))
        .filter_map(|l| l.split_once(' '))
        .filter(|(_, rest)| rest.starts_with("struct") || rest.starts_with("int32"))
        .map(|(name, _)| name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROTO: &str = r#"syntax = "proto3";
package nexus.protobuf; // the package
import "google/protobuf/wrappers.proto";
import public "proto/validate/validate.proto";
option go_package = "nexus/v1/config";

/* message Fake { } */
message Config {
  Proxy proxy = 1 [(buf.validate.field).required = true, deprecated = true];
  map<string, .other.Item> items = 2;
  repeated google.protobuf.StringValue names = 3;
  option (buf.validate.message).disabled = true;
  oneof choice { Kind kind = 4; string text = 5 [(x.y) = { a: "}" }]; }
  enum Kind { A = 0; B = 1 [(enum_opt) = 2]; }
  message Proxy { Kind kind = 1; }
}
service Discovery {
  rpc Watch(stream Config) returns (stream Config.Proxy) { option (auth) = "x"; }
  rpc Get(Config) returns (Config);
}
extend google.protobuf.FieldOptions { optional Config field = 1159; }
"#;

    #[test]
    fn declarations_and_references_follow_the_braces() {
        let p = parse(PROTO);
        assert!(!p.incomplete);
        assert_eq!(p.package, "nexus.protobuf");
        assert_eq!(
            p.imports,
            [
                "google/protobuf/wrappers.proto",
                "proto/validate/validate.proto"
            ]
        );
        assert_eq!(p.go_package.as_deref(), Some("nexus/v1/config"));
        let decls: Vec<_> = p.decls.iter().map(|d| (d.name.as_str(), d.kind)).collect();
        assert_eq!(
            decls,
            [
                ("Config", "proto_message"),
                ("Config.Kind", "proto_enum"),
                ("Config.Proxy", "proto_message"),
                ("Discovery", "proto_service"),
                ("Discovery.Watch", "proto_rpc"),
                ("Discovery.Get", "proto_rpc"),
                ("field", "proto_extension"),
            ]
        );
        let config = &p.decls[0];
        assert!(PROTO[config.start..config.end].starts_with("message Config {"));
        assert!(PROTO[config.start..config.end].ends_with("Kind kind = 1; }\n}"));
        let get = &p.decls[5];
        assert_eq!(
            &PROTO[get.start..get.end],
            "rpc Get(Config) returns (Config);"
        );
        let refs: Vec<_> = p
            .refs
            .iter()
            .map(|r| {
                (
                    r.owner.map(|o| p.decls[o].name.as_str()),
                    r.name.as_str(),
                    r.scope.as_str(),
                    r.option,
                )
            })
            .collect();
        assert_eq!(
            refs,
            [
                (Some("Config"), "Proxy", "Config", false),
                (Some("Config"), "buf.validate.field", "Config", true),
                (Some("Config"), ".other.Item", "Config", false),
                (
                    Some("Config"),
                    "google.protobuf.StringValue",
                    "Config",
                    false
                ),
                (Some("Config"), "buf.validate.message", "Config", true),
                (Some("Config"), "Kind", "Config", false),
                (Some("Config"), "x.y", "Config", true),
                (Some("Config.Kind"), "enum_opt", "Config.Kind", true),
                (Some("Config.Proxy"), "Kind", "Config.Proxy", false),
                (Some("Discovery.Watch"), "Config", "Discovery", false),
                (Some("Discovery.Watch"), "Config.Proxy", "Discovery", false),
                (Some("Discovery.Watch"), "auth", "Discovery", true),
                (Some("Discovery.Get"), "Config", "Discovery", false),
                (Some("Discovery.Get"), "Config", "Discovery", false),
                (Some("field"), "Config", "", false),
            ]
        );
    }

    #[test]
    fn an_unclosed_declaration_is_dropped_and_marked() {
        let p = parse("message A { B b = 1; }\nmessage C { D d = 1;\n");
        assert!(p.incomplete);
        assert_eq!(
            p.decls.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["A"]
        );
        assert_eq!(
            p.refs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["B"]
        );
    }

    #[test]
    fn a_file_cut_off_anywhere_parses_without_panicking() {
        for end in 0..=PROTO.len() {
            if PROTO.is_char_boundary(end) {
                parse(&PROTO[..end]);
            }
        }
        for cut in [
            "service S { rpc X(",
            "service S { rpc X(stream",
            "message A { map<",
            "option (",
        ] {
            assert!(parse(cut).decls.iter().all(|d| d.end > 0));
        }
    }

    #[test]
    fn names_resolve_innermost_scope_first() {
        assert_eq!(
            candidates("a.b", "Outer.Inner", "T"),
            ["a.b.Outer.Inner.T", "a.b.Outer.T", "a.b.T", "a.T", "T"]
        );
        assert_eq!(candidates("", "", "x.T"), ["x.T"]);
        assert_eq!(candidates("a", "", ".x.T"), ["x.T"]);
    }

    #[test]
    fn go_names_follow_protoc_gen_go() {
        assert_eq!(
            go_name("NexusServiceConfig.Proxy"),
            "NexusServiceConfig_Proxy"
        );
        // protoc-gen-go's strs.GoCamelCase over the whole nested name: a dot
        // before a lowercase letter is dropped, and `_` after a dot becomes `X`.
        assert_eq!(go_name("foo_bar.baz_qux"), "FooBarBazQux");
        assert_eq!(
            go_name("CommentCreated.postDataType"),
            "CommentCreatedPostDataType"
        );
        assert_eq!(go_name("Outer._inner"), "Outer_XInner");
        assert_eq!(go_name("Outer.Mid.Leaf"), "Outer_Mid_Leaf");
        assert_eq!(go_name("_x"), "XX");
        assert_eq!(go_name("http2Config"), "Http2Config");
        let go = "type Config struct {\ntype Config_Kind int32\ntype isConfig_Choice interface {\n";
        assert_eq!(go_types(go), BTreeSet::from(["Config", "Config_Kind"]));
    }

    #[test]
    fn a_large_file_parses_in_linear_time() {
        let text: String = (0..50_000)
            .map(|i| {
                format!(
                    "message M{i} {{ M{} next = 1 [(o) = {{ a: 1 }}]; }}\n",
                    i + 1
                )
            })
            .collect();
        let started = std::time::Instant::now();
        let p = parse(&text);
        assert_eq!((p.decls.len(), p.refs.len()), (50_000, 100_000));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
