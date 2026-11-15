package main

import (
	"bytes"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

// A dependency missing from the offline module cache leaves the packages that
// import it incomplete. Calls whose target go/types still identifies exactly
// are kept, marked partial; calls that need the missing types stay unresolved.
func TestIncompletePackagesKeepExactCallTargets(t *testing.T) {
	root := t.TempDir()
	helper := filepath.Join(root, "helper")
	build := exec.Command("go", "build", "-o", helper, "go_types.go", "go_review.go", "go_semantic.go", "go_review_cache.go", "go_review_tags.go")
	build.Env = append(os.Environ(), "GOFLAGS=", "GOWORK=off", "GOENV=off", "GOTOOLCHAIN=local", "CGO_ENABLED=0")
	if out, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build: %v: %s", err, out)
	}
	module := filepath.Join(root, "module")
	files := map[string]string{
		"go.mod": "module example/partial\ngo 1.23\nrequire example.com/missing v1.0.0\n",
		"a/a.go": "package a\nimport \"example.com/missing\"\n" +
			"func Validate(c missing.Config) []string { return check() }\n" +
			"func check() []string { return nil }\n",
		"b/b.go": "package b\nimport (\n\t\"example.com/missing\"\n\t\"example/partial/a\"\n)\n" +
			"func Convert(c missing.Config) int {\n\terrs := a.Validate(c)\n\tc.Apply()\n\treturn len(errs)\n}\n",
	}
	paths := []string{}
	for name, source := range files {
		path := filepath.Join(module, name)
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(source), 0600); err != nil {
			t.Fatal(err)
		}
		if filepath.Ext(name) == ".go" {
			paths = append(paths, path)
		}
	}
	payload, _ := json.Marshal(request{Paths: paths})
	run := exec.Command(helper)
	run.Dir = module
	run.Stdin = bytes.NewReader(payload)
	run.Env = append(os.Environ(), "GOMODCACHE="+filepath.Join(root, "modcache"), "CGO_ENABLED=0", "GOENV=off", "GOFLAGS=",
		"GOWORK=off", "GOTOOLCHAIN=local", "GOPROXY=off", "GOSUMDB=off", "GOVCS=*:off")
	out, err := run.Output()
	if err != nil {
		t.Fatal(err)
	}
	got := map[string]site{}
	for _, call := range groupSites(t, out) {
		got[call.Caller+"->"+call.Target] = call
	}
	for _, want := range []string{"Convert->Validate", "Validate->check"} {
		call, ok := got[want]
		if !ok || call.Reason != "" || !call.Partial || call.TargetFile == "" {
			t.Fatalf("%s not resolved as a partial exact call: %+v in %s", want, call, out)
		}
	}
	// c.Apply() needs missing.Config's method set: no target, a reason.
	if call, ok := got["Convert->"]; !ok || call.Reason == "" {
		t.Fatalf("call on a missing type should stay unresolved: %+v in %s", call, out)
	}
}
