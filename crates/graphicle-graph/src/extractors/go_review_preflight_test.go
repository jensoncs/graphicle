package main

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestReviewPreflightChecksSharedTreeOnceAndStillRefusesEveryWorld(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("command-count wrapper uses a POSIX shell")
	}
	root := t.TempDir()
	helper := filepath.Join(root, "helper")
	build := exec.Command("go", "build", "-mod=vendor", "-o", helper, "go_types.go", "go_review.go", "go_semantic.go", "go_review_cache.go")
	if out, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build: %v: %s", err, out)
	}
	realGo, err := exec.LookPath("go")
	if err != nil {
		t.Fatal(err)
	}
	write := func(name, data string, mode os.FileMode) {
		t.Helper()
		path := filepath.Join(root, name)
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(data), mode); err != nil {
			t.Fatal(err)
		}
	}
	write("bin/go", "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GO_TEST_LOG\"\nexec \"$GO_TEST_REAL\" \"$@\"\n", 0700)
	var roots []reviewRoot
	for i := 0; i < 6; i++ {
		name := fmt.Sprintf("m%d", i)
		write("repo/"+name+"/go.mod", "module example/"+name+"\ngo 1.23\n", 0600)
		write("repo/"+name+"/main.go", "package app\nfunc Entry() {}\n", 0600)
		roots = append(roots, reviewRoot{Directory: "repo", Module: name})
	}
	// Normalized duplicate directory names must share the same safety scan.
	roots[0].Directory = "repo/."
	request := reviewRequest{Roots: roots, GOOS: "linux", GOARCH: "amd64", CacheIdentity: "test"}
	log := filepath.Join(root, "commands")
	run := func(refused bool) {
		t.Helper()
		write("commands", "", 0600)
		payload, _ := json.Marshal(map[string]any{"review": request})
		cmd := exec.Command(helper)
		cmd.Dir = root
		cmd.Stdin = bytes.NewReader(payload)
		cmd.Env = append(os.Environ(), "PATH="+filepath.Join(root, "bin")+string(os.PathListSeparator)+os.Getenv("PATH"), "GO_TEST_LOG="+log, "GO_TEST_REAL="+realGo,
			"GOENV=off", "GOFLAGS=", "GOWORK=off", "GOTOOLCHAIN=local", "GOPROXY=off", "GOSUMDB=off", "CGO_ENABLED=0")
		out, err := cmd.Output()
		if err != nil {
			t.Fatal(err)
		}
		var envelope struct{ Facts string }
		if err := json.Unmarshal(out, &envelope); err != nil {
			t.Fatal(err)
		}
		data, err := base64.StdEncoding.DecodeString(envelope.Facts)
		if err != nil {
			t.Fatal(err)
		}
		reader, err := gzip.NewReader(bytes.NewReader(data))
		if err != nil {
			t.Fatal(err)
		}
		defer reader.Close()
		var facts struct{ Worlds []reviewWorld }
		if err := json.NewDecoder(reader).Decode(&facts); err != nil {
			t.Fatal(err)
		}
		if len(facts.Worlds) != len(roots) {
			t.Fatalf("lost worlds: %+v", facts)
		}
		for _, world := range facts.Worlds {
			if refused {
				if len(world.Issues) != 1 || world.Issues[0] != "unsafe_or_invalid_module_configuration" || len(world.Symbols) != 0 {
					t.Fatalf("unsafe tree analyzed: %+v", world)
				}
			} else if len(world.Issues) != 0 || len(world.Symbols) != 1 {
				t.Fatalf("lost valid compiler facts: %+v", world)
			}
		}
		commands, err := os.ReadFile(log)
		if err != nil {
			t.Fatal(err)
		}
		lists, configs := 0, 0
		for _, line := range strings.Split(string(commands), "\n") {
			if strings.HasPrefix(line, "list ") {
				lists++
			}
			if strings.HasPrefix(line, "mod edit ") {
				configs++
			}
		}
		if refused && lists != 0 {
			t.Fatalf("discovery ran before safety checks: %s", commands)
		}
		if !refused && (lists != len(roots) || configs != len(roots)) {
			t.Fatalf("expected one safety scan and discovery per module, got: %s", commands)
		}
	}
	run(false)
	for _, config := range []struct{ name, content string }{
		{"repo/unselected/go.mod", "module example/unselected\ngo 1.23\nreplace example/escape => ../../../outside\n"},
		{"repo/go.work", "go 1.23\nuse ../../outside\n"},
		{"repo/unselected/go.mod", "not a valid module file\n"},
	} {
		write(config.name, config.content, 0600)
		run(true)
		if err := os.Remove(filepath.Join(root, config.name)); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Symlink(filepath.Join(root, "outside"), filepath.Join(root, "repo/link")); err != nil {
		t.Fatal(err)
	}
	run(true)
}
