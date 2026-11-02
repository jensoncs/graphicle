use super::*;

#[test]
fn sync_reports_cold_revision_rebuild_and_warm_reuse_separately() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let f = Fixture::new();
    let repo = f.repo("sync-diagnostics");
    let files = [("a.py", "value = 1\n"), ("b.py", "value = 2\n")];
    let prepare = || {
        let state = f
            .store
            .project_status(&f.workspace, &f.token, &json!({"repository_id":repo}))
            .unwrap();
        f.store.sync_prepare(&f.workspace,&f.token,&json!({"repository_id":repo,
            "expected_snapshot":state["repository"]["snapshot"],
            "manifest":files.iter().map(|(p,c)|json!({"path":p,"bytes":c.len(),"sha256":text::sha256(c.as_bytes())})).collect::<Vec<_>>()
        })).unwrap()
    };
    let first = prepare();
    f.store.sync_upload(&f.workspace,&f.token,&json!({"repository_id":repo,"sync_id":first["sync_id"],
        "files":files.iter().map(|(p,c)|json!({"path":p,"content_base64":STANDARD.encode(c)})).collect::<Vec<_>>()
    })).unwrap();
    let commit = |sync: &Value| {
        f.store
            .sync_commit(
                &f.workspace,
                &f.token,
                &json!({"repository_id":repo,"sync_id":sync["sync_id"],"compact":true}),
            )
            .unwrap()
    };
    let cold = commit(&first);
    assert_eq!(cold["analysis_cache"]["parsed_files"], 2);
    assert_eq!(cold["analysis_cache"]["uncached_files"], 2);
    assert_eq!(cold["analysis_cache"]["cache_rebuilt"], false);
    f.store.transaction(|db| {
        db.execute("UPDATE repo_parses SET revision='previous-revision' WHERE repository_id=? AND path='a.py'",[&repo])?;
        Ok(())
    }).unwrap();
    let stale = prepare();
    assert!(stale["missing"].as_array().unwrap().is_empty());
    let rebuilt = commit(&stale);
    assert_eq!(rebuilt["analysis_cache"]["cache_rebuilt"], true);
    assert_eq!(rebuilt["analysis_cache"]["parsed_files"], 1);
    assert_eq!(rebuilt["analysis_cache"]["revision_reparsed_files"], 1);
    assert_eq!(rebuilt["analysis_cache"]["uncached_files"], 0);
    assert_eq!(rebuilt["analysis_cache"]["reused_files"], 1);
    assert_eq!(
        rebuilt["analysis_cache"]["structural_revision"],
        graphicle_memory::structure::REVISION
    );
    let replay = commit(&stale);
    assert_eq!(replay["analysis_cache"]["cache_rebuilt"], false);
    assert_eq!(replay["analysis_cache"]["parsed_files"], 0);
    assert_eq!(replay["analysis_cache"]["reused_files"], 2);
    assert_eq!(prepare()["sync_id"], Value::Null);
}
