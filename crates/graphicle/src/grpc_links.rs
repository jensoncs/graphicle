//! gRPC links between projects, derived from their import links.
//!
//! A client file that creates `pb.New{Svc}Client` and a server file that
//! calls `pb.Register{Svc}Server`, both importing the generated package `pb`
//! from the same project, are linked client -> server (`grpc_calls`). The
//! service is identified by the generated package and its name, not by an
//! address: which deployment a client dials is not in the code, so the link
//! is `INFERRED` and a service several projects serve links to each.
//!
//! The rows are recomputed on every link pass from the live import rows, so
//! they are never part of one importer's own link: the per-importer
//! bookkeeping ([`OWN`]) leaves them out.
use crate::now;
use crate::store::{rows, s};
use graphicle_memory::graph::cross_repo_imports::{INFERRED, is_test_path};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const RELATION: &str = "grpc_calls";
/// SQL condition selecting the rows an importer's own link writes.
pub const OWN: &str = "relation<>'grpc_calls'";

/// One end of a link: the importing file's node in a project's episode.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct End {
    repo: String,
    episode: String,
    node: String,
    stable: Option<String>,
    line: Option<i64>,
}

/// (client, server, generated package, service).
type Link = (End, End, String, String);

/// `New{Svc}Client` -> (false, Svc); `Register{Svc}Server` -> (true, Svc).
fn role(symbol: &str) -> Option<(bool, &str)> {
    if let Some(svc) = symbol
        .strip_prefix("Register")
        .and_then(|r| r.strip_suffix("Server"))
    {
        return (!svc.is_empty()).then_some((true, svc));
    }
    let svc = symbol.strip_prefix("New")?.strip_suffix("Client")?;
    (!svc.is_empty()).then_some((false, svc))
}

fn wanted(db: &Connection, workspace: &str) -> crate::Result<BTreeSet<Link>> {
    let found = rows(
        db,
        "SELECT e.source_repository_id AS repo,e.source_episode AS episode,e.source_node_id AS node,\
         e.source_stable_id AS stable,e.line AS line,e.target_repository_id AS stub_repo,e.module_path AS module,\
         e.symbol AS symbol,json_extract(n.attributes,'$.source_file') AS file \
         FROM gf_cross_edges e LEFT JOIN gf_nodes n ON n.repository_id=e.source_repository_id \
         AND n.node_id=e.source_node_id AND n.valid_to_episode IS NULL \
         WHERE e.workspace_id=?1 AND e.valid_to_episode IS NULL AND e.relation IN ('imports','imports_from') \
         AND (e.symbol LIKE 'New%Client' OR e.symbol LIKE 'Register%Server')",
        [workspace],
    )?;
    // (stub project, package, service) -> (clients, servers).
    type Sides = (BTreeSet<End>, BTreeSet<End>);
    let mut sides: BTreeMap<(String, String, String), Sides> = BTreeMap::new();
    for row in &found {
        let Some((server, service)) = role(s(row, "symbol")) else {
            continue;
        };
        if is_test_path(s(row, "file")) {
            continue;
        }
        let end = End {
            repo: s(row, "repo").into(),
            episode: s(row, "episode").into(),
            node: s(row, "node").into(),
            stable: row
                .get("stable")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            line: row.get("line").and_then(serde_json::Value::as_i64),
        };
        let key = (
            s(row, "stub_repo").into(),
            s(row, "module").into(),
            service.to_string(),
        );
        let entry = sides.entry(key).or_default();
        if server {
            entry.1.insert(end)
        } else {
            entry.0.insert(end)
        };
    }
    let mut out = BTreeSet::new();
    for ((_, module, service), (clients, servers)) in sides {
        for client in &clients {
            for server in servers.iter().filter(|s| s.repo != client.repo) {
                out.insert((
                    client.clone(),
                    server.clone(),
                    module.clone(),
                    service.clone(),
                ));
            }
        }
    }
    Ok(out)
}

