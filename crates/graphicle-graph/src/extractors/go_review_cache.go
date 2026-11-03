// Successful dependency types are optional, content-bound compiler artifacts.
// Owned packages always take the source path. No package code is executed.
package main

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"go/types"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"golang.org/x/tools/go/gcexportdata"
)

const packageCacheBytes = 64 << 20
const packageCacheEntryBytes = 4 << 20 // base64, matching the store bound
const packageCacheRawBytes = packageCacheEntryBytes * 3 / 4

type reviewCacheResult struct {
	Entries  map[string]string `json:"entries"`
	Used     []string          `json:"used"`
	Hits     int               `json:"hits"`
	Misses   int               `json:"misses"`
	Rejected int               `json:"rejected"`
}

type reviewPackageCache struct {
	compressor             *gzip.Writer
	identity               string
	entries                map[string]string
	created                map[string]string
	used                   map[string]bool
	bytes                  int
	keys                   map[string]string
	busy                   map[string]bool
	hits, misses, rejected int
}

func newReviewPackageCache(req reviewRequest, root string) *reviewPackageCache {
	c := &reviewPackageCache{entries: map[string]string{}, created: map[string]string{}, used: map[string]bool{}}
	if req.CacheIdentity == "" {
		return c
	}
	// Changes to discovery configuration, including a custom GOWORK filename,
	// invalidate reuse. Package fingerprints below also cover transitive imports.
	config := map[string]string{}
	{
		err := filepath.WalkDir(root, func(path string, entry os.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if entry.IsDir() {
				return nil
			}
			name := entry.Name()
			if name != "go.mod" && name != "go.sum" && name != "go.work" && name != "go.work.sum" && name != "modules.txt" && path != filepath.Join(root, req.Workspace) {
				return nil
			}
			data, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			relative, err := filepath.Rel(root, path)
			if err != nil {
				return err
			}
			config[filepath.ToSlash(relative)] = digestBytes(data)
			return nil
		})
		if err != nil {
			return c
		}
	}
	identity, _ := json.Marshal([]any{req.CacheIdentity, req.GOOS, req.GOARCH, req.Tags, req.Tests, req.Workspace, config})
	c.identity = digestBytes(identity)
	keys := make([]string, 0, len(req.PackageCache))
	for key := range req.PackageCache {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	for _, key := range keys {
		value := req.PackageCache[key]
		if !validPackageCacheKey(key) || len(value) > packageCacheEntryBytes || len(c.entries) >= 4096 || c.bytes+len(value) > packageCacheBytes {
			continue
		}
		c.entries[key] = value
		c.bytes += len(value)
	}
	return c
}

func (c *reviewPackageCache) world() {
	c.keys = map[string]string{}
	c.busy = map[string]bool{}
}

// Source selection belongs to go list. Cache identity includes its selected
// files, native/embed inputs, import mapping and the identities of every import.
// A dependency importing owned code is deliberately not cached: importing an
// exported copy of an owned package would split its go/types object identities.
func (c *reviewPackageCache) key(l *loader, path string) string {
	if c.identity == "" {
		return ""
	}
	if path == "unsafe" {
		return digestBytes([]byte(c.identity + "/unsafe"))
	}
	if key, ok := c.keys[path]; ok {
		return key
	}
	if c.busy[path] {
		return ""
	}
	c.busy[path] = true
	defer delete(c.busy, path)
	c.keys[path] = ""
	m := l.meta[path]
	if m == nil || m.Error != nil || m.Incomplete || len(m.DepsErrors) > 0 || len(m.CgoFiles) > 0 || (!m.Standard && (!l.review.allowed(m.Dir) || l.review.owns(m.Dir))) {
		return ""
	}
	imports := map[string]string{}
	for _, original := range m.Imports {
		name := original
		if mapped := m.ImportMap[name]; mapped != "" {
			name = mapped
		}
		key := c.key(l, name)
		if key == "" {
			return ""
		}
		imports[name] = key
	}
	names := map[string]bool{}
	for _, group := range [][]string{m.GoFiles, m.IgnoredGoFiles, m.TestGoFiles, m.XTestGoFiles, m.EmbedFiles, m.TestEmbedFiles, m.XTestEmbedFiles, m.CFiles, m.CXXFiles, m.MFiles, m.HFiles, m.FFiles, m.SFiles, m.SwigFiles, m.SwigCXXFiles, m.SysoFiles} {
		for _, name := range group {
			names[name] = true
		}
	}
	files := map[string]string{}
	for _, name := range sortedKeys(names) {
		file := filepath.Join(m.Dir, name)
		if !m.Standard && !l.review.allowed(file) {
			return ""
		}
		data, err := os.ReadFile(file)
		if err != nil {
			return ""
		}
		files[name] = digestBytes(data)
	}
	var module, replacement any
	if m.Module != nil {
		module = []string{m.Module.Path, m.Module.Version, m.Module.GoVersion}
		if r := m.Module.Replace; r != nil {
			replacement = []string{r.Path, r.Version, r.GoVersion, l.review.relative(r.Dir)}
		}
	}
	directory := m.Dir
	if !m.Standard {
		directory = l.review.relative(m.Dir)
	}
	encoded, _ := json.Marshal([]any{c.identity, path, m.Name, directory, module, replacement, m.GoFiles, m.ImportMap, imports, files})
	c.keys[path] = digestBytes(encoded)
	return c.keys[path]
}

func readPackage(data []byte, l *loader, path string) (pkg *types.Package) {
	// An incompatible or damaged optional cache entry is a miss. Imports already
	// loaded from verified inputs retain their identities; partial new maps do not.
	defer func() {
		if recover() != nil {
			pkg = nil
		}
	}()
	imports := map[string]*types.Package{"unsafe": types.Unsafe}
	for name, p := range l.packages {
		imports[name] = p
	}
	p, err := gcexportdata.Read(bytes.NewReader(data), l.fset, imports, path)
	if err != nil || p == nil || !p.Complete() || p.Path() != path {
		return nil
	}
	for name, imported := range imports {
		if name != path && name != "unsafe" && (l.packages[name] != imported || !imported.Complete()) {
			return nil
		}
	}
	l.packages[path] = p
	return p
}

func (c *reviewPackageCache) load(l *loader, path string) *types.Package {
	key := c.key(l, path)
	if key == "" {
		return nil
	}
	encoded, ok := c.entries[key]
	if !ok {
		c.misses++
		return nil
	}
	for _, original := range l.meta[path].Imports {
		name := original
		if mapped := l.meta[path].ImportMap[name]; mapped != "" {
			name = mapped
		}
		if _, err := l.Import(name); err != nil {
			c.misses++
			return nil
		}
	}
	data, err := base64.StdEncoding.DecodeString(encoded)
	if err == nil {
		data, err = unpackPackage(data)
	}
	if err == nil {
		if pkg := readPackage(data, l, path); pkg != nil {
			c.hits++
			c.used[key] = true
			return pkg
		}
	}
	c.rejected++
	c.misses++
	delete(c.entries, key)
	delete(c.created, key)
	c.bytes -= len(encoded)
	return nil
}

func unpackPackage(data []byte) ([]byte, error) {
	reader, err := gzip.NewReader(bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	defer reader.Close()
	decoded, err := io.ReadAll(io.LimitReader(reader, packageCacheRawBytes+1))
	if err == nil && len(decoded) > packageCacheRawBytes {
		return nil, io.ErrShortBuffer
	}
	return decoded, err
}

func exportPackage(pkg *types.Package) (data []byte) {
	defer func() {
		if recover() != nil {
			data = nil
		}
	}()
	var out bytes.Buffer
	// API boundaries have no source spans. Omitting the file set also prevents
	// temporary checkout paths from entering persistent artifacts.
	if err := gcexportdata.Write(&out, nil, pkg); err != nil {
		return nil
	}
	return out.Bytes()
}

func (c *reviewPackageCache) save(l *loader, path string, pkg *types.Package) {
	key := c.key(l, path)
	if key == "" || !pkg.Complete() {
		return
	}
	data := exportPackage(pkg)
	if data == nil || len(data) > packageCacheRawBytes {
		return
	}
	// Exports repeat dependency type descriptions. Compress each artifact, so
	// the persistent pool can retain a real closure within its existing limit.
	var packed bytes.Buffer
	if c.compressor == nil {
		c.compressor, _ = gzip.NewWriterLevel(&packed, gzip.BestSpeed)
	} else {
		c.compressor.Reset(&packed)
	}
	if _, err := c.compressor.Write(data); err != nil {
		return
	}
	if err := c.compressor.Close(); err != nil {
		return
	}
	encoded := base64.StdEncoding.EncodeToString(packed.Bytes())
	previous, exists := c.entries[key]
	if len(encoded) > packageCacheEntryBytes || (!exists && len(c.entries) >= 4096) || c.bytes-len(previous)+len(encoded) > packageCacheBytes {
		return
	}
	if previous, ok := c.entries[key]; ok {
		c.bytes -= len(previous)
	}
	c.entries[key] = encoded
	c.created[key] = encoded
	c.used[key] = true
	c.bytes += len(encoded)
}

func (c *reviewPackageCache) result() reviewCacheResult {
	used := make([]string, 0, len(c.used))
	for key := range c.used {
		used = append(used, key)
	}
	sort.Strings(used)
	return reviewCacheResult{Entries: c.created, Used: used, Hits: c.hits, Misses: c.misses, Rejected: c.rejected}
}

// Positionless imported APIs are still resolved dependency objects.
func (r *reviewCollector) dependencyObject(l *loader, obj types.Object) bool {
	if obj == nil || obj.Pkg() == nil {
		return false
	}
	m := l.meta[obj.Pkg().Path()]
	return m != nil && !m.Standard && r.allowed(m.Dir) && !r.owns(m.Dir)
}

// Keep this check next to the format bounds, rather than accepting arbitrary
// path-like cache identifiers in future storage adapters.
func validPackageCacheKey(key string) bool {
	return len(key) == 64 && strings.Trim(key, "0123456789abcdef") == ""
}

// Individual exports are already compressed/base64. Retain as many as fit in
// the remaining native transport; a full optional cache must not discard all
// artifacts or conceal the hit/miss counters. No dependency source is emitted.
func (c *reviewPackageCache) writeResult(out *countingWriter, limit int64) {
	result := c.result()
	entries := result.Entries
	result.Entries = map[string]string{}
	metadata, _ := json.Marshal(result)
	const prefix = `,"dependency_cache":`
	remaining := limit - out.bytes - int64(len(prefix)+len(metadata)+3)
	if remaining < 0 {
		return
	}
	keys := make([]string, 0, len(entries))
	for key := range entries {
		keys = append(keys, key)
	}
	sort.Slice(keys, func(i, j int) bool {
		if len(entries[keys[i]]) != len(entries[keys[j]]) {
			return len(entries[keys[i]]) < len(entries[keys[j]])
		}
		return keys[i] < keys[j]
	})
	for _, key := range keys {
		// Keys are hex and values are base64, so JSON needs no escape expansion.
		cost := int64(len(key) + len(entries[key]) + 6)
		if cost <= remaining {
			result.Entries[key] = entries[key]
			remaining -= cost
		}
	}
	fmt.Fprint(out, prefix)
	if err := json.NewEncoder(out).Encode(result); err != nil {
		panic(err)
	}
}
