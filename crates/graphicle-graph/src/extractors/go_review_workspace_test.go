package main

import (
	"os"
	"path/filepath"
	"testing"
)

// A profile workspace is named by the request, not found by the go.mod/go.work
// walk. Whatever its name, its use/replace paths must stay inside the pins.
func TestPreflightChecksTheRequestedWorkspaceWhateverItsName(t *testing.T) {
	root := t.TempDir()
	write := func(name, data string) {
		t.Helper()
		path := filepath.Join(root, name)
		if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(data), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("snap/app/go.mod", "module example/app\ngo 1.23\n")
	write("host/go.mod", "module example/host\ngo 1.23\n")
	write("snap/inside.work", "go 1.23\nuse ./app\n")
	write("snap/custom.work", "go 1.23\nuse ./app\nuse ../host\n")
	write("outside.work", "go 1.23\nuse ./snap/app\n")
	collector := &reviewCollector{root: root, allowedRoots: []string{filepath.Join(root, "snap")}}
	env := os.Environ()
	if err := collector.preflight(env, filepath.Join(root, "snap/inside.work")); err != nil {
		t.Fatalf("a contained custom workspace must pass: %v", err)
	}
	if err := collector.preflight(env, filepath.Join(root, "snap/custom.work")); err == nil {
		t.Fatal("custom workspace using a directory outside the pins was accepted")
	}
	if err := collector.preflight(env, filepath.Join(root, "outside.work")); err == nil {
		t.Fatal("workspace file outside the pins was accepted")
	}
	if err := os.Symlink(filepath.Join(root, "outside.work"), filepath.Join(root, "snap/link.work")); err != nil {
		t.Fatal(err)
	}
	if err := collector.preflight(env, filepath.Join(root, "snap/link.work")); err == nil {
		t.Fatal("symlinked workspace file was accepted")
	}
}
