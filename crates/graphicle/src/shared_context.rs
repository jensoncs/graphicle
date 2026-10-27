//! Immutable source-only context sharing. GCS owns object durability and IAM;
//! the caller owns commit/dependency attestation and baseline selection.
//! No live store, credential, connector document or reviewed memory is exported.
use crate::{
    Error, Result, local, query,
    store::{self, Store, s},
};
use chrono::Utc;
use clap::Subcommand;
use graphicle_memory::text::{canonical, sha256};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

const FORMAT: &str = "graphicle.shared-context.v1";
const MAX_BYTES: usize = 64 * 1024 * 1024;

/// The trusted materializer supplies this binding. Hashes represent the actual
/// selected dependency bytes, build profile and exclusion policy, not names.
/// Graphicle checks the admitted source manifest itself. This is not GitHub
/// merge attestation, complete dependency discovery, or qualified call evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub repository: String,
    pub commit: String,
    pub source_snapshot: String,
    pub dependencies_sha256: String,
    pub profile_sha256: String,
    pub exclusions_sha256: String,
}
impl Identity {
    fn validate(&self) -> Result<()> {
        if self.repository.is_empty()
            || self.repository.len() > 512
            || self.repository.chars().any(char::is_control)
            || ![40, 64].contains(&self.commit.len())
            || !self
                .commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::bad(
                "Expected repository identity and a full lowercase Git commit ID",
            ));
        }
        for hash in [
            &self.source_snapshot,
            &self.dependencies_sha256,
            &self.profile_sha256,
            &self.exclusions_sha256,
        ] {
            store::hash(hash)?;
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    path: String,
    sha256: String,
    content: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    format: String,
    identity: Identity,
    engine_sha256: String,
    expires_at: i64,
    sources: Vec<Source>,
    graph: Value,
}

fn allowed_path(path: &str) -> Result<()> {
    store::path(path)?;
    if local::skipped_file(path)
        || path.split('/').any(|part| {
            local::sensitive(part)
                || part.starts_with(".env")
                || part == "secrets"
                || local::SKIPPED_NAMES.contains(&part)
        })
    {
        return Err(Error::bad(
            "Snapshot contains a source excluded from sharing",
        ));
    }
    Ok(())
}

impl Bundle {
    fn validate(&self, expected: &Identity, engine: &str, now: i64) -> Result<()> {
        expected.validate()?;
        if self.format != FORMAT || &self.identity != expected || self.engine_sha256 != engine {
            return Err(Error::conflict(
                "Shared context identity, format or engine does not match",
            ));
        }
        if self.expires_at <= now {
            return Err(Error::conflict("Shared context has expired"));
        }
        if self.sources.is_empty() || self.sources.len() > crate::limits::REQUEST_FILES * 100 {
            return Err(Error::bad("Shared context source count is invalid"));
        }
        let mut manifest = BTreeMap::new();
        for source in &self.sources {
            allowed_path(&source.path)?;
            if sha256(source.content.as_bytes()) != source.sha256 || source.content.contains('\0') {
                return Err(Error::bad(
                    "Shared context source digest or text is invalid",
                ));
            }
            if manifest
                .insert(
                    &source.path,
                    json!({"sha256":source.sha256,"bytes":source.content.len()}),
                )
                .is_some()
            {
                return Err(Error::bad("Shared context contains duplicate source paths"));
            }
        }
        if crate::digest(&json!(manifest)) != expected.source_snapshot {
            return Err(Error::conflict(
                "Shared context source manifest does not match",
            ));
        }
        if !self.graph.is_object() {
            return Err(Error::bad("Shared context graph is invalid"));
        }
        Ok(())
    }

    fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = canonical(&serde_json::to_value(self)?).into_bytes();
        if bytes.len() > MAX_BYTES {
            return Err(Error::bad(
                "Shared context exceeds the 64 MiB artifact limit",
            ));
        }
        Ok(bytes)
    }

    fn decode(
        bytes: &[u8],
        digest: &str,
        identity: &Identity,
        engine: &str,
        now: i64,
    ) -> Result<Self> {
        store::hash(digest)?;
        if bytes.len() > MAX_BYTES || sha256(bytes) != digest {
            return Err(Error::bad(
                "Shared context object failed size or SHA-256 verification",
            ));
        }
        let bundle: Self = serde_json::from_slice(bytes)?;
        bundle.validate(identity, engine, now)?;
        Ok(bundle)
    }

    /// Uses the existing local retrieval/packing implementation; no live sync,
    /// model invocation, repository mutation, or upgrade of semantic authority.
    pub fn context(
        &self,
        expected: &Identity,
        engine: &str,
        question: &str,
        max_tokens: usize,
    ) -> Result<Value> {
        self.validate(expected, engine, Utc::now().timestamp())?;
        if question.is_empty() || question.len() > 64 * 1024 || !(1..=32000).contains(&max_tokens) {
            return Err(Error::bad(
                "Expected a question and a token budget of 1–32000",
            ));
        }
        let repository = json!({"id":sha256(expected.repository.as_bytes()),"name":expected.repository,"snapshot":expected.source_snapshot});
        let documents = self.sources.iter().map(|source| json!({
            "id":format!("file:{}",source.path),"path":source.path,"title":source.path,
            "source_kind":"repository_file","sha256":source.sha256,"version":source.sha256,
            "snapshot":expected.source_snapshot,"content":source.content,"recorded_at":null,
            "observed_at":null,"authority":"original_repository_source","url":null,"metadata":{}
        })).collect();
        let snapshot = query::Snapshot {
            repository,
            documents,
            memories: vec![],
            graph: self.graph.clone(),
            state: sha256(&self.bytes()?),
        };
        let mut packet = query::context(
            &snapshot,
            &json!({"question":question,"max_tokens":max_tokens,"max_chars":max_tokens*4}),
            json!([]),
        )?;
        packet["shared_context"] = json!({"identity":self.identity,"expires_at":self.expires_at,"engine_sha256":self.engine_sha256,
            "scope":"repository_sources_only","semantic_authority":"unverified_candidate","commit_binding":"caller_attested",
            "source_files":self.sources.len(),"complete_review_context":false,
            "excluded_context_sources":["connector_documents","reviewed_memories","sessions"],
            "unresolved_calls":self.graph["unresolved_calls"].as_array().map(Vec::len),
            "unsupported_files":self.graph["unsupported_files"]});
        Ok(packet)
    }
}

