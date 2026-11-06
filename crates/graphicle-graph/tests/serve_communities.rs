//! Community grouping keeps the engine's ordering and numeric-ID semantics.
use graphicle::{graph::Graph, serve::communities};
use serde_json::{Map, Value, json};

fn add(graph: &mut Graph, community: Option<Value>) {
    let mut attributes = Map::new();
    if let Some(community) = community {
        attributes.insert("community".into(), community);
    }
    graph
        .add_node(json!(graph.nodes().len()), attributes)
        .unwrap();
}

#[test]
fn groups_keep_first_encounter_order_and_coerce_ids_as_before() {
    let mut graph = Graph::new(true, false);
    for value in [
        Some(json!(9)),
        None,
        Some(json!(-2)),
        Some(json!("9")),
        Some(json!(9.75)),
        Some(Value::Null),
        Some(json!("-2")),
        Some(json!(0)),
        Some(json!(u64::MAX)),
        Some(json!(i64::MAX)),
    ] {
        add(&mut graph, value);
    }
    assert_eq!(
        communities(&graph).unwrap(),
        vec![
            (9, vec![0, 3, 4]),
            (-2, vec![2, 6]),
            (0, vec![7]),
            (i64::MAX, vec![8, 9]),
        ]
    );
    assert!(communities(&Graph::new(true, false)).unwrap().is_empty());
}

#[test]
fn invalid_community_ids_remain_errors() {
    for invalid in [
        json!(true),
        json!([]),
        json!({}),
        json!("bad"),
        json!("1.5"),
    ] {
        let mut graph = Graph::new(true, false);
        add(&mut graph, Some(json!(3)));
        add(&mut graph, Some(invalid));
        assert!(
            communities(&graph)
                .unwrap_err()
                .to_string()
                .contains("invalid community ID")
        );
    }
}

#[test]
fn many_interleaved_communities_keep_group_and_member_order() {
    const GROUPS: usize = 20_000;
    let mut graph = Graph::new(true, false);
    let id = |position: usize| ((position * 7919) % GROUPS) as i64 - 10_000;
    for pass in 0..3 {
        for i in 0..GROUPS {
            let position = if pass == 1 { GROUPS - 1 - i } else { i };
            add(&mut graph, Some(json!(id(position))));
        }
    }
    let grouped = communities(&graph).unwrap();
    assert_eq!(grouped.len(), GROUPS);
    for (i, (community, members)) in grouped.iter().enumerate() {
        assert_eq!(*community, id(i));
        assert_eq!(*members, vec![i, 2 * GROUPS - 1 - i, 2 * GROUPS + i]);
    }
}
