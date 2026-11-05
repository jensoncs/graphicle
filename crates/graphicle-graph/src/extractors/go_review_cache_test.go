package main

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"go/types"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestPackageExportExpansionBound(t *testing.T) {
	var compressed bytes.Buffer
	writer := gzip.NewWriter(&compressed)
	if _, err := writer.Write(bytes.Repeat([]byte{0}, packageCacheRawBytes+1)); err != nil {
		t.Fatal(err)
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := unpackPackage(compressed.Bytes()); err == nil {
		t.Fatal("oversized export expansion accepted")
	}
}

func TestDependencyExportsPreserveFactsAndInvalidate(t *testing.T) {
	root := t.TempDir()
	helper := filepath.Join(root, "helper")
	build := exec.Command("go", "build", "-mod=vendor", "-o", helper, "go_types.go", "go_review.go", "go_semantic.go", "go_review_cache.go")
	if out, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build: %v: %s", err, out)
	}
	write := func(name, data string) {
		t.Helper()
		path := filepath.Join(root, "app", name)
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(data), 0600); err != nil {
			t.Fatal(err)
		}
	}
	write("go.mod", "module example/app\ngo 1.23\nrequire example/dep v1.0.0\n")
	write("vendor/modules.txt", "# example/dep v1.0.0\n## explicit; go 1.23\nexample/dep\nexample/dep/left\nexample/dep/right\n")
	dep := "package dep\ntype T struct{}\nfunc (T) Run() int {return 1}\ntype Alias = T\ntype Box[T any] struct {Value T}\nfunc F[T any](v T) T {return v}\n"
	write("vendor/example/dep/dep.go", dep)
	write("vendor/example/dep/left/left.go", "package left\nimport \"example/dep\"\nfunc F() dep.T{return dep.T{}}\n")
	write("vendor/example/dep/right/right.go", "package right\nimport \"example/dep\"\nfunc F(v dep.T) dep.T{return v}\n")
	owned := "package app\nimport (\"example/dep\";\"example/dep/left\";\"example/dep/right\")\ntype Local struct{dep.T}\nfunc Entry(i interface{Run() int}) int {v:=right.F(left.F()); _=dep.Box[dep.Alias]{Value:v}; f:=dep.F[int]; _=f(1); g:=v.Run; return g()+i.Run()}\n"
	write("main.go", owned)
	request := reviewRequest{Roots: []reviewRoot{{Directory: "app", Module: "."}}, GOOS: "linux", GOARCH: "amd64", CacheIdentity: "test-v1", TraceKey: strings.Repeat("a", 64)}
	timings := false
	run := func(entries map[string]string) (json.RawMessage, reviewCacheResult) {
		t.Helper()
		request.PackageCache = entries
		payload, _ := json.Marshal(map[string]any{"review": request})
		cmd := exec.Command(helper)
		cmd.Dir = root
		cmd.Stdin = bytes.NewReader(payload)
		cmd.Env = append(os.Environ(), "GOENV=off", "GOFLAGS=", "GOWORK=off", "GOTOOLCHAIN=local", "GOPROXY=off", "GOSUMDB=off", "CGO_ENABLED=0")

		timingValue := "0"
		if timings {
			timingValue = "1"
		}
		cmd.Env = append(cmd.Env, "GRAPHICLE_PHASE_TIMINGS="+timingValue)
		var stderr bytes.Buffer
		cmd.Stderr = &stderr
		out, err := cmd.Output()
		if timings != strings.Contains(stderr.String(), "go_phase=type_check ns=") {
			t.Fatalf("timing opt-in mismatch: %s", stderr.String())
		}
		if err != nil {
			if e, ok := err.(*exec.ExitError); ok {
				t.Fatalf("run: %v %s", err, e.Stderr)
			}
			t.Fatal(err)
		}
		var envelope struct {
			Facts string
			Cache reviewCacheResult `json:"dependency_cache"`
		}
		if err := json.Unmarshal(out, &envelope); err != nil {
			t.Fatal(err)
		}
		traces := 0
		for _, line := range strings.Split(stderr.String(), "\n") {
			if !strings.HasPrefix(line, "go_cache=") {
				continue
			}
			traces++
			var trace struct {
				Key, Identity string
				Configuration string `json:"configuration_sha256"`
				Counts        map[string]int
			}
			if err := json.Unmarshal([]byte(strings.TrimPrefix(line, "go_cache=")), &trace); err != nil {
				t.Fatal(err)
			}
			if trace.Key != request.TraceKey || !validPackageCacheKey(trace.Identity) || !validPackageCacheKey(trace.Configuration) {
				t.Fatalf("uncorrelated cache trace: %s", line)
			}
			counts := trace.Counts
			if counts["hit_standard_library"]+counts["hit_dependency"] != envelope.Cache.Hits ||
				counts["miss_not_present"]+counts["miss_import_failed"]+counts["miss_invalid_export"] != envelope.Cache.Misses ||
				counts["transport_entries"] != len(envelope.Cache.Entries) {
				t.Fatalf("cache counters disagree with result: %s", line)
			}
		}
		if (timings && traces != 1) || (!timings && traces != 0) {
			t.Fatalf("cache diagnostics opt-in mismatch: %s", stderr.String())
		}
		decode := func(encoded string, target any) {
			t.Helper()
			packed, err := base64.StdEncoding.DecodeString(encoded)
			if err != nil {
				t.Fatal(err)
			}
			r, err := gzip.NewReader(bytes.NewReader(packed))
			if err != nil {
				t.Fatal(err)
			}
			defer r.Close()
			if err := json.NewDecoder(r).Decode(target); err != nil {
				t.Fatal(err)
			}
		}
		var facts json.RawMessage
		var cache reviewCacheResult
		decode(envelope.Facts, &facts)
		cache = envelope.Cache
		return facts, cache
	}
	cold, created := run(nil)
	if len(created.Entries) != 3 || created.Misses != 3 {
		t.Fatalf("cold stats: %+v", created)
	}
	assertSame := func(a, b json.RawMessage) {
		t.Helper()
		if !bytes.Equal(a, b) {
			t.Fatalf("facts differ\nfresh: %s\ncached: %s", a, b)
		}
	}
	timings = true
	instrumented, _ := run(nil)
	assertSame(cold, instrumented)
	warm, hits := run(created.Entries)
	assertSame(cold, warm)
	if hits.Hits != 3 || hits.Misses != 0 {
		t.Fatalf("warm stats: %+v", hits)
	}
	write("main.go", strings.Replace(owned, "return g()+i.Run()", "return 2+g()+i.Run()", 1))
	edited, editHits := run(created.Entries)
	fresh, _ := run(nil)
	assertSame(fresh, edited)
	if editHits.Hits != 3 {
		t.Fatalf("owned edit lost reuse: %+v", editHits)
	}
	for _, change := range []struct {
		name    string
		apply   func()
		restore func()
	}{
		{"transitive source", func() { write("vendor/example/dep/dep.go", dep+"const Added=2\n") }, func() { write("vendor/example/dep/dep.go", dep) }},
		{"profile", func() { request.Tags = []string{"extra"} }, func() { request.Tags = nil }},
		{"namespace", func() { request.CacheIdentity = "test-v2" }, func() { request.CacheIdentity = "test-v1" }},
		{"module config", func() { write("go.sum", "\n") }, func() { os.Remove(filepath.Join(root, "app/go.sum")) }},
		{"native input", func() { write("vendor/example/dep/dep.s", "// input\n") }, func() { os.Remove(filepath.Join(root, "app/vendor/example/dep/dep.s")) }},
		{"missing source", func() { os.Remove(filepath.Join(root, "app/vendor/example/dep/dep.go")) }, func() { write("vendor/example/dep/dep.go", dep) }},
		{"broken source", func() { write("vendor/example/dep/dep.go", dep+"var Broken = absent\n") }, func() { write("vendor/example/dep/dep.go", dep) }},
	} {
		t.Run(change.name, func(t *testing.T) {
			change.apply()
			defer change.restore()
			cached, stats := run(created.Entries)
			fresh, _ := run(nil)
			assertSame(fresh, cached)
			if stats.Hits != 0 {
				t.Fatalf("stale cache used: %+v", stats)
			}
		})
	}
	damaged := map[string]string{}
	for key := range created.Entries {
		damaged[key] = "bm90IGV4cG9ydCBkYXRh"
	}
	repaired, stats := run(damaged)
	fresh, _ = run(nil)
	assertSame(fresh, repaired)
	if stats.Rejected != 3 || len(stats.Entries) != 3 {
		t.Fatalf("corruption was not repaired: %+v", stats)
	}
	// A dependency reaching an owned package must use source identities.
	write("local/local.go", "package local\ntype T struct{}\n")
	write("vendor/example/dep/left/left.go", "package left\nimport (\"example/dep\";\"example/app/local\")\nfunc F() dep.T{_ = local.T{};return dep.T{}}\n")
	_, guard := run(nil)
	if len(guard.Entries) != 2 {
		t.Fatalf("cached dependency reaching owned code: %+v", guard)
	}
	// Embedded files affect dependency keys even when the API shape is unchanged.
	write("vendor/example/dep/dep.go", "package dep\nimport _ \"embed\"\n//go:embed asset.txt\nvar Asset string\n"+strings.TrimPrefix(dep, "package dep\n"))
	write("vendor/example/dep/asset.txt", "one")
	_, embedded := run(nil)
	write("vendor/example/dep/asset.txt", "two")
	embedReused, embedStats := run(embedded.Entries)
	embedFresh, _ := run(nil)
	assertSame(embedFresh, embedReused)
	if embedStats.Misses < 2 {
		t.Fatalf("embedded input did not invalidate dependency and consumer: %+v", embedStats)
	}
	// Separate module worlds can use different versions of the same import path.
	write("vendor/example/dep/dep.go", dep)
	write("../other/go.mod", "module example/other\ngo 1.23\nrequire example/dep v1.1.0\n")
	write("../other/vendor/modules.txt", "# example/dep v1.1.0\n## explicit; go 1.23\nexample/dep\n")
	write("../other/vendor/example/dep/dep.go", "package dep\nfunc F() string{return \"other-version\"}\n")
	write("../other/main.go", "package other\nimport \"example/dep\"\nfunc Entry() string{return dep.F()}\n")
	request.Roots = append(request.Roots, reviewRoot{Directory: "other", Module: "."})
	multiFresh, multiCache := run(nil)
	multiWarm, multiStats := run(multiCache.Entries)
	assertSame(multiFresh, multiWarm)
	if multiStats.Hits != 3 || multiStats.Misses != 0 {
		t.Fatalf("cross-world cache identity: %+v", multiStats)
	}
}

