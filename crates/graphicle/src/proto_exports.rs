//! Go packages generated from `.proto` files at build time. A repository
//! that commits the `.proto` sources but not the generated `.pb.go` still
//! exports the packages their `go_package` options name: each becomes a
//! module of the repository's export index (derivation `proto:go_package`,
//! linked `INFERRED`) with the identifiers protoc-gen-go and
//! protoc-gen-go-grpc generate. Committed Go code at the same path wins.
use crate::review::basic_proto::{self, Proto};
use graphicle_memory::graph::cross_repo_imports::{
    Ecosystem, ExportIndex, ModuleExport, PackageIdentity, SymbolRef,
};
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Derivation of a module built from `.proto` sources.
pub const DERIVATION: &str = "proto:go_package";

/// The `.proto` file a node id minted here stands for (`proto:<relpath>` or
/// `proto:<relpath>#<Name>`): such ids have no code-graph node, so readers
/// of cross-repository edges find the file through this.
pub fn relpath_of(node_id: &str) -> Option<&str> {
    let rest = node_id.strip_prefix("proto:")?;
    Some(rest.split_once('#').map_or(rest, |(path, _)| path))
}

const SKIP_DIRS: &[&str] = &["vendor/", "third_party/", "node_modules/", "testdata/"];

/// Whether `relpath` is a `.proto` source that may define exported packages
/// (not a vendored or test copy).
pub fn is_proto(relpath: &str) -> bool {
    relpath.ends_with(".proto")
        && !SKIP_DIRS
            .iter()
            .any(|dir| relpath.starts_with(dir) || relpath.contains(&format!("/{dir}")))
}

/// The Go import path of a `go_package` option (`path;name` is `path`);
/// `None` for one without a slash, which names no importable path.
fn import_path(go_package: &str) -> Option<&str> {
    let path = go_package.split(';').next()?.trim();
    path.contains('/').then_some(path)
}

/// `text` with comments blanked to spaces, so byte offsets still match.
fn without_comments(text: &str) -> String {
    static COMMENT: OnceLock<Regex> = OnceLock::new();
    let comment = COMMENT.get_or_init(|| Regex::new(r"(?s)//[^\n]*|/\*.*?\*/").unwrap());
    comment
        .replace_all(text, |c: &regex::Captures| " ".repeat(c[0].len()))
        .into_owned()
}

/// The innermost message whose body holds byte `at`.
fn message_at(proto: &Proto, at: usize) -> Option<&str> {
    proto
        .decls
        .iter()
        .filter(|d| d.kind == "proto_message" && d.body_start <= at && at < d.end)
        .min_by_key(|d| d.end - d.body_start)
        .map(|d| d.name.as_str())
}

/// Enum value constants (`Enum_VALUE`; `Parent_VALUE` for an enum nested in
/// a message) and oneof wrapper types (`Message_Field`).
fn value_identifiers(proto: &Proto, text: &str) -> Vec<(String, &'static str)> {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    static ONEOF: OnceLock<Regex> = OnceLock::new();
    static FIELD: OnceLock<Regex> = OnceLock::new();
    let value = VALUE.get_or_init(|| Regex::new(r"(?:^|[;{\s])([A-Za-z_]\w*)\s*=\s*-?\d").unwrap());
    let oneof = ONEOF.get_or_init(|| Regex::new(r"\boneof\s+\w+\s*\{([^}]*)\}").unwrap());
    let field = FIELD.get_or_init(|| Regex::new(r"[\w.]+\s+([A-Za-z_]\w*)\s*=\s*\d").unwrap());
    let code = without_comments(text);
    let mut out = Vec::new();
    for decl in proto.decls.iter().filter(|d| d.kind == "proto_enum") {
        let Some(body) = code.get(decl.body_start..decl.end) else {
            continue;
        };
        let prefix = match decl.name.rsplit_once('.') {
            Some((parent, _)) => basic_proto::go_name(parent),
            None => basic_proto::go_name(&decl.name),
        };
        for c in value.captures_iter(body) {
            out.push((format!("{prefix}_{}", &c[1]), "const"));
        }
    }
    for c in oneof.captures_iter(&code) {
        let Some(message) = message_at(proto, c.get(0).unwrap().start()) else {
            continue;
        };
        let message = basic_proto::go_name(message);
        for f in field.captures_iter(&c[1]) {
            out.push((
                format!("{message}_{}", basic_proto::go_name(&f[1])),
                "struct",
            ));
        }
    }
    out
}

