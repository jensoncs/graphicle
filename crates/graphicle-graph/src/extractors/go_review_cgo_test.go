package main

import (
	"os"
	"path/filepath"
	"testing"
)

// The helper runs go list with CGO_ENABLED=0, so cgo files are reported as
// IgnoredGoFiles, never CgoFiles. Those files must still fail closed.
func TestCgoGatedDetectsFilesHiddenByDisabledCgo(t *testing.T) {
	dir := t.TempDir()
	write := func(name, text string) {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(text), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("lib.go", "package p\n\nfunc Exported() {}\n")
	write("other_os.go", "//go:build windows\n\npackage p\n")
	write("ignored.go", "//go:build ignore\n\npackage p\n")
	if cgoGated(dir, []string{"other_os.go", "ignored.go"}, "linux", "amd64", nil) {
		t.Fatal("files excluded by platform or ignore tags are not cgo-dependent")
	}

	write("cgo_caller.go", "package p\n\nimport \"C\"\n\nfunc caller() { Exported() }\n")
	if !cgoGated(dir, []string{"other_os.go", "cgo_caller.go"}, "linux", "amd64", nil) {
		t.Fatal("import \"C\" file hidden by CGO_ENABLED=0 was not detected")
	}

	write("cgo_tag.go", "//go:build cgo\n\npackage p\n")
	if !cgoGated(dir, []string{"cgo_tag.go"}, "linux", "amd64", nil) {
		t.Fatal("file selected only by the cgo build tag was not detected")
	}

	write("feature.go", "//go:build feature\n\npackage p\n\nimport \"C\"\n")
	if cgoGated(dir, []string{"feature.go"}, "linux", "amd64", nil) {
		t.Fatal("cgo file excluded by an unselected profile tag is not in this build")
	}
	if !cgoGated(dir, []string{"feature.go"}, "linux", "amd64", []string{"feature"}) {
		t.Fatal("profile tags must apply when checking cgo dependence")
	}
}