func TestFullOptionalCacheKeepsArtifactsAndCountersWithinTransport(t *testing.T) {
	c := &reviewPackageCache{created: map[string]string{}, used: map[string]bool{}, hits: 7, misses: 3}
	for n := 0; n < 10; n++ {
		key := fmt.Sprintf("%064x", n)
		c.created[key] = strings.Repeat("a", 100+n*100)
		c.used[key] = true
	}
	for _, limit := range []int64{64, 1200, 1800, 10000} {
		c.stats = nil
		var buffer bytes.Buffer
		out := &countingWriter{writer: &buffer}
		fmt.Fprint(out, `{"facts":"existing"`)
		c.writeResult(out, limit)
		fmt.Fprintln(out, `}`)
		if out.bytes > limit {
			t.Fatalf("transport overflow: %d > %d", out.bytes, limit)
		}
		var envelope struct {
			Cache *reviewCacheResult `json:"dependency_cache"`
		}
		if err := json.Unmarshal(buffer.Bytes(), &envelope); err != nil {
			t.Fatal(err)
		}
		if limit == 64 {
			if envelope.Cache != nil {
				t.Fatal("metadata cannot fit")
			}
			if c.stats["transport_metadata_omitted"] != 1 || c.stats["transport_omitted_entries"] != len(c.created) {
				t.Fatalf("missing transport refusal: %+v", c.stats)
			}
			continue
		}
		if envelope.Cache == nil || envelope.Cache.Hits != 7 || envelope.Cache.Misses != 3 || len(envelope.Cache.Entries) == 0 {
			t.Fatalf("lost bounded cache: %s", buffer.Bytes())
		}
		if c.stats["transport_entries"] != len(envelope.Cache.Entries) || c.stats["transport_entries"]+c.stats["transport_omitted_entries"] != len(c.created) {
			t.Fatalf("transport counters disagree: %+v", c.stats)
		}
		if limit == 10000 && len(envelope.Cache.Entries) != 10 {
			t.Fatal("discarded artifacts that fit")
		}
	}
}

