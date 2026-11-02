use super::*;
use graphicle_memory::structure;

const DEPLOY: &str = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: app\n  namespace: company\nspec:\n  template:\n    spec:\n      containers:\n        - name: app\n          image: app:v1\n          env:\n            - name: TIMEOUT\n              value: \"10\"\n            - name: OTHER\n              value: \"unchanged\"\n";
const SETTINGS: &str =
    "import os as runtime\ndef timeout():\n    return runtime.getenv('TIMEOUT', '10')\n";
const CALLER: &str = "from settings import timeout\ndef run():\n    return timeout()\n";
fn issue(packet: &Value, reason: &str) -> bool {
    packet["coverage"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["reason"] == reason)
}
fn names(packet: &Value) -> Vec<&str> {
    packet["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["side"] == "head")
        .map(|e| s(e, "file"))
        .collect()
}
fn request_full(repo: &str, base: Value, head: Value) -> Value {
    let mut args = request(repo, vec![base], vec![head]);
    args["max_tokens"] = json!(64000);
    args
}
fn manifest(directory: &str) -> String {
    json!({"format":"graphicle.service-boundaries.v2","inventory_complete":true,"bindings":[],"environment_scopes":[{
        "id":"python-env","service":{"namespace":"company","name":"app","environment":"prod","configuration":"v1"},
        "module":".","deployment":{"directory":directory,"file":"deployment.yaml","container":"app"}}]}).to_string()
}
#[test]
fn python_env_forms_are_source_located_and_not_strings_or_comments() {
    let code = r#"import os as runtime
from os import getenv as read, environ as environment
a = runtime.getenv("TIMEOUT")
b = runtime.environ["TIMEOUT"]
c = runtime.environ.get("TIMEOUT", "10")
d = read(key="TIMEOUT")
e = environment['TIMEOUT']
f = environment.get('TIMEOUT')
# runtime.getenv("NOT_A_READ")
text = 'environment.get("NOT_A_READ")'
"#;
    let parsed = structure::analyze("settings.py", code).unwrap();
    let env = &parsed["environment"];
    assert_eq!(env["issues"], json!([]), "{env}");
    assert_eq!(env["operations"].as_array().unwrap().len(), 6, "{env}");
    for op in env["operations"].as_array().unwrap() {
        assert_eq!(op["key"], "TIMEOUT");
        assert_eq!(op["kind"], "read");
        let span = &code[op["byte_start"].as_u64().unwrap() as usize
            ..op["byte_end"].as_u64().unwrap() as usize];
        assert!(span.contains("TIMEOUT") && !span.contains("NOT_A_READ"));
    }
}
#[test]
fn python_rebinding_and_conditional_imports_do_not_invent_standard_library_reads() {
    for code in [
        "import os\ndef f(os):\n    return os.getenv('TIMEOUT')\n",
        "import os\nos = object()\na = os.getenv('TIMEOUT')\n",
        "import os\nos.getenv = lambda key: 'fake'\na = os.getenv('TIMEOUT')\n",
        "if enabled:\n    import os\na = os.getenv('TIMEOUT')\n",
        "from os import environ as e\nfor e in items:\n    print(e['TIMEOUT'])\n",
        "import os\nfrom fake import os\na = os.getenv('TIMEOUT')\n",
    ] {
        let parsed = structure::analyze("settings.py", code).unwrap();
        assert!(
            parsed["environment"]["operations"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{code}: {parsed}"
        );
        assert!(
            !parsed["environment"]["issues"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
#[test]
fn python_default_mapping_includes_reader_and_import_consumers_and_measures_scope() {
    let f = Fixture::new();
    let repo = f.repo("python-env");
    let unrelated: Vec<_> = (0..30)
        .map(|i| {
            (
                format!("unrelated{i}.py"),
                format!("def unrelated(): return {i}\n"),
            )
        })
        .collect();
    let changed = DEPLOY.replace("\"10\"", "\"20\"");
    let build = |commit, deploy: &str, disable: bool| {
        let mut files = vec![
            ("settings.py", SETTINGS),
            ("caller.py", CALLER),
            ("other.py", "import os\nVALUE = os.environ.get('OTHER')\n"),
            ("deployment.yaml", deploy),
        ];
        files.extend(unrelated.iter().map(|(p, c)| (p.as_str(), c.as_str())));
        if disable {
            files.push(("graphicle-services.json",r#"{"format":"graphicle.service-boundaries.v2","inventory_complete":true,"bindings":[],"environment_scopes":[]}"#));
        }
        f.pin(&repo, "app", commit, &files)
    };
    let baseline = f.run(&request_full(
        &repo,
        build('a', DEPLOY, true),
        build('b', &changed, true),
    ));
    let base = build('c', DEPLOY, false);
    let head = build('d', &changed, false);
    let packet = f.run(&request_full(&repo, base.clone(), head.clone()));
    let entries = names(&packet);
    assert!(
        entries.contains(&"settings.py") && entries.contains(&"caller.py"),
        "{packet}"
    );
    assert!(!entries.contains(&"other.py"));
    assert!(!entries.iter().any(|name| name.starts_with("unrelated")));
    assert!(!issue(&packet, "uncertain_file_scope"));
    assert_eq!(packet["impact"], "incomplete");
    let stats = &packet["service_graph"]["head"]["environment"]["scopes"][0];
    assert_eq!(stats["literal_reads"], 2);
    assert_eq!(stats["resolved_reads"], 2);
    assert_eq!(stats["narrowing_eligible"], true);
    assert!(
        packet["entries"]
            .to_string()
            .contains("python_import_binding")
    );
    let count = |p: &Value| {
        p["entries"].as_array().unwrap().len()
            + p["diffs"].as_array().unwrap().len()
            + p["coverage"]["budget_omitted"].as_u64().unwrap() as usize
    };
    eprintln!(
        "python environment probe: candidates {} -> {}; affected files 2/2; unrelated files 0/30",
        count(&baseline),
        count(&packet)
    );
    assert!(count(&baseline) > 60 && count(&packet) < 10);
    let mut args = request_full(&repo, base, head);
    args["previous_packet_id"] = packet["packet_id"].clone();
    assert!(f.run(&args)["entries"].as_array().unwrap().is_empty());
}
#[test]
fn python_dynamic_keys_mutations_and_local_os_modules_keep_conservative_scope() {
    let f = Fixture::new();
    let repo = f.repo("python-env-uncertain");
    for (code, extra, reason) in [
        (
            "from os import *\nVALUE=getenv('TIMEOUT')\n",
            None,
            "environment_python_analysis_incomplete",
        ),
        (
            "os=__import__('os')\nVALUE=os.getenv('TIMEOUT')\n",
            None,
            "environment_python_analysis_incomplete",
        ),
        (
            "from os import path\nVALUE=path.expandvars('$TIMEOUT')\n",
            None,
            "environment_python_analysis_incomplete",
        ),
        (
            "import third_party\nimport os\nVALUE=os.getenv('TIMEOUT')\n",
            None,
            "environment_python_import_unresolved",
        ),
        (
            "from os import environ as e\ndel e['TIMEOUT']\n",
            None,
            "environment_runtime_override",
        ),
        (
            "import os\nVALUE = os.getenv(KEY)\n",
            None,
            "environment_dynamic_read",
        ),
        (
            "import os\nos.environ['TIMEOUT'] = '99'\nVALUE = os.getenv('TIMEOUT')\n",
            None,
            "environment_runtime_override",
        ),
        (
            "import os\nos.environ.update({'TIMEOUT':'99'})\n",
            None,
            "environment_runtime_override",
        ),
        (
            "from os import putenv\nputenv('TIMEOUT','99')\n",
            None,
            "environment_runtime_override",
        ),
        (
            "import os\ndef f(os): return os.getenv('TIMEOUT')\n",
            None,
            "environment_python_analysis_incomplete",
        ),
        (
            SETTINGS,
            Some(("os.py", "def getenv(key): return 'fake'\n")),
            "environment_python_analysis_incomplete",
        ),
    ] {
        let pins = [DEPLOY.to_owned(), DEPLOY.replace("\"10\"", "\"20\"")].map(|deploy| {
            let mut files = vec![
                ("settings.py", code),
                ("unrelated.py", "def unrelated(): return 1\n"),
                ("deployment.yaml", deploy.as_str()),
            ];
            files.extend(extra);
            f.pin(&repo, "app", 'a', &files)
        });
        let packet = f.run(&request_full(&repo, pins[0].clone(), pins[1].clone()));
        assert!(issue(&packet, reason), "{code}: {packet}");
        assert!(issue(&packet, "uncertain_file_scope"));
        assert!(names(&packet).contains(&"unrelated.py"));
        if extra.is_some() {
            assert_eq!(
                packet["service_graph"]["head"]["environment"]["scopes"][0]["resolved_reads"],
                0
            );
        }
    }
}
#[test]
fn python_explicit_scope_can_use_a_separately_admitted_deployment_repository() {
    let f = Fixture::new();
    let repo = f.repo("python-app");
    let deploy = f.repo("python-deploy");
    let m = manifest("deploy");
    let app = f.pin(
        &repo,
        "app",
        'a',
        &[
            ("settings.py", SETTINGS),
            ("caller.py", CALLER),
            ("graphicle-services.json", &m),
        ],
    );
    let base = f.pin(&deploy, "deploy", 'a', &[("deployment.yaml", DEPLOY)]);
    let head = f.pin(
        &deploy,
        "deploy",
        'b',
        &[("deployment.yaml", &DEPLOY.replace("\"10\"", "\"20\""))],
    );
    let mut args = request(&repo, vec![app.clone(), base], vec![app, head]);
    args["max_tokens"] = json!(64000);
    let packet = f.run(&args);
    assert!(names(&packet).contains(&"settings.py") && names(&packet).contains(&"caller.py"));
    assert!(!issue(&packet, "environment_compiler_scope_unavailable"));
    assert!(!issue(&packet, "uncertain_file_scope"));
}

#[test]
fn python_environment_limits_invalidate_partial_parse_cache_entries() {
    let code = "import os\na=os.environ['TIMEOUT']\nb=os.environ['OTHER']\nc=os.environ['THIRD']\n";
    let limited = structure::analyze_with(
        "settings.py",
        code,
        &structure::Limits {
            calls: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        limited["environment"]["operations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(limited["limited"], "calls");
    assert!(!structure::limit_issues(&limited).is_empty());
    assert!(!structure::declaration_spans_complete(&limited));
    assert_eq!(
        structure::analyze("settings.py", code).unwrap()["environment"]["operations"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let invalid = structure::analyze("settings.py", "import os\nvalue = os.getenv(\n").unwrap();
    assert!(
        invalid["environment"]["operations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !invalid["environment"]["issues"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