/// Export only the current, exact source set, in one authorized read transaction.
pub fn export(
    store: &Store,
    session: &local::Session,
    project: &str,
    identity: &Identity,
    engine: &str,
    expires_at: i64,
) -> Result<Bundle> {
    identity.validate()?;
    store::hash(engine)?;
    if expires_at <= Utc::now().timestamp() {
        return Err(Error::bad("An explicit future expires_at is required"));
    }
    store.authorized_read(&session.workspace, &session.capability, "read", |db, principal| {
        // Reading locally does not grant permission to export all project source.
        if principal.role != "owner" { return Err(Error::denied()); }
        let repository = store::repo(db, principal, project)?;
        if s(&repository, "snapshot") != identity.source_snapshot {
            return Err(Error::conflict("Project changed: expected a different source snapshot"));
        }
        // Bound the read before collecting source bodies, not only after JSON
        // serialization. The store may admit more data than one shared object.
        let source_bytes: i64 = db.query_row("SELECT COALESCE(SUM(o.bytes),0) FROM repo_heads h JOIN repo_objects o ON o.repository_id=h.repository_id AND o.sha256=h.sha256 WHERE h.repository_id=?", [project], |row| row.get(0))?;
        if source_bytes < 0 || source_bytes as u64 > MAX_BYTES as u64 {
            return Err(Error::bad("Shared context source exceeds the 64 MiB artifact limit"));
        }
        let rows = store::rows(db, "SELECT h.path,h.sha256,o.content FROM repo_heads h JOIN repo_objects o ON o.repository_id=h.repository_id AND o.sha256=h.sha256 WHERE h.repository_id=? ORDER BY h.path", [project])?;
        let sources = rows.iter().map(|row| Source { path:s(row,"path").into(), sha256:s(row,"sha256").into(), content:s(row,"content").into() }).collect();
        let mut bundle = Bundle { format:FORMAT.into(), identity:identity.clone(), engine_sha256:engine.into(), expires_at,
            sources, graph:json!({}) };
        bundle.validate(identity, engine, Utc::now().timestamp())?;
        bundle.bytes()?;
        // Reuse the responsible extraction cache, keyed by the running engine,
        // rather than relabelling a persisted graph from a previous build.
        let (episode, graph, report) = crate::graph_projection::build_graph_with_report(db, project)?;
        bundle.graph = query::episode_graph(&graph, &episode);
        bundle.graph["unresolved_calls"] = graph.attributes().get("unresolved_calls").cloned().unwrap_or(json!([]));
        bundle.graph["unsupported_files"] = report["unsupported"].clone();
        bundle.bytes()?;
        Ok(bundle)
    })
}

