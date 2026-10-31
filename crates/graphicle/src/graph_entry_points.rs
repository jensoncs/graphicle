//! Entry navigation scans the full immutable episode, not a sampled neighborhood.
//! Lack of incoming calls is only a candidate signal, never endpoint proof.
use super::*;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Request {
    #[serde(default)]
    query: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

pub(super) fn run(g: &Graph, args: &Value) -> Result<(Value, Vec<usize>)> {
    let request: Request =
        serde_json::from_value(args.clone()).map_err(|e| Error::bad(e.to_string()))?;
    let limit = bounded(request.limit, 50, 100)?;
    if request.query.len() > 256
        || request.offset > 1_000_000
        || !matches!(
            request.category.as_str(),
            "" | "all" | "declared" | "candidate"
        )
        || !matches!(
            request.kind.as_str(),
            "" | "all"
                | "http_route"
                | "bash_script"
                | "program_entry_name"
                | "no_recorded_incoming_calls"
        )
    {
        return Err(Error::bad("Invalid entry-point filters"));
    }
    let mut incoming = vec![0usize; g.node_count()];
    let mut outgoing = vec![0usize; g.node_count()];
    let mut handlers = BTreeMap::<usize, BTreeSet<usize>>::new();
    for (from, to, edge) in g.iter_edges() {
        let relation = edge
            .attributes
            .get("relation")
            .and_then(Value::as_str)
            .unwrap_or("");
        if matches!(
            relation,
            "calls" | "may_call" | "http_calls" | "grpc_calls" | "event_delivers" | "invokes"
        ) {
            if from != to {
                incoming[to] += 1;
            }
            outgoing[from] += 1;
        }
        // Existing route extraction stores handler -> route, not the reverse.
        if relation == "handles" {
            handlers.entry(to).or_default().insert(from);
        }
    }
    let query = request.query.to_lowercase();
    let mut entries = Vec::new();
    let mut declared = 0usize;
    let mut candidates = 0usize;
    let mut by_kind = BTreeMap::<&str, usize>::new();
    for (i, node) in g.nodes().iter().enumerate() {
        let attr = &node.attributes;
        let name = label(g, i);
        let file = attr
            .get("source_file")
            .and_then(Value::as_str)
            .unwrap_or("");
        let kind = attr.get("node_kind").and_then(Value::as_str).unwrap_or("");
        let metadata = attr.get("metadata").unwrap_or(&Value::Null);
        let callable = attr.get("_callable") == Some(&json!(true)) || name.ends_with("()");
        let (category, basis) = if kind == "route"
            && attr.contains_key("route_method")
            && attr.contains_key("route_path")
        {
            ("declared", "http_route")
        } else if metadata["kind"] == "bash_entrypoint" {
            ("declared", "bash_script")
        } else if callable && matches!(name.as_str(), "main()" | "main") {
            ("candidate", "program_entry_name")
        } else if callable && incoming[i] == 0 {
            ("candidate", "no_recorded_incoming_calls")
        } else {
            continue;
        };
        *by_kind.entry(basis).or_default() += 1;
        if category == "declared" {
            declared += 1;
        } else {
            candidates += 1;
        }
        if !matches!(request.kind.as_str(), "" | "all") && request.kind != basis {
            continue;
        }
        if !matches!(request.category.as_str(), "" | "all") && request.category != category {
            continue;
        }
        if !query.is_empty() && !format!("{name} {file}").to_lowercase().contains(&query) {
            continue;
        }
        let owners: Vec<_> = handlers.get(&i).into_iter().flatten().map(|index| {
            let handler = &g.nodes()[*index];
            json!({"node_id":handler.id,"label":label(g,*index),"source_file":handler.attributes.get("source_file"),"source_location":handler.attributes.get("source_location")})
        }).collect();
        entries.push((
            i,
            json!({"node_id":node.id,"label":name,"source_file":file,
            "source_location":attr.get("source_location"),"category":category,"basis":basis,
            "outgoing_calls":outgoing[i],"handlers":owners,
            "ambiguous_handler":handlers.get(&i).is_some_and(|h|h.len()>1)}),
        ));
    }
    entries.sort_by(|(_, a), (_, b)| {
        let rank = |v: &Value| {
            if v["category"] == "declared" {
                0
            } else if v["basis"] == "program_entry_name" {
                1
            } else {
                2
            }
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| s(a, "source_file").cmp(s(b, "source_file")))
            .then_with(|| s(a, "node_id").cmp(s(b, "node_id")))
    });
    let total = entries.len();
    let page: Vec<_> = entries
        .into_iter()
        .skip(request.offset)
        .take(limit)
        .collect();
    let indices = page.iter().map(|(i, _)| *i).collect();
    let next = request.offset.saturating_add(page.len());
    Ok((
        json!({"entries":page.into_iter().map(|(_,v)|v).collect::<Vec<_>>(),"total":total,
        "counts":{"declared":declared,"candidates":candidates,"by_kind":by_kind},
        "next_offset":if next<total {Some(next)} else {None},
        "coverage":{"complete":false,"scan":"all_nodes_in_selected_episode","nodes_scanned":g.node_count(),
            "observed_execution_available":false,"declared_support":["recorded_http_routes","bash_script_metadata"],
            "limitations":["Call roots and main-named functions are candidates, not proven runtime entry points", "Dynamic registration, gRPC and event entry discovery require additional extraction or configured boundary evidence", "Missing call edges can create false root candidates; absence of a detected entry is not absence of an endpoint"]}}),
        indices,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(g: &mut Graph, id: &str, attrs: Value) {
        g.add_node(json!(id), attrs.as_object().unwrap().clone())
            .unwrap();
    }
    fn edge(g: &mut Graph, a: &str, b: &str, relation: &str) {
        g.add_edge(
            json!(a),
            json!(b),
            None,
            json!({"relation":relation}).as_object().unwrap().clone(),
        )
        .unwrap();
    }
    #[test]
    fn scans_beyond_neighborhood_and_pages_without_repeating() {
        let mut g = Graph::new(true, true);
        for i in 0..130 {
            node(
                &mut g,
                &format!("n{i:03}"),
                json!({"label":"Handle()","source_file":"api.go"}),
            );
        }
        let (a, _) = run(&g, &json!({"limit":100})).unwrap();
        let (b, _) = run(&g, &json!({"limit":100,"offset":100})).unwrap();
        assert_eq!(a["total"], 130);
        assert_eq!(a["next_offset"], 100);
        assert_eq!(b["entries"].as_array().unwrap().len(), 30);
        assert!(b["next_offset"].is_null());
        assert_ne!(a["entries"][99]["node_id"], b["entries"][0]["node_id"]);
        assert_eq!(a["coverage"]["complete"], false);
    }
    #[test]
    fn routes_keep_handler_ambiguity_and_recursive_roots_are_candidates() {
        let mut g = Graph::new(true, true);
        node(
            &mut g,
            "route",
            json!({"label":"GET /users","node_kind":"route","route_method":"GET","route_path":"/users"}),
        );
        for id in ["a", "b", "helper"] {
            node(
                &mut g,
                id,
                json!({"label":format!("{id}()"),"source_file":"app.go"}),
            );
        }
        edge(&mut g, "a", "route", "handles");
        edge(&mut g, "b", "route", "handles");
        edge(&mut g, "a", "helper", "may_call");
        edge(&mut g, "a", "a", "calls");
        let (v, _) = run(&g, &json!({})).unwrap();
        assert_eq!(v["entries"][0]["category"], "declared");
        assert_eq!(v["entries"][0]["ambiguous_handler"], true);
        assert_eq!(v["entries"][0]["handlers"].as_array().unwrap().len(), 2);
        assert!(
            !v["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["node_id"] == "helper")
        );
        let (c, _) = run(&g, &json!({"category":"candidate","query":"app.go"})).unwrap();
        assert_eq!(c["total"], 2);
    }
    #[test]
    fn bad_filters_and_unclassified_cycles_do_not_claim_completeness() {
        let mut g = Graph::new(true, true);
        for id in ["a", "b"] {
            node(&mut g, id, json!({"label":format!("{id}()")}));
        }
        edge(&mut g, "a", "b", "calls");
        edge(&mut g, "b", "a", "calls");
        let (v, _) = run(&g, &json!({})).unwrap();
        assert_eq!(v["total"], 0);
        assert_eq!(v["coverage"]["complete"], false);
        assert!(run(&g, &json!({"category":"definitely_runtime"})).is_err());
    }

    #[test]
    fn entry_kinds_keep_generic_roots_out_of_the_program_view() {
        let mut g = Graph::new(true, true);
        node(
            &mut g,
            "main",
            json!({"label":"main()","source_file":"main.go"}),
        );
        node(
            &mut g,
            "helper",
            json!({"label":"helper()","source_file":"main.go"}),
        );
        node(
            &mut g,
            "script",
            json!({"label":"setup script","metadata":{"kind":"bash_entrypoint"}}),
        );
        let (v, _) = run(&g, &json!({"kind":"program_entry_name"})).unwrap();
        assert_eq!(v["total"], 1);
        assert_eq!(v["entries"][0]["node_id"], "main");
        assert_eq!(v["counts"]["by_kind"]["no_recorded_incoming_calls"], 1);
        assert_eq!(v["counts"]["by_kind"]["bash_script"], 1);
        assert_eq!(v["counts"]["candidates"], 2);
        assert!(run(&g, &json!({"kind":"unknown"})).is_err());
    }
}
