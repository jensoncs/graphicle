// Compiler-backed call targets. The pinned x/tools library serializes types;
// package selection and type checking use the Go standard library.
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
	"runtime"
	"sort"
	"strings"
	"sync"
)

type request struct {
	Semantic *semanticRequest `json:"semantic,omitempty"`
	Paths    []string
	Review   *reviewRequest `json:"review,omitempty"`
}
type moduleMetadata struct {
	Path, Version, Dir, GoMod, GoVersion string
	Replace                              *moduleMetadata
}
type packageError struct {
	Err, Pos    string
	ImportStack []string
}
type metadata struct {
	Deps                                                                                 []string
	Dir, ImportPath, Name                                                                string
	GoFiles, CgoFiles                                                                    []string
	IgnoredGoFiles, EmbedFiles, TestEmbedFiles, XTestEmbedFiles                          []string
	ImportMap                                                                            map[string]string
	Error                                                                                *packageError
	Module                                                                               *moduleMetadata
	Standard, DepOnly, Incomplete                                                        bool
	Imports, TestGoFiles, XTestGoFiles                                                   []string
	DepsErrors                                                                           []packageError
	EmbedPatterns, TestEmbedPatterns, XTestEmbedPatterns                                 []string
	CFiles, CXXFiles, MFiles, HFiles, FFiles, SFiles, SwigFiles, SwigCXXFiles, SysoFiles []string
}
type site struct {
	// Dispatch: Caller is an interface method, Target a method implementing it.
	Dispatch   bool   `json:"dispatch,omitempty"`
	Candidate  bool   `json:"candidate,omitempty"`
	Partial    bool   `json:"partial,omitempty"`
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

// One stdout line per module group, written when the group finishes: a caller
// stopped mid-run keeps every finished group, and knows which files it covers.
type group struct {
	Root  string   `json:"root"`
	Files []string `json:"files"`
	Sites []site   `json:"sites"`
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
	cache    *reviewPackageCache
}

func (l *loader) Import(path string) (result *types.Package, failure error) {
	defer reviewPhase("import")()
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
			l.review.diagnostic(path, "missing_package", "", 0, "Package missing from selected go list metadata")
			return nil, fmt.Errorf("package missing from selected build: %s", path)
		}
		return l.fallback.Import(path)
	}
	if l.review != nil && m.Error != nil && m.Dir == "" {
		// Discovery already recorded the real cause. An absent directory is not
		// evidence of source escaping the snapshots, and cannot be type-checked.
		return nil, fmt.Errorf("package discovery failed: %s", path)
	}
	if l.review != nil && !m.Standard && !l.review.allowed(m.Dir) {
		l.review.diagnostic(path, "outside_snapshot", "", 0, "Package source is outside the admitted snapshots")
		return nil, fmt.Errorf("package outside pinned snapshots: %s", path)
	}
	if l.busy[path] {
		if l.review != nil {
			l.review.diagnostic(path, "import_cycle", "", 0, "Import cycle in selected packages")
		}
		return nil, fmt.Errorf("import cycle: %s", path)
	}
	l.busy[path] = true
	defer delete(l.busy, path)
	if l.cache != nil {
		if pkg := l.cache.load(l, path); pkg != nil {
			if !m.Standard {
				l.review.collectDependency(m, true)
			}
			return pkg, nil
		}
	}
	files := []*ast.File{}
	for _, name := range m.GoFiles {
		doneParse := reviewPhase("parse")
		f, err := parser.ParseFile(l.fset, filepath.Join(m.Dir, name), nil, 0)
		doneParse()
		if err != nil {
			if l.review != nil {
				l.review.diagnostic(path, "parse_error", l.review.relative(filepath.Join(m.Dir, name)), 0, err.Error())
			}
			return nil, err
		}
		files = append(files, f)
	}
	// Dependencies are still parsed and checked, but their per-expression
	// annotations are not retained for a graph we do not build.
	var info *types.Info
	if l.review == nil || l.review.owns(m.Dir) {
		info = &types.Info{Defs: map[*ast.Ident]types.Object{}, Uses: map[*ast.Ident]types.Object{}, Selections: map[*ast.SelectorExpr]*types.Selection{}, Types: map[ast.Expr]types.TypeAndValue{}}
	}
	conf := types.Config{Importer: mappedImporter{l, m.ImportMap}, Sizes: l.sizes, Error: func(err error) {
		if l.review == nil {
			return
		}
		file, line, message := "", 0, ""
		if diagnostic, ok := err.(types.Error); ok {
			position := diagnostic.Fset.PositionFor(diagnostic.Pos, false)
			if l.review.allowed(position.Filename) {
				file = l.review.relative(position.Filename)
				line = position.Line
			}
			message = diagnostic.Msg
		} else {
			message = err.Error()
		}
		l.review.diagnostic(path, "type_error", file, line, message)
	}}
	if m.Module != nil && m.Module.GoVersion != "" {
		conf.GoVersion = "go" + m.Module.GoVersion
	}
	doneCheck := reviewPhase("type_check")
	pkg, err := conf.Check(path, l.fset, files, info)
	doneCheck()
	valid := err == nil && m.Error == nil && !m.Incomplete && len(m.DepsErrors) == 0 && len(m.CgoFiles) == 0
	if l.review != nil {
		valid = err == nil && !reviewDiscoveryIncomplete(m, l.review.externalErrors) && len(m.CgoFiles) == 0
	}
	if l.review != nil && !m.Standard {
		l.review.collect(l, m, files, info, valid)
	}
	if valid {
		l.packages[path] = pkg
		if l.cache != nil {
			l.cache.save(l, path, pkg)
		}
	}
	// A graph build keeps what go/types identified in a package it could not
	// fully check (a dependency missing offline): an object it records is
	// exact, and importers still see this package's declarations. Pinned
	// review keeps requiring complete packages.
	partial := !valid && l.review == nil
	var bindings map[types.Object]ast.Expr
	var flow *callableFlow
	for _, file := range files {
		filename := l.fset.PositionFor(file.Pos(), false).Filename
		if !l.wanted[filename] {
			continue
		}
		if flow == nil {
			bindings = localValueBindings(files, info)
			flow = functionValueFlow(files, info, bindings)
		}
		for _, decl := range file.Decls {
			// A closure has no node of its own: its calls belong to the
			// function that writes it. A package-level var initialiser runs at
			// package init, so its calls belong to the file (empty caller).
			var body ast.Node
			caller, callerLine := "", 0
			switch d := decl.(type) {
			case *ast.FuncDecl:
				if d.Body == nil {
					continue
				}
				body, caller, callerLine = d.Body, d.Name.Name, l.fset.PositionFor(d.Pos(), false).Line
			case *ast.GenDecl:
				if d.Tok != token.VAR {
					continue
				}
				body = d
			default:
				continue
			}
			ast.Inspect(body, func(n ast.Node) bool {
				call, ok := n.(*ast.CallExpr)
				if !ok {
					return true
				}
				pos := l.fset.PositionFor(call.Pos(), false)
				s := site{File: filename, Line: pos.Line, Column: pos.Column, Caller: caller, CallerLine: callerLine, Reason: "unresolved_type"}
				fun := boundExpression(call.Fun, bindings, info)
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
						// A call through an interface value names the interface
						// method; dispatch() links it to the implementations.
						obj = sel.Obj()
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
				if target, ok := obj.(*types.Func); ok && (valid || partial) {
					p := l.fset.PositionFor(target.Origin().Pos(), false)
					if p.IsValid() {
						s.TargetFile, s.TargetLine, s.Target, s.Reason, s.Partial = p.Filename, p.Line, target.Name(), "", partial
					}
				}
				if valid && s.TargetFile == "" {
					candidates, limited := flow.candidates(call.Fun)
					if limited {
						s.Reason = "function_value_budget"
					}
					seen := map[token.Pos]bool{}
					for _, candidate := range candidates {
						fn := callableTarget(candidate, info)
						if fn == nil || seen[fn.Origin().Pos()] {
							continue
						}
						seen[fn.Origin().Pos()] = true
						p := l.fset.PositionFor(fn.Origin().Pos(), false)
						if !p.IsValid() {
							continue
						}
						possible := s
						possible.Candidate = true
						possible.Reason = ""
						possible.TargetFile, possible.TargetLine, possible.Target = p.Filename, p.Line, fn.Name()
						l.sites = append(l.sites, possible)
					}
				}
				l.sites = append(l.sites, s)
				return true
			})
		}
	}
	if partial {
		l.packages[path] = pkg
	} else if !valid {
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
	if req.Semantic != nil {
		runSemantic(*req.Semantic)
		return
	}
	if req.Review != nil {
		runReview(*req.Review)
		return
	}
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
		return
	}
	sizes := types.SizesFor("gc", config["GOARCH"])
	if sizes == nil {
		panic("unsupported Go architecture")
	}
	wanted := map[string]bool{}
	dirs := map[string]bool{}
	files := map[string][]string{}
	testDirs := map[string]bool{}
	for _, p := range req.Paths {
		p, _ = filepath.Abs(p)
		wanted[p] = true
		dirs[filepath.Dir(p)] = true
		files[filepath.Dir(p)] = append(files[filepath.Dir(p)], p)
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
	// Groups share nothing but the requested files, so each is discovered and
	// type-checked on its own loader, on every core, and reported as it
	// finishes. Lines arrive in finish order; a reader sorts them by root.
	roots := sortedKeys(groupRoots)
	next := make(chan int)
	out := json.NewEncoder(os.Stdout)
	var written sync.Mutex
	var wg sync.WaitGroup
	for w := 0; w < min(runtime.GOMAXPROCS(0), len(roots)); w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := range next {
				g := group{Root: roots[i], Files: []string{}, Sites: discover(roots[i], groups[roots[i]], modules[roots[i]], testDirs, wanted, sizes)}
				for _, dir := range sortedKeys(groups[roots[i]]) {
					g.Files = append(g.Files, files[dir]...)
				}
				if g.Sites == nil {
					g.Sites = []site{}
				}
				// Encode writes the whole line at once; the lock keeps lines whole.
				written.Lock()
				err := out.Encode(g)
				written.Unlock()
				if err != nil {
					panic(err)
				}
			}
		}()
	}
	for i := range roots {
		next <- i
	}
	close(next)
	wg.Wait()
}

