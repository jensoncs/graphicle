//! sqlc-aware SQL for basic review. A query file's `-- name: X :kind` blocks
//! are declarations; a query uses the tables it reads or writes; a schema
//! file (a migration) touches the tables it creates, alters, drops or writes.
//! Table names are lexical candidates: no catalog, search path, view
//! expansion or dynamic SQL.
use graphicle::extractors::sql::{norm_ident, scan_sql};
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::LazyLock;

/// One `-- name:` block: its header line, then its statement.
pub(super) struct Query {
    pub name: String,
    pub start: usize,
    pub body_start: usize,
    pub end: usize,
    pub line: usize,
    pub end_line: usize,
}

/// One sqlc package: its schema and query paths and generated Go directory,
/// repository paths as basic review keys them.
pub(super) struct Config {
    pub schema: Vec<String>,
    pub queries: Vec<String>,
    pub out: Option<String>,
}

const IDENT: &str = r#"(?:"[^"\n]+"|`[^`\n]+`|\[[^\]\n]+\]|[A-Za-z_][\w$]*)(?:\s*\.\s*(?:"[^"\n]+"|`[^`\n]+`|\[[^\]\n]+\]|[A-Za-z_][\w$]*))*"#;

static NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*--[ \t]*name:[ \t]*([A-Za-z_][A-Za-z0-9_]*)[ \t]+:[A-Za-z]+").unwrap()
});
static TOUCHED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?:CREATE\s+(?:(?:GLOBAL|LOCAL)\s+)?(?:TEMP(?:ORARY)?\s+|UNLOGGED\s+)?TABLE|ALTER\s+TABLE|TRUNCATE(?:\s+TABLE)?|INSERT\s+INTO|UPDATE|DELETE\s+FROM|RENAME\s+TO|COMMENT\s+ON\s+TABLE|CREATE\s+(?:UNIQUE\s+)?INDEX(?:\s+CONCURRENTLY)?(?:\s+IF\s+NOT\s+EXISTS)?(?:\s+{IDENT})?\s+ON)\s+(?:IF\s+(?:NOT\s+)?EXISTS\s+)?(?:ONLY\s+)?({IDENT})"
    ))
    .unwrap()
});
static DROPPED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bDROP\s+TABLE\s+(?:IF\s+EXISTS\s+)?([^;]+)").unwrap());
static USED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(FROM|JOIN|INTO|UPDATE)\s+(?:ONLY\s+)?({IDENT})"
    ))
    .unwrap()
});
static CTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)(?:\bWITH\s+(?:RECURSIVE\s+)?|,\s*)({IDENT})\s*(?:\([^)]*\)\s*)?AS\s*(?:NOT\s+)?(?:MATERIALIZED\s+)?\("
    ))
    .unwrap()
});
static IDENTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(IDENT).unwrap());

/// Words a pattern can capture that are never a table (`ON UPDATE CASCADE`,
/// `DO UPDATE SET`).
const NOT_TABLES: &[&str] = &[
    "cascade", "restrict", "set", "no", "only", "if", "not", "exists", "table",
];

/// A table's name without its schema, as written names compare.
fn table(raw: &str) -> Option<String> {
    let name = norm_ident(raw);
    let name = name.rsplit('.').next().unwrap_or("").to_string();
    (!name.is_empty() && !NOT_TABLES.contains(&name.as_str())).then_some(name)
}

/// `content` with comments and string literals blanked.
fn masked(content: &str) -> String {
    scan_sql(content).0
}

pub(super) fn queries(content: &str) -> Vec<Query> {
    let starts: Vec<_> = NAME
        .captures_iter(content)
        .map(|c| (c.get(0).unwrap().start(), c[1].to_string()))
        .collect();
    // Offsets only grow, so lines are counted forward once over the file.
    let mut cursor = (0, 1);
    let mut line = |at: usize| {
        cursor = (at, cursor.1 + content[cursor.0..at].matches('\n').count());
        cursor.1
    };
    starts
        .iter()
        .enumerate()
        .map(|(i, (start, name))| {
            let next = starts.get(i + 1).map_or(content.len(), |n| n.0);
            let end = start + content[*start..next].trim_end().len();
            let body_start = content[*start..end]
                .find('\n')
                .map_or(end, |n| start + n + 1);
            Query {
                name: name.clone(),
                start: *start,
                body_start,
                end,
                line: line(*start),
                end_line: line(end),
            }
        })
        .collect()
}

