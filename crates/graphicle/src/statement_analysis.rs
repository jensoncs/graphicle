//! Source capture is authorized and revision-bound; compiler work runs outside the DB lock.
use super::*;
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    node_id: String,
    #[serde(default)]
    goos: String,
    #[serde(default)]
    goarch: String,
    #[serde(default)]
    tags: Vec<String>,
}
pub(super) fn call(store: &Store, workspace: &str, token: &str, args: &Value) -> Result<Value> {
    let q: Request = serde_json::from_value(engine_arguments(args))?;
    if q.node_id.len() > 2000 || q.tags.len() > 32 || q.tags.iter().any(|s| s.len() > 100) {
        return Err(Error::bad("Statement query exceeds limits"));
    }
    let captured = store.authorized_read(workspace, token, "read", |db, p| {
        let repository = repo(db, p, string(args, "repository_id", 100)?)?;
        let repository = s(&repository, "id").to_owned();
        let reading = reading(db, p, &repository, args.get("as_of").and_then(Value::as_str))?;
        let resident = resident(db, &repository, &reading.episode)?;
        let graph = &resident.context.graph;
        let index = graph.node_index(&json!(q.node_id)).ok_or_else(|| Error::bad("Node not found"))?;
        let node = &graph.nodes()[index];
        let path = code_graph_arm::relpath(&node.attributes).ok_or_else(|| Error::bad("Node has no source"))?;
        if !path.ends_with(".go") || path.ends_with("_test.go") {
            return Err(Error::bad("Statement analysis currently supports non-test Go files"));
        }
        let manifest = if reading.episode == reading.current {
            &reading.heads
        } else {
            reading.recorded.as_ref().ok_or_else(|| Error::conflict("Episode source manifest unavailable"))?
        };
        let parent = std::path::Path::new(&path).parent();
        let mut files = BTreeMap::new();
        let mut hashes = BTreeMap::new();
        let mut bytes = 0usize;
        for (name, sha) in manifest.iter().filter(|(name, _)| {
            name.ends_with(".go") && std::path::Path::new(name).parent() == parent
        }) {
            if files.len() >= 128 {
                return Err(Error::bad("Statement package exceeds 128 files"));
            }
            let row = one(
                db,
                "SELECT content FROM repo_objects WHERE repository_id=? AND sha256=? AND typeof(content)='text' AND length(CAST(content AS BLOB))<=2097152",
                params![repository, sha],
            )?.ok_or_else(|| Error::conflict("Pinned source unavailable or too large"))?;
            let content = s(&row, "content").to_owned();
            bytes += content.len();
            if bytes > 2 * 1024 * 1024 {
                return Err(Error::bad("Statement package exceeds 2 MiB"));
            }
            if graphicle_memory::text::sha256(content.as_bytes()) != *sha {
                return Err(Error::conflict("Pinned source hash mismatch"));
            }
            files.insert(name.clone(), content);
            hashes.insert(name.clone(), sha.clone());
        }
        let source = files.get(&path).ok_or_else(|| Error::conflict("Node source unavailable"))?;
        // Line numbers alone cannot distinguish multiple declarations on a line.
        // Reuse the extractor's exact AST span, and refuse legacy/ambiguous nodes.
        let declarations = node.attributes.get("symbol_metadata")
            .and_then(Value::as_array)
            .filter(|items| items.len() == 1)
            .ok_or_else(|| Error::conflict("Exact declaration span unavailable or ambiguous; reproject the source"))?;
        let declaration = &declarations[0];
        let offset = declaration["byte_start"].as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|start| declaration["language"] == "go" && source.get(*start..).is_some_and(|text| text.starts_with("func")))
            .ok_or_else(|| Error::bad("Node is not a supported Go function declaration"))?;

        Ok((repository, reading.episode, path, offset, files, hashes))
    })?;
    let (repository, episode, path, offset, files, hashes) = captured;
    let mut result = graphicle::extractors::go_types::semantic(
        &json!({"mode":"flow","files":files,"file":path,"offset":offset,"goos":q.goos,"goarch":q.goarch,"tags":q.tags}),
    )?;
    if result.get("error").is_none() && result["function_byte_start"] != json!(offset) {
        return Err(Error::bad(
            "Selected node is not a supported Go function declaration",
        ));
    }
    let current = store.authorized_read(workspace, token, "read", |db, p| {
        repo(db, p, &repository)?;
        let reading = reading(db, p, &repository, None)?;
        Ok(hashes
            .iter()
            .all(|(path, sha)| reading.heads.get(path) == Some(sha))
            && reading
                .heads
                .keys()
                .filter(|name| {
                    name.ends_with(".go")
                        && std::path::Path::new(name).parent()
                            == std::path::Path::new(&path).parent()
                })
                .count()
                == hashes.len())
    })?;
    if result.get("error").is_some() {
        result["coverage"]["complete"] = json!(false);
    }
    result["tool"] = json!("statement_flow");
    result["repo"] = json!(repository);
    result["analyzer"] = json!("go-statements-v1");
    result["requested_profile"] =
        json!({"goos":q.goos,"goarch":q.goarch,"tags":q.tags,"tests":false,"cgo":false});
    result["repository_id"] = json!(repository);
    result["episode"] = json!(episode);
    result["node_id"] = json!(q.node_id);
    result["source_hashes"] = json!(hashes);
    result["coverage"]["current_sources_match"] = json!(current);
    if !current {
        result["coverage"]["complete"] = json!(false);
    }
    for statement in result["statements"].as_array_mut().into_iter().flatten() {
        let start = statement["byte_start"].as_u64().unwrap_or(0) as usize;
        let end = statement["byte_end"].as_u64().unwrap_or(0) as usize;
        statement["citation"] = json!({"repository_id":repository,"episode":episode,"file":path,"sha256":hashes[&path],"byte_start":start,"byte_end":end});
        if !matches!(statement["kind"].as_str(), Some("entry" | "exit")) {
            statement["text"] = json!(files[&path].get(start..end).filter(|s| s.len() <= 4096));
        }
    }
    result["text"] = json!(result.to_string());
    Ok(result)
}