/// GCS JSON API, with immutable create-only writes. No alternate hosts or
/// redirects receive the credential. No list, overwrite, delete, IAM or bucket
/// lifecycle operation is needed. IAM read access is checked on every query.
pub struct Gcs {
    client: reqwest::Client,
    bucket: String,
    prefix: String,
    token: String,
    endpoint: url::Url,
}
impl Gcs {
    pub fn new(prefix: &str, token: String) -> Result<Self> {
        let rest = prefix
            .strip_prefix("gs://")
            .ok_or_else(|| Error::bad("Expected gs://bucket/prefix/"))?;
        let (bucket, path) = rest
            .split_once('/')
            .ok_or_else(|| Error::bad("An explicit GCS object prefix is required"))?;
        let path = path.trim_end_matches('/');
        if !(3..=63).contains(&bucket.len())
            || !bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
            || !bucket.as_bytes()[0].is_ascii_alphanumeric()
            || !bucket.as_bytes()[bucket.len() - 1].is_ascii_alphanumeric()
            || path.is_empty()
            || path.len() > 512
            || !path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/-_".contains(&b))
            || path.split('/').any(str::is_empty)
            || token.is_empty()
        {
            return Err(Error::bad("Invalid GCS bucket, prefix or access token"));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| Error::bad("Could not construct the GCS client"))?;
        Ok(Self {
            client,
            bucket: bucket.into(),
            prefix: path.into(),
            token,
            endpoint: url::Url::parse("https://storage.googleapis.com/").unwrap(),
        })
    }
    fn object(&self, identity: &Identity, digest: &str) -> Result<String> {
        identity.validate()?;
        store::hash(digest)?;
        Ok(format!(
            "{}/v1/{}/{}.json",
            self.prefix,
            sha256(identity.repository.as_bytes()),
            digest
        ))
    }
    fn url(&self, upload: bool, object: &str) -> url::Url {
        let mut url = self.endpoint.clone();
        let mut path = url.path_segments_mut().unwrap();
        path.clear();
        if upload {
            path.push("upload");
        }
        path.extend(["storage", "v1", "b", &self.bucket, "o"]);
        if !upload {
            path.push(object);
        }
        drop(path);
        if upload {
            url.query_pairs_mut()
                .append_pair("uploadType", "media")
                .append_pair("name", object)
                .append_pair("ifGenerationMatch", "0");
        } else {
            url.query_pairs_mut().append_pair("alt", "media");
        }
        url
    }
    async fn read(&self, object: &str) -> Result<Vec<u8>> {
        let mut response = self
            .client
            .get(self.url(false, object))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|_| Error::bad("GCS download failed"))?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::bad(format!(
                "GCS download returned HTTP {}",
                response.status().as_u16()
            )));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(Error::bad("GCS object exceeds the 64 MiB limit"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::bad("GCS download interrupted"))?
        {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(Error::bad("GCS object exceeds the 64 MiB limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    pub async fn publish(&self, bundle: &Bundle) -> Result<Value> {
        bundle.validate(
            &bundle.identity,
            &bundle.engine_sha256,
            Utc::now().timestamp(),
        )?;
        let bytes = bundle.bytes()?;
        let digest = sha256(&bytes);
        let object = self.object(&bundle.identity, &digest)?;
        let response = self
            .client
            .post(self.url(true, &object))
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(bytes.clone())
            .send()
            .await
            .map_err(|_| Error::bad("GCS upload failed; retrying the same snapshot is safe"))?;
        let reused = response.status() == reqwest::StatusCode::PRECONDITION_FAILED;
        if !response.status().is_success() && !reused {
            return Err(Error::bad(format!(
                "GCS upload returned HTTP {}",
                response.status().as_u16()
            )));
        }
        // A pre-existing key is a hit only if its complete contents match. Read
        // back new objects too; an HTTP success alone is not delivery evidence.
        if self.read(&object).await? != bytes {
            return Err(Error::conflict("GCS object verification failed"));
        }
        bundle.validate(
            &bundle.identity,
            &bundle.engine_sha256,
            Utc::now().timestamp(),
        )?;
        Ok(
            json!({"uri":format!("gs://{}/{}",self.bucket,object),"snapshot":digest,"bytes":bytes.len(),"reused":reused,
            "expires_at":bundle.expires_at,"verified":true,"baseline_published":false}),
        )
    }
    pub async fn fetch(&self, identity: &Identity, digest: &str, engine: &str) -> Result<Bundle> {
        let bytes = self.read(&self.object(identity, digest)?).await?;
        Bundle::decode(&bytes, digest, identity, engine, Utc::now().timestamp())
    }
}

fn engine_digest() -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(std::env::current_exe()?)?;
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 65536];
    loop {
        let n = f.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn identity_file(path: &Path) -> Result<Identity> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(8193)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 8192 {
        return Err(Error::bad("Identity file exceeds 8 KiB"));
    }
    let identity: Identity = serde_json::from_slice(&bytes)?;
    identity.validate()?;
    Ok(identity)
}

#[derive(Subcommand)]
pub enum Command {
    /// Verify a GCS artifact and sync its source into an existing local project.
    /// This changes that project's current source snapshot.
    Import {
        #[arg(long)]
        project: String,
        #[arg(long)]
        identity: PathBuf,
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long)]
        snapshot: String,
        /// Attest that the bundle includes the complete intended source scope.
        #[arg(long)]
        scope_complete: bool,
    },
    /// Export current repository source and graph to an immutable GCS object.
    /// The trusted harness must attest commit/dependencies in the identity file.
    Publish {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        identity: PathBuf,
        /// GCS destination; otherwise GRAPHICLE_SHARED_CONTEXT_PREFIX.
        #[arg(long)]
        prefix: Option<String>,
        /// Explicit expiry as Unix seconds. No implicit retention policy.
        #[arg(long)]
        expires_at: i64,
    },
    /// Retrieve verified context for an exact snapshot, without syncing a folder.
    Context {
        #[arg(long)]
        identity: PathBuf,
        /// GCS destination; otherwise GRAPHICLE_SHARED_CONTEXT_PREFIX.
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long)]
        snapshot: String,
        #[arg(long, default_value_t = 8400)]
        max_tokens: usize,
        question: String,
    },
}

