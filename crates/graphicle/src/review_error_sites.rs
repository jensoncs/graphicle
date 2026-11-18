//! Error handling that differs between call sites of one callee the change
//! calls. Every review already reads the files (the ledger's misses were in
//! code the reviewer opened); what it lacked was the hazard said out loud
//! (nexus #963: one gateway builder returns a failed merged-config read, a
//! sibling builds routes without the configs).
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

use super::Call;

/// For each callee a changed declaration calls, when a site outside the
/// changed declarations handles the callee's error differently from every
/// changed site: the changed sites and, per other handling, the site nearest
/// them (same file, then same directory). Test files are left out. `path`
/// names a call's file as the snapshot does; a statement with a site it
/// cannot name is dropped.
pub(super) fn divergence(calls: &[(String, Call)], seeds: &BTreeSet<(&str, &str)>, path: impl Fn(&str) -> Option<String>) -> Vec<Value> {
    let mut by_callee: BTreeMap<(&str, &str), Vec<&Call>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (world, call) in calls {
        if call.error_handling.is_empty() || call.target.is_empty() || call.file.ends_with("_test.go")
            || !matches!(call.relation.as_str(), "calls" | "may_call")
            || !seen.insert((world.as_str(), call.target.as_str(), call.file.as_str(), call.start))
        {
            continue;
        }
        by_callee.entry((world.as_str(), call.target.as_str())).or_default().push(call);
    }
    let mut out = vec![];
    for ((world, _), sites) in by_callee {
        let mine: Vec<&Call> = sites.iter().copied().filter(|c| seeds.contains(&(world, c.caller.as_str()))).collect();
        let Some(first) = mine.first() else { continue };
        let dir = |f: &str| f.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
        let mut others: Vec<&Call> = sites.iter().copied()
            .filter(|c| !seeds.contains(&(world, c.caller.as_str())))
            .filter(|c| mine.iter().all(|m| m.error_handling != c.error_handling))
            .collect();
        others.sort_by_key(|c| (c.file != first.file, dir(&c.file) != dir(&first.file), c.file.clone(), c.start));
        let mut kinds = BTreeSet::new();
        others.retain(|c| kinds.insert(c.error_handling.as_str()));
        if others.is_empty() {
            continue;
        }
        let site = |c: &Call, changed: bool| Some(json!({"file":path(&c.file)?,"line":c.line,"handling":c.error_handling,"in_changed_code":changed,"spelling":c.spelling}));
        let named: Option<Vec<Value>> = mine.iter().map(|c| site(c, true)).chain(others.iter().map(|c| site(c, false))).collect();
        // one site reaching two targets (an interface method and its implementation) says the same thing twice
        let statement = named.map(|sites| json!({"callee":first.spelling,"sites":sites}));
        if let Some(statement) = statement.filter(|s| !out.contains(s)) {
            out.push(statement);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn call(caller: &str, target: &str, file: &str, line: usize, handling: &str) -> (String, Call) {
        ("app".into(), Call {
            boundary: Value::Null, caller: caller.into(), target: target.into(), target_file: String::new(),
            target_package: String::new(), target_offset: 0, spelling: target.into(), file: file.into(),
            start: line * 10, end: line * 10 + 4, line, relation: "calls".into(), reason: String::new(),
            evidence: String::new(), discarded_error: false, error_handling: handling.into(),
        })
    }

    #[test]
    fn a_sibling_that_handles_the_callees_error_differently_is_stated() {
        let calls = vec![
            call("Clusters", "Load", "b/cluster.go", 131, "returns_on_error"),
            call("Routes", "Load", "b/route.go", 79, "proceeds_on_error"),
            call("Other", "Load", "c/x.go", 5, "proceeds_on_error"),   // same handling as route.go: one site per kind
            call("Tests", "Load", "b/route_test.go", 9, "discarded"),
            call("Clusters", "Same", "b/cluster.go", 140, "returns_on_error"),
            call("Routes", "Same", "b/route.go", 90, "returns_on_error"),   // agrees: nothing to say
            call("Routes", "Unrelated", "b/route.go", 95, "discarded"),   // no changed caller
        ];
        let seeds: BTreeSet<(&str, &str)> = [("app", "Clusters")].into_iter().collect();
        let got = divergence(&calls, &seeds, |f| Some(f.to_owned()));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["callee"], "Load");
        let sites: Vec<(String, u64, String)> = got[0]["sites"].as_array().unwrap().iter()
            .map(|s| (s["file"].as_str().unwrap().into(), s["line"].as_u64().unwrap(), s["handling"].as_str().unwrap().into())).collect();
        assert_eq!(sites, vec![("b/cluster.go".into(), 131, "returns_on_error".into()), ("b/route.go".into(), 79, "proceeds_on_error".into())]);
    }

    #[test]
    fn one_call_bound_to_two_targets_is_stated_once() {
        // an interface method and its implementation: the same sites reach both targets
        let mut calls = vec![];
        for target in ["Store.Add", "pgStore.Add"] {
            let mut a = call("Changed", target, "b/a.go", 10, "returns_on_error");
            let mut b = call("Other", target, "b/b.go", 20, "proceeds_on_error");
            a.1.spelling = "db.Add".into();
            b.1.spelling = "db.Add".into();
            calls.extend([a, b]);
        }
        let seeds: BTreeSet<(&str, &str)> = [("app", "Changed")].into_iter().collect();
        assert_eq!(divergence(&calls, &seeds, |f| Some(f.to_owned())).len(), 1);
    }
}
