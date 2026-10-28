// Compiler-backed call targets. This helper uses only the Go standard library.
// go list selects the active build; go/types owns declaration and method identity.
package main

import (
	"encoding/json"
	"fmt"
	"go/ast"
	"go/importer"
	"go/parser"
	"go/token"
	"go/types"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
)

type request struct {
	Paths  []string
	Review *reviewRequest `json:"review,omitempty"`
}
type moduleMetadata struct {
	Path, Version, Dir, GoMod, GoVersion string
	Replace                              *moduleMetadata
}
type metadata struct {
	Dir, ImportPath, Name              string
	GoFiles, CgoFiles                  []string
	ImportMap                          map[string]string
	Error                              *struct{ Err string }
	Module                             *moduleMetadata
	Standard, DepOnly, Incomplete      bool
	Imports, TestGoFiles, XTestGoFiles []string
	DepsErrors                         []struct{ Err string }
}
type site struct {
	File       string `json:"file"`
	Line       int    `json:"line"`
	Column     int    `json:"column"`
	Caller     string `json:"caller"`
	CallerLine int    `json:"caller_line"`
	TargetFile string `json:"target_file,omitempty"`
	TargetLine int    `json:"target_line,omitempty"`
	Target     string `json:"target,omitempty"`
	Reason     string `json:"reason,omitempty"`
}
type loader struct {
	fset     *token.FileSet
	meta     map[string]*metadata
	packages map[string]*types.Package
	failures map[string]error
	busy     map[string]bool
	sites    []site
	wanted   map[string]bool
	fallback types.Importer
	sizes    types.Sizes
	review   *reviewCollector
}

