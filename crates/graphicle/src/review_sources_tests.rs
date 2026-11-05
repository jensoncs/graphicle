use super::*;
use rusqlite::{Connection, StatementStatus};

fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE repo_objects(repository_id TEXT NOT NULL, sha256 TEXT NOT NULL, content BLOB NOT NULL, bytes INTEGER NOT NULL, PRIMARY KEY(repository_id,sha256)); CREATE INDEX repo_objects_size ON repo_objects(repository_id,sha256,bytes);").unwrap();
    db
}

fn insert(db: &Connection, repository: &str, content: Content) -> Source {
    let sha = text::sha256(content.bytes());
    db.execute(
        "INSERT OR IGNORE INTO repo_objects VALUES(?,?,?,?)",
        params![repository, sha, content.sql(), content.bytes().len()],
    )
    .unwrap();
    Source {
        repository: repository.into(),
        path: "file".into(),
        sha,
        content: content.into(),
    }
}

#[test]
fn shared_sources_preserve_bytes_types_and_repository_boundaries() {
    let db = database();
    let mut pool = SourcePool::default();
    for content in [
        Content::Text("source text".into()),
        Content::Binary(vec![0, 255, 128, 10]),
    ] {
        let source = insert(&db, "repo", content);
        let first = pool.load(&db, "repo", &source.sha).unwrap().unwrap();
        let second = pool.load(&db, "repo", &source.sha).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.bytes(), source.content.bytes());
        assert_eq!(first.text(), source.content.text());
        assert!(pool.load(&db, "other", &source.sha).unwrap().is_none());
        db.execute("INSERT INTO repo_objects SELECT 'other',sha256,content,bytes FROM repo_objects WHERE repository_id='repo' AND sha256=?",[&source.sha]).unwrap();
        let other = pool.load(&db, "other", &source.sha).unwrap().unwrap();
        assert!(!Arc::ptr_eq(&first, &other));
        assert_eq!(first.bytes(), other.bytes());
    }
}

#[test]
fn new_read_rechecks_corruption_and_missing_objects() {
    let db = database();
    let source = insert(&db, "repo", Content::Text("original".into()));
    let mut pool = SourcePool::default();
    assert!(pool.load(&db, "repo", &source.sha).unwrap().is_some());
    db.execute("UPDATE repo_objects SET content='corrupt'", [])
        .unwrap();
    assert!(
        SourcePool::default()
            .load(&db, "repo", &source.sha)
            .is_err()
    );
    db.execute("DELETE FROM repo_objects", []).unwrap();
    assert!(
        SourcePool::default()
            .load(&db, "repo", &source.sha)
            .unwrap()
            .is_none()
    );
    assert!(check_retained(&db, std::iter::once(&source)).is_err());
}

fn reference_retention<'a>(
    db: &Connection,
    sources: impl Iterator<Item = &'a Source>,
) -> Result<()> {
    let mut query = db.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM repo_objects WHERE repository_id=? AND sha256=? AND bytes=?)",
    )?;
    for source in sources {
        if !query.query_row(
            params![source.repository, source.sha, source.content.bytes().len()],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(Error::conflict(
                "Pinned source was removed during review; retry with an available snapshot",
            ));
        }
    }
    Ok(())
}

