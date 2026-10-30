//! Hypothetical topology changes on an isolated graph; never writes an episode.
use crate::{Result, graph::Graph};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Scenario {
    #[serde(default)]
    remove_nodes: Vec<String>,
    #[serde(default)]
    remove_edges: Vec<Removal>,
    #[serde(default)]
    modules: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Removal {
    source: String,
    target: String,
    relation: String,
}
const RELATIONS: &[&str] = &[
    "calls",
    "may_call",
    "imports",
    "imports_from",
    "uses_type",
    "uses_value",
    "http_calls",
    "grpc_calls",
    "event_delivers",
    "invokes_boundary",
    "handles_boundary",
];

fn module(g: &Graph, i: usize) -> String {
    let a = &g.nodes()[i].attributes;
    a.get("scenario_module")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let path = a.get("source_file").and_then(Value::as_str).unwrap_or("");
            if path.is_empty() {
                "<unassigned>".into()
            } else {
                path.rsplit_once('/').map_or(".", |(p, _)| p).into()
            }
        })
}
fn metrics(g: &Graph) -> Result<Value> {
    let names: Vec<_> = (0..g.node_count())
        .map(|i| module(g, i))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if names.len() > 2048 {
        return Err("Architecture module limit exceeded (2048)".into());
    }
    let positions: BTreeMap<_, _> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let assignments: Vec<_> = (0..g.node_count())
        .map(|i| positions[module(g, i).as_str()])
        .collect();
    let mut links = BTreeMap::<(usize, usize), usize>::new();
    let (mut internal, mut external, mut considered) = (0usize, 0usize, 0usize);
    for (a, b, e) in g.iter_edges() {
        if !e
            .attributes
            .get("relation")
            .and_then(Value::as_str)
            .is_some_and(|r| RELATIONS.contains(&r))
        {
            continue;
        }
        considered += 1;
        if assignments[a] == assignments[b] {
            internal += 1;
        } else {
            external += 1;
            *links.entry((assignments[a], assignments[b])).or_default() += 1;
        }
    }
    let mut adjacency = vec![vec![]; names.len()];
    let mut fan_in = vec![0; names.len()];
    for &(a, b) in links.keys() {
        fan_in[b] += 1;
        adjacency[a].push(b);
    }
    let order: Vec<_> = (0..names.len()).collect();
    let mut cycles: Vec<_> =
        crate::graph_algorithms::nx_scc(&order, &|i| adjacency[i].clone(), names.len())
            .into_iter()
            .filter(|c| c.len() > 1)
            .map(|c| {
                let mut labels: Vec<_> = c.into_iter().map(|i| names[i].clone()).collect();
                labels.sort();
                labels
            })
            .collect();
    cycles.sort();
    let modules: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(i, n)| json!({"name":n,"fan_in":fan_in[i],"fan_out":adjacency[i].len()}))
        .collect();
    Ok(
        json!({"nodes":g.node_count(),"dependency_edges":considered,"internal_edges":internal,"cross_module_edges":external,"coupling_pairs":links.len(),"cyclic_module_components":cycles,"modules":modules,"unassigned_nodes":(0..g.node_count()).filter(|i|module(g,*i)=="<unassigned>").count()}),
    )
}
pub fn preview(graph: &Graph, args: &Value) -> Result<Value> {
    if !graph.is_directed() {
        return Err("Architecture preview requires directed dependencies".into());
    }
    if graph.node_count() > 50000 || graph.edge_count() > 200000 {
        return Err("Architecture graph limit exceeded".into());
    }
    let q: Scenario = serde_json::from_value(args.clone())?;
    if q.remove_nodes.len() + q.remove_edges.len() + q.modules.len() > 256 {
        return Err("At most 256 scenario changes".into());
    }
    let known = |id: &str| {
        graph
            .node_index(&json!(id))
            .ok_or_else(|| crate::Error::invalid("Unknown node identity"))
    };
    for id in &q.remove_nodes {
        known(id)?;
    }
    for (id, name) in &q.modules {
        known(id)?;
        if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            return Err("Invalid module name".into());
        }
        if q.remove_nodes.contains(id) {
            return Err("Cannot regroup a removed node".into());
        }
    }
    let before = metrics(graph)?;
    let mut copy = graph.clone();
    for removal in &q.remove_edges {
        known(&removal.source)?;
        known(&removal.target)?;
        if !RELATIONS.contains(&removal.relation.as_str()) {
            return Err("Unsupported dependency relation".into());
        }
        let edges = copy.edges_between(&json!(removal.source), &json!(removal.target));
        let matching: Vec<_> = edges
            .iter()
            .filter(|e| e.attributes.get("relation") == Some(&json!(removal.relation)))
            .map(|e| e.key.to_value())
            .collect();
        if matching.len() != 1 {
            return Err("Dependency removal is missing or ambiguous; select unique endpoint/relation identities".into());
        }
        copy.remove_edge(
            &json!(removal.source),
            &json!(removal.target),
            Some(&matching[0]),
        )?;
    }
    copy.remove_nodes_from(&q.remove_nodes.iter().map(|n| json!(n)).collect::<Vec<_>>())?;
    for node in copy.nodes_mut() {
        if let Some(name) = node.id.as_str().and_then(|id| q.modules.get(id)) {
            node.attributes
                .insert("scenario_module".into(), json!(name));
        }
    }
    let after = metrics(&copy)?;
    let delta: BTreeMap<_, _> = [
        "nodes",
        "dependency_edges",
        "internal_edges",
        "cross_module_edges",
        "coupling_pairs",
    ]
    .into_iter()
    .map(|k| (k, after[k].as_i64().unwrap() - before[k].as_i64().unwrap()))
    .collect();
    Ok(
        json!({"kind":"hypothetical_static","applied":false,"before":before,"after":after,"delta":delta,"scenario":args,"coverage":{"complete":false,"basis":"selected_episode_edges","missing_dependencies_may_change_results":true,"relations":RELATIONS,"module_cycle_definition":"strongly_connected_components_of_size_greater_than_one"}}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Graph {
        let mut g = Graph::new(true, true);
        for (id, file) in [
            ("a", "first/a.go"),
            ("b", "second/b.go"),
            ("c", "third/c.go"),
        ] {
            g.add_node(
                json!(id),
                json!({"source_file":file,"label":"same"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap();
        }
        for (a, b) in [("a", "b"), ("b", "c"), ("c", "a")] {
            g.add_edge(
                json!(a),
                json!(b),
                None,
                json!({"relation":"calls"}).as_object().unwrap().clone(),
            )
            .unwrap();
        }
        g
    }
    #[test]
    fn removal_and_regrouping_are_isolated_and_measure_cycles() {
        let g = fixture();
        let original = g.to_node_link("links");
        let r=preview(&g,&json!({"remove_edges":[{"source":"c","target":"a","relation":"calls"}],"modules":{"b":"first"}})).unwrap();
        assert_eq!(r["before"]["cross_module_edges"], 3);
        assert_eq!(r["after"]["cross_module_edges"], 1);
        assert_eq!(
            r["before"]["cyclic_module_components"],
            json!([["first", "second", "third"]])
        );
        assert_eq!(r["after"]["cyclic_module_components"], json!([]));
        assert_eq!(g.to_node_link("links"), original);
    }
    #[test]
    fn ambiguous_missing_and_conflicting_scenarios_are_refused() {
        let mut g = fixture();
        g.add_edge(
            json!("a"),
            json!("b"),
            None,
            json!({"relation":"calls"}).as_object().unwrap().clone(),
        )
        .unwrap();
        assert!(
            preview(
                &g,
                &json!({"remove_edges":[{"source":"a","target":"b","relation":"calls"}]})
            )
            .is_err()
        );
        assert!(preview(&g, &json!({"remove_nodes":["absent"]})).is_err());
        assert!(preview(&g, &json!({"remove_nodes":["a"],"modules":{"a":"new"}})).is_err());
    }
}
