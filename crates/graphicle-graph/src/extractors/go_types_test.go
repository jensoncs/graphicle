package main

import (
	"bytes"
	"encoding/json"
	"fmt"
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
	build := exec.Command("go", "build", "-o", helper, "go_types.go", "go_review.go")
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
