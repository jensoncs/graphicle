//! Build, CI, configuration, markup, template, Lua and C sources: a change to
//! a file or declaration they name selects them.
use super::tests::{analysis, input, report, request, with_go};
use super::*;

/// Head entries after changing `path` to `text`: `file` for a file node,
/// `file#name` for a declaration.
fn impact(files: &[(&str, &str)], (path, text): (&str, &str)) -> BTreeSet<String> {
    let head: Vec<_> = files
        .iter()
        .map(|(p, t)| (*p, if *p == path { text } else { *t }))
        .collect();
    let r = report(&input(files), &input(&head));
    r["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["side"] == "head")
        .map(|e| match e["kind"].as_str() {
            Some("basic_file") => s(e, "file").to_owned(),
            _ => format!("{}#{}", s(e, "file"), s(e, "name")),
        })
        .collect()
}

/// Each basic link as `from -> to (evidence)`, a node as `file` or `file#name`.
fn links(files: &[(&str, &str)]) -> BTreeSet<String> {
    let node = |key: &str| {
        let key = key
            .strip_prefix("symbol:")
            .or_else(|| key.strip_prefix("file:"))
            .unwrap_or(key);
        key[5..].to_owned()
    };
    analysis(&input(files))
        .calls
        .iter()
        .map(|(_, c)| {
            format!(
                "{} -> {} ({})",
                node(&c.caller),
                node(&c.target),
                c.evidence
            )
        })
        .collect()
}

fn languages(files: &[(&str, &str)]) -> BTreeMap<String, &'static str> {
    let i = input(files);
    files
        .iter()
        .map(|(p, _)| (p[5..].to_owned(), language(&i, p).unwrap_or("-")))
        .collect()
}

#[test]
fn new_source_kinds_have_a_language() {
    let l = languages(&[
        ("repo/Dockerfile", "FROM alpine\n"),
        ("repo/build/Dockerfile.test", "FROM alpine\n"),
        ("repo/api.Dockerfile", "FROM alpine\n"),
        ("repo/deploy/Dockerfile.dockerignore", "*\n"),
        ("repo/Makefile", "all:\n"),
        ("repo/sub/makefile", "all:\n"),
        ("repo/rules.mk", "x:\n"),
        ("repo/Jenkinsfile", "pipeline {}\n"),
        ("repo/vars/deploy.groovy", "def call() {}\n"),
        ("repo/.github/workflows/ci.yml", "on: push\n"),
        ("repo/.github/actions/deploy/action.yml", "runs: {}\n"),
        ("repo/k8s/deployment.yaml", "kind: Deployment\n"),
        ("repo/config/app.json", "{}\n"),
        ("repo/package.json", "{}\n"),
        ("repo/web/index.html", "<p>\n"),
        ("repo/web/site.scss", "a{}\n"),
        ("repo/tmpl/mail.mustache", "x\n"),
        ("repo/tmpl/page.njk", "x\n"),
        ("repo/tmpl/conf.j2", "x\n"),
        ("repo/tmpl/alert.tmpl", "x\n"),
        ("repo/filters/location.lua", "x = 1\n"),
        ("repo/native/util.c", "int x;\n"),
        ("repo/native/util.h", "int x;\n"),
        ("repo/native/lib.cpp", "int x;\n"),
        ("repo/infra/tests/main.tftest.hcl", "run \"x\" {}\n"),
        ("repo/infra/terragrunt.hcl", "x = 1\n"),
        ("repo/main.go", "package main\n"),
        ("repo/go.mod", "module x\n"),
    ]);
    let want = [
        ("Dockerfile", "dockerfile"),
        ("build/Dockerfile.test", "dockerfile"),
        ("api.Dockerfile", "dockerfile"),
        ("deploy/Dockerfile.dockerignore", "-"),
        ("Makefile", "make"),
        ("sub/makefile", "make"),
        ("rules.mk", "make"),
        ("Jenkinsfile", "groovy"),
        ("vars/deploy.groovy", "groovy"),
        (".github/workflows/ci.yml", "github_actions"),
        (".github/actions/deploy/action.yml", "github_actions"),
        ("k8s/deployment.yaml", "yaml"),
        ("config/app.json", "json"),
        ("package.json", "javascript"),
        ("web/index.html", "html"),
        ("web/site.scss", "css"),
        ("tmpl/mail.mustache", "template"),
        ("tmpl/page.njk", "template"),
        ("tmpl/conf.j2", "template"),
        ("tmpl/alert.tmpl", "template"),
        ("filters/location.lua", "lua"),
        ("native/util.c", "c"),
        ("native/util.h", "c"),
        ("native/lib.cpp", "cpp"),
        ("infra/tests/main.tftest.hcl", "terraform"),
        ("infra/terragrunt.hcl", "hcl"),
        ("main.go", "-"),
        ("go.mod", "-"),
    ];
    for (file, language) in want {
        assert_eq!(l[file], language, "{file}");
    }
}