#[test]
fn batched_retention_matches_reference_across_boundaries_and_mutations() {
    let db = database();
    let sources: Vec<_> = (0..1030)
        .map(|i| insert(&db, "repo", Content::Text(format!("source {i}"))))
        .collect();
    let check = || {
        let all = || sources.iter().chain(sources.iter());
        assert_eq!(
            check_retained(&db, all()).map_err(|e| e.to_string()),
            reference_retention(&db, all()).map_err(|e| e.to_string())
        );
    };
    check();
    assert_eq!(
        db.prepare_cached(&retention_query(RETENTION_BATCH))
            .unwrap()
            .get_status(StatementStatus::Run),
        4
    );
    assert!(check_retained(&db, std::iter::empty()).is_ok());
    for index in [0, 511, 512, 1029] {
        let sha = &sources[index].sha;
        db.execute(
            "UPDATE repo_objects SET repository_id='other' WHERE sha256=?",
            [sha],
        )
        .unwrap();
        check();
        assert!(check_retained(&db, sources.iter()).is_err());
        db.execute(
            "UPDATE repo_objects SET repository_id='repo' WHERE sha256=?",
            [sha],
        )
        .unwrap();
        db.execute(
            "UPDATE repo_objects SET bytes=bytes+1 WHERE sha256=?",
            [sha],
        )
        .unwrap();
        check();
        assert!(check_retained(&db, sources.iter()).is_err());
        db.execute(
            "UPDATE repo_objects SET bytes=bytes-1 WHERE sha256=?",
            [sha],
        )
        .unwrap();
    }
    // A second expected size for the same digest must not disappear in deduplication.
    let mut wrong = sources[0].clone();
    wrong.content = Content::Text("wrong size".into()).into();
    assert!(check_retained(&db, sources.iter().chain(std::iter::once(&wrong))).is_err());
    db.execute(
        "DELETE FROM repo_objects WHERE sha256=?",
        [&sources[1029].sha],
    )
    .unwrap();
    check();
    assert!(check_retained(&db, sources.iter()).is_err());
}

#[test]
fn retention_uses_covering_index_without_payload_reads() {
    let db = database();
    let mut statement = db
        .prepare(&format!("EXPLAIN QUERY PLAN {}", retention_query(1)))
        .unwrap();
    let plan: Vec<String> = statement
        .query_map(params!["repo", "sha", 10], |row| row.get(3))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert!(
        plan.iter()
            .any(|line| line.contains("COVERING INDEX repo_objects_size")),
        "{plan:?}"
    );
}

#[test]
#[ignore = "manual source sharing and retention before/after microbenchmark"]
fn source_io_benchmark() {
    for count in [20_000usize, 50_000] {
        for changed_percent in [1usize, 10, 100] {
            let db = database();
            let content = |i, side| Content::Text(format!("{i:08}:{side}:{}", "x".repeat(1013)));
            let base: Vec<_> = (0..count)
                .map(|i| insert(&db, "repo", content(i, 0)))
                .collect();
            let head: Vec<_> = (0..count)
                .map(|i| {
                    if i < count * changed_percent / 100 {
                        insert(&db, "repo", content(i, 1))
                    } else {
                        base[i].clone()
                    }
                })
                .collect();
            // One unreported warm-up per arm, then ABBA. No disk-cache-cold claim.
            for (iteration, shared) in [false, true, false, true, true, false]
                .into_iter()
                .enumerate()
            {
                let start = std::time::Instant::now();
                let mut pool = SourcePool::default();
                let loaded: Vec<_> = base
                    .iter()
                    .chain(head.iter())
                    .map(|source| {
                        if shared {
                            pool.load(&db, &source.repository, &source.sha)
                                .unwrap()
                                .unwrap()
                        } else {
                            Arc::new(
                                crate::source_object::load(&db, &source.repository, &source.sha)
                                    .unwrap()
                                    .unwrap(),
                            )
                        }
                    })
                    .collect();
                drop(pool);
                let load_ms = start.elapsed().as_secs_f64() * 1000.0;
                let start = std::time::Instant::now();
                if shared {
                    check_retained(&db, base.iter().chain(head.iter())).unwrap()
                } else {
                    reference_retention(&db, base.iter().chain(head.iter())).unwrap()
                }
                let retention_ms = start.elapsed().as_secs_f64() * 1000.0;
                // Exact payload equality and allocation sharing outside timing.
                for (source, actual) in base.iter().chain(head.iter()).zip(&loaded) {
                    assert_eq!(source.content.bytes(), actual.bytes());
                }
                let unique: BTreeMap<_, _> = loaded
                    .iter()
                    .map(|p| (Arc::as_ptr(p) as usize, p.bytes().len()))
                    .collect();
                let retained_payload_bytes: usize = unique.values().sum();
                if iteration >= 2 {
                    eprintln!(
                        "SOURCE_IO_BENCH={}",
                        json!({"files_per_side":count,"changed_percent":changed_percent,"arm":if shared {"after"} else {"before"},"order":iteration-1,"load_ms":load_ms,"retention_ms":retention_ms,"retained_payload_bytes":retained_payload_bytes})
                    );
                }
            }
        }
    }
}

