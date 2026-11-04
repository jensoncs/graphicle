package main

import (
	"fmt"
	"testing"
)

func TestPackageCausesSurviveMessageSampling(t *testing.T) {
	world := &reviewWorld{}
	collector := &reviewCollector{world: world, root: t.TempDir()}
	for i := 0; i < 40; i++ {
		path := fmt.Sprintf("example/app/p%d", i)
		for j := 0; j < 10; j++ {
			collector.diagnostic(path, "type_error", "", 0, "undefined: Missing")
		}
	}
	collector.diagnostic("example/app/p39", "unsupported_cgo", "", 0, "Selected package requires cgo")
	if world.DiagnosticCount != 401 || len(world.Diagnostics) > 32 {
		t.Fatalf("incorrect diagnostic accounting: %d / %d", world.DiagnosticCount, len(world.Diagnostics))
	}
	if len(world.PackageDiagnostics) != 40 || world.PackageDiagnostics["example/app/p39"]["type_error"] != 10 || world.PackageDiagnostics["example/app/p39"]["unsupported_cgo"] != 1 {
		t.Fatalf("package causes lost to message sampling: %v", world.PackageDiagnostics)
	}
}
