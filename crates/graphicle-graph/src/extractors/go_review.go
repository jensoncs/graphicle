// Native Go review facts. No reviewed code is executed and no modules are fetched.
package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"go/ast"
	"go/format"
	"go/importer"
	"go/token"
	"go/types"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
)

type reviewRoot struct {
	Directory string `json:"directory"`
	Module    string `json:"module"`
}
type reviewRequest struct {
	Roots     []reviewRoot `json:"roots"`
	GOOS      string       `json:"goos"`
	GOARCH    string       `json:"goarch"`
	Tags      []string     `json:"tags"`
	Tests     bool         `json:"tests"`
	Workspace string       `json:"workspace"`
}
type reviewSymbol struct {
	Key            string `json:"key"`
	File           string `json:"file"`
	Package        string `json:"package"`
	Name           string `json:"name"`
	Kind           string `json:"kind"`
	Parent         string `json:"parent"`
	Start          int    `json:"start"`
	End            int    `json:"end"`
	BodyStart      int    `json:"body_start"`
	Line           int    `json:"line"`
	EndLine        int    `json:"end_line"`
	Revision       string `json:"revision"`
	Signature      string `json:"signature"`
	Implementation string `json:"implementation"`
	Correspondence string `json:"correspondence"`
}
type reviewCall struct {
	Caller        string `json:"caller"`
	Target        string `json:"target,omitempty"`
	TargetFile    string `json:"target_file,omitempty"`
	TargetOffset  int    `json:"target_offset"`
	TargetPackage string `json:"target_package,omitempty"`
	Spelling      string `json:"spelling"`
	File          string `json:"file"`
	Start         int    `json:"start"`
	End           int    `json:"end"`
	Line          int    `json:"line"`
	Relation      string `json:"relation"`
	Reason        string `json:"reason,omitempty"`
}
type reviewPackage struct {
	Path        string            `json:"path"`
	Directory   string            `json:"directory"`
	Module      string            `json:"module"`
	Version     string            `json:"version"`
	Replacement string            `json:"replacement"`
	Files       []string          `json:"files"`
	Imports     map[string]string `json:"imports"`
}
type reviewWorld struct {
	Root     string          `json:"root"`
	Symbols  []reviewSymbol  `json:"symbols"`
	Calls    []reviewCall    `json:"calls"`
	Packages []reviewPackage `json:"packages"`
	Issues   []string        `json:"issues"`
}
type reviewCollector struct {
	root         string
	allowedRoots []string
	world        *reviewWorld
}