// Previous loader, retained only to verify source-input equivalence.
fn reference_load(
    store: &Store,
    workspace: &str,
    token: &str,
    pins: &[Pin],
    profile: &Profile,
) -> Result<Input> {
    store.authorized_read(workspace,token,"read",|db,principal|{
        let mut input=Input{pins:pins.to_vec(),sources:BTreeMap::new(),issues:vec![],bindings:vec![]};let mut budget=admission::SourceBudget::default();
        for pin in pins {
            if store::repo(db,principal,&pin.repository_id).is_err(){input.issues.push(issue("unavailable_snapshot",json!(pin.directory)));continue}
            let row=store::one(db,"SELECT manifest,metadata FROM repo_snapshots WHERE repository_id=? AND sha256=?",params![pin.repository_id,pin.snapshot])?;
            let Some(row)=row else {input.issues.push(issue("unavailable_snapshot",json!(pin.directory)));continue};
            let manifest:Value=serde_json::from_str(s(&row,"manifest"))?;let metadata:Value=serde_json::from_str(s(&row,"metadata"))?;
            let binding=store::one(db,"SELECT scope_complete,withheld_paths FROM repo_snapshot_bindings WHERE repository_id=? AND snapshot=? AND commit_id=? AND policy_sha256=?",params![pin.repository_id,pin.snapshot,pin.commit,pin.policy_sha256])?;
            let legacy=metadata["commit"]==pin.commit && metadata["policy_sha256"]==pin.policy_sha256;
            if (binding.is_none() && !legacy)||crate::digest(&manifest)!=pin.snapshot {input.issues.push(issue("snapshot_binding_mismatch",json!(pin.directory)));continue}
            let complete = binding.as_ref().map(|v|v["scope_complete"]==1).unwrap_or(legacy && metadata["scope_complete"]==true);
            let withheld = match &binding {
                Some(row) if !row["withheld_paths"].is_null() => Some(serde_json::from_str::<Vec<String>>(s(row,"withheld_paths"))?),
                Some(_) => None,
                None => admission::withheld_paths(&metadata)?,
            };
            input.bindings.push(admission::Binding { pin:pin.clone(), complete, withheld, original:binding });
            if let Some(preparation) = metadata.get("dependency_preparation") {
                let prepared_profile = serde_json::from_value::<Profile>(preparation["profile"].clone());
                if prepared_profile.ok().and_then(|p| serde_json::to_value(p).ok()) != Some(serde_json::to_value(profile)?) {
                    input.issues.push(issue("dependency_preparation_profile_mismatch", json!(pin.directory)));
                }
            }
            if !complete {input.issues.push(issue("source_admission_incomplete",json!(pin.directory)));}
            let files=manifest.as_object().ok_or_else(||Error::bad("Invalid stored source manifest"))?;
            for (path,entry) in files {
                store::path(path)?;
                if local::excluded_pinned_source_path(path) {input.issues.push(issue("excluded_source",json!(pin.directory)));continue}
                let size=entry["bytes"].as_u64().ok_or_else(||Error::bad("Invalid source size"))?;
                budget.reserve(size, admission::vendored(path, files))?;
                let source=crate::source_object::load(db,&pin.repository_id,s(entry,"sha256"))?;
                let Some(content)=source else {input.issues.push(issue("missing_source_object",json!(pin.directory)));continue};
                if content.bytes().len()!=size as usize{return Err(Error::conflict("Pinned source failed integrity verification"));}
                input.sources.insert(format!("{}/{}",pin.directory,path),Source{repository:pin.repository_id.clone(),path:path.clone(),sha:s(entry,"sha256").into(),content:content.into()});
            }
        }
        Ok(input)
    })
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    workspace: String,
    token: String,
    repository: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("source sharing").unwrap();
        let workspace = s(&ws, "id").to_owned();
        let token = s(&ws, "capability").to_owned();
        let repo = store
            .register(
                &workspace,
                &token,
                &json!({"project_key":"sources","name":"sources"}),
            )
            .unwrap();
        Self {
            _dir: dir,
            store,
            workspace,
            token,
            repository: s(&repo, "id").into(),
        }
    }
    fn pin(&self, files: &[(&str, Content)]) -> Pin {
        let manifest: Value = json!(
            files
                .iter()
                .map(|(path, content)| (
                    path.to_string(),
                    json!({"sha256":text::sha256(content.bytes()),"bytes":content.bytes().len()})
                ))
                .collect::<BTreeMap<_, _>>()
        );
        self.pin_manifest(&manifest, files)
    }
    fn pin_manifest(&self, manifest: &Value, files: &[(&str, Content)]) -> Pin {
        let pin = Pin {
            repository_id: self.repository.clone(),
            directory: "app".into(),
            snapshot: crate::digest(manifest),
            commit: "a".repeat(40),
            policy_sha256: text::sha256(b"policy"),
            modules: vec![".".into()],
            excluded_modules: vec![],
        };
        self.store.transaction(|db| {
            for (_,content) in files {db.execute("INSERT OR IGNORE INTO repo_objects VALUES(?,?,?,?)",params![self.repository,text::sha256(content.bytes()),content.sql(),content.bytes().len()])?;}
            db.execute("INSERT OR REPLACE INTO repo_snapshots VALUES(?,?,?,?,?)",params![self.repository,pin.snapshot,manifest.to_string(),json!({"commit":pin.commit,"policy_sha256":pin.policy_sha256,"scope_complete":true}).to_string(),crate::now()])?;
            Ok(())
        }).unwrap();
        pin
    }
    fn request(&self, base: Pin, head: Pin) -> Request {
        serde_json::from_value(json!({"repository_id":self.repository,"base":[base],"head":[head],"profile":{"goos":"linux","goarch":"amd64","tests":false}})).unwrap()
    }
}
fn input_value(input: &Input) -> Value {
    json!({"pins":input.pins,"issues":input.issues,"bindings":input.bindings.iter().map(|b|json!([b.pin,b.complete,b.withheld,b.original])).collect::<Vec<_>>(),"sources":input.sources.iter().map(|(path,s)|json!([path,s.repository,s.path,s.sha,s.content.wire()])).collect::<Vec<_>>()})
}

