use super::*;

#[test]
fn java_only_packet_explains_empty_entries_without_claiming_complete_impact() {
    let f = Fixture::new();
    let repo = f.repo("java-only");
    let base = f.pin(
        &repo,
        "app",
        'a',
        &[("src/Index.java", "class Index { int value = 1; }\n")],
    );
    let head = f.pin(
        &repo,
        "app",
        'b',
        &[("src/Index.java", "class Index { int value = 2; }\n")],
    );
    let packet = f.run(&request(&repo, vec![base], vec![head]));
    assert_eq!(packet["entries"], json!([]));
    assert_eq!(
        packet["empty_reason"],
        "unsupported_source_language_or_contract"
    );
    assert_eq!(packet["impact"], "incomplete");
    assert_eq!(packet["coverage"]["budget_omitted_entries"], 0);
    assert_eq!(
        packet["coverage"]["issue_histogram"]["unsupported_count"],
        2
    );
}

/// TypeScript has basic file-level impact: its changed file is an entry and
/// no unsupported-language issue is raised, while impact stays incomplete.
#[test]
fn typescript_only_packet_has_basic_entries_and_incomplete_impact() {
    let f = Fixture::new();
    let repo = f.repo("typescript-only");
    let base = f.pin(
        &repo,
        "app",
        'a',
        &[("src/index.ts", "export const value = 1;\n")],
    );
    let head = f.pin(
        &repo,
        "app",
        'b',
        &[("src/index.ts", "export const value = 2;\n")],
    );
    let packet = f.run(&request(&repo, vec![base], vec![head]));
    assert!(
        packet["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["file"] == "src/index.ts" && e["language"] == "typescript"),
        "{packet:#}"
    );
    assert_eq!(packet["impact"], "incomplete");
    assert!(
        packet["coverage"]["issue_histogram"]["unsupported_count"]
            .as_u64()
            .unwrap_or(0)
            == 0,
        "{}",
        packet["coverage"]["issue_histogram"]
    );
}

#[test]
fn unsupported_entries_are_distinct_from_omitted_raw_diffs() {
    let f = Fixture::new();
    let repo = f.repo("unsupported-large-diff");
    let before = "static int a = 1;\n".repeat(4000);
    let after = "static int a = 2;\n".repeat(4000);
    let base = f.pin(&repo, "app", 'a', &[("Index.java", &before)]);
    let head = f.pin(&repo, "app", 'b', &[("Index.java", &after)]);
    let mut args = request(&repo, vec![base], vec![head]);
    args["max_tokens"] = json!(3825);
    let packet = f.run(&args);
    assert_eq!(
        packet["empty_reason"],
        "unsupported_source_language_or_contract"
    );
    assert_eq!(packet["coverage"]["required_entries"], 0);
    assert_eq!(packet["coverage"]["budget_omitted_entries"], 0);
    assert!(packet["coverage"]["budget_omitted_diffs"].as_u64().unwrap() > 0);
    assert_eq!(
        packet["coverage"]["budget_omitted"],
        packet["coverage"]["budget_omitted_diffs"]
    );
    assert!(text::count(&packet.to_string()) <= 3825);
}

#[test]
fn empty_unchanged_and_no_change_packets_have_distinct_reasons() {
    let f = Fixture::new();
    let repo = f.repo("empty-reasons");
    let base = f.pin(
        &repo,
        "app",
        'a',
        &[
            ("go.mod", MOD),
            ("code.go", "package tax\nfunc F() int {return 1}\n"),
        ],
    );
    let unchanged = f.run(&request(&repo, vec![base.clone()], vec![base.clone()]));
    assert_eq!(unchanged["entries"], json!([]));
    assert_eq!(unchanged["empty_reason"], "no_changes_in_scope");
    let head = f.pin(
        &repo,
        "app",
        'b',
        &[
            ("go.mod", MOD),
            ("code.go", "package tax\nfunc F() int {return 2}\n"),
        ],
    );
    let mut args = request(&repo, vec![base], vec![head]);
    let first = f.run(&args);
    assert!(first.get("empty_reason").is_none());
    args["previous_packet_id"] = first["packet_id"].clone();
    let delta = f.run(&args);
    assert_eq!(delta["entries"], json!([]));
    assert_eq!(delta["empty_reason"], "unchanged_from_previous_packet");
}

#[test]
fn unsupported_breakdown_counts_both_sides_beyond_issue_sample() {
    let f = Fixture::new();
    let repo = f.repo("unsupported");
    let mut owned: Vec<(String, String)> = (0..24)
        .map(|i| (format!("db/{i:02}.sql"), "select 1;\n".into()))
        .collect();
    for (path, content) in [
        ("api.proto", "syntax = \"proto3\";\n"),
        ("config.yaml", "replicas: 1\n"),
        ("config.json", "{\"replicas\":1}\n"),
        ("Dockerfile", "FROM scratch\n"),
        ("entrypoint", "#!/usr/bin/custom\nrun\n"),
        ("native.S", ".text\n"),
        ("README.md", "Documentation\n"),
    ] {
        owned.push((path.into(), content.into()));
    }
    let files: Vec<_> = owned
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let base = f.pin(&repo, "app", 'a', &files);
    let head = f.pin(&repo, "app", 'b', &files);
    let mut args = request(&repo, vec![base], vec![head]);
    let packet = f.run(&args);
    let histogram = &packet["coverage"]["issue_histogram"];
    assert_eq!(packet["impact"], "incomplete");
    assert_eq!(packet["coverage"]["issues"].as_array().unwrap().len(), 20);
    assert_eq!(histogram["unsupported_count"], 60);
    assert_eq!(histogram["counted"], packet["coverage"]["issue_count"]);
    assert_eq!(histogram["counted"], 60);
    assert_eq!(histogram["other_count"], 0);
    let groups = histogram["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 14);
    for side in ["base", "head"] {
        for (extension, cause, count) in [
            ("sql", "code_extension", 24),
            ("yaml", "code_extension", 1),
            ("json", "code_extension", 1),
            ("proto", "contract_extension", 1),
            ("", "build_filename", 1),
            ("", "shebang", 1),
            ("s", "assembly_extension", 1),
        ] {
            assert!(
                groups.contains(&json!({"reason":"unsupported_source_language_or_contract",
                "side":side,"extension":extension,"cause":cause,"count":count})),
                "missing {side}/{extension}/{cause}"
            );
        }
    }
    let examples = histogram["unsupported_examples"].as_array().unwrap();
    assert!(!examples.is_empty());
    assert!(examples.len() <= 8);
    assert_eq!(
        histogram["unsupported_examples_omitted"].as_u64().unwrap(),
        60 - examples.len() as u64
    );
    for example in examples {
        assert!(
            files
                .iter()
                .any(|(p, _)| format!("app/{p}") == s(example, "file"))
        );
        assert!(example.get("text").is_none());
    }
    args["max_tokens"] = json!(8000);
    let smaller = f.run(&args);
    assert_eq!(
        smaller["coverage"]["issue_histogram"]["groups"],
        histogram["groups"]
    );
    assert_eq!(
        smaller["coverage"]["issue_histogram"]["unsupported_count"],
        60
    );
    assert_eq!(smaller["impact"], packet["impact"]);
    assert!(text::count(&smaller.to_string()) <= 8000);
}