func (l *loader) Import(path string) (result *types.Package, failure error) {
	if err := l.failures[path]; err != nil {
		return nil, err
	}
	defer func() {
		if failure != nil {
			if l.failures == nil {
				l.failures = map[string]error{}
			}
			l.failures[path] = failure
		}
	}()
	if path == "unsafe" {
		return types.Unsafe, nil
	}
	if p := l.packages[path]; p != nil {
		return p, nil
	}
	m := l.meta[path]
	if m == nil {
		if l.review != nil {
			return nil, fmt.Errorf("package missing from selected build: %s", path)
		}
		return l.fallback.Import(path)
	}
	if l.review != nil && !m.Standard && !l.review.allowed(m.Dir) {
		return nil, fmt.Errorf("package outside pinned snapshots: %s", path)
	}
	if l.busy[path] {
		return nil, fmt.Errorf("import cycle: %s", path)
	}
	l.busy[path] = true
	defer delete(l.busy, path)
	files := []*ast.File{}
	for _, name := range m.GoFiles {
		f, err := parser.ParseFile(l.fset, filepath.Join(m.Dir, name), nil, 0)
		if err != nil {
			return nil, err
		}
		files = append(files, f)
	}
	info := &types.Info{Defs: map[*ast.Ident]types.Object{}, Uses: map[*ast.Ident]types.Object{}, Selections: map[*ast.SelectorExpr]*types.Selection{}, Types: map[ast.Expr]types.TypeAndValue{}}
	conf := types.Config{Importer: mappedImporter{l, m.ImportMap}, Sizes: l.sizes, Error: func(error) {}}
	if m.Module != nil && m.Module.GoVersion != "" {
		conf.GoVersion = "go" + m.Module.GoVersion
	}
	pkg, err := conf.Check(path, l.fset, files, info)
	valid := err == nil && m.Error == nil && !m.Incomplete && len(m.DepsErrors) == 0 && len(m.CgoFiles) == 0
	if l.review != nil && !m.Standard {
		l.review.collect(l, m, files, info, valid)
	}
	if valid {
		l.packages[path] = pkg
	}
	for _, file := range files {
		filename := l.fset.PositionFor(file.Pos(), false).Filename
		if !l.wanted[filename] {
			continue
		}
		for _, decl := range file.Decls {
			fn, ok := decl.(*ast.FuncDecl)
			if !ok || fn.Body == nil {
				continue
			}
			ast.Inspect(fn.Body, func(n ast.Node) bool {
				// A closure has no declaration node in the current graph. Do not
				// attribute its calls to the enclosing function.
				if _, ok := n.(*ast.FuncLit); ok {
					return false
				}
				call, ok := n.(*ast.CallExpr)
				if !ok {
					return true
				}
				pos := l.fset.PositionFor(call.Pos(), false)
				s := site{File: filename, Line: pos.Line, Column: pos.Column, Caller: fn.Name.Name, CallerLine: l.fset.PositionFor(fn.Pos(), false).Line, Reason: "unresolved_type"}
				fun := unparen(call.Fun)
				if index, ok := fun.(*ast.IndexExpr); ok {
					fun = unparen(index.X)
				}
				if index, ok := fun.(*ast.IndexListExpr); ok {
					fun = unparen(index.X)
				}
				var obj types.Object
				switch f := fun.(type) {
				case *ast.Ident:
					obj = info.Uses[f]
				case *ast.SelectorExpr:
					if sel := info.Selections[f]; sel != nil {
						recv := sel.Recv()
						if p, ok := recv.(*types.Pointer); ok {
							recv = p.Elem()
						}
						// Interface dispatch requires runtime information.
						if _, dynamic := recv.Underlying().(*types.Interface); !dynamic {
							obj = sel.Obj()
						}
					} else {
						obj = info.Uses[f.Sel]
					}
				}
				if _, builtin := obj.(*types.Builtin); builtin {
					return true
				}
				if tv, ok := info.Types[call.Fun]; ok && tv.IsType() {
					return true
				}
				if target, ok := obj.(*types.Func); ok && valid {
					// A concrete struct may promote an embedded interface's
					// method. That declaration is still dynamic dispatch.
					sig := target.Type().(*types.Signature)
					dynamic := false
					if recv := sig.Recv(); recv != nil {
						_, dynamic = recv.Type().Underlying().(*types.Interface)
					}
					if !dynamic {
						p := l.fset.PositionFor(target.Origin().Pos(), false)
						if p.IsValid() {
							s.TargetFile, s.TargetLine, s.Target, s.Reason = p.Filename, p.Line, target.Name(), ""
						}
					}
				}
				l.sites = append(l.sites, s)
				return true
			})
		}
	}
	if !valid {
		return nil, fmt.Errorf("incomplete package %s", path)
	}
	return pkg, nil
}

type mappedImporter struct {
	l       *loader
	imports map[string]string
}