#[test]
fn paired_load_matches_previous_inputs_and_shares_unchanged_content() {
    let f = Fixture::new();
    let base = f.pin(&[
        ("main.py", Content::Text("x = 1".into())),
        ("data.bin", Content::Binary(vec![0, 255])),
        ("copy.bin", Content::Binary(vec![0, 255])),
    ]);
    let head = f.pin(&[
        ("main.py", Content::Text("x = 2".into())),
        ("moved.bin", Content::Binary(vec![0, 255])),
    ]);
    let request = f.request(base, head);
    let (base, head) = load_sides(&f.store, &f.workspace, &f.token, &request).unwrap();
    for (actual, pins) in [(&base, &request.base), (&head, &request.head)] {
        let expected =
            reference_load(&f.store, &f.workspace, &f.token, pins, &request.profile).unwrap();
        assert_eq!(input_value(actual), input_value(&expected));
    }
    assert!(Arc::ptr_eq(
        &base.sources["app/data.bin"].content,
        &base.sources["app/copy.bin"].content
    ));
    assert!(Arc::ptr_eq(
        &base.sources["app/data.bin"].content,
        &head.sources["app/moved.bin"].content
    ));
    assert!(!Arc::ptr_eq(
        &base.sources["app/main.py"].content,
        &head.sources["app/main.py"].content
    ));
    assert!(load_sides(&f.store, &f.workspace, "invalid", &request).is_err());
    let foreign = f.store.create_workspace("foreign").unwrap();
    let (base, head) = load_sides(
        &f.store,
        s(&foreign, "id"),
        s(&foreign, "capability"),
        &request,
    )
    .unwrap();
    assert!(base.sources.is_empty() && head.sources.is_empty());
    assert_eq!(base.issues[0]["reason"], "unavailable_snapshot");
}