#[test]
fn a_dockerfile_stage_is_selected_by_what_it_copies_and_the_stages_it_copies_from() {
    let files = [
        (
            "repo/Dockerfile",
            "FROM golang AS build\nCOPY scripts/build.sh /b.sh\nRUN /b.sh\n\nFROM alpine AS final\nCOPY --from=build /out /app\nCOPY config/app.yaml /etc/app.yaml\n",
        ),
        ("repo/scripts/build.sh", "#!/bin/sh\necho 1\n"),
        ("repo/config/app.yaml", "port: 1\n"),
    ];
    let script = impact(&files, ("repo/scripts/build.sh", "#!/bin/sh\necho 2\n"));
    assert!(script.contains("Dockerfile#build"), "{script:?}");
    assert!(script.contains("Dockerfile#final"), "{script:?}");
    // A YAML change keeps repository-wide scope; the link is still exact.
    let config = impact(&files, ("repo/config/app.yaml", "port: 2\n"));
    assert!(config.contains("Dockerfile#final"), "{config:?}");
    let l = links(&files);
    assert!(
        l.contains("Dockerfile#final -> config/app.yaml (docker_copy_source)"),
        "{l:?}"
    );
    assert!(
        !l.iter()
            .any(|x| x.starts_with("Dockerfile#build -> config/app.yaml")),
        "{l:?}"
    );
}

#[test]
fn a_make_target_is_selected_by_its_recipe_scripts_and_prerequisites() {
    let files = [
        (
            "repo/Makefile",
            "include common.mk\nTAG ?= latest\n\nbuild: gen\n\tgo build ./...\n\ngen:\n\t./hack/gen.sh\n\nlint:\n\tgolangci-lint run\n\n.PHONY: build gen lint\n",
        ),
        ("repo/common.mk", "X := 1\n"),
        ("repo/hack/gen.sh", "#!/bin/sh\necho 1\n"),
    ];
    let script = impact(&files, ("repo/hack/gen.sh", "#!/bin/sh\necho 2\n"));
    assert!(script.contains("Makefile#gen"), "{script:?}");
    assert!(script.contains("Makefile#build"), "{script:?}");
    assert!(!script.contains("Makefile#lint"), "{script:?}");
    let common = impact(&files, ("repo/common.mk", "X := 2\n"));
    assert!(common.contains("Makefile"), "{common:?}");
}

#[test]
fn a_jenkinsfile_is_selected_by_the_scripts_and_libraries_it_names() {
    let files = [
        (
            "repo/Jenkinsfile",
            "pipeline {\n  stages {\n    stage('build') {\n      steps {\n        sh './ci/build.sh'\n        script { def lib = load 'ci/lib.groovy' }\n      }\n    }\n  }\n}\n",
        ),
        ("repo/ci/build.sh", "#!/bin/sh\necho 1\n"),
        ("repo/ci/lib.groovy", "def x() { 1 }\n"),
    ];
    let script = impact(&files, ("repo/ci/build.sh", "#!/bin/sh\necho 2\n"));
    assert!(script.contains("Jenkinsfile"), "{script:?}");
    let lib = impact(&files, ("repo/ci/lib.groovy", "def x() { 2 }\n"));
    assert!(lib.contains("Jenkinsfile"), "{lib:?}");
}

