//! Sibling services that each ship `src/classifier.py` make `src.classifier` an
//! ambiguous module name for the repository, but not for a file inside one
//! service: only that service's root is an ancestor of the importer, so only
//! its copy is on the importer's path.
use graphicle::api::{ExtractOptions, extract};
use serde_json::Value;
use std::path::PathBuf;

const CLASSIFIER: &str =
    "class NSFWClassifier:\n    def predict(self, path):\n        return path\n";
const SERVER: &str = "from src.classifier import NSFWClassifier\n\nclassifier = NSFWClassifier()\n\n\
def handle(path):\n    return classifier.predict(path)\n";

fn run(receiver_resolution: bool, services: &[&str]) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut paths: Vec<PathBuf> = vec![];
    for service in services {
        for (name, text) in [("src/classifier.py", CLASSIFIER), ("src/server.py", SERVER)] {
            let path = root.join(service).join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            paths.push(path);
        }
    }
    let options = ExtractOptions {
        root: Some(root.clone()),
        cache_root: Some(root.clone()),
        parallel: false,
        receiver_resolution,
        ..Default::default()
    };
    extract(&paths, &options).unwrap()
}

fn calls(result: &Value) -> Vec<(String, String)> {
    result["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["relation"] == "calls" && e["target"].as_str().unwrap_or("").ends_with("_predict")
        })
        .map(|e| {
            (
                e["source"].as_str().unwrap().into(),
                e["target"].as_str().unwrap().into(),
            )
        })
        .collect()
}

#[test]
fn each_service_binds_its_own_module() {
    let mut got = calls(&run(true, &["nsfw", "nsfw_moj"]));
    got.sort();
    assert_eq!(
        got,
        [
            (
                "nsfw_moj_src_server_handle",
                "nsfw_moj_src_classifier_nsfwclassifier_predict"
            ),
            (
                "nsfw_src_server_handle",
                "nsfw_src_classifier_nsfwclassifier_predict"
            ),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
    );
}

#[test]
fn parity_mode_still_leaves_the_name_ambiguous() {
    assert!(calls(&run(false, &["nsfw", "nsfw_moj"])).is_empty());
}

const GEN: &str = "class Gen:\n    def make(self):\n        return 1\n";
const TRAIN: &str = "from dataset.gen import Gen\n\n\ndef main():\n    return Gen().make()\n";

/// ad-relevance layout: each sub-project root carries an `__init__.py` but a hyphenated name, which no
/// import statement can spell, so the directory is only ever a path root for the scripts under it.
fn run_hyphenated(services: &[&str]) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut paths: Vec<PathBuf> = vec![];
    for service in services {
        for (name, text) in [
            ("__init__.py", ""),
            ("dataset/__init__.py", ""),
            ("dataset/gen.py", GEN),
            ("train/__init__.py", ""),
            ("train/run.py", TRAIN),
        ] {
            let path = root.join(service).join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            paths.push(path);
        }
    }
    let options = ExtractOptions {
        root: Some(root.clone()),
        cache_root: Some(root.clone()),
        parallel: false,
        receiver_resolution: true,
        precise: true,
        ..Default::default()
    };
    extract(&paths, &options).unwrap()
}

#[test]
fn a_hyphenated_root_with_an_initializer_is_still_a_path_root() {
    let mut got = run_hyphenated(&["ads-ctr", "ads-cvr"])["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["relation"] == "calls" && e["target"].as_str().unwrap_or("").ends_with("_make")
        })
        .map(|e| {
            (
                e["source"].as_str().unwrap().to_string(),
                e["target"].as_str().unwrap().to_string(),
            )
        })
        .collect::<Vec<_>>();
    got.sort();
    assert_eq!(
        got,
        [
            ("ads_ctr_train_run_main", "ads_ctr_dataset_gen_gen_make"),
            ("ads_cvr_train_run_main", "ads_cvr_dataset_gen_gen_make"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
    );
}