/// Tables a schema file creates, alters, drops or writes.
pub(super) fn touched(content: &str) -> BTreeSet<String> {
    let text = masked(content);
    let mut out: BTreeSet<String> = TOUCHED
        .captures_iter(&text)
        .filter_map(|c| table(&c[1]))
        .collect();
    for list in DROPPED.captures_iter(&text) {
        out.extend(IDENTS.find_iter(&list[1]).filter_map(|m| table(m.as_str())));
    }
    out
}

/// Tables a statement reads or writes, its own CTE names and table functions
/// (`FROM unnest(...)`) left out.
pub(super) fn used(statement: &str) -> BTreeSet<String> {
    let text = masked(statement);
    let ctes: BTreeSet<_> = CTE
        .captures_iter(&text)
        .filter_map(|c| table(&c[1]))
        .collect();
    USED.captures_iter(&text)
        .filter(|c| {
            let after = text[c.get(0).unwrap().end()..].trim_start();
            !(matches!(c[1].to_ascii_uppercase().as_str(), "FROM" | "JOIN")
                && after.starts_with('('))
        })
        .filter_map(|c| table(&c[2]))
        .filter(|t| !ctes.contains(t))
        .collect()
}

/// The sqlc packages of a `sqlc.yaml`/`sqlc.yml`/`sqlc.json` at `path`
/// (version 2 `sql`, or version 1 `packages`), paths resolved beside it.
pub(super) fn configs(path: &str, content: &str) -> Vec<Config> {
    let parsed = if path.ends_with(".json") {
        serde_json::from_str(content).ok()
    } else {
        graphicle::extractors::yaml_frontmatter::parse(content).ok()
    };
    let Some(parsed) = parsed else {
        return vec![];
    };
    let paths = |v: &Value| -> Vec<String> {
        let listed = match v {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        listed
            .into_iter()
            .filter_map(|p| super::basic::relative(path, p.trim_start_matches('/')))
            .collect()
    };
    let (packages, out): (&Value, fn(&Value) -> Value) = if parsed["sql"].is_array() {
        (&parsed["sql"], |p| p["gen"]["go"]["out"].clone())
    } else {
        (&parsed["packages"], |p| p["path"].clone())
    };
    packages
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| Config {
            schema: paths(&p["schema"]),
            queries: paths(&p["queries"]),
            out: paths(&out(p)).into_iter().next(),
        })
        .collect()
}

/// `path` names an sqlc configuration file.
pub(super) fn config_file(path: &str) -> bool {
    matches!(
        path.rsplit('/').next(),
        Some("sqlc.yaml" | "sqlc.yml" | "sqlc.json")
    )
}

/// `file` is one of `paths` or under one of them.
pub(super) fn under(file: &str, paths: &[String]) -> bool {
    paths
        .iter()
        .any(|p| file == p || file.starts_with(&format!("{p}/")))
}