/// Bring the live `grpc_calls` rows of `workspace` in line with its live
/// import rows; `episodes` holds each linked project's current episode.
/// Returns (live links, inserted, closed).
pub fn relink(
    db: &Connection,
    workspace: &str,
    episodes: &HashMap<String, String>,
) -> crate::Result<(usize, usize, usize)> {
    let want = wanted(db, workspace)?;
    let live = rows(
        db,
        "SELECT id,source_repository_id,source_episode,source_node_id,target_repository_id,target_episode,\
         target_node_id,module_path,symbol FROM gf_cross_edges \
         WHERE workspace_id=?1 AND relation=?2 AND valid_to_episode IS NULL",
        params![workspace, RELATION],
    )?;
    let key = |c: (&str, &str, &str), t: (&str, &str, &str), module: &str, service: &str| {
        [c.0, c.1, c.2, t.0, t.1, t.2, module, service].map(str::to_string)
    };
    let wanted_keys: BTreeSet<[String; 8]> = want
        .iter()
        .map(|(c, t, m, svc)| {
            key(
                (&c.repo, &c.episode, &c.node),
                (&t.repo, &t.episode, &t.node),
                m,
                svc,
            )
        })
        .collect();
    let mut kept = BTreeSet::new();
    let mut closed = 0;
    for row in &live {
        let k = key(
            (
                s(row, "source_repository_id"),
                s(row, "source_episode"),
                s(row, "source_node_id"),
            ),
            (
                s(row, "target_repository_id"),
                s(row, "target_episode"),
                s(row, "target_node_id"),
            ),
            s(row, "module_path"),
            s(row, "symbol"),
        );
        if wanted_keys.contains(&k) {
            kept.insert(k);
            continue;
        }
        // Closed by the side that moved on: the client's new episode, else the server's.
        let (repo, episode) = [&k[0], &k[3]]
            .into_iter()
            .zip([&k[1], &k[4]])
            .find_map(|(repo, was)| {
                episodes
                    .get(repo)
                    .filter(|now| *now != was)
                    .map(|now| (repo, now))
            })
            .unwrap_or((&k[0], &k[1]));
        closed += db.execute(
            "UPDATE gf_cross_edges SET valid_to_episode=?1,valid_to_repository_id=?2 WHERE id=?3",
            params![
                episode,
                repo,
                row.get("id").and_then(serde_json::Value::as_i64)
            ],
        )?;
    }
    let stamp = now();
    let mut inserted = 0;
    for (client, server, module, service) in &want {
        let k = key(
            (&client.repo, &client.episode, &client.node),
            (&server.repo, &server.episode, &server.node),
            module,
            service,
        );
        if kept.contains(&k) {
            continue;
        }
        inserted += db.execute(
            "INSERT INTO gf_cross_edges(workspace_id,source_repository_id,source_episode,source_node_id,source_stable_id,\
             target_repository_id,target_episode,target_node_id,target_stable_id,relation,confidence,derivation,\
             module_path,symbol,line,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                workspace,
                client.repo,
                client.episode,
                client.node,
                client.stable,
                server.repo,
                server.episode,
                server.node,
                server.stable,
                RELATION,
                INFERRED,
                format!("grpc_service:{module}#{service}"),
                module,
                service,
                client.line,
                stamp
            ],
        )?;
    }
    Ok((want.len(), inserted, closed))
}

#[cfg(test)]
mod tests {
    use super::role;

    #[test]
    fn generated_names_give_the_service_and_side() {
        assert_eq!(
            role("NewUserEntityServiceClient"),
            Some((false, "UserEntityService"))
        );
        assert_eq!(
            role("RegisterUserEntityServiceServer"),
            Some((true, "UserEntityService"))
        );
        assert_eq!(role("NewClient"), None);
        assert_eq!(role("RegisterServer"), None);
        assert_eq!(role("NewConfig"), None);
    }
}