pub async fn run(directory: &Path, command: Command) -> Result<Value> {
    let token = std::env::var("GRAPHICLE_GCS_ACCESS_TOKEN").map_err(|_| {
        Error::bad("Set GRAPHICLE_GCS_ACCESS_TOKEN to a short-lived Google access token")
    })?;
    let engine = engine_digest()?;
    match command {
        Command::Import {
            project,
            identity,
            prefix,
            snapshot,
            scope_complete,
        } => {
            let identity = identity_file(&identity)?;
            let store = Store::open(directory)?;
            let session = local::session(&store, false)?;
            let project = local::project(&store, &session, Some(&project))?;
            store.authorized_read(
                &session.workspace,
                &session.capability,
                "collect",
                |db, p| store::repo(db, p, &project).map(|_| ()),
            )?;
            let bundle = Gcs::new(&destination(prefix)?, token)?
                .fetch(&identity, &snapshot, &engine)
                .await?;
            import_sources(&store, &session, &project, &bundle, &engine, scope_complete)
        }
        Command::Publish {
            project,
            identity,
            prefix,
            expires_at,
        } => {
            let identity = identity_file(&identity)?;
            let gcs = Gcs::new(&destination(prefix)?, token)?;
            let store = Store::open(directory)?;
            let session = local::session(&store, false)?;
            let project = local::project(&store, &session, project.as_deref())?;
            let bundle = export(&store, &session, &project, &identity, &engine, expires_at)?;
            gcs.publish(&bundle).await
        }
        Command::Context {
            identity,
            prefix,
            snapshot,
            max_tokens,
            question,
        } => {
            let identity = identity_file(&identity)?;
            let bundle = Gcs::new(&destination(prefix)?, token)?
                .fetch(&identity, &snapshot, &engine)
                .await?;
            bundle.context(&identity, &engine, &question, max_tokens)
        }
    }
}

/// Verified source admission reuses the normal sync's limits, permissions and
/// compare-and-swap semantics. Cached graph claims are never imported.
fn import_sources(
    store: &Store,
    session: &local::Session,
    project: &str,
    bundle: &Bundle,
    engine: &str,
    complete: bool,
) -> Result<Value> {
    bundle.validate(&bundle.identity, engine, Utc::now().timestamp())?;
    let status = store.project_status(
        &session.workspace,
        &session.capability,
        &json!({"repository_id":project}),
    )?;
    let metadata = json!({"commit":bundle.identity.commit,"policy_sha256":bundle.identity.exclusions_sha256,"scope_complete":complete,"shared_identity":bundle.identity});
    let mut prepared = Value::Null;
    let chunks = bundle.sources.chunks(crate::limits::REQUEST_FILES);
    let count = chunks.len();
    for (i, chunk) in chunks.enumerate() {
        let mut args = json!({"repository_id":project,"expected_snapshot":status["repository"]["snapshot"],"metadata":metadata,"manifest_complete":i+1==count,"manifest":chunk.iter().map(|s|json!({"path":s.path,"sha256":s.sha256,"bytes":s.content.len()})).collect::<Vec<_>>()});
        if i > 0 {
            args["sync_id"] = prepared["sync_id"].clone();
        }
        prepared = store.sync_prepare(&session.workspace, &session.capability, &args)?;
    }
    if !prepared["sync_id"].is_null() {
        // One source per upload also bounds aggregate request bytes.
        for source in &bundle.sources {
            store.sync_upload(&session.workspace,&session.capability,&json!({"repository_id":project,"sync_id":prepared["sync_id"],"files":[{"path":source.path,"content":source.content}]}))?;
        }
        store.sync_commit(
            &session.workspace,
            &session.capability,
            &json!({"repository_id":project,"sync_id":prepared["sync_id"]}),
        )?;
    }
    Ok(
        json!({"repository_id":project,"commit":bundle.identity.commit,"snapshot":bundle.identity.source_snapshot,"policy_sha256":bundle.identity.exclusions_sha256,"scope_complete":complete}),
    )
}

