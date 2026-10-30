//! Structured navigation over an authorized immutable episode. No source reads
//! or label-based target guesses occur here; the caller attaches checked cites.
use super::*;
use serde::Deserialize;

pub(super) fn specs() -> Vec<Value> {
    let mut specs = vec![
        json!({"name":"architecture_preview","description":"Preview dependency removal or module regrouping on an isolated copy of this authorized episode; compare coupling and cyclic module groups. Never applies changes or proves dependencies complete.","inputSchema":{"type":"object","properties":{"remove_nodes":{"type":"array","items":{"type":"string"}},"remove_edges":{"type":"array","items":{"type":"object","properties":{"source":{"type":"string"},"target":{"type":"string"},"relation":{"type":"string"}},"required":["source","target","relation"],"additionalProperties":false}},"modules":{"type":"object","additionalProperties":{"type":"string"}}},"additionalProperties":false}}),
        json!({"name":"statement_flow","description":"Analyze Go statement control gates and scalar value flow at an exact function node; cited source snapshots, bounded intra-procedural analysis, explicit unsupported constructs. Not taint analysis or observed execution.","inputSchema":{"type":"object","properties":{"node_id":{"type":"string"},"goos":{"type":"string"},"goarch":{"type":"string"},"tags":{"type":"array","items":{"type":"string"},"maxItems":32}},"required":["node_id"],"additionalProperties":false}}),
        json!({"name":"search_symbols","description":"Find callable declarations by exact parameter name/type, decorator or annotation spelling. Filters are ANDed; name and type must match the same parameter. Reports syntax/language coverage and stale episodes.","inputSchema":{"type":"object","properties":{"parameter_name":{"type":"string"},"parameter_type":{"type":"string"},"decorator":{"type":"string"},"annotation":{"type":"string"},"language":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}},"required":[],"additionalProperties":false}}),
        json!({"name":"execution_flows","description":"Return ordered, cited potential static invocation paths from an exact node ID. These are possible graph paths, not observed execution or a guarantee of runtime order. Reports cycles and traversal limits.","inputSchema":{"type":"object","properties":{"node_id":{"type":"string"},"max_depth":{"type":"integer","minimum":1,"maximum":16},"limit":{"type":"integer","minimum":1,"maximum":32}},"required":["node_id"],"additionalProperties":false}}),
    ];
    specs.rotate_left(2);
    specs
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Search {
    parameter_name: Option<String>,
    parameter_type: Option<String>,
    decorator: Option<String>,
    annotation: Option<String>,
    language: Option<String>,
    limit: Option<usize>,
}
fn bounded(value: Option<usize>, default: usize, max: usize) -> Result<usize> {
    let n = value.unwrap_or(default);
    if !(1..=max).contains(&n) {
        return Err(Error::bad(format!("Expected integer in 1..={max}")));
    }
    Ok(n)
}
fn search(g: &Graph, args: &Value) -> Result<(Value, Vec<usize>)> {
    let q: Search = serde_json::from_value(args.clone()).map_err(|e| Error::bad(e.to_string()))?;
    let limit = bounded(q.limit, 20, 100)?;
    let requested_supported = q
        .language
        .as_ref()
        .is_none_or(|l| graphicle::extractors::symbol_metadata::supported(l));
    for field in [
        &q.parameter_name,
        &q.parameter_type,
        &q.decorator,
        &q.annotation,
        &q.language,
    ]
    .into_iter()
    .flatten()
    {
        if field.is_empty() || field.len() > 256 {
            return Err(Error::bad("Symbol filters must contain 1–256 bytes"));
        }
    }
    let mut matches = vec![];
    let mut cited = vec![];
    let mut missing = 0;
    let mut partial = 0;
    let mut total = 0;
    let mut coverage = BTreeMap::<String, Value>::new();
    let mut unsupported_fields = BTreeMap::<String, BTreeSet<&str>>::new();
    for (index, node) in g.nodes().iter().enumerate() {
        let Some(variants) = node
            .attributes
            .get("symbol_metadata")
            .and_then(Value::as_array)
        else {
            if node.attributes.get("_callable") == Some(&json!(true))
                || label(g, index).ends_with("()")
            {
                missing += 1;
            }
            continue;
        };
        for m in variants {
            let language = s(m, "language");
            coverage.insert(language.into(), m["coverage"].clone());
            if m["complete"] != true {
                partial += 1;
            }
            if q.language.as_ref().is_some_and(|l| l != language) {
                continue;
            }
            for (filter, field) in [
                (&q.parameter_name, "parameter_name"),
                (&q.parameter_type, "parameter_type"),
                (&q.decorator, "decorator"),
                (&q.annotation, "annotation"),
            ] {
                if filter.is_some() && m["coverage"][field] != true {
                    unsupported_fields
                        .entry(language.into())
                        .or_default()
                        .insert(field);
                }
            }
            let parameters = m["parameters"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            if (q.parameter_name.is_some() || q.parameter_type.is_some())
                && !parameters.iter().any(|p| {
                    q.parameter_name.as_ref().is_none_or(|v| p["name"] == *v)
                        && q.parameter_type.as_ref().is_none_or(|v| p["type"] == *v)
                })
            {
                continue;
            }
            if [(&q.decorator, "decorators"), (&q.annotation, "annotations")]
                .iter()
                .any(|(q, key)| {
                    q.as_ref().is_some_and(|v| {
                        !m[*key]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|d| d["name"] == *v))
                    })
                })
            {
                continue;
            }
            total += 1;
            if matches.len() < limit {
                matches.push(json!({"node_id":node.id,"label":label(g,index),"metadata":m}));
                if !cited.contains(&index) {
                    cited.push(index);
                }
            }
        }
    }
    Ok((
        json!({"matches":matches,"coverage":{"basis":"syntax_exact_spelling","languages":coverage,"supported_languages":["python","javascript","typescript","go","java"],"missing_callable_metadata":missing,"partial_declarations":partial,"complete":missing==0 && partial==0 && requested_supported && unsupported_fields.is_empty() && total==matches.len(),"requested_language_supported":requested_supported,"unsupported_requested_fields":unsupported_fields,"omitted_matches":total-matches.len(),"type_aliases_resolved":false}}),
        cited,
    ))
}
pub(super) fn invocation(relation: &str) -> bool {
    matches!(
        relation,
        "calls"
            | "may_call"
            | "invokes_boundary"
            | "handles_boundary"
            | "http_calls"
            | "grpc_calls"
            | "event_delivers"
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Flow {
    node_id: String,
    max_depth: Option<usize>,
    limit: Option<usize>,
}
fn flows(g: &Graph, args: &Value) -> Result<(Value, Vec<usize>)> {
    let q: Flow = serde_json::from_value(args.clone()).map_err(|e| Error::bad(e.to_string()))?;
    if !g.is_directed() {
        return Ok((
            json!({"flows":[],"coverage":{"complete":false,"issues":["edge_direction_unavailable"]}}),
            vec![],
        ));
    }
    let depth = bounded(q.max_depth, 6, 16)?;
    let limit = bounded(q.limit, 8, 32)?;
    if q.node_id.is_empty() || q.node_id.len() > 2000 {
        return Err(Error::bad("Invalid node_id"));
    }
    let Some(start) = g.node_index(&json!(q.node_id)) else {
        return Ok((
            json!({"flows":[],"coverage":{"complete":false,"issues":["node_not_found"]}}),
            vec![],
        ));
    };
    let mut pending = VecDeque::from([(vec![start], Vec::<Value>::new())]);
    let mut paths = vec![];
    let mut ids = BTreeSet::new();
    let mut issues = BTreeSet::new();
    let mut visits = 0;
    while let Some((nodes, steps)) = pending.pop_front() {
        visits += 1;
        if visits > 2048 || paths.len() >= limit {
            issues.insert("path_budget");
            break;
        }
        let current = *nodes.last().unwrap();
        let edges: Vec<_> = g
            .out_edges(current)
            .into_iter()
            .filter(|(_, _, e)| {
                e.attributes
                    .get("relation")
                    .and_then(Value::as_str)
                    .is_some_and(invocation)
            })
            .collect();
        if edges.is_empty() || steps.len() >= depth {
            if !edges.is_empty() {
                issues.insert("depth_limit");
            }
            if !steps.is_empty() {
                ids.extend(nodes.iter().copied());
                paths.push(json!({"kind":"potential_static","observed":false,"node_ids":nodes.iter().map(|i|&g.nodes()[*i].id).collect::<Vec<_>>(),"steps":steps}));
            }
            continue;
        }
        let mut extended = false;
        for (_, target, edge) in edges {
            if nodes.contains(&target) {
                issues.insert("cycle");
                continue;
            }
            if pending.len() + visits >= 2048 {
                issues.insert("path_budget");
                break;
            }
            let mut next = nodes.clone();
            next.push(target);
            let mut evidence = steps.clone();
            evidence.push(json!({"caller":g.nodes()[current].id,"callee":g.nodes()[target].id,"evidence":edge.attributes}));
            pending.push_back((next, evidence));
            extended = true;
        }
        if !extended && !steps.is_empty() {
            ids.extend(nodes.iter().copied());
            paths.push(json!({"kind":"potential_static","observed":false,"node_ids":nodes.iter().map(|i|&g.nodes()[*i].id).collect::<Vec<_>>(),"steps":steps}));
        }
    }
    Ok((
        json!({"flows":paths,"coverage":{"complete":false,"path_selection":"bounded_static_paths","issues":issues,"max_depth":depth,"observed_execution_available":false,"graph_completeness_proven":false}}),
        ids.into_iter().collect(),
    ))
}
pub(super) fn run(g: &Graph, tool: &str, args: &Value) -> Result<(Value, Vec<usize>)> {
    match tool {
        "architecture_preview" => {
            let result = graphicle::architecture::preview(g, args)?;
            let mut ids = BTreeSet::new();
            ids.extend(
                args["remove_nodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            );
            ids.extend(
                args["modules"]
                    .as_object()
                    .into_iter()
                    .flat_map(|m| m.keys().map(String::as_str)),
            );
            for e in args["remove_edges"].as_array().into_iter().flatten() {
                for field in ["source", "target"] {
                    if let Some(id) = e[field].as_str() {
                        ids.insert(id);
                    }
                }
            }
            Ok((
                result,
                ids.into_iter()
                    .filter_map(|id| g.node_index(&json!(id)))
                    .collect(),
            ))
        }
        "search_symbols" => search(g, args),
        "execution_flows" => flows(g, args),
        _ => Err(Error::bad("Unknown structured navigation tool")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn graph() -> Graph {
        let mut g = Graph::new(true, true);
        for id in ["handler", "validate", "rpc", "publish", "consume"] {
            g.add_node(
                json!(id),
                json!({"label":format!("{id}()"),"source_file":"app.go","source_location":"L1"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap();
        }
        for (a, b, r) in [
            ("handler", "validate", "calls"),
            ("validate", "rpc", "grpc_calls"),
            ("rpc", "publish", "calls"),
            ("publish", "consume", "event_delivers"),
        ] {
            g.add_edge(
                json!(a),
                json!(b),
                None,
                json!({"relation":r,"source_file":"app.go","source_location":"L1"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap();
        }
        g
    }
    #[test]
    fn flows_are_ordered_potential_paths_and_limits_are_visible() {
        let g = graph();
        let (v, _) = flows(&g, &json!({"node_id":"handler"})).unwrap();
        assert_eq!(
            v["flows"][0]["node_ids"],
            json!(["handler", "validate", "rpc", "publish", "consume"])
        );
        assert_eq!(v["flows"][0]["observed"], false);
        let (v, _) = flows(&g, &json!({"node_id":"handler","max_depth":1})).unwrap();
        assert_eq!(v["coverage"]["issues"], json!(["depth_limit"]));
        let (v, _) = flows(&g, &json!({"node_id":"missing"})).unwrap();
        assert_eq!(v["coverage"]["issues"], json!(["node_not_found"]));
        assert!(flows(&g, &json!({"node_id":"handler","max_depth":0})).is_err());
    }
    #[test]
    fn search_matches_name_and_type_on_the_same_parameter_not_unrelated_symbols() {
        let mut g = graph();
        let metadata = json!({"version":1,"language":"python","complete":true,"coverage":{"parameter_name":true,"parameter_type":true},"parameters":[{"name":"tenant","type":"Tenant"},{"name":"admin","type":"Root"}],"decorators":[{"name":"route.post"}],"annotations":[]});
        g.add_node(
            json!("handler"),
            json!({"symbol_metadata":[metadata]})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
        let (v, _) = search(
            &g,
            &json!({"parameter_name":"tenant","parameter_type":"Root"}),
        )
        .unwrap();
        assert_eq!(v["matches"], json!([]));
        let (v, _) = search(
            &g,
            &json!({"parameter_name":"tenant","parameter_type":"Tenant","decorator":"route.post"}),
        )
        .unwrap();
        assert_eq!(v["matches"].as_array().unwrap().len(), 1);
        assert_eq!(v["coverage"]["missing_callable_metadata"], 4);
        assert_eq!(v["coverage"]["complete"], false);
        let (v, _) = search(&g, &json!({"decorator":"missing"})).unwrap();
        assert_eq!(v["matches"], json!([]));
        assert!(search(&g, &json!({"parameter_name":"x","limit":0})).is_err());
    }
    #[test]
    fn references_are_not_execution_and_cycles_do_not_hang() {
        let mut g = graph();
        g.add_edge(
            json!("consume"),
            json!("handler"),
            None,
            json!({"relation":"calls"}).as_object().unwrap().clone(),
        )
        .unwrap();
        let (v, _) = flows(&g, &json!({"node_id":"handler"})).unwrap();
        assert!(
            v["coverage"]["issues"]
                .as_array()
                .unwrap()
                .contains(&json!("cycle"))
        );
        let mut g = Graph::new(true, false);
        g.add_edge(
            json!("a"),
            json!("b"),
            None,
            json!({"relation":"references_callable"})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
        let (v, _) = flows(&g, &json!({"node_id":"a"})).unwrap();
        assert_eq!(v["flows"], json!([]));
    }
    #[test]
    fn unsupported_fields_and_overload_limits_do_not_claim_complete_search() {
        let mut g = Graph::new(true, false);
        let metadata = json!({"language":"javascript","complete":true,"coverage":{"parameter_name":true,"parameter_type":false},"parameters":[{"name":"value","type":null}]});
        g.add_node(
            json!("f"),
            json!({"symbol_metadata":[metadata,metadata]})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
        let (v, _) = search(&g, &json!({"parameter_type":"Tenant"})).unwrap();
        assert_eq!(v["coverage"]["complete"], false);
        assert_eq!(
            v["coverage"]["unsupported_requested_fields"]["javascript"],
            json!(["parameter_type"])
        );
        let (v, _) = search(&g, &json!({"parameter_name":"value","limit":1})).unwrap();
        assert_eq!(v["coverage"]["omitted_matches"], 1);
        assert_eq!(v["coverage"]["complete"], false);
    }
}

#[cfg(test)]
mod cgc_regression {
    use super::*;
    // Adapted fixture: CodeGraphContext/tests/unit/tools/test_find_functions_by_argument.py
    // test_argument_search_filters_functions_on_a_real_graph (MIT).
    // License retained in crates/graphicle/licenses/CodeGraphContext-MIT.txt.
    #[test]
    fn cgc_argument_search_excludes_unrelated_functions() {
        let mut g = Graph::new(true, false);
        for (name, param, ty) in [
            ("byParamName", "orderFilter", None),
            ("byArgType", "f", Some("OrderFilter")),
            ("unrelated", "x", Some("String")),
        ] {
            g.add_node(json!(name),json!({"label":name,"symbol_metadata":[{"language":"java","complete":true,"coverage":{"parameter_name":true,"parameter_type":true},"parameters":[{"name":param,"type":ty}],"annotations":[],"decorators":[]}]}).as_object().unwrap().clone()).unwrap();
        }
        let (v, _) = search(&g, &json!({"parameter_type":"OrderFilter"})).unwrap();
        assert_eq!(v["matches"].as_array().unwrap().len(), 1);
        assert_eq!(v["matches"][0]["node_id"], "byArgType");
        let (v, _) = search(&g, &json!({"parameter_name":"orderFilter"})).unwrap();
        assert_eq!(v["matches"].as_array().unwrap().len(), 1);
        assert_eq!(v["matches"][0]["node_id"], "byParamName");
    }
}