#[test]
fn a_workflow_job_is_selected_by_its_scripts_local_actions_and_needed_jobs() {
    let files = [
        (
            "repo/.github/workflows/ci.yml",
            "name: ci\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - run: ./ci/build.sh\n  deploy:\n    needs: build\n    runs-on: ubuntu-latest\n    steps:\n      - uses: ./.github/actions/deploy\n  lint:\n    runs-on: ubuntu-latest\n    steps:\n      - run: make lint\n",
        ),
        (
            "repo/.github/actions/deploy/action.yml",
            "name: deploy\nruns:\n  using: composite\n  steps:\n    - run: echo 1\n      shell: bash\n",
        ),
        ("repo/ci/build.sh", "#!/bin/sh\necho 1\n"),
    ];
    let script = impact(&files, ("repo/ci/build.sh", "#!/bin/sh\necho 2\n"));
    assert!(
        script.contains(".github/workflows/ci.yml#build"),
        "{script:?}"
    );
    assert!(
        script.contains(".github/workflows/ci.yml#deploy"),
        "{script:?}"
    );
    assert!(
        !script.contains(".github/workflows/ci.yml#lint"),
        "{script:?}"
    );
    let action = impact(
        &files,
        (
            "repo/.github/actions/deploy/action.yml",
            "name: deploy\nruns:\n  using: composite\n  steps:\n    - run: echo 2\n      shell: bash\n",
        ),
    );
    assert!(
        action.contains(".github/workflows/ci.yml#deploy"),
        "{action:?}"
    );
    assert!(
        !action.contains(".github/workflows/ci.yml#build"),
        "{action:?}"
    );
}

#[test]
fn kubernetes_objects_and_kustomizations_are_selected_by_what_they_name() {
    let files = [
        (
            "repo/base/kustomization.yaml",
            "resources:\n  - deployment.yaml\n  - configmap.yaml\n",
        ),
        (
            "repo/overlays/prod/kustomization.yaml",
            "bases:\n  - ../../base\npatchesStrategicMerge:\n  - patch.yaml\n",
        ),
        (
            "repo/overlays/prod/patch.yaml",
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: api\nspec:\n  replicas: 3\n",
        ),
        (
            "repo/base/deployment.yaml",
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: api\nspec:\n  template:\n    spec:\n      containers:\n        - name: api\n          envFrom:\n            - configMapRef:\n                name: api-config\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: api\nspec:\n  ports:\n    - port: 80\n",
        ),
        (
            "repo/base/configmap.yaml",
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: api-config\ndata:\n  LEVEL: info\n",
        ),
        ("repo/config/app.yaml", "level: info\n"),
    ];
    let l = links(&files);
    for link in [
        "base/deployment.yaml#Deployment/api -> base/configmap.yaml#ConfigMap/api-config (kubernetes_object_name)",
        "base/kustomization.yaml -> base/deployment.yaml (kustomize_path)",
        "base/kustomization.yaml -> base/configmap.yaml (kustomize_path)",
        "overlays/prod/kustomization.yaml -> base/kustomization.yaml (kustomize_path)",
        "overlays/prod/kustomization.yaml -> overlays/prod/patch.yaml (kustomize_path)",
    ] {
        assert!(l.contains(link), "{link} not in {l:?}");
    }
    assert!(
        !l.iter()
            .any(|x| x.starts_with("base/deployment.yaml#Service/api")),
        "{l:?}"
    );
    // Code reads configuration in ways basic review cannot see: a YAML
    // change keeps repository-wide scope.
    let config = impact(
        &files,
        (
            "repo/base/configmap.yaml",
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: api-config\ndata:\n  LEVEL: debug\n",
        ),
    );
    assert!(config.contains("config/app.yaml"), "{config:?}");
    let a = analysis(&input(&files));
    assert!(
        !a.issues
            .iter()
            .any(|i| i["reason"] == "basic_unresolved_dependency"),
        "{:?}",
        a.issues
    );
    let missing = analysis(&input(&[(
        "repo/k/kustomization.yaml",
        "resources:\n  - gone.yaml\n  - github.com/org/repo//base?ref=v1\n",
    )]));
    let unresolved: Vec<_> = missing
        .issues
        .iter()
        .filter(|i| i["reason"] == "basic_unresolved_dependency")
        .collect();
    assert_eq!(unresolved.len(), 1, "{unresolved:?}");
}