/// The Go identifiers generated for one file's declarations, with a kind.
fn go_identifiers(proto: &Proto, text: &str) -> Vec<(String, &'static str)> {
    let mut out = value_identifiers(proto, text);
    for decl in &proto.decls {
        let name = basic_proto::go_name(&decl.name);
        match decl.kind {
            "proto_message" => out.push((name, "struct")),
            "proto_enum" => {
                out.push((format!("{name}_name"), "var"));
                out.push((format!("{name}_value"), "var"));
                out.push((name, "type"));
            }
            "proto_service" => {
                for id in [
                    format!("{name}Client"),
                    format!("{name}Server"),
                    format!("New{name}Client"),
                    format!("Register{name}Server"),
                    format!("Unimplemented{name}Server"),
                    format!("Unsafe{name}Server"),
                    format!("{name}_ServiceDesc"),
                ] {
                    out.push((id, "service"));
                }
            }
            "proto_rpc" => {
                if let Some((service, method)) = decl.name.rsplit_once('.') {
                    out.push((
                        format!("{}_{method}_FullMethodName", basic_proto::go_name(service)),
                        "const",
                    ));
                }
            }
            "proto_extension" => out.push((format!("E_{name}"), "var")),
            _ => {}
        }
    }
    out
}

fn dir_of(relpath: &str) -> String {
    relpath
        .rsplit_once('/')
        .map_or(String::new(), |(dir, _)| dir.to_string())
}