/// The generated Go file sqlc writes for query file `file` in `out`, when it
/// is in the snapshot, and the methods it declares.
pub(super) fn generated<'a>(
    out: &str,
    file: &str,
    text: impl Fn(&str) -> Option<&'a str>,
) -> Option<(String, BTreeSet<&'a str>)> {
    let go = format!("{out}/{}.go", file.rsplit('/').next()?);
    let methods = text(&go)?
        .lines()
        .filter_map(|l| l.strip_prefix("func ("))
        .filter_map(|l| l.split_once(") ")?.1.split_once('('))
        .map(|(name, _)| name)
        .collect();
    Some((go, methods))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_blocks_are_split_at_their_names() {
        let sql = "-- leading comment\n-- name: GetUser :one\nSELECT 1;\n\n-- name: ListUsers :many\nSELECT 2;\n";
        let q = queries(sql);
        assert_eq!(
            q.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(),
            ["GetUser", "ListUsers"]
        );
        assert_eq!(
            &sql[q[0].start..q[0].end],
            "-- name: GetUser :one\nSELECT 1;"
        );
        assert_eq!(&sql[q[0].body_start..q[0].end], "SELECT 1;");
        assert_eq!((q[1].line, q[1].end_line), (5, 6));
    }

    #[test]
    fn many_query_blocks_and_methods_cost_linear_time() {
        let n = 40_000;
        let sql: String = (0..n)
            .map(|i| format!("-- name: Q{i} :one\nSELECT * FROM t{i};\n"))
            .collect();
        let go: String = (0..n)
            .map(|i| format!("func (q *Queries) Q{i}(ctx context.Context) error {{\n"))
            .collect();
        let started = std::time::Instant::now();
        let q = queries(&sql);
        assert_eq!((q.len(), q[n - 1].line), (n, 2 * n - 1));
        let methods = generated("gen", "q/a.sql", |_| Some(go.as_str())).unwrap();
        assert!(q.iter().all(|q| methods.1.contains(q.name.as_str())));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn a_migration_touches_what_it_defines_or_writes() {
        let sql = r#"
            CREATE TABLE IF NOT EXISTS public."Users" (id int REFERENCES teams(id) ON UPDATE CASCADE);
            ALTER TABLE ONLY orders ADD COLUMN note text; -- ALTER TABLE ignored
            CREATE UNIQUE INDEX CONCURRENTLY idx_x ON items (id);
            DROP TABLE IF EXISTS a, "b" CASCADE;
            INSERT INTO seeds (id) VALUES ('UPDATE fake');
            ALTER TABLE old RENAME TO renamed;
        "#;
        let expected: BTreeSet<_> = [
            "users", "orders", "items", "a", "b", "seeds", "old", "renamed",
        ]
        .map(String::from)
        .into();
        assert_eq!(touched(sql), expected);
    }

    #[test]
    fn a_query_uses_tables_not_its_ctes_or_functions() {
        let sql = "WITH recent AS (SELECT * FROM public.orders o JOIN users u ON u.id = o.user_id)\nSELECT * FROM recent, unnest($1::int[]) AS ids\nWHERE x IN (SELECT id FROM \"Teams\");\nINSERT INTO audit (id) VALUES (1) ON CONFLICT DO UPDATE SET id = 2;";
        let expected: BTreeSet<_> = ["orders", "users", "teams", "audit"]
            .map(String::from)
            .into();
        assert_eq!(used(sql), expected);
    }

    #[test]
    fn sqlc_configs_resolve_beside_the_file() {
        let v2 = "version: \"2\"\nsql:\n  - schema: ./db/migration/\n    queries: [./db/query/]\n    gen:\n      go:\n        out: ./db/sqlc\n";
        let c = configs("repo/svc/sqlc.yaml", v2);
        assert_eq!(c[0].schema, ["repo/svc/db/migration"]);
        assert_eq!(c[0].queries, ["repo/svc/db/query"]);
        assert_eq!(c[0].out.as_deref(), Some("repo/svc/db/sqlc"));
        // sqlc joins every path onto the config's folder, a leading `/` too.
        let rooted = "version: \"2\"\nsql:\n  - schema: \"/db/migration/\"\n    queries: ../../q\n";
        let c = configs("repo/svc/sqlc.yaml", rooted);
        assert_eq!(c[0].schema, ["repo/svc/db/migration"]);
        assert!(c[0].queries.is_empty(), "leaves the pin");
        let v1 =
            r#"{"version":"1","packages":[{"path":"gen","schema":"schema.sql","queries":"q"}]}"#;
        let c = configs("repo/sqlc.json", v1);
        assert_eq!(
            (c[0].schema.as_slice(), c[0].out.as_deref()),
            (&["repo/schema.sql".to_string()][..], Some("repo/gen"))
        );
        assert!(under("repo/q/a.sql", &c[0].queries) && !under("repo/qq/a.sql", &c[0].queries));
        let go = "package db\nfunc (q *Queries) GetUser(ctx context.Context) error {\n";
        let text = |p: &str| (p == "repo/gen/a.sql.go").then_some(go);
        let (file, methods) = generated("repo/gen", "repo/q/a.sql", text).unwrap();
        assert_eq!(file, "repo/gen/a.sql.go");
        assert_eq!(methods, BTreeSet::from(["GetUser"]));
        assert!(generated("repo/gen", "repo/q/b.sql", text).is_none());
    }
}