func (m mappedImporter) Import(path string) (*types.Package, error) {
	if mapped := m.imports[path]; mapped != "" {
		path = mapped
	}
	return m.l.Import(path)
}
func unparen(e ast.Expr) ast.Expr {
	for {
		p, ok := e.(*ast.ParenExpr)
		if !ok {
			return e
		}
		e = p.X
	}
}
func main() {
	var req request
	if err := json.NewDecoder(os.Stdin).Decode(&req); err != nil {
		panic(err)
	}
	if req.Review != nil {
		runReview(*req.Review)
		return
	}
	l := &loader{fset: token.NewFileSet(), meta: map[string]*metadata{}, packages: map[string]*types.Package{}, busy: map[string]bool{}, wanted: map[string]bool{}}
	l.fallback = importer.ForCompiler(l.fset, "source", nil)
	configuration, err := exec.Command("go", "env", "-json", "GOARCH", "GOFLAGS", "GO111MODULE").Output()
	if err != nil {
		panic(err)
	}
	var config map[string]string
	if err := json.Unmarshal(configuration, &config); err != nil {
		panic(err)
	}
	// The Rust graph reads disk sources, not Go overlay replacements. An
	// overlay cannot be safely mapped back to those declaration identities.
	if strings.Contains(config["GOFLAGS"], "-overlay") || strings.Contains(config["GOFLAGS"], "-compiler") {
		fmt.Println("[]")
		return
	}
	l.sizes = types.SizesFor("gc", config["GOARCH"])
	if l.sizes == nil {
		panic("unsupported Go architecture")
	}
	dirs := map[string]bool{}
	testDirs := map[string]bool{}
	for _, p := range req.Paths {
		p, _ = filepath.Abs(p)
		l.wanted[p] = true
		dirs[filepath.Dir(p)] = true
		if strings.HasSuffix(p, "_test.go") {
			testDirs[filepath.Dir(p)] = true
		}
	}
	// One discovery process per module, with all requested package directories.
	// Standalone packages have no shared module root and remain separate.
	groups := map[string]map[string]bool{}
	modules := map[string]bool{}
	for _, dir := range sortedKeys(dirs) {
		root := dir
		for p := dir; ; p = filepath.Dir(p) {
			if _, err := os.Stat(filepath.Join(p, "go.mod")); err == nil {
				root = p
				modules[root] = true
				break
			}
			if filepath.Dir(p) == p {
				break
			}
		}
		if groups[root] == nil {
			groups[root] = map[string]bool{}
		}
		groups[root][dir] = true
	}
	groupRoots := map[string]bool{}
	for root := range groups {
		groupRoots[root] = true
	}
	for _, root := range sortedKeys(groupRoots) {
		// Module instances must not share go/types packages across builds.
		l.meta = map[string]*metadata{}
		l.packages = map[string]*types.Package{}
		l.failures = map[string]error{}
		roots := map[string]bool{}
		args := []string{"list", "-e", "-deps", "-json"}
		if modules[root] {
			args = append(args, moduleMode(root, os.Getenv("GOWORK")))
		}
		patterns := []string{}
		tests := false
		for _, dir := range sortedKeys(groups[root]) {
			rel, err := filepath.Rel(root, dir)
			if err != nil {
				continue
			}
			patterns = append(patterns, "./"+filepath.ToSlash(rel))
			tests = tests || testDirs[dir]
		}
		if tests {
			args = append(args, "-test")
		}
		cmd := exec.Command("go", append(args, patterns...)...)
		cmd.Dir = root
		if !modules[root] && (os.Getenv("GOWORK") == "" || os.Getenv("GOWORK") == "off") {
			cmd.Env = append(os.Environ(), "GO111MODULE=off")
		}
		data, err := cmd.Output()
		if err != nil {
			continue
		}
		dec := json.NewDecoder(strings.NewReader(string(data)))
		for {
			var m metadata
			if err := dec.Decode(&m); err != nil {
				if err != io.EOF {
					panic(err)
				}
				break
			}
			if m.Dir == "" || m.ImportPath == "" || (m.Name == "main" && strings.HasSuffix(m.ImportPath, ".test")) {
				continue
			}
			l.meta[m.ImportPath] = &m
			if groups[root][m.Dir] {
				roots[m.ImportPath] = true
			}
		}
		for _, path := range sortedKeys(roots) {
			_, _ = l.Import(path)
		}
	}
	if l.sites == nil {
		l.sites = []site{}
	}
	if err := json.NewEncoder(os.Stdout).Encode(l.sites); err != nil {
		panic(err)
	}
}

func sortedKeys(values map[string]bool) []string {
	keys := make([]string, 0, len(values))
	for key := range values {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	return keys
}

// Respect a supplied vendor tree while preventing module-file updates otherwise.
func moduleMode(root, workspace string) string {
	if workspace != "" && workspace != "off" {
		root = filepath.Dir(workspace)
	}
	if info, err := os.Stat(filepath.Join(root, "vendor", "modules.txt")); err == nil && !info.IsDir() {
		return "-mod=vendor"
	}
	return "-mod=readonly"
}