/// `index` with a module for each `go_package` of `protos` (relpath, text)
/// that no committed Go code exports: a path under one of the index's Go
/// modules, or one with a segment naming the repository (`slug`), which then
/// gains a Go package claiming it.
pub fn with_proto_packages(
    index: ExportIndex,
    protos: &[(String, String)],
    slug: &str,
) -> ExportIndex {
    let mut by_path: BTreeMap<String, Vec<(&str, &str, Proto)>> = BTreeMap::new();
    for (relpath, text) in protos {
        if !is_proto(relpath) {
            continue;
        }
        let proto = basic_proto::parse(text);
        let Some(path) = proto.go_package.as_deref().and_then(import_path) else {
            continue;
        };
        by_path
            .entry(path.to_string())
            .or_default()
            .push((relpath, text, proto));
    }
    let mut packages = index.packages;
    let mut modules = index.modules;
    for (path, files) in by_path {
        if modules.contains_key(&path) {
            continue;
        }
        let first = files[0].0;
        let claimed = packages.iter().any(|p| {
            p.ecosystem == Ecosystem::Go
                && p.import_roots
                    .iter()
                    .any(|root| path == *root || path.starts_with(&format!("{root}/")))
        });
        // A copy of someone else's `.proto` (googleapis, a service's own
        // copy of a shared contract) must not claim their package.
        if !claimed && !path.split('/').any(|segment| segment == slug) {
            continue;
        }
        if !claimed {
            packages.push(PackageIdentity {
                ecosystem: Ecosystem::Go,
                published_as: None,
                name: path.clone(),
                version: None,
                root: dir_of(first),
                manifest: first.to_string(),
                import_roots: vec![path.clone()],
                dependencies: BTreeMap::new(),
                exports_map: BTreeMap::new(),
                main: None,
                source_prefix: None,
            });
        }
        let mut symbols = BTreeMap::new();
        for (relpath, text, proto) in &files {
            for (name, kind) in go_identifiers(proto, text) {
                symbols.entry(name.clone()).or_insert_with(|| SymbolRef {
                    node_id: format!("proto:{relpath}#{name}"),
                    name,
                    stable_id: None,
                    kind: kind.to_string(),
                    relpath: relpath.to_string(),
                });
            }
        }
        modules.insert(
            path.clone(),
            ModuleExport {
                module_path: path,
                node_id: format!("proto:{first}"),
                stable_id: None,
                relpath: first.to_string(),
                derivation: DERIVATION.to_string(),
                symbols,
            },
        );
    }
    ExportIndex {
        packages,
        modules,
        ..index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"syntax = "proto3";
package feed.v1;
option go_package = "github.com/acme/protos/proto/feed;feedpb";
message GetFeedRequest {
  message Page { int32 size = 1; }
  // oneof in a comment { string fake = 9; }
  enum Mode { MODE_A = 0; MODE_B = 1; }
  oneof kind { string text_value = 2; Page page_ref = 3; }
}
enum FeedKind { option allow_alias = true; FEED_KIND_UNSPECIFIED = 0; FEED_KIND_HOT = -1; }
service FeedService { rpc GetFeed(GetFeedRequest) returns (GetFeedRequest); }
"#;

    fn index(packages: Vec<PackageIdentity>) -> ExportIndex {
        ExportIndex {
            repo: "protos".into(),
            packages,
            ..Default::default()
        }
    }

    #[test]
    fn a_go_package_becomes_a_module_with_generated_names() {
        let out = with_proto_packages(
            index(vec![]),
            &[("proto/feed/feed.proto".into(), FEED.into())],
            "protos",
        );
        let module = &out.modules["github.com/acme/protos/proto/feed"];
        assert_eq!(module.derivation, DERIVATION);
        assert_eq!(module.relpath, "proto/feed/feed.proto");
        for name in [
            "GetFeedRequest",
            "GetFeedRequest_Page",
            "FeedKind",
            "FeedKind_name",
            "FeedServiceClient",
            "NewFeedServiceClient",
            "RegisterFeedServiceServer",
            "UnimplementedFeedServiceServer",
            "FeedService_GetFeed_FullMethodName",
            "FeedKind_FEED_KIND_UNSPECIFIED",
            "FeedKind_FEED_KIND_HOT",
            "GetFeedRequest_MODE_A",
            "GetFeedRequest_TextValue",
            "GetFeedRequest_PageRef",
        ] {
            assert!(
                module.symbols.contains_key(name),
                "{name}: {:?}",
                module.symbols.keys()
            );
        }
        assert_eq!(relpath_of(&module.node_id), Some("proto/feed/feed.proto"));
        assert_eq!(
            relpath_of(&module.symbols["GetFeedRequest"].node_id),
            Some("proto/feed/feed.proto")
        );
        assert_eq!(relpath_of("feed_service_index"), None);
        assert!(!module.symbols.contains_key("GetFeedRequest_Fake"));
        assert!(!module.symbols.contains_key("FeedKind_allow_alias"));
        // No Go package claimed the path: the proto claims it.
        assert_eq!(out.packages.len(), 1);
        assert_eq!(
            out.packages[0].import_roots,
            vec!["github.com/acme/protos/proto/feed".to_string()]
        );
    }

    #[test]
    fn committed_go_code_and_skipped_or_bare_protos_add_nothing() {
        let committed = ModuleExport {
            module_path: "github.com/acme/protos/proto/feed".into(),
            node_id: "feed".into(),
            stable_id: None,
            relpath: "proto/feed/feed.pb.go".into(),
            derivation: "path".into(),
            symbols: BTreeMap::new(),
        };
        let mut with_code = index(vec![]);
        with_code
            .modules
            .insert(committed.module_path.clone(), committed.clone());
        let out = with_proto_packages(
            with_code,
            &[("proto/feed/feed.proto".into(), FEED.into())],
            "protos",
        );
        assert_eq!(out.modules["github.com/acme/protos/proto/feed"], committed);
        let bare = FEED.replace("github.com/acme/protos/proto/feed;feedpb", "feedpb");
        let out = with_proto_packages(
            index(vec![]),
            &[
                ("vendor/acme/feed.proto".into(), FEED.into()),
                ("proto/bare.proto".into(), bare),
            ],
            "protos",
        );
        assert!(out.modules.is_empty() && out.packages.is_empty());
    }

    /// A copy of another repository's `.proto` exports nothing: the path is
    /// neither under one of this repository's Go modules nor named for it.
    #[test]
    fn a_copied_proto_is_not_exported() {
        let copied = with_proto_packages(
            index(vec![]),
            &[("api/feed.proto".into(), FEED.into())],
            "feed-service",
        );
        assert!(copied.modules.is_empty() && copied.packages.is_empty());
        let own_module = PackageIdentity {
            ecosystem: Ecosystem::Go,
            published_as: None,
            name: "github.com/acme/protos".into(),
            version: None,
            root: "compiled/go".into(),
            manifest: "compiled/go/go.mod".into(),
            import_roots: vec!["github.com/acme/protos".into()],
            dependencies: BTreeMap::new(),
            exports_map: BTreeMap::new(),
            main: None,
            source_prefix: None,
        };
        let owned = with_proto_packages(
            index(vec![own_module]),
            &[("api/feed.proto".into(), FEED.into())],
            "renamed-checkout",
        );
        assert!(
            owned
                .modules
                .contains_key("github.com/acme/protos/proto/feed")
        );
        assert_eq!(
            owned.packages.len(),
            1,
            "the module already claims the path"
        );
    }
}