// discover lists one group's packages with go list and type-checks them on a
// loader of its own: module instances must not share go/types packages
// across builds. It returns the group's call sites in check order.
func discover(root string, dirs map[string]bool, module bool, testDirs, wanted map[string]bool, sizes types.Sizes) []site {
	fset := token.NewFileSet()
	l := &loader{fset: fset, meta: map[string]*metadata{}, packages: map[string]*types.Package{}, failures: map[string]error{}, busy: map[string]bool{}, wanted: wanted, sizes: sizes}
	l.fallback = importer.ForCompiler(fset, "source", nil)
	roots := map[string]bool{}
	args := []string{"list", "-e", "-deps", "-json"}
	if module {
		args = append(args, moduleMode(root, os.Getenv("GOWORK")))
	}
	patterns := []string{}
	tests := false
	for _, dir := range sortedKeys(dirs) {
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
	if !module && (os.Getenv("GOWORK") == "" || os.Getenv("GOWORK") == "off") {
		cmd.Env = append(os.Environ(), "GO111MODULE=off")
	}
	goList <- struct{}{}
	data, err := cmd.Output()
	<-goList
	if err != nil {
		return nil
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
		if dirs[m.Dir] {
			roots[m.ImportPath] = true
		}
	}
	for _, path := range sortedKeys(roots) {
		_, _ = l.Import(path)
	}
	return append(l.sites, l.dispatch()...)
}

// dispatch links each method of a scanned interface to its implementation on
// every scanned type whose value or pointer satisfies the interface. go/types
// decides satisfaction, so a same-named method with another signature never
// counts. Which implementation a call reaches stays a runtime question.
// ponytail: package-level, non-generic named types only; local and generic
// types get no dispatch edges.
func (l *loader) dispatch() []site {
	type declared struct {
		named *types.Named
		where token.Position
	}
	var interfaces, concrete []declared
	paths := make([]string, 0, len(l.packages))
	for path := range l.packages {
		paths = append(paths, path)
	}
	sort.Strings(paths)
	for _, path := range paths {
		scope := l.packages[path].Scope()
		for _, name := range scope.Names() {
			tn, ok := scope.Lookup(name).(*types.TypeName)
			if !ok || tn.IsAlias() {
				continue
			}
			named, ok := tn.Type().(*types.Named)
			where := l.fset.PositionFor(tn.Pos(), false)
			if !ok || named.TypeParams().Len() > 0 || !l.wanted[where.Filename] {
				continue
			}
			if iface, ok := named.Underlying().(*types.Interface); ok {
				if iface.NumMethods() > 0 {
					interfaces = append(interfaces, declared{named, where})
				}
			} else {
				concrete = append(concrete, declared{named, where})
			}
		}
	}
	// A test build checks a package again: one pair per source position.
	seen := map[[2]token.Position]bool{}
	var out []site
	for _, i := range interfaces {
		iface := i.named.Underlying().(*types.Interface)
		for _, c := range concrete {
			pointer := types.NewPointer(c.named)
			if !types.Implements(pointer, iface) {
				continue
			}
			for m := 0; m < iface.NumMethods(); m++ {
				method := iface.Method(m)
				obj, _, _ := types.LookupFieldOrMethod(pointer, false, method.Pkg(), method.Name())
				impl, ok := obj.(*types.Func)
				if !ok {
					continue
				}
				// A method promoted from an embedded interface is not an implementation.
				if _, abstract := impl.Type().(*types.Signature).Recv().Type().Underlying().(*types.Interface); abstract {
					continue
				}
				from := l.fset.PositionFor(method.Pos(), false)
				to := l.fset.PositionFor(impl.Origin().Pos(), false)
				if !from.IsValid() || !to.IsValid() || !l.wanted[from.Filename] || !l.wanted[to.Filename] || seen[[2]token.Position{from, to}] {
					continue
				}
				seen[[2]token.Position{from, to}] = true
				out = append(out, site{Dispatch: true, File: from.Filename, Line: from.Line, Caller: method.Name(), CallerLine: from.Line, TargetFile: to.Filename, TargetLine: to.Line, Target: impl.Name()})
			}
		}
	}
	return out
}

// goListSlots bounds concurrent go list runs. They contend on the module
// cache and file system: on hyperion (325 modules, 18 cores) four at once were
// as fast as eighteen with a quarter of the memory.
const goListSlots = 4

var goList = make(chan struct{}, goListSlots)

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
	} else {
		// A selected package subtree still uses its owning module's vendor tree.
		for {
			if info, err := os.Stat(filepath.Join(root, "go.mod")); err == nil && !info.IsDir() {
				break
			}
			parent := filepath.Dir(root)
			if parent == root {
				break
			}
			root = parent
		}
	}
	if info, err := os.Stat(filepath.Join(root, "vendor", "modules.txt")); err == nil && !info.IsDir() {
		return "-mod=vendor"
	}
	return "-mod=readonly"
}
