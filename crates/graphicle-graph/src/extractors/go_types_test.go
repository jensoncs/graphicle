package main

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"go/token"
	"go/types"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestDiscoveryBatchesModuleAndResolvesLinuxHTTP(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("command-count wrapper uses a POSIX shell")
	}
	root := t.TempDir()
	helper := filepath.Join(root, "helper")
	build := exec.Command("go", "build", "-o", helper, "go_types.go", "go_review.go", "go_semantic.go", "go_review_cache.go")
	build.Env = append(os.Environ(), "GOFLAGS=", "GOWORK=off", "GOENV=off", "GOTOOLCHAIN=local", "CGO_ENABLED=0")
	if out, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build: %v: %s", err, out)
	}
	realGo, err := exec.LookPath("go")
	if err != nil {
		t.Fatal(err)
	}
	wrapper := filepath.Join(root, "bin")
	if err := os.Mkdir(wrapper, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(wrapper, "go"), []byte("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GO_TEST_LOG\"\nexec \"$GO_TEST_REAL\" \"$@\"\n"), 0700); err != nil {
		t.Fatal(err)
	}
	module := filepath.Join(root, "module")
	if err := os.Mkdir(module, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(module, "go.mod"), []byte("module example/batch\ngo 1.23\n"), 0600); err != nil {
		t.Fatal(err)
	}
	paths := []string{}
	for i := 0; i < 70; i++ {
		dir := filepath.Join(module, fmt.Sprintf("p%02d", i))
		if err := os.Mkdir(dir, 0700); err != nil {
			t.Fatal(err)
		}
		source := "package p\nfunc F() int { return 1 }\nfunc G() int { return F() }\n"
		if i == 0 {
			source = "package p\nimport \"net/http\"\nfunc F() int { return http.StatusOK }\nfunc G() int { return F() }\n"
		}
		path := filepath.Join(dir, "main.go")
		if err := os.WriteFile(path, []byte(source), 0600); err != nil {
			t.Fatal(err)
		}
		paths = append(paths, path)
	}
	payload, _ := json.Marshal(request{Paths: paths})
	log := filepath.Join(root, "commands")
	run := exec.Command(helper)
	run.Dir = module
	run.Stdin = bytes.NewReader(payload)
	run.Env = append(os.Environ(), "PATH="+wrapper+string(os.PathListSeparator)+os.Getenv("PATH"), "GO_TEST_LOG="+log, "GO_TEST_REAL="+realGo,
		"GOOS=linux", "GOARCH=amd64", "CGO_ENABLED=0", "GOENV=off", "GOFLAGS=", "GOWORK=off", "GOTOOLCHAIN=local", "GOPROXY=off", "GOSUMDB=off", "GOVCS=*:off")
	out, err := run.Output()
	if err != nil {
		t.Fatal(err)
	}
	var sites []site
	if err := json.Unmarshal(out, &sites); err != nil {
		t.Fatal(err)
	}
	resolved := 0
	for _, call := range sites {
		if call.Target == "F" && call.Reason == "" {
			resolved++
		}
	}
	if resolved != len(paths) {
		t.Fatalf("resolved %d/%d: %s", resolved, len(paths), out)
	}
	commands, err := os.ReadFile(log)
	if err != nil {
		t.Fatal(err)
	}
	lists := 0
	for _, line := range strings.Split(string(commands), "\n") {
		if strings.HasPrefix(line, "list ") {
			lists++
		}
	}
	if lists != 1 {
		t.Fatalf("expected one go list, got %d: %s", lists, commands)
	}
}

func TestReviewDiscoverySeparatesExternalFailures(t *testing.T) {
	allowed := func(path string) bool { return path == "/pin" }
	own := &metadata{ImportPath: "example/app", Dir: "/pin", Module: &moduleMetadata{Path: "example/app", Dir: "/pin"}, Incomplete: true,
		DepsErrors: []packageError{{Err: "unavailable", ImportStack: []string{"example/app"}, Pos: "code.go:2:8"}}}
	problem := own.DepsErrors[0]
	external := &metadata{ImportPath: "outside/module", Error: &problem}
	all := map[string]*metadata{"example/app": own, "outside/module": external}
	errors := reviewExternalErrors(all, allowed)
	if reviewDiscoveryIncomplete(own, errors) {
		t.Fatal("external dependency smeared into local discovery")
	}
	if reviewLocalPackage("outside/module", all, allowed) {
		t.Fatal("external module became local")
	}
	all["example/app/missing"] = &metadata{ImportPath: "example/app/missing", Error: &problem}
	if !reviewDiscoveryIncomplete(own, reviewExternalErrors(all, allowed)) {
		t.Fatal("diagnostic also owned by a missing local package was hidden")
	}
	delete(all, "example/app/missing")
	own.Error = &problem
	if !reviewDiscoveryIncomplete(own, errors) {
		t.Fatal("own package error was hidden")
	}
	own.Error = nil
	own.DepsErrors[0].ImportStack = []string{"example/app", "example/app/missing"}
	if !reviewDiscoveryIncomplete(own, errors) {
		t.Fatal("missing local package hidden")
	}
	own.DepsErrors[0].ImportStack = nil
	if !reviewDiscoveryIncomplete(own, errors) {
		t.Fatal("unattributed failure hidden")
	}
	own.DepsErrors = nil
	if !reviewDiscoveryIncomplete(own, errors) {
		t.Fatal("unexplained incomplete metadata hidden")
	}
}