#[test]
fn json_schemas_html_and_css_are_selected_by_the_files_they_reference() {
    let files = [
        (
            "repo/schema/order.json",
            "{\"properties\":{\"total\":{\"$ref\":\"./money.json#/definitions/m\"}}}\n",
        ),
        ("repo/schema/money.json", "{\"definitions\":{\"m\":{}}}\n"),
        (
            "repo/web/index.html",
            "<html><head><link rel=\"stylesheet\" href=\"css/site.css\"><script src=\"js/app.js\"></script></head>\n<a href=\"https://example.com/x.css\">x</a></html>\n",
        ),
        (
            "repo/web/css/site.css",
            "@import \"base.css\";\nbody { background: url(../img/bg.png); }\n",
        ),
        ("repo/web/css/base.css", "a { color: red; }\n"),
        ("repo/web/js/app.js", "export const a = 1;\n"),
        ("repo/web/other.html", "<p>nothing</p>\n"),
    ];
    let money = impact(
        &files,
        (
            "repo/schema/money.json",
            "{\"definitions\":{\"m\":{\"type\":\"string\"}}}\n",
        ),
    );
    assert!(money.contains("schema/order.json"), "{money:?}");
    let base = impact(&files, ("repo/web/css/base.css", "a { color: blue; }\n"));
    assert!(base.contains("web/css/site.css"), "{base:?}");
    assert!(base.contains("web/index.html"), "{base:?}");
    assert!(!base.contains("web/other.html"), "{base:?}");
    let script = impact(&files, ("repo/web/js/app.js", "export const a = 2;\n"));
    assert!(script.contains("web/index.html"), "{script:?}");
}

#[test]
fn templates_are_selected_by_their_partials_includes_and_called_definitions() {
    let files = [
        ("repo/mail/welcome.mustache", "{{> header}}\nHi {{name}}\n"),
        ("repo/mail/header.mustache", "<h1>Hello</h1>\n"),
        (
            "repo/templates/page.njk",
            "{% extends \"layout.njk\" %}\n{% include \"parts/nav.njk\" %}\n",
        ),
        (
            "repo/templates/layout.njk",
            "<body>{% block c %}{% endblock %}</body>\n",
        ),
        ("repo/templates/parts/nav.njk", "<nav></nav>\n"),
        (
            "repo/alerts/alert.tmpl",
            "{{ define \"a\" }}{{ if .X }}{{ template \"b\" . }}{{ end }}{{ end }}\n{{ define \"b\" }}x{{ end }}\n{{ define \"c\" }}y{{ end }}\n",
        ),
    ];
    let header = impact(&files, ("repo/mail/header.mustache", "<h1>Hey</h1>\n"));
    assert!(header.contains("mail/welcome.mustache"), "{header:?}");
    let nav = impact(&files, ("repo/templates/parts/nav.njk", "<nav>x</nav>\n"));
    assert!(nav.contains("templates/page.njk"), "{nav:?}");
    let layout = impact(
        &files,
        (
            "repo/templates/layout.njk",
            "<main>{% block c %}{% endblock %}</main>\n",
        ),
    );
    assert!(layout.contains("templates/page.njk"), "{layout:?}");
    let b = impact(
        &files,
        (
            "repo/alerts/alert.tmpl",
            "{{ define \"a\" }}{{ if .X }}{{ template \"b\" . }}{{ end }}{{ end }}\n{{ define \"b\" }}z{{ end }}\n{{ define \"c\" }}y{{ end }}\n",
        ),
    );
    assert!(b.contains("alerts/alert.tmpl#a"), "{b:?}");
    assert!(!b.contains("alerts/alert.tmpl#c"), "{b:?}");
}

#[test]
fn lua_functions_c_includes_and_terraform_tests_select_their_users() {
    let lua = "local function helper(x)\n  if x then\n    return \"end\" -- end\n  end\n  return x\nend\n\nfunction envoy_on_request(handle)\n  helper(1)\nend\n\nfunction envoy_on_response(handle)\nend\n";
    let files = [
        ("repo/filters/location.lua", lua),
        (
            "repo/filters/main.lua",
            "local loc = require(\"location\")\n",
        ),
        (
            "repo/native/main.c",
            "#include <stdio.h>\n#include \"util.h\"\nint main() { return f(); }\n",
        ),
        ("repo/native/util.h", "int f(void);\n"),
        ("repo/infra/main.tf", "variable \"x\" {}\n"),
        (
            "repo/infra/tests/main.tftest.hcl",
            "run \"plan\" {\n  command = plan\n}\n",
        ),
    ];
    let helper = impact(
        &files,
        (
            "repo/filters/location.lua",
            &lua.replace("return x\n", "return x + 1\n"),
        ),
    );
    assert!(
        helper.contains("filters/location.lua#envoy_on_request"),
        "{helper:?}"
    );
    assert!(
        !helper.contains("filters/location.lua#envoy_on_response"),
        "{helper:?}"
    );
    assert!(helper.contains("filters/main.lua"), "{helper:?}");
    let header = impact(&files, ("repo/native/util.h", "int f(int);\n"));
    assert!(header.contains("native/main.c"), "{header:?}");
    let module = impact(&files, ("repo/infra/main.tf", "variable \"y\" {}\n"));
    assert!(module.contains("infra/tests/main.tftest.hcl"), "{module:?}");
}

