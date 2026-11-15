//! Package-level Go constants and variables are definitions an importer
//! names (`resourcev3.EndpointType`, `corev3.SocketAddress_TCP`), so precise
//! mode gives each exported one a node in its file; default output keeps the
//! upstream node set.
use graphicle::api::{ExtractOptions, extract};
use serde_json::Value;

const SOURCE: &str = "package resource\n\
\n\
import \"errors\"\n\
\n\
const APITypePrefix = \"type.googleapis.com/\"\n\
\n\
const (\n\
\tEndpointType = APITypePrefix + \"envoy.config.endpoint.v3.ClusterLoadAssignment\"\n\
\tinternalType = \"x\"\n\
)\n\
\n\
type SocketAddress_Protocol int32\n\
\n\
const (\n\
\tSocketAddress_TCP SocketAddress_Protocol = 0\n\
\tSocketAddress_UDP SocketAddress_Protocol = 1\n\
)\n\
\n\
var ErrNotFound = errors.New(\"not found\")\n\
\n\
var (\n\
\tDefaultA, DefaultB = 1, 2\n\
\tcache = map[string]int{}\n\
)\n\
\n\
func Lookup() int {\n\
\tconst Local = 3\n\
\tvar Inner = Local\n\
\treturn Inner\n\
}\n";

fn graph(precise: bool) -> (Value, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("pkg/resource/resource.go");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, SOURCE).unwrap();
    let options = ExtractOptions {
        root: Some(root.clone()),
        cache_root: Some(root.clone()),
        parallel: false,
        precise,
        ..Default::default()
    };
    let graph = extract(std::slice::from_ref(&path), &options).unwrap();
    (graph, path.to_string_lossy().into_owned())
}

fn labels(graph: &Value) -> Vec<String> {
    let mut out: Vec<String> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["label"].as_str().map(str::to_string))
        .collect();
    out.sort();
    out
}

#[test]
fn precise_mode_declares_exported_package_constants_and_variables() {
    let (graph, _) = graph(true);
    let labels = labels(&graph);
    for name in [
        "APITypePrefix",
        "EndpointType",
        "SocketAddress_TCP",
        "SocketAddress_UDP",
        "ErrNotFound",
        "DefaultA",
        "DefaultB",
    ] {
        assert!(
            labels.iter().any(|l| l == name),
            "{name} missing: {labels:?}"
        );
    }
    for name in ["internalType", "cache", "Local", "Inner"] {
        assert!(
            !labels.iter().any(|l| l == name),
            "{name} must not be declared"
        );
    }
    let nodes = graph["nodes"].as_array().unwrap();
    let file = nodes
        .iter()
        .find(|n| n["label"] == "resource.go")
        .expect("file node");
    let value = nodes.iter().find(|n| n["label"] == "EndpointType").unwrap();
    assert_eq!(value["source_file"], file["source_file"]);
    assert_eq!(value["source_location"], "L8");
    assert!(
        graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "contains"
                && e["source"] == file["id"]
                && e["target"] == value["id"]),
        "file contains the constant"
    );
}

#[test]
fn default_mode_keeps_the_upstream_node_set() {
    let (graph, _) = graph(false);
    let labels = labels(&graph);
    assert!(
        !labels
            .iter()
            .any(|l| l == "EndpointType" || l == "ErrNotFound")
    );
}
