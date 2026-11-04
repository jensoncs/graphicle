use super::*;
use rusqlite::StatementStatus;

fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE repo_objects(repository_id TEXT NOT NULL,sha256 TEXT NOT NULL,bytes INTEGER NOT NULL,PRIMARY KEY(repository_id,sha256)); CREATE INDEX repo_objects_size ON repo_objects(repository_id,sha256,bytes);").unwrap();
    db
}

fn sync(files: BTreeMap<String, Value>) -> Value {
    let manifest = json!(files);
    json!({"id":"sync","snapshot":crate::digest(&manifest),"manifest":manifest.to_string()})
}

fn item(id: usize, bytes: usize) -> Value {
    json!({"sha256":format!("{id:064x}"),"bytes":bytes})
}

#[test]
fn missing_lookup_uses_size_index_without_reading_source_payloads() {
    let db = database();
    let mut statement = db.prepare(&format!("EXPLAIN QUERY PLAN {MISSING_QUERY}")).unwrap();
    let plan: Vec<String> = statement.query_map(params!["{}","repo",Option::<String>::None,257],|row|row.get(3)).unwrap().map(|row| row.unwrap()).collect();
    assert!(plan.iter().any(|line| line.contains("COVERING INDEX repo_objects_size")),"{plan:?}");
}