func digestBytes(b []byte) string { h := sha256.Sum256(b); return hex.EncodeToString(h[:]) }
func (r *reviewCollector) relative(p string) string {
	v, e := filepath.Rel(r.root, p)
	if e != nil {
		return ""
	}
	return filepath.ToSlash(v)
}
func (r *reviewCollector) allowed(p string) bool {
	for _, root := range r.allowedRoots {
		v, e := filepath.Rel(root, p)
		if e == nil && v != ".." && !strings.HasPrefix(v, ".."+string(filepath.Separator)) {
			return true
		}
	}
	return false
}
func formatted(fs *token.FileSet, n ast.Node) []byte {
	var b bytes.Buffer
	if n == nil {
		return nil
	}
	_ = format.Node(&b, fs, n)
	return b.Bytes()
}
func (r *reviewCollector) collect(l *loader, m *metadata, files []*ast.File, info *types.Info, valid bool) {
	if !valid {
		r.world.Issues = append(r.world.Issues, "incomplete_package:"+m.ImportPath)
	}
	p := reviewPackage{Path: m.ImportPath, Directory: r.relative(m.Dir), Imports: map[string]string{}}
	if m.Module != nil {
		p.Module = m.Module.Path
		p.Version = m.Module.Version
		if m.Module.Replace != nil {
			p.Replacement = m.Module.Replace.Path + "@" + m.Module.Replace.Version
			if m.Module.Replace.Dir != "" {
				p.Replacement = r.relative(m.Module.Replace.Dir)
			}
		}
	}
	for _, name := range m.Imports {
		target := m.ImportMap[name]
		if target == "" {
			target = name
		}
		p.Imports[name] = target
	}
	for _, f := range files {
		filename := l.fset.PositionFor(f.Pos(), false).Filename
		if !r.allowed(filename) {
			r.world.Issues = append(r.world.Issues, "source_outside_snapshot")
			continue
		}
		source, err := os.ReadFile(filename)
		if err != nil {
			r.world.Issues = append(r.world.Issues, "unreadable_source")
			continue
		}
		relative := r.relative(filename)
		p.Files = append(p.Files, relative)
		var local []reviewSymbol
		add := func(n ast.Node, name, kind string, body *ast.BlockStmt, identifier *ast.Ident) {
			a, z := l.fset.PositionFor(n.Pos(), false), l.fset.PositionFor(n.End(), false)
			if a.Offset < 0 || z.Offset > len(source) || a.Offset >= z.Offset {
				return
			}
			bodyStart := z.Offset
			if body != nil {
				bodyStart = l.fset.PositionFor(body.Pos(), false).Offset
			}
			key := m.ImportPath + "|" + relative + "|" + name + "|" + kind
			sig := append([]byte{}, source[a.Offset:bodyStart]...)
			correspondence := append([]byte{}, sig...)
			if identifier != nil {
				start := l.fset.PositionFor(identifier.Pos(), false).Offset - a.Offset
				end := l.fset.PositionFor(identifier.End(), false).Offset - a.Offset
				if start >= 0 && end <= len(sig) {
					correspondence = append(append(append([]byte{}, sig[:start]...), []byte("<name>")...), sig[end:]...)
				}
			}
			implementation := source[a.Offset:z.Offset]
			if body != nil {
				implementation = formatted(l.fset, body)
			}
			symbol := reviewSymbol{Key: key, File: relative, Package: m.ImportPath, Name: name, Kind: kind, Start: a.Offset, End: z.Offset, BodyStart: bodyStart, Line: a.Line, EndLine: z.Line, Revision: digestBytes(source[a.Offset:z.Offset]), Signature: digestBytes(sig), Implementation: digestBytes(implementation), Correspondence: digestBytes(correspondence)}
			local = append(local, symbol)
		}
		ast.Inspect(f, func(n ast.Node) bool {
			switch node := n.(type) {
			case *ast.FuncDecl:
				name := node.Name.Name
				if fn, ok := info.Defs[node.Name].(*types.Func); ok {
					name = fn.FullName()
				}
				add(node, name, "function", node.Body, node.Name)
			case *ast.FuncLit:
				add(node, fmt.Sprintf("<closure@%d>", l.fset.PositionFor(node.Pos(), false).Offset), "closure", node.Body, nil)
			case *ast.TypeSpec:
				add(node, node.Name.Name, "type", nil, node.Name)
			}
			return true
		})
		// Exact lexical nesting. The outer declaration keeps its whole body.
		for i := range local {
			length := len(source) + 1
			for j := range local {
				if i != j && local[j].Start <= local[i].Start && local[i].End <= local[j].End && local[j].End-local[j].Start < length {
					local[i].Parent = local[j].Key
					length = local[j].End - local[j].Start
				}
			}
		}
		owner := func(pos token.Pos) string {
			offset := l.fset.PositionFor(pos, false).Offset
			key := ""
			length := len(source) + 1
			for _, s := range local {
				if s.Start <= offset && offset < s.End && s.End-s.Start < length {
					key = s.Key
					length = s.End - s.Start
				}
			}
			return key
		}
		ast.Inspect(f, func(n ast.Node) bool {
			call, ok := n.(*ast.CallExpr)
			if !ok {
				return true
			}
			position := l.fset.PositionFor(call.Pos(), false)
			site := reviewCall{Caller: owner(call.Pos()), File: relative, Start: position.Offset, End: l.fset.PositionFor(call.End(), false).Offset, Line: position.Line, Relation: "calls", Reason: "unresolved_dynamic_target"}
			fun := unparen(call.Fun)
			if index, ok := fun.(*ast.IndexExpr); ok {
				fun = unparen(index.X)
			}
			if index, ok := fun.(*ast.IndexListExpr); ok {
				fun = unparen(index.X)
			}
			site.Spelling = string(formatted(l.fset, call.Fun))
			var obj types.Object
			dynamic := false
			switch f := fun.(type) {
			case *ast.Ident:
				obj = info.Uses[f]
			case *ast.SelectorExpr:
				if sel := info.Selections[f]; sel != nil {
					obj = sel.Obj()
					recv := sel.Recv()
					if ptr, ok := recv.(*types.Pointer); ok {
						recv = ptr.Elem()
					}
					_, dynamic = recv.Underlying().(*types.Interface)
				} else {
					obj = info.Uses[f.Sel]
				}
			}
			if _, ok := obj.(*types.Builtin); ok {
				return true
			}
			if v, ok := info.Types[call.Fun]; ok && v.IsType() {
				return true
			}
			if target, ok := obj.(*types.Func); ok && valid && !dynamic {
				signature := target.Type().(*types.Signature)
				if recv := signature.Recv(); recv != nil {
					_, dynamic = recv.Type().Underlying().(*types.Interface)
				}
				if !dynamic {
					p := l.fset.PositionFor(target.Origin().Pos(), false)
					if target.Pkg() != nil {
						site.TargetPackage = target.Pkg().Path()
					}
					if p.IsValid() && r.allowed(p.Filename) {
						site.TargetFile = r.relative(p.Filename)
						site.TargetOffset = p.Offset
						site.Reason = ""
					} else {
						site.Reason = "outside_pinned_scope"
					}
				}
			}
			if !valid {
				site.Reason = "incomplete_package"
			}
			if site.Caller == "" {
				site.Reason = "package_initialization"
			}
			r.world.Calls = append(r.world.Calls, site)
			return true
		})
		// Package values are a dependency even without a call. Until their
		// initializer/data-flow graph is modeled, expose the missing edge.
		for id, obj := range info.Uses {
			switch obj.(type) {
			case *types.Const, *types.Var:
			default:
				continue
			}
			if obj.Pkg() == nil || obj.Parent() != obj.Pkg().Scope() {
				continue
			}
			a := l.fset.PositionFor(id.Pos(), false)
			if a.Filename != filename {
				continue
			}
			r.world.Calls = append(r.world.Calls, reviewCall{Caller: owner(id.Pos()), File: relative, Start: a.Offset, End: l.fset.PositionFor(id.End(), false).Offset, Line: a.Line, Spelling: id.Name, Relation: "uses_value", Reason: "unsupported_package_value_dependency"})
		}
		// Named type references retain contracts without pretending to be calls.
		for id, obj := range info.Uses {
			if _, ok := obj.(*types.TypeName); !ok || !valid {
				continue
			}
			a := l.fset.PositionFor(id.Pos(), false)
			if a.Filename != filename {
				continue
			}
			target := l.fset.PositionFor(obj.Pos(), false)
			if !target.IsValid() || !r.allowed(target.Filename) {
				continue
			}
			caller := owner(id.Pos())
			if caller == "" {
				continue
			}
			r.world.Calls = append(r.world.Calls, reviewCall{Caller: caller, TargetFile: r.relative(target.Filename), TargetOffset: target.Offset, TargetPackage: obj.Pkg().Path(), File: relative, Start: a.Offset, End: l.fset.PositionFor(id.End(), false).Offset, Line: a.Line, Spelling: id.Name, Relation: "uses_type"})
		}
		r.world.Symbols = append(r.world.Symbols, local...)
	}
	sort.Strings(p.Files)
	r.world.Packages = append(r.world.Packages, p)
}
func reviewCommand(dir string, env []string, args ...string) ([]byte, error) {
	cmd := exec.Command("go", args...)
	cmd.Dir = dir
	cmd.Env = env
	return cmd.Output()
}
func (r *reviewCollector) preflight(env []string) error {
	for _, root := range r.allowedRoots {
		err := filepath.WalkDir(root, func(path string, entry os.DirEntry, e error) error {
			if e != nil {
				return e
			}
			if entry.Type()&os.ModeSymlink != 0 {
				return fmt.Errorf("symlink")
			}
			if entry.IsDir() {
				return nil
			}
			if entry.Name() != "go.mod" && entry.Name() != "go.work" {
				return nil
			}
			args := []string{"mod", "edit", "-json"}
			if entry.Name() == "go.work" {
				args = []string{"work", "edit", "-json", path}
			}
			data, e := reviewCommand(filepath.Dir(path), env, args...)
			if e != nil {
				return e
			}
			var config struct {
				Replace []struct {
					New struct{ Path, Version string }
				}
				Use []struct{ DiskPath string }
			}
			if e = json.Unmarshal(data, &config); e != nil {
				return e
			}
			paths := []string{}
			for _, replace := range config.Replace {
				if replace.New.Version == "" {
					paths = append(paths, replace.New.Path)
				}
			}
			for _, use := range config.Use {
				paths = append(paths, use.DiskPath)
			}
			for _, p := range paths {
				if !filepath.IsAbs(p) {
					p = filepath.Join(filepath.Dir(path), p)
				}
				if !r.allowed(filepath.Clean(p)) {
					return fmt.Errorf("dependency outside pinned roots")
				}
			}
			return nil
		})
		if err != nil {
			return err
		}
	}
	return nil
}
func runReview(req reviewRequest) {
	root, _ := os.Getwd()
	root, _ = filepath.EvalSymlinks(root)
	result := struct {
		Version   string        `json:"analyzer"`
		Toolchain string        `json:"toolchain"`
		Worlds    []reviewWorld `json:"worlds"`
	}{Version: "graphicle-go-review-v1"}
	version, _ := exec.Command("go", "version").Output()
	result.Toolchain = strings.TrimSpace(string(version))
	var allowed []string
	for _, item := range req.Roots {
		allowed = append(allowed, filepath.Join(root, item.Directory))
	}
	env := append(os.Environ(), "GOOS="+req.GOOS, "GOARCH="+req.GOARCH)
	if req.Workspace != "" {
		env = append(env, "GOWORK="+filepath.Join(root, req.Workspace))
	}
	for _, item := range req.Roots {
		world := reviewWorld{Root: filepath.ToSlash(filepath.Join(item.Directory, item.Module)), Symbols: []reviewSymbol{}, Calls: []reviewCall{}, Packages: []reviewPackage{}, Issues: []string{}}
		collector := &reviewCollector{root: root, allowedRoots: allowed, world: &world}
		if collector.preflight(env) != nil {
			world.Issues = append(world.Issues, "unsafe_or_invalid_module_configuration")
			result.Worlds = append(result.Worlds, world)
			continue
		}
		args := []string{"list", "-e", "-deps", "-json", "-mod=readonly"}
		if req.Tests {
			args = append(args, "-test")
		}
		if len(req.Tags) > 0 {
			args = append(args, "-tags="+strings.Join(req.Tags, ","))
		}
		args = append(args, "./...")
		data, err := reviewCommand(filepath.Join(root, item.Directory, item.Module), env, args...)
		if err != nil {
			world.Issues = append(world.Issues, "package_discovery_failed")
		}
		l := &loader{fset: token.NewFileSet(), meta: map[string]*metadata{}, packages: map[string]*types.Package{}, busy: map[string]bool{}, wanted: map[string]bool{}, sizes: types.SizesFor("gc", req.GOARCH), review: collector}
		l.fallback = importer.ForCompiler(l.fset, "source", nil)
		dec := json.NewDecoder(bytes.NewReader(data))
		roots := map[string]bool{}
		for {
			var m metadata
			e := dec.Decode(&m)
			if e == io.EOF {
				break
			}
			if e != nil {
				world.Issues = append(world.Issues, "invalid_package_metadata")
				break
			}
			if m.ImportPath == "" {
				continue
			}
			if strings.HasSuffix(m.ImportPath, ".test") && m.Name == "main" {
				continue
			}
			if m.Error != nil || m.Incomplete || len(m.DepsErrors) > 0 {
				world.Issues = append(world.Issues, "incomplete_discovery:"+m.ImportPath)
			}
			l.meta[m.ImportPath] = &m
			if !m.DepOnly {
				roots[m.ImportPath] = true
			}
		}
		if l.sizes == nil {
			world.Issues = append(world.Issues, "unsupported_architecture")
		} else {
			for _, p := range sortedKeys(roots) {
				if _, err := l.Import(p); err != nil {
					world.Issues = append(world.Issues, "type_check_failed:"+p)
				}
			}
		}
		// Bind by compiler package identity and exact declaration positions.
		for i := range world.Calls {
			site := &world.Calls[i]
			if site.Reason != "" {
				continue
			}
			var targets []string
			for _, s := range world.Symbols {
				if s.Package == site.TargetPackage && s.File == site.TargetFile && s.Start <= site.TargetOffset && site.TargetOffset < s.BodyStart {
					targets = append(targets, s.Key)
				}
			}
			if len(targets) == 1 {
				site.Target = targets[0]
			} else {
				site.Reason = "ambiguous_or_missing_declaration"
			}
		}
		sort.Slice(world.Symbols, func(i, j int) bool { return world.Symbols[i].Key < world.Symbols[j].Key })
		sort.Slice(world.Calls, func(i, j int) bool {
			a, b := world.Calls[i], world.Calls[j]
			if a.File != b.File {
				return a.File < b.File
			}
			if a.Start != b.Start {
				return a.Start < b.Start
			}
			return a.Relation < b.Relation
		})
		sort.Slice(world.Packages, func(i, j int) bool { return world.Packages[i].Path < world.Packages[j].Path })
		sort.Strings(world.Issues)
		result.Worlds = append(result.Worlds, world)
	}
	if err := json.NewEncoder(os.Stdout).Encode(result); err != nil {
		panic(err)
	}
}