func TestReviewExternalDiagnosticsStillRequireLocalTypeChecking(t *testing.T) {
	root := t.TempDir()
	problem := packageError{Err: "unavailable outside dependency", Pos: "other.go:2:8", ImportStack: []string{"example/app"}}
	for _, source := range []string{"package app\nfunc F() int { return 1 }\n", "package app\nfunc F() int { return \"wrong type\" }\n"} {
		if err := os.WriteFile(filepath.Join(root, "code.go"), []byte(source), 0600); err != nil {
			t.Fatal(err)
		}
		own := &metadata{ImportPath: "example/app", Dir: root, GoFiles: []string{"code.go"}, Module: &moduleMetadata{Path: "example/app", Dir: root}, Incomplete: true, DepsErrors: []packageError{problem}}
		all := map[string]*metadata{"example/app": own, "outside/module": {ImportPath: "outside/module", Error: &problem}}
		collector := &reviewCollector{root: root, allowedRoots: []string{root}, world: &reviewWorld{Root: root}}
		collector.externalErrors = reviewExternalErrors(all, collector.allowed)
		l := &loader{fset: token.NewFileSet(), meta: all, packages: map[string]*types.Package{}, busy: map[string]bool{}, sizes: types.SizesFor("gc", "amd64"), review: collector}
		_, err := l.Import("example/app")
		if (err != nil) != strings.Contains(source, "wrong type") {
			t.Fatalf("type-check validity lost: %s: %v", source, err)
		}
		if err != nil {
			if len(collector.world.Diagnostics) != 1 || collector.world.Diagnostics[0].Cause != "type_error" || collector.world.Diagnostics[0].File != "code.go" || !strings.Contains(collector.world.Diagnostics[0].Message, "wrong type") {
				t.Fatalf("lost actionable compiler diagnostic: %+v", collector.world.Diagnostics)
			}
		}
	}
}

func TestReviewDiagnosticExamplesAreBoundedAndKeepMultipleCauses(t *testing.T) {
	r := &reviewCollector{root: "/private/snapshot", world: &reviewWorld{}}
	for i := 0; i < 100; i++ {
		r.diagnostic(fmt.Sprintf("p%d", i), "discovery_error", "", 0, "/private/snapshot/"+strings.Repeat("λ", 600))
	}
	r.diagnostic("app", "type_error", "app/code.go", 4, "undefined: GeneratedType")
	if r.world.DiagnosticCount != 101 || len(r.world.Diagnostics) != 9 {
		t.Fatalf("invalid bounds: %+v", r.world)
	}
	if strings.Contains(r.world.Diagnostics[0].Message, r.root) || len([]rune(r.world.Diagnostics[0].Message)) > 513 {
		t.Fatal("unbounded or temporary-path diagnostic")
	}
	if r.world.Diagnostics[8].Cause != "type_error" {
		t.Fatal("discovery failures crowded out source errors")
	}
}

func TestPinnedReviewDoesNotConsultBrokenVCSMetadata(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("VCS sentinel uses a POSIX shell")
	}
	root := t.TempDir()
	helper := filepath.Join(root, "helper")
	build := exec.Command("go", "build", "-o", helper, "go_types.go", "go_review.go", "go_semantic.go", "go_review_cache.go")
	build.Env = append(os.Environ(), "GOFLAGS=", "GOWORK=off", "GOENV=off", "GOTOOLCHAIN=local", "CGO_ENABLED=0")
	if out, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build: %v: %s", err, out)
	}
	app := filepath.Join(root, "app")
	for _, directory := range []string{filepath.Join(app, ".git"), filepath.Join(root, "bin")} {
		if err := os.MkdirAll(directory, 0700); err != nil {
			t.Fatal(err)
		}
	}
	for name, content := range map[string]string{"go.mod": "module example/app\ngo 1.23\n", "main.go": "package main\nfunc main() {}\n"} {
		if err := os.WriteFile(filepath.Join(app, name), []byte(content), 0600); err != nil {
			t.Fatal(err)
		}
	}
	gitLog := filepath.Join(root, "git-called")
	script := "#!/bin/sh\nprintf called > \"$VCS_TEST_LOG\"\nexit 128\n"
	if err := os.WriteFile(filepath.Join(root, "bin/git"), []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	payload, err := json.Marshal(map[string]any{"review": reviewRequest{Roots: []reviewRoot{{Directory: "app", Module: "."}}, GOOS: "linux", GOARCH: "amd64"}})
	if err != nil {
		t.Fatal(err)
	}
	run := exec.Command(helper)
	run.Dir = root
	run.Stdin = bytes.NewReader(payload)
	run.Env = append(os.Environ(), "PATH="+filepath.Join(root, "bin")+string(os.PathListSeparator)+os.Getenv("PATH"), "VCS_TEST_LOG="+gitLog,
		"GOENV=off", "GOFLAGS=", "GOWORK=off", "GOTOOLCHAIN=local", "GOPROXY=off", "GOSUMDB=off", "GOVCS=*:off", "CGO_ENABLED=0")
	output, err := run.Output()
	if err != nil {
		t.Fatal(err)
	}
	var envelope struct {
		Facts string `json:"facts"`
	}
	if err = json.Unmarshal(output, &envelope); err != nil {
		t.Fatal(err)
	}
	packed, err := base64.StdEncoding.DecodeString(envelope.Facts)
	if err != nil {
		t.Fatal(err)
	}
	reader, err := gzip.NewReader(bytes.NewReader(packed))
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()
	var result struct {
		Worlds []reviewWorld `json:"worlds"`
	}
	if err = json.NewDecoder(reader).Decode(&result); err != nil {
		t.Fatal(err)
	}
	if len(result.Worlds) != 1 || len(result.Worlds[0].Issues) != 0 || len(result.Worlds[0].Symbols) != 1 {
		t.Fatalf("invalid compiler facts: %+v", result)
	}
	if _, err = os.Stat(gitLog); !os.IsNotExist(err) {
		t.Fatalf("pinned analysis consulted Git: %v", err)
	}
}