func TestPackageCacheCapacityDiagnostics(t *testing.T) {
	root := t.TempDir()
	entries := map[string]string{"invalid": "data", strings.Repeat("f", 64): strings.Repeat("x", packageCacheEntryBytes+1)}
	for n := 0; n < 4097; n++ {
		entries[fmt.Sprintf("%064x", n)] = "data"
	}
	c := newReviewPackageCache(reviewRequest{CacheIdentity: "test", PackageCache: entries}, root)
	if len(c.entries) != 4096 || c.stats["input_invalid_key"] != 1 || c.stats["input_entry_limit"] != 1 || c.stats["input_count_limit"] != 1 {
		t.Fatalf("count/entry refusals: %+v", c.stats)
	}
	pkg := types.NewPackage("example/dep", "dep")
	pkg.MarkComplete()
	c.world()
	c.keys[pkg.Path()] = strings.Repeat("b", 64)
	c.save(&loader{}, pkg.Path(), pkg)
	if c.stats["save_count_limit"] != 1 || len(c.created) != 0 {
		t.Fatalf("save count refusal: %+v", c.stats)
	}
	entries = map[string]string{}
	for n := 0; n < 17; n++ {
		entries[fmt.Sprintf("%064x", n)] = strings.Repeat("x", packageCacheEntryBytes)
	}
	c = newReviewPackageCache(reviewRequest{CacheIdentity: "test", PackageCache: entries}, root)
	if c.bytes != packageCacheBytes || c.stats["input_byte_limit"] != 1 {
		t.Fatalf("load byte refusal: %+v", c.stats)
	}
	c.world()
	c.keys[pkg.Path()] = strings.Repeat("b", 64)
	c.save(&loader{}, pkg.Path(), pkg)
	if c.stats["save_byte_limit"] != 1 || len(c.created) != 0 {
		t.Fatalf("save byte refusal: %+v", c.stats)
	}
}

// Recursive/nested phases partition elapsed time rather than accumulating
// inclusive parent time on top of child time.
func TestReviewTimingsPartitionNestedImports(t *testing.T) {
	nativeReviewTimings = &reviewTimings{totals: map[string]time.Duration{}}
	defer func() { nativeReviewTimings = nil }()
	outer := reviewPhase("type_check")
	started := nativeReviewTimings.since
	load := reviewPhase("export_load")
	inner := reviewPhase("type_check")
	inner()
	if nativeReviewTimings.active != "export_load" {
		t.Fatal("lost parent phase")
	}
	load()
	if nativeReviewTimings.active != "type_check" {
		t.Fatal("lost outer phase")
	}
	outer()
	if nativeReviewTimings.active != "" {
		t.Fatal("unbalanced phases")
	}
	var total time.Duration
	for _, duration := range nativeReviewTimings.totals {
		total += duration
	}
	if total != nativeReviewTimings.since.Sub(started) {
		t.Fatal("nested time counted twice")
	}
}
