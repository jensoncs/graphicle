package main

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

// A module built with -tags vtprotobuf type-checks only under that tag: without
// it, generated methods are missing and every importer is incomplete.
func TestBuildTagsComeFromTheModulesOwnBuildCommands(t *testing.T) {
	dir := t.TempDir()
	write := func(name, text string) {
		t.Helper()
		if err := os.MkdirAll(filepath.Dir(filepath.Join(dir, name)), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, name), []byte(text), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	if got := moduleBuildTags(dir, nil); len(got) != 0 {
		t.Fatalf("a module with no build files has no tags, got %v", got)
	}
	write("Makefile", "build:\n\tGOOS=linux CGO_ENABLED=0 go build -tags vtprotobuf -o out/x cmd/x.go\n"+
		"test:\n\tgo test -v -tags=integration,vtprotobuf ./...\n")
	write("Dockerfile", "FROM golang\n# RUN go install -tags 'postgres' example.com/migrate@latest\n"+
		"RUN GOFLAGS=\"-tags=netgo\" go build ./...\n")
	write("Jenkinsfile", "// sh 'go test -tags commented ./...'\nsh \"go vet -tags 'lint extra' ./...\"\n")
	write("scripts/ci.sh", "go build -tags nested ./...\n") // below the module directory: not this module's build
	write("README.md", "go build -tags documented ./...\n") // documentation is not a build file
	want := []string{"extra", "integration", "lint", "netgo", "vtprotobuf"}
	if got := moduleBuildTags(dir, nil); !reflect.DeepEqual(got, want) {
		t.Fatalf("tags %v, want %v", got, want)
	}
	if got := moduleBuildTags(dir, []string{"chosen"}); !reflect.DeepEqual(got, []string{"chosen"}) {
		t.Fatalf("an explicit profile wins, got %v", got)
	}

	write("build.sh", "go build -tags 'bad$tag,"+strings.Repeat("x", 101)+",ok' ./...\n")
	got := moduleBuildTags(dir, nil)
	for _, tag := range got {
		if !validBuildTag(tag) {
			t.Fatalf("invalid tag %q kept", tag)
		}
	}
	if !reflect.DeepEqual(got, append([]string{}, "extra", "integration", "lint", "netgo", "ok", "vtprotobuf")) {
		t.Fatalf("tags %v", got)
	}
}

func TestBuildTagsAreBounded(t *testing.T) {
	dir := t.TempDir()
	var b strings.Builder
	for i := 0; i < 100; i++ {
		b.WriteString("go build -tags t")
		b.WriteString(strings.Repeat("a", i%7+1))
		b.WriteString(string(rune('a' + i%26)))
		b.WriteString(" ./...\n")
	}
	if err := os.WriteFile(filepath.Join(dir, "Makefile"), []byte(b.String()), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := moduleBuildTags(dir, nil); len(got) > maxInferredTags {
		t.Fatalf("%d tags inferred, bound is %d", len(got), maxInferredTags)
	}
	big := strings.Repeat("#", maxBuildFileBytes) + "\ngo build -tags late ./...\n"
	if err := os.WriteFile(filepath.Join(dir, "Makefile"), []byte(big), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := moduleBuildTags(dir, nil); len(got) != 0 {
		t.Fatalf("a build file over the size bound is not read, got %v", got)
	}
}