#[test]
fn embedded_and_chart_files_reach_the_code_and_templates_that_load_them() {
    const GO: &str = "package filter\n\nimport _ \"embed\"\n\n//go:embed location.lua\nvar Script string\n\nvar Other = 1\n";
    let files = |body: &str| {
        input(&[
            ("repo/filter/filter.go", GO),
            ("repo/filter/location.lua", body),
            ("repo/chart/Chart.yaml", "name: c\n"),
            (
                "repo/chart/templates/cm.yaml",
                "data:\n  f.lua: {{ .Files.Get \"files/location.lua\" | quote }}\n",
            ),
            ("repo/chart/files/location.lua", body),
            ("repo/chart/files/other.lua", "y = 1\n"),
        ])
    };
    let go = [
        (
            "repo/filter/filter.go",
            "example.com/app/filter.Script",
            "variable",
            "var Script string",
        ),
        (
            "repo/filter/filter.go",
            "example.com/app/filter.Other",
            "variable",
            "var Other = 1",
        ),
    ];
    let a = with_go(&files("x = 1\n"), &go, &[]);
    let embed: Vec<_> = a
        .services
        .edges
        .iter()
        .filter(|e| e.evidence["evidence"] == "go_embed_pattern")
        .map(|e| (e.from.as_str(), e.to.as_str(), e.relation))
        .collect();
    let script = a
        .symbols
        .iter()
        .find(|(_, (_, s))| s.name == "example.com/app/filter.Script")
        .unwrap()
        .0;
    let lua = key("basic:repo", "file:repo/filter/location.lua");
    assert_eq!(embed, [(script.as_str(), lua.as_str(), "potential_import")]);
    let head = files("x = 2\n");
    let r = compare(
        &files("x = 1\n"),
        &head,
        &a,
        &with_go(&head, &go, &[]),
        &request(),
    )
    .unwrap();
    let names: Vec<_> = r["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["side"] == "head")
        .map(|e| s(e, "name"))
        .collect();
    assert!(
        names.contains(&"example.com/app/filter.Script"),
        "{names:?}"
    );
    assert!(
        !names.contains(&"example.com/app/filter.Other"),
        "{names:?}"
    );
    assert!(
        a.calls
            .iter()
            .any(|(_, c)| c.file == "repo/chart/templates/cm.yaml"
                && c.target_file == "repo/chart/files/location.lua"
                && c.evidence == "helm_files_path")
    );
    assert!(
        !a.calls
            .iter()
            .any(|(_, c)| c.file == "repo/chart/templates/cm.yaml"
                && c.target_file == "repo/chart/files/other.lua"
                && c.evidence == "helm_files_path")
    );
}

#[test]
fn build_files_are_known_by_their_hyphenated_and_suffixed_names() {
    let l = languages(&[
        ("repo/Dockerfile-dev", "FROM alpine\n"),
        ("repo/envoy-Dockerfile", "FROM alpine\n"),
        ("repo/Dockerfile_ci", "FROM alpine\n"),
        ("repo/Makefile.Common", "all:\n"),
        ("repo/Makefile-docker", "all:\n"),
        ("repo/Jenkinsfile-sc", "pipeline {}\n"),
        ("repo/JenkinsFile-prod", "pipeline {}\n"),
        ("repo/JenkinsfileNightlyRun", "pipeline {}\n"),
        // A source, data or document file named after a build file keeps the
        // language of its extension.
        ("repo/tools/dockerfile_test.go", "package tools\n"),
        ("repo/tools/dockerfile.py", "x = 1\n"),
        ("repo/Makefile.toml", "[tasks]\n"),
        ("repo/docs/Jenkinsfile.md", "# CI\n"),
    ]);
    let want = [
        ("Dockerfile-dev", "dockerfile"),
        ("envoy-Dockerfile", "dockerfile"),
        ("Dockerfile_ci", "dockerfile"),
        ("Makefile.Common", "make"),
        ("Makefile-docker", "make"),
        ("Jenkinsfile-sc", "groovy"),
        ("JenkinsFile-prod", "groovy"),
        ("JenkinsfileNightlyRun", "groovy"),
        ("tools/dockerfile_test.go", "-"),
        ("tools/dockerfile.py", "python"),
        ("Makefile.toml", "-"),
        ("docs/Jenkinsfile.md", "-"),
    ];
    for (file, language) in want {
        assert_eq!(l[file], language, "{file}");
    }
}

#[test]
fn only_the_files_a_chart_renders_or_configures_are_helm() {
    let l = languages(&[
        ("repo/chart/Chart.yaml", "name: c\n"),
        ("repo/chart/Chart.lock", "x\n"),
        ("repo/chart/values.yaml", "a: 1\n"),
        ("repo/chart/values-prod.yaml", "a: 2\n"),
        ("repo/chart/values.schema.json", "{}\n"),
        ("repo/chart/templates/cm.yaml", "kind: ConfigMap\n"),
        (
            "repo/chart/templates/_helpers.tpl",
            "{{ define \"x\" }}{{ end }}\n",
        ),
        ("repo/chart/templates/NOTES.txt", "hi\n"),
        ("repo/chart/charts/sub/Chart.yaml", "name: s\n"),
        (
            "repo/chart/charts/sub/templates/svc.yaml",
            "kind: Service\n",
        ),
        // Loaded by .Files.Get or installed as is: their own language.
        ("repo/chart/files/location.lua", "function f() end\n"),
        ("repo/chart/scripts/run.sh", "#!/bin/bash\necho\n"),
        (
            "repo/chart/crds/crd.yaml",
            "kind: CustomResourceDefinition\n",
        ),
        // A templates/ folder with no chart above it is not a chart.
        ("repo/infra/templates/vm.tpl", "name = ${x}\n"),
        ("repo/deploy/templates/svc.yaml", "kind: Service\n"),
    ]);
    let want = [
        ("chart/Chart.yaml", "helm"),
        ("chart/Chart.lock", "helm"),
        ("chart/values.yaml", "helm"),
        ("chart/values-prod.yaml", "helm"),
        ("chart/values.schema.json", "helm"),
        ("chart/templates/cm.yaml", "helm"),
        ("chart/templates/_helpers.tpl", "helm"),
        ("chart/templates/NOTES.txt", "helm"),
        ("chart/charts/sub/Chart.yaml", "helm"),
        ("chart/charts/sub/templates/svc.yaml", "helm"),
        ("chart/files/location.lua", "lua"),
        ("chart/scripts/run.sh", "bash"),
        ("chart/crds/crd.yaml", "yaml"),
        ("infra/templates/vm.tpl", "template"),
        ("deploy/templates/svc.yaml", "yaml"),
    ];
    for (file, language) in want {
        assert_eq!(l[file], language, "{file}");
    }
}

#[test]
fn html_pages_written_as_templates_are_selected_by_what_they_call_and_extend() {
    let files = [
        (
            "repo/invoice/fonts.tmpl",
            "{{ define \"fonts\" }}<link href=\"f.css\">{{ end }}\n",
        ),
        (
            "repo/invoice/invoice.html",
            "<html><head>{{ template \"fonts\" . }}</head><body></body></html>\n",
        ),
        (
            "repo/site/base.html",
            "<html>{% block content %}{% endblock %}</html>\n",
        ),
        (
            "repo/site/page.html",
            "{% extends \"base.html\" %}\n{% block content %}x{% endblock %}\n",
        ),
    ];
    let fonts = impact(
        &files,
        (
            "repo/invoice/fonts.tmpl",
            "{{ define \"fonts\" }}<link href=\"g.css\">{{ end }}\n",
        ),
    );
    assert!(fonts.contains("invoice/invoice.html"), "{fonts:?}");
    let base = impact(
        &files,
        (
            "repo/site/base.html",
            "<main>{% block content %}{% endblock %}</main>\n",
        ),
    );
    assert!(base.contains("site/page.html"), "{base:?}");
}

#[test]
fn stylesheets_import_through_the_load_paths_their_build_states() {
    let angular = r#"{"projects":{"app":{"root":"","architect":{"build":{"options":{"stylePreprocessorOptions":{"includePaths":["src/styles"]}}}}}}}"#;
    let nx_project = r#"{"targets":{"build":{"options":{"stylePreprocessorOptions":{"includePaths":["libs/ui/styles"]}}}}}"#;
    let files = [
        ("repo/web/angular.json", angular),
        ("repo/web/src/styles/theme.less", "@c: red;\n"),
        ("repo/web/src/styles/_vars.scss", "$x: 1;\n"),
        (
            "repo/web/src/app/a.component.less",
            "@import \"theme\";\n.a { color: @c; }\n",
        ),
        ("repo/web/src/app/b.component.scss", "@use \"vars\";\n"),
        // Less has no partials: `_theme.less` is not what `theme` names.
        ("repo/web/src/app/_theme.less", "@d: 1;\n"),
        ("repo/other/c.less", "@import \"theme\";\n"),
        ("repo/nx/nx.json", "{}\n"),
        ("repo/nx/apps/shop/project.json", nx_project),
        ("repo/nx/libs/ui/styles/_mixins.scss", "@mixin m {}\n"),
        ("repo/nx/apps/shop/src/app.scss", "@import \"mixins\";\n"),
    ];
    let l = links(&files);
    for want in [
        "web/src/app/a.component.less -> web/src/styles/theme.less (css_import)",
        "web/src/app/b.component.scss -> web/src/styles/_vars.scss (css_import)",
        "nx/apps/shop/src/app.scss -> nx/libs/ui/styles/_mixins.scss (css_import)",
    ] {
        assert!(l.contains(want), "{want}: {l:?}");
    }
    assert!(!l.iter().any(|x| x.contains("_theme.less")), "{l:?}");
    assert!(
        !l.iter().any(|x| x.starts_with("other/c.less")),
        "outside the workspace: {l:?}"
    );
}

#[test]
fn scripts_run_by_path_or_from_their_own_directory_select_their_callers() {
    let run = "#!/bin/bash\nDIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\nHERE=$(dirname \"${BASH_SOURCE[0]}\")\nUP=\"$(cd \"$(dirname \"$0\")/..\" && pwd)\"\nTOP=$(git rev-parse --show-toplevel)\n\"$DIR\"/install.sh\n${HERE}/lib/test.sh --fast\nOUT=$(./build.sh)\nbash -x check.sh\n\"$UP\"/tools/gen.sh\n\"$TOP\"/ci/top.sh\ndeploy.sh\n/usr/bin/env true\nbash -c \"./never.sh\"\n";
    let mut files = vec![("repo/ci/run.sh", run)];
    for script in [
        "install", "lib/test", "build", "check", "top", "deploy", "never", "moved",
    ] {
        files.push((
            Box::leak(format!("repo/ci/{script}.sh").into_boxed_str()),
            "echo ok\n",
        ));
    }
    files.push(("repo/tools/gen.sh", "echo ok\n"));
    files.push((
        "repo/ci/again.sh",
        "D=$(dirname \"$0\")\nD=/opt/elsewhere\n\"$D\"/moved.sh\n",
    ));
    let got = links(&files);
    let from_run: BTreeSet<_> = got
        .iter()
        .filter(|l| l.starts_with("ci/run.sh -> "))
        .cloned()
        .collect();
    let expect: BTreeSet<_> = [
        "ci/install.sh",
        "ci/lib/test.sh",
        "ci/build.sh",
        "ci/check.sh",
        "tools/gen.sh",
    ]
    .iter()
    .map(|t| format!("ci/run.sh -> {t} (literal_shell_path_candidate)"))
    .collect();
    assert_eq!(
        from_run, expect,
        "$TOP is not the script's directory, deploy.sh is found through PATH, never.sh is a string"
    );
    assert!(
        !got.iter().any(|l| l.starts_with("ci/again.sh")),
        "a variable assigned anything else stays dynamic: {got:?}"
    );
}