fn choose_destination(explicit: Option<&str>, configured: Option<&str>) -> Result<String> {
    explicit
        .or(configured)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            Error::bad("Set GRAPHICLE_SHARED_CONTEXT_PREFIX or pass --prefix gs://bucket/path/")
        })
}
fn destination(explicit: Option<String>) -> Result<String> {
    choose_destination(
        explicit.as_deref(),
        std::env::var("GRAPHICLE_SHARED_CONTEXT_PREFIX")
            .ok()
            .as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Bytes,
        extract::{Query, State},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::{get, post},
    };
    use std::sync::{Arc, Mutex};

    fn fixture() -> Bundle {
        let content = "def calculate_total(price, quantity):\n    return price * quantity\n";
        let sha = sha256(content.as_bytes());
        let manifest = json!({"billing.py":{"sha256":sha,"bytes":content.len()}});
        Bundle {
            format: FORMAT.into(),
            identity: Identity {
                repository: "example/billing".into(),
                commit: "a".repeat(40),
                source_snapshot: crate::digest(&manifest),
                dependencies_sha256: "b".repeat(64),
                profile_sha256: "c".repeat(64),
                exclusions_sha256: "d".repeat(64),
            },
            engine_sha256: "e".repeat(64),
            expires_at: Utc::now().timestamp() + 3600,
            sources: vec![Source {
                path: "billing.py".into(),
                sha256: sha,
                content: content.into(),
            }],
            graph: json!({"edges":[],"unresolved_calls":[{"name":"dynamic","reason":"unsupported"}]}),
        }
    }
    #[test]
    fn round_trip_preserves_sources_and_unsupported_diagnostics() {
        let bundle = fixture();
        let bytes = bundle.bytes().unwrap();
        let digest = sha256(&bytes);
        let decoded = Bundle::decode(
            &bytes,
            &digest,
            &bundle.identity,
            &bundle.engine_sha256,
            Utc::now().timestamp(),
        )
        .unwrap();
        assert_eq!(decoded.graph, bundle.graph);
        let packet = decoded
            .context(
                &bundle.identity,
                &bundle.engine_sha256,
                "calculate_total price quantity",
                300,
            )
            .unwrap();
        assert!(packet.to_string().contains("calculate_total"), "{packet}");
        assert_eq!(
            packet["shared_context"]["semantic_authority"],
            "unverified_candidate"
        );
    }
    #[test]
    fn rejects_corruption_expiry_and_every_identity_mismatch() {
        let bundle = fixture();
        let bytes = bundle.bytes().unwrap();
        let digest = sha256(&bytes);
        assert!(
            Bundle::decode(
                b"corrupt",
                &digest,
                &bundle.identity,
                &bundle.engine_sha256,
                0
            )
            .is_err()
        );
        assert!(
            Bundle::decode(
                &bytes,
                &digest,
                &bundle.identity,
                &bundle.engine_sha256,
                bundle.expires_at
            )
            .is_err()
        );
        assert!(Bundle::decode(&bytes, &digest, &bundle.identity, &"f".repeat(64), 0).is_err());
        let value = serde_json::to_value(&bundle.identity).unwrap();
        for field in [
            "repository",
            "commit",
            "source_snapshot",
            "dependencies_sha256",
            "profile_sha256",
            "exclusions_sha256",
        ] {
            let mut changed = value.clone();
            changed[field] = json!(if field == "repository" {
                "different/repo".into()
            } else if field == "commit" {
                "f".repeat(40)
            } else {
                "f".repeat(64)
            });
            let expected = serde_json::from_value(changed).unwrap();
            assert!(
                Bundle::decode(&bytes, &digest, &expected, &bundle.engine_sha256, 0).is_err(),
                "{field}"
            );
        }
    }
    #[test]
    fn rehashing_a_tampered_source_does_not_bypass_the_expected_manifest() {
        let mut bundle = fixture();
        bundle.sources[0].content.push_str("wrong");
        bundle.sources[0].sha256 = sha256(bundle.sources[0].content.as_bytes());
        let bytes = bundle.bytes().unwrap();
        assert!(
            Bundle::decode(
                &bytes,
                &sha256(&bytes),
                &bundle.identity,
                &bundle.engine_sha256,
                0
            )
            .is_err()
        );
    }
    #[test]
    fn excluded_and_duplicate_paths_cannot_be_exported() {
        for path in [
            "../outside.py",
            "secrets/config.py",
            ".env.production",
            ".aws/config",
            "key.pem",
            ".graphicle/data",
            ".claude/settings.local.json",
        ] {
            assert!(allowed_path(path).is_err(), "{path}");
        }
        let mut bundle = fixture();
        bundle.sources.push(bundle.sources[0].clone());
        assert!(
            bundle
                .validate(&bundle.identity, &bundle.engine_sha256, 0)
                .is_err()
        );
    }
    #[test]
    fn destinations_cannot_escape_the_explicit_prefix() {
        assert!(choose_destination(None, None).is_err());
        assert_eq!(
            choose_destination(None, Some("gs://another-bucket/context/")).unwrap(),
            "gs://another-bucket/context/"
        );
        assert_eq!(
            choose_destination(
                Some("gs://override-bucket/reviews/"),
                Some("gs://another-bucket/context/")
            )
            .unwrap(),
            "gs://override-bucket/reviews/"
        );
        for prefix in [
            "https://attacker/",
            "gs://bucket/../x",
            "gs://bucket/x?name=other",
            "gs://bucket/",
            "gs://bucket//x",
            "gs://bucket/x%2fy",
            "gs://bucket/x#f",
        ] {
            assert!(Gcs::new(prefix, "test".into()).is_err(), "{prefix}");
        }
        let gcs = Gcs::new("gs://code-review-transcripts/graphicle/", "test".into()).unwrap();
        let bundle = fixture();
        let object = gcs.object(&bundle.identity, &"f".repeat(64)).unwrap();
        let url = gcs.url(true, &object);
        assert_eq!(url.host_str(), Some("storage.googleapis.com"));
        assert!(
            url.query_pairs()
                .any(|(k, v)| k == "ifGenerationMatch" && v == "0")
        );
        assert!(object.starts_with("graphicle/v1/"));
    }

    #[derive(Default)]
    struct Objects {
        objects: BTreeMap<String, Vec<u8>>,
        writes: usize,
        reads: usize,
        deny: bool,
        preconditions: Vec<String>,
    }
    type Shared = Arc<Mutex<Objects>>;
    async fn upload(
        State(state): State<Shared>,
        Query(query): Query<BTreeMap<String, String>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> impl IntoResponse {
        assert_eq!(headers.get("authorization").unwrap(), "Bearer test-token");
        assert_eq!(query["uploadType"], "media");
        assert!(query["name"].starts_with("graphicle/v1/"));
        let mut state = state.lock().unwrap();
        state.preconditions.push(query["ifGenerationMatch"].clone());
        if state.objects.contains_key(&query["name"]) {
            return StatusCode::PRECONDITION_FAILED;
        }
        state.objects.insert(query["name"].clone(), body.to_vec());
        state.writes += 1;
        StatusCode::OK
    }
    async fn download(
        State(state): State<Shared>,
        axum::extract::Path(object): axum::extract::Path<String>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        assert_eq!(headers.get("authorization").unwrap(), "Bearer test-token");
        let mut state = state.lock().unwrap();
        state.reads += 1;
        if state.deny {
            return (StatusCode::FORBIDDEN, vec![]);
        }
        match state.objects.get(&object) {
            Some(bytes) => (StatusCode::OK, bytes.clone()),
            None => (StatusCode::NOT_FOUND, vec![]),
        }
    }
    struct Server(tokio::task::JoinHandle<()>);
    impl Drop for Server {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    async fn mock() -> (Gcs, Shared, Server) {
        let state: Shared = Default::default();
        let app = Router::new()
            .route(
                "/upload/storage/v1/b/code-review-transcripts/o",
                post(upload),
            )
            .route(
                "/storage/v1/b/code-review-transcripts/o/{*object}",
                get(download),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = Server(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        let mut gcs = Gcs::new(
            "gs://code-review-transcripts/graphicle/",
            "test-token".into(),
        )
        .unwrap();
        gcs.endpoint = url::Url::parse(&format!("http://{addr}/")).unwrap();
        (gcs, state, server)
    }
    #[tokio::test]
    async fn concurrent_publish_is_create_only_and_context_is_reusable() {
        let (gcs, state, _server) = mock().await;
        let bundle = fixture();
        let (a, b) = tokio::join!(gcs.publish(&bundle), gcs.publish(&bundle));
        let a = a.unwrap();
        let b = b.unwrap();
        assert_ne!(a["reused"], b["reused"]);
        assert_eq!(a["snapshot"], b["snapshot"]);
        assert_eq!(state.lock().unwrap().writes, 1);
        assert_eq!(state.lock().unwrap().preconditions, vec!["0", "0"]);
        let fetched = gcs
            .fetch(&bundle.identity, s(&a, "snapshot"), &bundle.engine_sha256)
            .await
            .unwrap();
        assert_eq!(
            fetched
                .context(
                    &bundle.identity,
                    &bundle.engine_sha256,
                    "calculate_total",
                    300
                )
                .unwrap(),
            bundle
                .context(
                    &bundle.identity,
                    &bundle.engine_sha256,
                    "calculate_total",
                    300
                )
                .unwrap()
        );
        state.lock().unwrap().deny = true;
        assert!(
            gcs.fetch(&bundle.identity, s(&a, "snapshot"), &bundle.engine_sha256)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn corrupt_existing_object_is_not_a_cache_hit() {
        let (gcs, state, _server) = mock().await;
        let bundle = fixture();
        let digest = sha256(&bundle.bytes().unwrap());
        state.lock().unwrap().objects.insert(
            gcs.object(&bundle.identity, &digest).unwrap(),
            b"wrong".to_vec(),
        );
        assert!(gcs.publish(&bundle).await.is_err());
        assert!(
            gcs.fetch(&bundle.identity, &digest, &bundle.engine_sha256)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn gcs_round_trip_imports_pinned_sources_for_review_and_push_invalidation() {
        let (gcs, _, _server) = mock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let session = local::session(&store, true).unwrap();
        let repo = store
            .register(
                &session.workspace,
                &session.capability,
                &json!({"project_key":"tax","name":"tax"}),
            )
            .unwrap();
        let id = s(&repo, "id");
        let mut pins = vec![];
        for (commit, name, body) in [
            (
                'a',
                "tax.go",
                "package tax\nfunc Calculate() int {return 1}\n",
            ),
            (
                'b',
                "tax.go",
                "package tax\nfunc Calculate() int {return 2}\n",
            ),
            (
                'c',
                "moved.go",
                "package tax\nfunc Calculate() int {return 3}\n",
            ),
        ] {
            let mut bundle = fixture();
            bundle.identity.repository = "example/tax".into();
            bundle.identity.commit = commit.to_string().repeat(40);
            bundle.sources = [("go.mod", "module example/tax\ngo 1.23\n"), (name, body)]
                .iter()
                .map(|(path, content)| Source {
                    path: (*path).into(),
                    content: (*content).into(),
                    sha256: sha256(content.as_bytes()),
                })
                .collect();
            bundle.identity.source_snapshot = crate::digest(&json!(
                bundle
                    .sources
                    .iter()
                    .map(|s| (&s.path, json!({"sha256":s.sha256,"bytes":s.content.len()})))
                    .collect::<BTreeMap<_, _>>()
            ));
            // Imported graph claims are not used as compiler facts.
            bundle.graph = json!({"nodes":[],"edges":[]});
            let published = gcs.publish(&bundle).await.unwrap();
            let fetched = gcs
                .fetch(
                    &bundle.identity,
                    s(&published, "snapshot"),
                    &bundle.engine_sha256,
                )
                .await
                .unwrap();
            let mut pin =
                import_sources(&store, &session, id, &fetched, &bundle.engine_sha256, true)
                    .unwrap();
            pin.as_object_mut().unwrap().remove("scope_complete");
            pin["directory"] = json!("tax");
            pins.push(pin);
        }
        let mut request = json!({"repository_id":id,"base":[pins[0]],"head":[pins[1]],"profile":{"goos":"linux","goarch":"amd64"}});
        let first =
            crate::review::run(&store, &session.workspace, &session.capability, &request).unwrap();
        assert_eq!(first["coverage"]["complete"], true, "{first}");
        assert!(
            first["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["side"] == "head" && e["text"].as_str().unwrap().contains("return 2"))
        );
        request["head"] = json!([pins[2]]);
        request["previous_packet_id"] = first["packet_id"].clone();
        let next =
            crate::review::run(&store, &session.workspace, &session.capability, &request).unwrap();
        let previous_head = first["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["side"] == "head")
            .unwrap();
        assert!(
            next["invalidated"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == previous_head["id"])
        );
        assert!(
            next["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["side"] == "head"
                    && e["file"] == "moved.go"
                    && e["text"].as_str().unwrap().contains("return 3"))
        );
    }
    #[tokio::test]
    async fn expired_upload_makes_no_network_request() {
        let (gcs, state, _server) = mock().await;
        let mut bundle = fixture();
        bundle.expires_at = 0;
        assert!(gcs.publish(&bundle).await.is_err());
        assert_eq!(state.lock().unwrap().writes, 0);
        assert_eq!(state.lock().unwrap().reads, 0);
    }

    #[tokio::test]
    async fn sibling_candidates_and_missing_or_expired_objects_never_substitute_context() {
        let (gcs, state, _server) = mock().await;
        let a = fixture();
        let mut b = fixture();
        b.identity.commit = "b".repeat(40);
        let (ra, rb) = tokio::join!(gcs.publish(&a), gcs.publish(&b));
        let ra = ra.unwrap();
        let rb = rb.unwrap();
        assert_eq!(state.lock().unwrap().writes, 2);
        assert_ne!(ra["snapshot"], rb["snapshot"]);
        assert!(
            gcs.fetch(&b.identity, s(&ra, "snapshot"), &a.engine_sha256)
                .await
                .is_err()
        );
        assert!(
            gcs.fetch(&a.identity, s(&ra, "snapshot"), &a.engine_sha256)
                .await
                .is_ok()
        );
        assert!(
            gcs.fetch(&b.identity, s(&rb, "snapshot"), &b.engine_sha256)
                .await
                .is_ok()
        );
        assert!(
            gcs.fetch(&a.identity, &"f".repeat(64), &a.engine_sha256)
                .await
                .is_err()
        );
        let mut expired = a.clone();
        expired.expires_at = 0;
        let bytes = expired.bytes().unwrap();
        let digest = sha256(&bytes);
        state
            .lock()
            .unwrap()
            .objects
            .insert(gcs.object(&a.identity, &digest).unwrap(), bytes);
        assert!(
            gcs.fetch(&a.identity, &digest, &a.engine_sha256)
                .await
                .is_err()
        );
        let object = gcs.object(&a.identity, s(&ra, "snapshot")).unwrap();
        state
            .lock()
            .unwrap()
            .objects
            .get_mut(&object)
            .unwrap()
            .truncate(10);
        assert!(
            gcs.fetch(&a.identity, s(&ra, "snapshot"), &a.engine_sha256)
                .await
                .is_err()
        );
    }

    #[test]
    fn missing_source_is_rejected_even_with_a_valid_object_digest() {
        let mut full = fixture();
        let mut second = full.sources[0].clone();
        second.path = "other.py".into();
        full.sources.push(second);
        let manifest: BTreeMap<_, _> = full
            .sources
            .iter()
            .map(|s| (&s.path, json!({"sha256":s.sha256,"bytes":s.content.len()})))
            .collect();
        full.identity.source_snapshot = crate::digest(&json!(manifest));
        full.validate(&full.identity, &full.engine_sha256, 0)
            .unwrap();
        full.sources.pop();
        let bytes = full.bytes().unwrap();
        assert!(
            Bundle::decode(
                &bytes,
                &sha256(&bytes),
                &full.identity,
                &full.engine_sha256,
                0
            )
            .is_err()
        );
    }

    #[test]
    fn verified_import_attests_pins_and_replays_without_importing_graph_claims() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("import").unwrap();
        let session = local::Session {
            workspace: s(&ws, "id").into(),
            capability: s(&ws, "capability").into(),
            project: None,
        };
        let repo = store
            .register(
                &session.workspace,
                &session.capability,
                &json!({"project_key":"billing","name":"billing"}),
            )
            .unwrap();
        let id = s(&repo, "id");
        let bundle = fixture();
        let pin =
            import_sources(&store, &session, id, &bundle, &bundle.engine_sha256, true).unwrap();
        assert_eq!(pin["snapshot"], bundle.identity.source_snapshot);
        assert_eq!(
            import_sources(&store, &session, id, &bundle, &bundle.engine_sha256, true).unwrap(),
            pin
        );
        store.authorized_read(&session.workspace,&session.capability,"read",|db,_| {
            let count:i64=db.query_row("SELECT count(*) FROM repo_snapshot_bindings WHERE repository_id=? AND commit_id=?",rusqlite::params![id,bundle.identity.commit],|r|r.get(0))?;
            assert_eq!(count,1);
            let graphs:i64=db.query_row("SELECT count(*) FROM gf_episodes WHERE repository_id=?",[id],|r|r.get(0))?;assert_eq!(graphs,0);Ok(())
        }).unwrap();
        let mut corrupt = bundle.clone();
        corrupt.sources[0].content.push('!');
        assert!(
            import_sources(&store, &session, id, &corrupt, &bundle.engine_sha256, true).is_err()
        );
    }

    #[test]
    fn export_is_owner_only_and_omits_non_repository_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let workspace = store.create_workspace("private workspace").unwrap();
        let ws = s(&workspace, "id");
        let key = s(&workspace, "capability");
        let project = store
            .register(ws, key, &json!({"project_key":"billing","name":"Billing"}))
            .unwrap();
        let id = s(&project, "id");
        let fixture = fixture();
        let source = &fixture.sources[0];
        let status = store
            .project_status(ws, key, &json!({"repository_id":id}))
            .unwrap();
        let prepared=store.sync_prepare(ws,key,&json!({"repository_id":id,"expected_snapshot":status["repository"]["snapshot"],"manifest":[{"path":source.path,"sha256":source.sha256,"bytes":source.content.len()}]})).unwrap();
        store.sync_upload(ws,key,&json!({"repository_id":id,"sync_id":prepared["sync_id"],"files":[{"path":source.path,"content":source.content}]})).unwrap();
        store
            .sync_commit(
                ws,
                key,
                &json!({"repository_id":id,"sync_id":prepared["sync_id"]}),
            )
            .unwrap();
        let session = local::Session {
            workspace: ws.into(),
            capability: key.into(),
            project: Some(id.into()),
        };
        let bundle = export(
            &store,
            &session,
            id,
            &fixture.identity,
            &fixture.engine_sha256,
            fixture.expires_at,
        )
        .unwrap();
        let repeated = export(
            &store,
            &session,
            id,
            &fixture.identity,
            &fixture.engine_sha256,
            fixture.expires_at,
        )
        .unwrap();
        assert_eq!(bundle.bytes().unwrap(), repeated.bytes().unwrap());
        let value = serde_json::to_value(&bundle).unwrap();
        assert_eq!(value["sources"].as_array().unwrap().len(), 1);
        assert!(!value.to_string().contains(key));
        assert!(!value.to_string().contains("private workspace"));
        assert!(value.get("memories").is_none());
        assert!(value.get("credentials").is_none());
        let reader = store
            .grant(ws, key, &json!({"label":"Reader","role":"reader"}))
            .unwrap();
        let reader_session = local::Session {
            capability: s(&reader, "capability").into(),
            ..session.clone()
        };
        assert!(
            export(
                &store,
                &reader_session,
                id,
                &fixture.identity,
                &fixture.engine_sha256,
                fixture.expires_at
            )
            .is_err()
        );
        let mut wrong = fixture.identity.clone();
        wrong.source_snapshot = "f".repeat(64);
        assert!(
            export(
                &store,
                &session,
                id,
                &wrong,
                &fixture.engine_sha256,
                fixture.expires_at
            )
            .is_err()
        );
        // A changed head removes the old path; its old immutable artifact still
        // answers only under its own expected identity.
        let renamed = "package/moved.py";
        let next=store.sync_prepare(ws,key,&json!({"repository_id":id,"expected_snapshot":fixture.identity.source_snapshot,"manifest":[{"path":renamed,"sha256":source.sha256,"bytes":source.content.len()}]})).unwrap();
        store
            .sync_commit(
                ws,
                key,
                &json!({"repository_id":id,"sync_id":next["sync_id"]}),
            )
            .unwrap();
        let mut next_identity = fixture.identity.clone();
        next_identity.commit = "f".repeat(40);
        next_identity.source_snapshot = s(&next, "snapshot").into();
        let moved = export(
            &store,
            &session,
            id,
            &next_identity,
            &fixture.engine_sha256,
            fixture.expires_at,
        )
        .unwrap();
        assert_eq!(moved.sources.len(), 1);
        assert_eq!(moved.sources[0].path, renamed);
        assert!(!moved.graph.to_string().contains("billing.py"));
        assert_ne!(bundle.bytes().unwrap(), moved.bytes().unwrap());
        assert!(
            bundle
                .context(
                    &next_identity,
                    &fixture.engine_sha256,
                    "calculate_total",
                    300
                )
                .is_err()
        );
    }
}