#[test]
#[ignore = "manual before/after complete missing-list pagination benchmark"]
fn missing_pagination_benchmark() {
    for size in [5_000usize, 20_000, 50_000] {
        let db = database();
        let sync = sync((0..size).map(|i| (format!("src/{i:06}.go"),item(i,10))).collect());
        let mut args = json!({"missing_limit":256});
        let (mut pages, mut paths) = (0, 0);
        let start = std::time::Instant::now();
        loop {
            let page = missing_page(&db,"repo",&sync,&args).unwrap();
            assert_eq!(page["missing_total"],size);
            pages += 1;
            paths += page["missing"].as_array().unwrap().len();
            if page["next_cursor"].is_null() { break; }
            args["cursor"] = page["next_cursor"].clone();
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(paths,size);
        let statements = db.prepare_cached(MISSING_QUERY).unwrap().get_status(StatementStatus::Run);
        eprintln!("MISSING_PAGINATION_BENCH={}",json!({"files":size,"pages":pages,"milliseconds":ms,"sql_statement_runs":statements}));
    }
}

#[test]
fn missing_pages_match_reference_after_uploads_removals_and_wrong_size_objects() {
    for limit in [1, 7, 256, 1024] {
        let db = database();
        let files: BTreeMap<_,_> = (0..600).map(|i| (format!("src/{i:04}.go"),item(i/2,10))).collect();
        let sync = sync(files.clone());
        // Neither another repository's object nor a wrong size supplies bytes.
        for (repo, id, size) in [("other",0,10),("repo",1,9),("repo",2,10),("repo",299,10)] {
            db.execute("INSERT INTO repo_objects VALUES(?,?,?)",params![repo,format!("{id:064x}"),size]).unwrap();
        }
        let mut args = json!({"missing_limit":limit});
        let mut names = BTreeSet::new();
        for step in 0..400 {
            let page = missing_page(&db,"repo",&sync,&args).unwrap();
            assert_eq!(page,reference_missing_page(&db,"repo",&sync,&args).unwrap());
            for name in page["missing"].as_array().unwrap() {
                assert!(names.insert(name.as_str().unwrap().to_owned()));
                let entry = &files[name.as_str().unwrap()];
                db.execute("INSERT OR REPLACE INTO repo_objects VALUES(?,?,?)",params!["repo",s(entry,"sha256"),entry["bytes"].as_u64()]).unwrap();
            }
            if step == 0 {
                // Upload a later page's object and remove an earlier one.
                db.execute("INSERT OR REPLACE INTO repo_objects VALUES('repo',?,10)",[format!("{:064x}",290)]).unwrap();
                db.execute("DELETE FROM repo_objects WHERE repository_id='repo' AND sha256=?",[format!("{:064x}",0)]).unwrap();
            }
            if page["next_cursor"].is_null() { break; }
            args["cursor"] = page["next_cursor"].clone();
            assert!(step < 399);
        }
        // A fresh enumeration still reports removals before the old cursor.
        let args = json!({"missing_limit":limit});
        assert_eq!(missing_page(&db,"repo",&sync,&args).unwrap(),reference_missing_page(&db,"repo",&sync,&args).unwrap());
    }
}

#[test]
fn missing_pages_preserve_escaping_order_byte_bounds_and_empty_results() {
    let db = database();
    for files in [BTreeMap::new(), (0..400).map(|i| (format!("{}é/{i:04}","q\"\t".repeat(200)),item(i,0))).collect()] {
        let sync = sync(files);
        let mut args = json!({"missing_limit":1024});
        loop {
            let page = missing_page(&db,"repo",&sync,&args).unwrap();
            assert_eq!(page,reference_missing_page(&db,"repo",&sync,&args).unwrap());
            assert!(serde_json::to_vec(&page).unwrap().len() < 160 * 1024);
            if page["next_cursor"].is_null() { break; }
            args["cursor"] = page["next_cursor"].clone();
        }
    }
}

#[test]
fn missing_cursor_requires_matching_sync_snapshot_and_existing_path() {
    let db = database();
    let sync = sync(BTreeMap::from([("a.go".into(),item(0,0)),("b.go".into(),item(0,0))]));
    for (id,snapshot,path) in [("other",s(&sync,"snapshot"),"a.go"),("sync","wrong","a.go"),("sync",s(&sync,"snapshot"),"absent")] {
        let cursor = URL_SAFE_NO_PAD.encode(json!({"sync_id":id,"snapshot":snapshot,"after":path}).to_string());
        assert!(missing_page(&db,"repo",&sync,&json!({"cursor":cursor})).is_err());
    }
    // A nonrepresentative duplicate is a valid manifest path, even with no
    // remaining candidates; preserve the existing cursor contract.
    let cursor = URL_SAFE_NO_PAD.encode(json!({"sync_id":"sync","snapshot":sync["snapshot"],"after":"b.go"}).to_string());
    let args = json!({"cursor":cursor});
    let page = missing_page(&db,"repo",&sync,&args).unwrap();
    assert_eq!(page,reference_missing_page(&db,"repo",&sync,&args).unwrap());
    assert_eq!(page["missing"],json!([]));
    assert_eq!(page["missing_total"],1);
    for manifest in ["[]", "null", "42", "\"text\"", "{invalid"] {
        let mut invalid = sync.clone();
        invalid["manifest"] = json!(manifest);
        assert!(missing_page(&db,"repo",&invalid,&json!({})).is_err());
    }
}

// Pre-optimization implementation retained as a test-only behavioral oracle.
fn reference_missing_page(
    db: &Connection,
    repository: &str,
    sync: &Value,
    args: &Value,
) -> Result<Value> {
    let limit = integer(args, "missing_limit", 256, 1024)?.max(1);
    let manifest: BTreeMap<String, Value> = serde_json::from_str(s(sync, "manifest"))?;
    let after = match args.get("cursor").filter(|v| !v.is_null()) {
        None => String::new(),
        Some(_) => {
            let cursor = string(args, "cursor", 16000)?;
            let bytes = URL_SAFE_NO_PAD
                .decode(cursor)
                .map_err(|_| Error::bad("Invalid missing cursor"))?;
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| Error::bad("Invalid missing cursor"))?;
            if value["sync_id"] != sync["id"]
                || value["snapshot"] != sync["snapshot"]
                || !manifest.contains_key(s(&value, "after"))
            {
                return Err(Error::bad("Missing cursor does not belong to this sync"));
            }
            s(&value, "after").to_owned()
        }
    };
    let mut exists = db.prepare_cached("SELECT EXISTS(SELECT 1 FROM repo_objects WHERE repository_id=? AND sha256=? AND bytes=?)")?;
    let (mut missing, mut seen) = (Vec::new(), BTreeSet::new());
    let (mut total, mut bytes, mut more) = (0usize, 0usize, false);
    for (name, item) in &manifest {
        // A single upload supplies every path with these bytes. The first
        // lexical path is a stable representative, even between page uploads.
        if !seen.insert(s(item, "sha256"))
            || exists.query_row(
                params![repository, s(item, "sha256"), item["bytes"].as_u64()],
                |r| r.get::<_, bool>(0),
            )?
        {
            continue;
        }
        total += 1;
        if name <= &after {
            continue;
        }
        let cost = serde_json::to_vec(name)?.len() + 1;
        if missing.len() >= limit || bytes + cost > PAGE_BYTES {
            more = true;
            continue;
        }
        // A byte bound, like the item bound, ends a contiguous lexical page.
        if more {
            continue;
        }
        bytes += cost;
        missing.push(name.clone());
    }
    let cursor = if more {
        missing.last().map(|last| {
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(
                    &json!({"sync_id":sync["id"],"snapshot":sync["snapshot"],"after":last}),
                )
                .expect("JSON cursor"),
            )
        })
    } else {
        None
    };
    Ok(
        json!({"format":"graphicle.sync-missing.v1","sync_id":sync["id"],"snapshot":sync["snapshot"],"missing":missing,
        "missing_total":total,"next_cursor":cursor,"replayed":false}),
    )
}

