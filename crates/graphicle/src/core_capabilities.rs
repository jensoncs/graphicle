//! Build capabilities are explicit on the wire; unsupported extension calls never
//! masquerade as successful Core results.
use crate::{
    Error, Result,
    extension::Extensions,
    store::{Store, repo},
};
use serde_json::{Value, json};

/// The `build.info` capabilities of this process (see [`capabilities_of`]).
pub fn capabilities() -> Value {
    capabilities_of(crate::extension::installed())
}

/// The edition of this process, as `build.info` reports it: "core", or the
/// edition the installed extensions declare (`Extension::edition`, such as
/// "pro"). Installing an extension does not by itself change it.
/// `graphicle --version` prints it, and the update check only offers
/// installers of this edition.
pub fn edition() -> &'static str {
    crate::extension::installed().edition().name()
}

/// The `build.info` capabilities of a program with `extensions` installed:
/// the declared edition, and the flags each extension sets for what it adds.
/// No capabilities hook can change the edition, so `build.info`,
/// `--version` and the update check always agree.
pub fn capabilities_of(extensions: &Extensions) -> Value {
    let edition = extensions.edition().name();
    let mut capabilities = json!({"edition": edition,
        "code_graph": true, "memory": true, "observer": true,
        "local_inference": cfg!(feature = "local-inference"),
        "change_impact": true,
        "serve_unix_socket": cfg!(unix),
        "review_deadline": if cfg!(unix) { Some("cooperative-timeout-ms-v1") } else { None },
        "secondary_context": "sdk-opt-in-pinned-evidence-v1",
        "statement_flow": "go-intraprocedural-v1",
        "architecture_preview": "isolated-episode-topology-v1",
        "semantic_rename": "local-cli-go-function-bindings-v1",
        "structured_symbol_search": ["python", "javascript", "typescript", "go", "java"],
        "function_value_resolution": "go-potential-targets-v1",
        "execution_flows": "potential-static-cited-paths-v1",
        "configured_service_boundaries": ["http", "grpc", "event"],
        "configured_service_boundaries_authority": "pinned_configuration_assertions",
        "review_compact_provenance": "graphicle.service-provenance.v1",
        "change_impact_scope": "pinned-go-snapshot-sets-v1",
        "review_source_admission": "exhaustive-withheld-paths-go-build-v1",
        "review_source_limits": crate::review::admission::source_limits(),
        "review_analysis_coverage": "graphicle.changed-go-coverage.v1",
        "review_analysis_diagnostics": "bounded-go-errors-and-python-causes-v1",
        "review_issue_histogram": "full-analysis-reason-cause-counts-v1",
        "review_unsupported_inputs": "extension-cause-side-v1",
        "review_empty_reason": "entry-delivery-reasons-v1",
        "review_provided_diff": "fully-covered-spans-v1",
        "review_context_ranking": "additional_context_then_overlap_v1",
        "review_python_import_diagnostics": "cause-and-module-counts-v1",
        "review_excluded_modules": "explicit-go-root-exclusions-v1",
        "sync_analysis_cache": "parse-reuse-and-rebuild-counts-v1",
        "review_go_roots": "admitted-module-discovery-v1",
        "basic_change_impact": {"languages":["terraform","helm","bash","python","rust","typescript","javascript"],"granularity":"file-module-chart","compiler_resolution":false,"declaration_callers":{"languages":["python","rust","typescript","javascript"],"relation":"may_call","analysis_level":"basic_symbol"}},
        "business_context": true,
        "business_context_scope": "static-candidates-and-pinned-history-v1",
        "execution_coverage": true,
        "execution_coverage_scope": "pinned-source-ranges-v1",
        "provider_permission_sync": false});
    if let Some(map) = capabilities.as_object_mut() {
        extensions.capabilities(map);
        map.insert("edition".into(), json!(edition));
    }
    capabilities
}

/// Compatibility helper for an extension that cannot serve an operation.
/// Core's impact and coverage routes have implementations and do not call this.
/// Authorization precedes the unsupported-operation response.
pub fn not_included(
    store: &Store,
    workspace: &str,
    token: &str,
    repository: &str,
) -> Result<Value> {
    store.authorized_read(workspace, token, "read", |db, principal| {
        repo(db, principal, repository)?;
        Err(Error {
            status: axum::http::StatusCode::NOT_IMPLEMENTED,
            message: "This operation is not included in the selected extension".into(),
            provider_failure: None,
        })
    })
}
