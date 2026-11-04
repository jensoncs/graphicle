use super::*;

fn span(repo: &str, side: &str, file: &str, source: &str, start: usize, end: usize) -> Value {
    json!({"repository_id":repo,"side":side,"file":file,"file_sha256":text::sha256(source.as_bytes()),
        "line_start":start,"line_end":end})
}

fn outside_count(packet: &Value) -> usize {
    packet["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["file"] != "changed.go")
        .count()
}

#[test]
fn provided_diff_reallocates_budget_to_callers_and_dependencies_and_keeps_proofs_conservative() {
    let f = Fixture::new();
    let repo = f.repo("supplemental-context");
    let before = format!(
        "package tax\nfunc Target() int {{\n v := 1\n{} return Dependency(v)\n}}\n",
        (0..300).map(|i| format!(" v += {i}\n")).collect::<String>()
    );
    let after = before.replace("v := 1", "v := 2");
    let files = |source| {
        [
            ("go.mod", MOD),
            ("changed.go", source),
            (
                "dependency.go",
                "package tax\nfunc Dependency(v int) int {return v}\n",
            ),
            (
                "caller.go",
                "package tax\nfunc Caller() int {return Target()}\n",
            ),
        ]
    };
    let base = f.pin(&repo, "app", 'a', &files(&before));
    let head = f.pin(&repo, "app", 'b', &files(&after));
    let mut args = request(&repo, vec![base], vec![head]);
    let full = f.run(&args);
    let provided = json!([
        span(
            &repo,
            "base",
            "changed.go",
            &before,
            1,
            before.lines().count()
        ),
        span(
            &repo,
            "head",
            "changed.go",
            &after,
            1,
            after.lines().count()
        )
    ]);
    args["provided_diff"] = provided.clone();
    let supplemental = f.run(&args);
    assert_eq!(supplemental["impact"], full["impact"]);
    assert_eq!(supplemental["impact"], "potential_impact");
    assert_eq!(supplemental["coverage"]["provided_diff"]["entries"], 2);
    assert_eq!(supplemental["coverage"]["provided_diff"]["diffs"], 1);
    assert_eq!(supplemental["coverage"]["budget_omitted"], 0);
    assert_eq!(supplemental["coverage"]["complete"], false);
    assert_eq!(
        outside_count(&supplemental),
        supplemental["entries"].as_array().unwrap().len()
    );
    assert!(has(&supplemental, "head", "Caller"));
    assert!(has(&supplemental, "head", "Dependency"));
    assert!(
        supplemental["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["review_basis"]["complete"] != true)
    );
    assert!(!supplemental.to_string().contains("_provided_diff"));
    let mut improved = false;
    for budget in [3500, 5000, 6500] {
        args["max_tokens"] = json!(budget);
        args.as_object_mut().unwrap().remove("provided_diff");
        let plain = f.run(&args);
        args["provided_diff"] = provided.clone();
        let extra = f.run(&args);
        let a = outside_count(&plain);
        let b = outside_count(&extra);
        eprintln!(
            "provided_diff budget={budget}: additional entries {a} -> {b}, total {} -> {}",
            plain["entries"].as_array().unwrap().len(),
            extra["entries"].as_array().unwrap().len()
        );
        for (label, p) in [("plain", &plain), ("extra", &extra)] {
            eprintln!(
                "{budget} {label}: metadata={} tokens={} flow={} entries={:?}",
                p["coverage"]["packing"]["metadata_tokens"],
                p["tokens"],
                p["execution_flows"]["coverage"]["reserved_tokens"],
                p["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| (s(e, "side"), s(e, "file"), text::count(&e.to_string())))
                    .collect::<Vec<_>>()
            );
        }
        improved |= b > a;
        if budget >= 5000 {
            assert!(
                b > a,
                "additional context should improve at the pilot-sized budgets"
            );
        }
        assert!(text::count(&extra.to_string()) <= budget);
        assert_eq!(extra["impact"], plain["impact"]);
        let c = &extra["coverage"];
        assert_eq!(
            c["required_entries"].as_u64().unwrap(),
            extra["entries"].as_array().unwrap().len() as u64
                + c["provided_diff"]["entries"].as_u64().unwrap()
                + c["budget_omitted_entries"].as_u64().unwrap()
        );
    }
    assert!(
        improved,
        "supplemental mode must deliver more additional context at a tight budget"
    );
    // Real prompts supply hunks, not entire declarations. The long changed
    // function stays eligible, but must not spend the extra-context allowance
    // before its unchanged caller and dependency have been considered.
    let partial_ranges = json!([
        span(&repo, "base", "changed.go", &before, 3, 3),
        span(&repo, "head", "changed.go", &after, 3, 3)
    ]);
    for budget in [3500, 5000, 6500] {
        args["max_tokens"] = json!(budget);
        args.as_object_mut().unwrap().remove("provided_diff");
        let plain = f.run(&args);
        args["provided_diff"] = partial_ranges.clone();
        let extra = f.run(&args);
        eprintln!("partial provided_diff budget={budget}: outside entries {} -> {}, overlap selected {}", outside_count(&plain), outside_count(&extra), extra["coverage"]["provided_diff"]["selected_overlapping_entries"]);
        assert!(outside_count(&extra) >= 2, "{}", extra["coverage"]);
        assert!(has(&extra, "head", "Caller"));
        // Randomized repository IDs change token costs near 3,500: the second
        // affordable outside entry can be the base caller. Require the head
        // dependency at the budgets with enough room for its full evidence.
        if budget >= 5000 {
            assert!(has(&extra, "head", "Dependency"));
        }
        assert!(outside_count(&extra) >= outside_count(&plain));
        assert_eq!(extra["coverage"]["provided_diff"]["entries"], 0);
        assert_eq!(extra["coverage"]["provided_diff"]["overlapping_entries"], 2);
        assert_eq!(extra["coverage"]["provided_diff"]["selected_outside_entries"], outside_count(&extra));
        assert_eq!(extra["coverage"]["packing"]["policy"], "additional_context_then_overlap_v1");
        assert_eq!(extra["impact"], plain["impact"]);
        assert!(text::count(&extra.to_string()) <= budget);
        assert!(!extra.to_string().contains("_provided_diff"));
    }
    args["provided_diff"] = provided;
    args["max_tokens"] = json!(16000);
    args["previous_packet_id"] = full["packet_id"].clone();
    args["reviewed_entries"] = json!(
        full["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["side"] == "head")
            .map(|e| e["id"].clone())
            .collect::<Vec<_>>()
    );
    let delta = f.run(&args);
    assert_eq!(delta["context_order"], supplemental["context_order"]);
    for entry in full["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["file"] == "changed.go")
    {
        assert!(
            delta["invalidated"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == entry["id"])
        );
    }
    assert_eq!(delta["review_reuse"]["eligible"], json!([]));
    args.as_object_mut().unwrap().remove("provided_diff");
    args["reviewed_entries"] = json!([]);
    args["previous_packet_id"] = delta["packet_id"].clone();
    let restored = f.run(&args);
    assert_eq!(restored["context_order"], full["context_order"]);
}

#[test]
fn provided_ranges_require_full_containment_exact_side_hash_and_valid_lines() {
    let f = Fixture::new();
    let repo = f.repo("provided-range-validation");
    let old = "package tax\nfunc F() int {\n return 1\n}\n";
    let new = old.replace("return 1", "return 2");
    let base = f.pin(&repo, "app", 'a', &[("go.mod", MOD), ("code.go", old)]);
    let head = f.pin(&repo, "app", 'b', &[("go.mod", MOD), ("code.go", &new)]);
    let mut args = request(&repo, vec![base], vec![head]);
    let default_packet = f.run(&args);
    args["provided_diff"] = json!([]);
    assert_eq!(f.run(&args)["packet_id"], default_packet["packet_id"]);
    args["provided_diff"] = json!([span(&repo, "head", "code.go", &new, 3, 3)]);
    let partial = f.run(&args);
    assert!(has(&partial, "head", "F"));
    assert_eq!(partial["coverage"]["provided_diff"]["entries"], 0);
    args["provided_diff"] = json!([
        span(&repo, "head", "code.go", &new, 1, 2),
        span(&repo, "head", "code.go", &new, 4, 4)
    ]);
    assert!(has(&f.run(&args), "head", "F")); // A gap in supplied lines is not covered.
    args["provided_diff"] = json!([
        span(&repo, "head", "code.go", &new, 1, 2),
        span(&repo, "head", "code.go", &new, 3, 4)
    ]);
    let merged = f.run(&args);
    assert!(!has(&merged, "head", "F"));
    assert!(has(&merged, "base", "F"));
    assert_eq!(merged["coverage"]["provided_diff"]["ranges"], 1);
    assert_eq!(merged["coverage"]["provided_diff"]["diffs"], 0); // Base side is not supplied.
    for (field, value) in [
        ("file_sha256", json!(text::sha256(old.as_bytes()))),
        ("line_start", json!(0)),
        ("line_end", json!(5)),
        ("side", json!("other")),
        ("repository_id", json!("other")),
        ("file", json!("../code.go")),
    ] {
        let mut invalid = span(&repo, "head", "code.go", &new, 1, 4);
        invalid[field] = value;
        args["provided_diff"] = json!([invalid]);
        assert!(
            review::run(&f.store, &f.workspace, &f.token, &args).is_err(),
            "accepted invalid {field}"
        );
    }
    args["provided_diff"] = json!([
        span(&repo, "base", "code.go", old, 1, 4),
        span(&repo, "head", "code.go", &new, 1, 4)
    ]);
    let all = f.run(&args);
    assert_eq!(all["entries"], json!([]));
    assert_eq!(all["diffs"], json!([]));
    assert_eq!(all["empty_reason"], "already_provided_in_diff");
    assert_eq!(all["coverage"]["budget_omitted"], 0);
    assert_eq!(all["impact"], "potential_impact");
}

#[test]
fn provided_diff_does_not_suppress_another_pinned_repository_with_identical_bytes() {
    let f = Fixture::new();
    let anchor = f.repo("anchor");
    let peer = f.repo("peer");
    let old = "package tax\nfunc F() int {return 1}\n";
    let new = old.replace("return 1", "return 2");
    let mut base = vec![];
    let mut head = vec![];
    for (repo, directory) in [(&anchor, "app"), (&peer, "peer")] {
        base.push(f.pin(repo, directory, 'a', &[("go.mod", MOD), ("code.go", old)]));
        head.push(f.pin(repo, directory, 'b', &[("go.mod", MOD), ("code.go", &new)]));
    }
    let mut args = request(&anchor, base, head);
    args["provided_diff"] = json!([
        span(&peer, "base", "code.go", old, 1, 2),
        span(&peer, "head", "code.go", &new, 1, 2)
    ]);
    let packet = f.run(&args);
    assert_eq!(packet["coverage"]["provided_diff"]["entries"], 2);
    assert!(
        packet["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["repository_id"] == anchor)
    );
    assert!(has(&packet, "head", "F"));
    assert!(has(&packet, "base", "F"));
    assert!(
        packet["diffs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["file"] == "app/code.go")
    );
}
