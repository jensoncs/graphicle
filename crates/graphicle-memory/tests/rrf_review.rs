use graphicle_memory::retrieval;
use serde_json::json;

#[test]
fn a_zero_weight_query_variant_cannot_add_candidates() {
    let mut request = json!({
        "scope": "zero-weight-review", "question": "cats", "limit": 6,
        "rows": [
            {"identity": "cats", "origin": "cats", "title": "", "text": "cats", "kind": "source"},
            {"identity": "dogs", "origin": "dogs", "title": "", "text": "dogs", "kind": "source"}
        ]
    });
    let plain = retrieval::rank(&request).unwrap();
    assert_eq!(plain["sources"], json!(["cats"]));
    request["query_variants"] = json!([{"text": "dogs", "weight": 0.0}]);
    let disabled = retrieval::rank(&request).unwrap();
    assert_eq!(disabled["sources"], plain["sources"]);
}

#[test]
fn disabling_all_rrf_arms_returns_no_candidates() {
    let result = retrieval::rrf_weighted(&[vec!["only".into()]], &[0.0], 61.0, 0.0).unwrap();
    assert!(result.is_empty());
}

/// argus replay: the relationship lift scores every passage of a file related
/// to the best one at about half the top score, more than a memory's
/// lexical score; 400 lifted passages filled the fused cap and no reviewed
/// memory reached selection, even the one matching the question best.
#[test]
fn lifted_passages_cannot_push_every_memory_out_of_the_fused_order() {
    let mut rows = vec![
        json!({"identity": "hub", "origin": "hub", "title": "", "text": "slack notification webhook",
               "kind": "source", "source": {"path": "hub.go"}}),
        json!({"identity": "memory:m", "origin": "memory:m", "title": "", "kind": "memory",
               "text": "slack notification webhook is best effort"}),
    ];
    let mut edges = Vec::new();
    for i in 0..450 {
        let path = format!("f{i}.go");
        rows.push(json!({"identity": format!("n{i}"), "origin": path, "title": "", "text": format!("filler {i}"),
                         "kind": "source", "source": {"path": path}}));
        edges.push(json!({"source": "hub.go", "target": path}));
    }
    let ranked = retrieval::rank(&json!({
        "scope": "lift-review", "question": "slack notification webhook", "limit": 8,
        "rows": rows, "edges": edges,
        "rankings": {"code_graph": ["hub"], "graph_local": ["hub"]}
    }))
    .unwrap();
    assert_eq!(ranked["memories"], json!(["memory:m"]));
}