#[test]
fn paired_load_still_validates_each_manifest_size_binding_and_budget() {
    let f = Fixture::new();
    let base = f.pin(&[("main.py", Content::Text("x = 1".into()))]);
    let sha = text::sha256(b"x = 1");
    for size in [6u64, super::super::MAX_SOURCE_BYTES as u64 + 1] {
        let head = f.pin_manifest(&json!({"main.py":{"sha256":sha,"bytes":size}}), &[]);
        assert!(
            load_sides(
                &f.store,
                &f.workspace,
                &f.token,
                &f.request(base.clone(), head)
            )
            .is_err()
        );
    }
    let mut head = base.clone();
    head.commit = "b".repeat(40);
    let (_, head) = load_sides(
        &f.store,
        &f.workspace,
        &f.token,
        &f.request(base.clone(), head),
    )
    .unwrap();
    assert!(head.sources.is_empty());
    assert_eq!(head.issues[0]["reason"], "snapshot_binding_mismatch");
    // Reusing one object cannot bypass the per-side path count.
    let manifest = json!(
        (0..=super::super::MAX_FILES)
            .map(|i| (format!("{i}.py"), json!({"sha256":sha,"bytes":5})))
            .collect::<BTreeMap<_, _>>()
    );
    let head = f.pin_manifest(&manifest, &[]);
    assert!(load_sides(&f.store, &f.workspace, &f.token, &f.request(base, head)).is_err());
}

#[test]
fn concurrent_removal_keeps_read_snapshot_consistent_but_fails_final_check() {
    let f = Fixture::new();
    let pin = f.pin(&[("main.py", Content::Text("x = 1".into()))]);
    let request = f.request(pin.clone(), pin);
    let (base, head) = f
        .store
        .authorized_read(&f.workspace, &f.token, "read", |db, p| {
            let mut pool = SourcePool::default();
            let base = load(db, p, &mut pool, &request.base, &request.profile)?;
            f.store.transaction(|writer| {
                writer.execute("DELETE FROM repo_objects", [])?;
                Ok(())
            })?;
            let head = load(db, p, &mut pool, &request.head, &request.profile)?;
            assert_eq!(input_value(&base), input_value(&head));
            assert!(check_retained(db, base.sources.values()).is_ok());
            Ok((base, head))
        })
        .unwrap();
    assert!(
        f.store
            .authorized(&f.workspace, &f.token, "read", |db, _| check_retained(
                db,
                base.sources.values().chain(head.sources.values())
            ))
            .is_err()
    );
    let (base, head) = load_sides(&f.store, &f.workspace, &f.token, &request).unwrap();
    assert!(base.sources.is_empty() && head.sources.is_empty());
    assert_eq!(base.issues[0]["reason"], "missing_source_object");
}

#[test]
#[cfg(unix)]
fn review_deadline_bounds_storage_wait_and_recovers() {
    let f = Fixture::new();
    let pin = f.pin(&[("main.py", Content::Text("x = 1\n".into()))]);
    let mut args = json!({"repository_id":f.repository,"base":[pin],"head":[pin],
        "profile":{"goos":"linux","goarch":"amd64","tests":false},"max_tokens":8000});
    let initial = crate::review::run(&f.store, &f.workspace, &f.token, &args).unwrap();
    // Exercise the execution budget directly. HTTP request-budget admission is
    // a separate transaction before this review starts, not covered by it.
    let writer = f.store.connect().unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    args["timeout_ms"] = json!(100);
    let started = std::time::Instant::now();
    let error = crate::review::run(&f.store, &f.workspace, &f.token, &args).unwrap_err();
    assert_eq!(error.code(), "timeout", "{}", error.message);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    writer.execute_batch("ROLLBACK").unwrap();
    args.as_object_mut().unwrap().remove("timeout_ms");
    let recovered = crate::review::run(&f.store, &f.workspace, &f.token, &args).unwrap();
    assert_eq!(recovered["packet_id"], initial["packet_id"]);
}
