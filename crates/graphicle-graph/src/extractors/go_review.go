// Native Go review facts. No reviewed code is executed and no modules are fetched.
package main

import (
	"bytes"
	"compress/gzip"
	"crypto/sha256"
	"encoding/base64"
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
	"strconv"
	"strings"
	"time"
)

// Review helpers run one request on one goroutine. Charge each interval to the
// innermost phase so recursive imports do not inflate type-check/cache timings.
// Diagnostics go only to stderr; neither facts nor persistent artifacts contain
// timing values. The Rust caller forwards only known numeric timing records.
type reviewTimings struct {
	active string
	since  time.Time
	totals map[string]time.Duration
}

var nativeReviewTimings *reviewTimings

func reviewPhase(name string) func() {
	t := nativeReviewTimings
	if t == nil {
		return func() {}
	}
	now := time.Now()
	if t.active != "" {
		t.totals[t.active] += now.Sub(t.since)
	}
	parent := t.active
	t.active, t.since = name, now
	return func() {
		now := time.Now()
		t.totals[t.active] += now.Sub(t.since)
		t.active, t.since = parent, now
	}
}

type reviewRoot struct {
	Directory string `json:"directory"`
	Module    string `json:"module"`
}
type reviewRequest struct {
	Roots         []reviewRoot      `json:"roots"`
	GOOS          string            `json:"goos"`
	GOARCH        string            `json:"goarch"`
	Tags          []string          `json:"tags"`
	Tests         bool              `json:"tests"`
	Workspace     string            `json:"workspace"`
	CacheIdentity string            `json:"cache_identity"`
	PackageCache  map[string]string `json:"package_cache"`
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

// A compiler-resolved API reference whose implementation is outside review
// ownership. It does not assert absence of callbacks or other library effects.
type reviewBoundary struct {
	Package   string `json:"package"`
	Module    string `json:"module,omitempty"`
	Version   string `json:"version,omitempty"`
	Symbol    string `json:"symbol"`
	Signature string `json:"signature"`
	Kind      string `json:"kind"`
}
type reviewCall struct {
	Boundary       *reviewBoundary `json:"boundary,omitempty"`
	Caller         string          `json:"caller"`
	Target         string          `json:"target,omitempty"`
	TargetFile     string          `json:"target_file,omitempty"`
	TargetOffset   int             `json:"target_offset"`
	TargetPackage  string          `json:"target_package,omitempty"`
	Spelling       string          `json:"spelling"`
	File           string          `json:"file"`
	Start          int             `json:"start"`
	End            int             `json:"end"`
	Line           int             `json:"line"`
	Relation       string          `json:"relation"`
	Reason         string          `json:"reason,omitempty"`
	Evidence       string          `json:"evidence,omitempty"`
	DiscardedError bool            `json:"discarded_error,omitempty"`
}
type reviewPackage struct {
	AnalysisRole  string            `json:"analysis_role"`
	EmbedPatterns []string          `json:"embed_patterns"`
	Dependencies  []string          `json:"dependencies"`
	Path          string            `json:"path"`
	Directory     string            `json:"directory"`
	Module        string            `json:"module"`
	Version       string            `json:"version"`
	Replacement   string            `json:"replacement"`
	Files         []string          `json:"files"`
	Imports       map[string]string `json:"imports"`
}
type reviewDiagnostic struct {
	Package string `json:"package"`
	Cause   string `json:"cause"`
	File    string `json:"file,omitempty"`
	Line    int    `json:"line,omitempty"`
	Message string `json:"message"`
}
type reviewWorld struct {
	PackageDiagnostics map[string]map[string]int `json:"package_diagnostics,omitempty"`
	Diagnostics        []reviewDiagnostic        `json:"diagnostics,omitempty"`
	DiagnosticCount    int                       `json:"diagnostic_count"`
	Root               string                    `json:"root"`
	Symbols            []reviewSymbol            `json:"symbols"`
	Calls              []reviewCall              `json:"calls"`
	Packages           []reviewPackage           `json:"packages"`
	Issues             []string                  `json:"issues"`
	Inputs             []reviewInput             `json:"inputs"`
}
type reviewInput struct {
	File    string `json:"file"`
	Package string `json:"package"`
	Role    string `json:"role"`
}
type reviewCollector struct {
	diagnosticCauses   map[string]int
	diagnosticPackages map[string]int
	externalErrors     map[string]bool
	root               string
	allowedRoots       []string
	world              *reviewWorld
	concreteTypes      []*types.Named
	interfaceCalls     []interfaceCall
}

// Keep actionable compiler evidence separately from per-reference limitations.
// Counts include every observed diagnostic; examples are bounded per build and
// package so one noisy package cannot consume the complete sample.
func (r *reviewCollector) diagnostic(path, cause, file string, line int, message string) {
	r.world.DiagnosticCount++
	// Keep every package's cause counts even when message examples are full.
	if r.world.PackageDiagnostics == nil {
		r.world.PackageDiagnostics = map[string]map[string]int{}
	}
	if r.world.PackageDiagnostics[path] == nil {
		r.world.PackageDiagnostics[path] = map[string]int{}
	}
	r.world.PackageDiagnostics[path][cause]++
	if r.diagnosticPackages == nil {
		r.diagnosticPackages = map[string]int{}
		r.diagnosticCauses = map[string]int{}
	}
	if len(r.world.Diagnostics) >= 32 || r.diagnosticPackages[path] >= 3 || r.diagnosticCauses[cause] >= 8 {
		return
	}
	r.diagnosticPackages[path]++
	r.diagnosticCauses[cause]++
	message = strings.ReplaceAll(message, r.root+string(filepath.Separator), "")
	chars := []rune(message)
	if len(chars) > 512 {
		message = string(chars[:512]) + "…"
	}
	r.world.Diagnostics = append(r.world.Diagnostics, reviewDiagnostic{Package: path, Cause: cause, File: file, Line: line, Message: message})
}

type interfaceCall struct {
	index    int
	contract *types.Interface
	method   *types.Func
}

// Only compiler-typed error results discarded by this call site get priority.
// This is source evidence, not a claim that the call fails at runtime.
func discardedErrors(file *ast.File, info *types.Info) map[*ast.CallExpr]bool {
	out := map[*ast.CallExpr]bool{}
	check := func(expr ast.Expr, lhs []ast.Expr) {
		call, ok := unparen(expr).(*ast.CallExpr)
		if !ok {
			return
		}
		sig, ok := info.TypeOf(call.Fun).(*types.Signature)
		if !ok || (lhs != nil && len(lhs) != sig.Results().Len()) {
			return
		}
		for i := 0; i < sig.Results().Len(); i++ {
			if !types.Identical(sig.Results().At(i).Type(), types.Universe.Lookup("error").Type()) {
				continue
			}
			if lhs == nil {
				out[call] = true
				continue
			}
			if id, ok := lhs[i].(*ast.Ident); ok && id.Name == "_" {
				out[call] = true
			}
		}
	}
	ast.Inspect(file, func(n ast.Node) bool {
		switch n := n.(type) {
		case *ast.AssignStmt:
			if len(n.Rhs) == 1 {
				check(n.Rhs[0], n.Lhs)
			}
		case *ast.ValueSpec:
			if len(n.Values) == 1 {
				lhs := make([]ast.Expr, len(n.Names))
				for i, name := range n.Names {
					lhs[i] = name
				}
				check(n.Values[0], lhs)
			}
		case *ast.ExprStmt:
			check(n.X, nil)
		case *ast.GoStmt:
			check(n.Call, nil)
		case *ast.DeferStmt:
			check(n.Call, nil)
		}
		return true
	})
	return out
}

// A method set proves that a type can implement the entire interface, not that
// it is the runtime receiver. Keep the unresolved call as well as may_call
// candidates, confined to this consumer build and admitted source roots.
func (r *reviewCollector) expandInterfaces(l *loader) {
	type implementation struct {
		receiver types.Type
		method   *types.Func
	}
	wanted := map[string]bool{}
	for _, call := range r.interfaceCalls {
		wanted[call.method.Id()] = true
	}
	if len(wanted) == 0 {
		return
	}
	sort.Slice(r.concreteTypes, func(i, j int) bool {
		return r.concreteTypes[i].Obj().Pkg().Path()+"."+r.concreteTypes[i].Obj().Name() < r.concreteTypes[j].Obj().Pkg().Path()+"."+r.concreteTypes[j].Obj().Name()
	})
	index := map[string][]implementation{}
	for _, named := range r.concreteTypes {
		if named.TypeParams().Len() != 0 {
			continue
		}
		for _, receiver := range []types.Type{named, types.NewPointer(named)} {
			set := types.NewMethodSet(receiver)
			for i := 0; i < set.Len(); i++ {
				method := set.At(i).Obj().(*types.Func)
				if !wanted[method.Id()] {
					continue
				}
				sig := method.Type().(*types.Signature)
				if recv := sig.Recv(); recv != nil {
					if _, dynamic := recv.Type().Underlying().(*types.Interface); dynamic {
						continue
					}
				}
				p := l.fset.PositionFor(method.Origin().Pos(), false)
				if (p.IsValid() && r.allowed(p.Filename)) || r.dependencyObject(l, method) {
					index[method.Id()] = append(index[method.Id()], implementation{receiver, method})
				}
			}
		}
	}
	// Prioritize discarded error contracts within the owned call sites.
	// Within either priority group, order is stable source order.
	sort.Slice(r.interfaceCalls, func(i, j int) bool {
		a, b := r.world.Calls[r.interfaceCalls[i].index], r.world.Calls[r.interfaceCalls[j].index]
		if a.DiscardedError != b.DiscardedError {
			return a.DiscardedError
		}
		if a.File != b.File {
			return a.File < b.File
		}
		return a.Start < b.Start
	})
	type lookup struct {
		contract *types.Interface
		method   string
	}
	type resolution struct {
		targets []*types.Func
		limited bool
	}
	cache := map[lookup]resolution{}
	checks, emitted := 0, 0
	for _, call := range r.interfaceCalls {
		key := lookup{call.contract, call.method.Id()}
		found, ok := cache[key]
		if !ok {
			seen := map[*types.Func]bool{}
			for _, candidate := range index[key.method] {
				if checks >= 200_000 {
					found.limited = true
					break
				}
				checks++
				if !types.Implements(candidate.receiver, call.contract) {
					continue
				}
				pos := candidate.method.Origin()
				if seen[pos] {
					continue
				}
				seen[pos] = true
				if len(found.targets) >= 128 {
					found.limited = true
					break
				}
				found.targets = append(found.targets, candidate.method)
			}
			cache[key] = found
		}
		if found.limited {
			r.world.Calls[call.index].Reason = "interface_candidate_budget"
		}
		for _, method := range found.targets {
			if emitted >= 20_000 {
				r.world.Calls[call.index].Reason = "interface_candidate_budget"
				break
			}
			candidate := r.world.Calls[call.index]
			candidate.Relation, candidate.Reason, candidate.Evidence = "may_call", "", "interface_implementation"
			r.bind(l, method.Origin(), &candidate)
			r.world.Calls = append(r.world.Calls, candidate)
			emitted++
		}
	}
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

// Vendoring admits compiler inputs, not ownership of their implementations.
// Test DepOnly is deliberately not used: first-party internal packages and
// explicitly pinned local replacements still receive full analysis.
func (r *reviewCollector) owns(path string) bool {
	for _, root := range r.allowedRoots {
		relative, err := filepath.Rel(root, path)
		if err != nil || relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
			continue
		}
		vendor := false
		for _, part := range strings.Split(filepath.ToSlash(relative), "/") {
			if part == "vendor" {
				vendor = true
				break
			}
		}
		if !vendor {
			return true
		}
	}
	return false
}

func (r *reviewCollector) bind(l *loader, obj types.Object, site *reviewCall) {
	pos := l.fset.PositionFor(obj.Pos(), false)
	if obj.Pkg() != nil {
		site.TargetPackage = obj.Pkg().Path()
		if m := l.meta[site.TargetPackage]; m != nil && (m.Standard || !r.owns(m.Dir)) {
			boundary := &reviewBoundary{Package: site.TargetPackage, Symbol: obj.Name(),
				Signature: types.ObjectString(obj, func(p *types.Package) string { return p.Path() }), Kind: "vendored"}
			if m.Standard {
				boundary.Kind = "standard_library"
			}
			if m.Module != nil {
				boundary.Module, boundary.Version = m.Module.Path, m.Module.Version
			}
			site.Boundary, site.Reason = boundary, ""
			// External APIs have identities, not owned declaration spans. Cached
			// export data intentionally carries no source positions.
			site.TargetFile, site.TargetOffset = "", 0
			return
		}
	}
	if pos.IsValid() && r.owns(pos.Filename) {
		site.TargetFile, site.TargetOffset, site.Reason = r.relative(pos.Filename), pos.Offset, ""
	} else {
		site.Reason = "outside_pinned_scope"
	}
}

func formatted(fs *token.FileSet, n ast.Node) []byte {
	var b bytes.Buffer
	if n == nil {
		return nil
	}
	_ = format.Node(&b, fs, n)
	return b.Bytes()
}

// Resolve only local values with one initializing write and no address escape.
// Parameters, fields, package variables and reassigned/captured writes stay unknown.
func localValueBindings(files []*ast.File, info *types.Info) map[types.Object]ast.Expr {
	values := map[types.Object]ast.Expr{}
	writes := map[types.Object]int{}
	escaped := map[types.Object]bool{}
	object := func(id *ast.Ident) types.Object {
		if obj := info.Defs[id]; obj != nil {
			return obj
		}
		return info.Uses[id]
	}
	write := func(id *ast.Ident, value ast.Expr, initializes bool) {
		obj := object(id)
		v, ok := obj.(*types.Var)
		if !ok || v.Pkg() == nil || v.Parent() == v.Pkg().Scope() {
			return
		}
		writes[obj]++
		if initializes && value != nil {
			values[obj] = value
		}
	}
	for _, file := range files {
		ast.Inspect(file, func(n ast.Node) bool {
			switch n := n.(type) {
			case *ast.ValueSpec:
				for i, id := range n.Names {
					var value ast.Expr
					if len(n.Values) == len(n.Names) {
						value = n.Values[i]
					}
					write(id, value, true)
				}
			case *ast.AssignStmt:
				for i, lhs := range n.Lhs {
					if id, ok := unparen(lhs).(*ast.Ident); ok {
						var value ast.Expr
						if len(n.Lhs) == len(n.Rhs) {
							value = n.Rhs[i]
						}
						write(id, value, info.Defs[id] != nil)
					}
				}
			case *ast.RangeStmt:
				for _, lhs := range []ast.Expr{n.Key, n.Value} {
					if lhs != nil {
						if id, ok := unparen(lhs).(*ast.Ident); ok {
							write(id, nil, false)
						}
					}
				}
			case *ast.IncDecStmt:
				if id, ok := unparen(n.X).(*ast.Ident); ok {
					write(id, nil, false)
				}
			case *ast.UnaryExpr:
				if n.Op == token.AND {
					if id, ok := unparen(n.X).(*ast.Ident); ok {
						escaped[object(id)] = true
					}
				}
			}
			return true
		})
	}
	for obj := range values {
		if writes[obj] != 1 || escaped[obj] {
			delete(values, obj)
		}
	}
	return values
}
func boundExpression(expr ast.Expr, bindings map[types.Object]ast.Expr, info *types.Info) ast.Expr {
	seen := map[types.Object]bool{}
	for {
		expr = unparen(expr)
		switch e := expr.(type) {
		case *ast.Ident:
			obj := info.Uses[e]
			value := bindings[obj]
			if value == nil || seen[obj] {
				return expr
			}
			seen[obj] = true
			expr = value
		case *ast.CallExpr:
			// Converting a known value to an interface does not change its
			// concrete dynamic type. Other calls are not statically executed.
			if tv, ok := info.Types[e.Fun]; ok && tv.IsType() && len(e.Args) == 1 {
				if _, ok := tv.Type.Underlying().(*types.Interface); ok {
					expr = e.Args[0]
					continue
				}
			}
			return expr
		default:
			return expr
		}
	}
}

// Exact dynamic receiver types, separate from the may-call constraint graph.
// No reviewed code is evaluated. Local aggregate storage is usable only when
// every use loads a field value without exposing or writing its storage.
type receiverFlow struct {
	info      *types.Info
	bindings  map[types.Object]ast.Expr
	unsafe    map[types.Object]bool
	returns   map[*types.Func][]ast.Expr
	summaries map[*types.Func]types.Type
}

func exactReceiverFlow(files []*ast.File, info *types.Info, bindings map[types.Object]ast.Expr) *receiverFlow {
	f := &receiverFlow{info: info, bindings: bindings, unsafe: map[types.Object]bool{},
		returns: map[*types.Func][]ast.Expr{}, summaries: map[*types.Func]types.Type{}}
	parents := map[ast.Node]ast.Node{}
	for _, file := range files {
		var stack []ast.Node
		ast.Inspect(file, func(n ast.Node) bool {
			if n == nil {
				stack = stack[:len(stack)-1]
				return false
			}
			if len(stack) > 0 {
				parents[n] = stack[len(stack)-1]
			}
			stack = append(stack, n)
			return true
		})
		for _, decl := range file.Decls {
			d, ok := decl.(*ast.FuncDecl)
			if !ok || d.Recv != nil || d.Body == nil {
				continue
			}
			fn, ok := info.Defs[d.Name].(*types.Func)
			if !ok {
				continue
			}
			sig := fn.Type().(*types.Signature)
			if sig.TypeParams().Len() != 0 || sig.Results().Len() != 1 || sig.Results().At(0).Name() != "" {
				continue
			}
			valid := true
			var results []ast.Expr
			ast.Inspect(d.Body, func(n ast.Node) bool {
				switch n := n.(type) {
				case *ast.FuncLit:
					return false
				case *ast.DeferStmt:
					valid = false
				case *ast.ReturnStmt:
					if len(n.Results) != 1 {
						valid = false
					} else {
						results = append(results, n.Results[0])
					}
				}
				return true
			})
			if valid && len(results) > 0 {
				f.returns[fn] = results
			}
		}
	}
	for id, obj := range info.Uses {
		if bindings[obj] != nil && !fieldValueUse(id, parents, info) {
			f.unsafe[obj] = true
		}
	}
	return f
}

func interfaceMethod(obj types.Object) (*types.Interface, bool) {
	fn, ok := obj.(*types.Func)
	if !ok {
		return nil, false
	}
	recv := fn.Type().(*types.Signature).Recv()
	if recv == nil {
		return nil, false
	}
	contract, ok := recv.Type().Underlying().(*types.Interface)
	return contract, ok
}

func fieldValueUse(id *ast.Ident, parents map[ast.Node]ast.Node, info *types.Info) bool {
	var current ast.Node = id
	field := false
	for {
		switch p := parents[current].(type) {
		case *ast.ParenExpr:
			current = p
		case *ast.StarExpr:
			current = p
		case *ast.SelectorExpr:
			if p.X != current {
				return false
			}
			sel := info.Selections[p]
			if sel == nil {
				return false
			}
			if sel.Kind() != types.FieldVal {
				// Loading an interface method copies its receiver value. Calling
				// a concrete holder method can take &holder implicitly and mutate it.
				_, dynamic := interfaceMethod(sel.Obj())
				return dynamic && (field || len(sel.Index()) > 1)
			}
			field, current = true, p
		default:
			if !field {
				return false
			}
			switch p := parents[current].(type) {
			case *ast.UnaryExpr:
				if p.Op == token.AND {
					return false
				}
			case *ast.AssignStmt:
				for _, lhs := range p.Lhs {
					if lhs == current {
						return false
					}
				}
			case *ast.RangeStmt:
				if p.Key == current || p.Value == current {
					return false
				}
			case *ast.IncDecStmt:
				return false
			}
			expr, ok := current.(ast.Expr)
			if !ok || info.TypeOf(expr) == nil {
				return false
			}
			switch info.TypeOf(expr).Underlying().(type) {
			case *types.Interface, *types.Basic:
				return true
			default:
				return false // aggregate/closure aliases may expose storage
			}
		}
	}
}

func receiverStep(remaining *int) bool {
	*remaining--
	return *remaining >= 0
}

// Project compiler-selected fields from fresh literals or unexposed local
// storage. Never pool fields by name across different objects or call contexts.
func (f *receiverFlow) field(expr ast.Expr, indexes []int, remaining *int) ast.Expr {
	if expr == nil || !receiverStep(remaining) {
		return nil
	}
	if len(indexes) == 0 {
		return expr
	}
	switch e := unparen(expr).(type) {
	case *ast.Ident:
		obj := f.info.Uses[e]
		if !f.unsafe[obj] {
			return f.field(f.bindings[obj], indexes, remaining)
		}
	case *ast.UnaryExpr:
		if e.Op == token.AND {
			return f.field(e.X, indexes, remaining)
		}
	case *ast.StarExpr:
		return f.field(e.X, indexes, remaining)
	case *ast.SelectorExpr:
		if sel := f.info.Selections[e]; sel != nil && sel.Kind() == types.FieldVal {
			return f.field(f.field(e.X, sel.Index(), remaining), indexes, remaining)
		}
	case *ast.CompositeLit:
		t := f.info.TypeOf(e)
		if t == nil {
			return nil
		}
		if ptr, ok := t.(*types.Pointer); ok {
			t = ptr.Elem()
		}
		st, ok := t.Underlying().(*types.Struct)
		if !ok || indexes[0] >= st.NumFields() {
			return nil
		}
		for i, item := range e.Elts {
			if pair, ok := item.(*ast.KeyValueExpr); ok {
				if key, ok := pair.Key.(*ast.Ident); ok && key.Name == st.Field(indexes[0]).Name() {
					return f.field(pair.Value, indexes[1:], remaining)
				}
			} else if i == indexes[0] {
				return f.field(item, indexes[1:], remaining)
			}
		}
	}
	return nil
}

func (f *receiverFlow) actual(expr ast.Expr, remaining *int, active map[*types.Func]bool) types.Type {
	if expr == nil || !receiverStep(remaining) {
		return nil
	}
	expr = unparen(expr)
	t := f.info.TypeOf(expr)
	if t == nil || t == types.Typ[types.UntypedNil] {
		return nil
	}
	if _, parameter := types.Unalias(t).(*types.TypeParam); parameter {
		return nil
	}
	if _, dynamic := t.Underlying().(*types.Interface); !dynamic {
		return t
	}
	switch e := expr.(type) {
	case *ast.Ident:
		return f.actual(f.bindings[f.info.Uses[e]], remaining, active)
	case *ast.SelectorExpr:
		if sel := f.info.Selections[e]; sel != nil && sel.Kind() == types.FieldVal {
			return f.actual(f.field(e.X, sel.Index(), remaining), remaining, active)
		}
	case *ast.CallExpr:
		if tv := f.info.Types[e.Fun]; tv.IsType() && len(e.Args) == 1 {
			return f.actual(e.Args[0], remaining, active)
		}
		// Only direct owned function declarations in this package have summaries.
		id, ok := unparen(e.Fun).(*ast.Ident)
		if !ok {
			return nil
		}
		fn, ok := f.info.Uses[id].(*types.Func)
		if !ok || active[fn] || len(f.returns[fn]) == 0 {
			return nil
		}
		if result, ok := f.summaries[fn]; ok {
			return result
		}
		active[fn] = true
		defer delete(active, fn)
		var result types.Type
		for _, value := range f.returns[fn] {
			actual := f.actual(value, remaining, active)
			if actual == nil || (result != nil && !types.Identical(result, actual)) {
				return nil
			}
			result = actual
		}
		f.summaries[fn] = result
		return result
	}
	return nil
}

func (f *receiverFlow) receiver(selector *ast.SelectorExpr) types.Type {
	remaining := 256
	selection := f.info.Selections[selector]
	indexes := selection.Index()
	expr := f.field(selector.X, indexes[:len(indexes)-1], &remaining)
	return f.actual(expr, &remaining, map[*types.Func]bool{})
}

func (r *reviewCollector) packageRecord(m *metadata) reviewPackage {
	p := reviewPackage{EmbedPatterns: append(append(append([]string{}, m.EmbedPatterns...), m.TestEmbedPatterns...), m.XTestEmbedPatterns...), Dependencies: m.Deps, Path: m.ImportPath, Directory: r.relative(m.Dir), Files: []string{}, Imports: map[string]string{}}
	p.AnalysisRole = "dependency"
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
	return p
}

func (r *reviewCollector) collectDependency(m *metadata, valid bool) {
	if !valid {
		r.world.Issues = append(r.world.Issues, "incomplete_package:"+m.ImportPath)
	}
	p := r.packageRecord(m)
	for _, name := range m.GoFiles {
		file := filepath.Join(m.Dir, name)
		if !r.allowed(file) {
			r.world.Issues = append(r.world.Issues, "source_outside_snapshot")
			continue
		}
		p.Files = append(p.Files, r.relative(file))
	}
	sort.Strings(p.Files)
	r.world.Packages = append(r.world.Packages, p)
}

func (r *reviewCollector) collect(l *loader, m *metadata, files []*ast.File, info *types.Info, valid bool) {
	defer reviewPhase("collect")()
	owned := r.owns(m.Dir)
	if !owned {
		r.collectDependency(m, valid)
		return
	}
	if valid && owned {
		for _, obj := range info.Defs {
			if obj == nil || obj.Pkg() == nil || obj.Parent() != obj.Pkg().Scope() {
				continue
			}
			if name, ok := obj.(*types.TypeName); ok && !name.IsAlias() {
				if named, ok := name.Type().(*types.Named); ok {
					if _, dynamic := named.Underlying().(*types.Interface); !dynamic {
						r.concreteTypes = append(r.concreteTypes, named)
					}
				}
			}
		}
	}
	if !valid {
		r.world.Issues = append(r.world.Issues, "incomplete_package:"+m.ImportPath)
	}
	var bindings map[types.Object]ast.Expr
	var flow *callableFlow
	var receivers *receiverFlow
	if owned {
		bindings = localValueBindings(files, info)
		flow = functionValueFlow(files, info, bindings)
		receivers = exactReceiverFlow(files, info, bindings)
	}
	p := r.packageRecord(m)
	p.AnalysisRole = "owned"
	for _, f := range files {
		filename := l.fset.PositionFor(f.Pos(), false).Filename
		if !r.allowed(filename) {
			r.world.Issues = append(r.world.Issues, "source_outside_snapshot")
			continue
		}
		relative := r.relative(filename)
		p.Files = append(p.Files, relative)
		if !owned {
			continue
		}
		source, err := os.ReadFile(filename)
		if err != nil {
			r.world.Issues = append(r.world.Issues, "unreadable_source")
			continue
		}
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
		topLevel := map[ast.Decl]bool{}
		for _, declaration := range f.Decls {
			topLevel[declaration] = true
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
			case *ast.GenDecl:
				if topLevel[node] && (node.Tok == token.CONST || node.Tok == token.VAR) && len(node.Specs) > 0 {
					value := node.Specs[0].(*ast.ValueSpec)
					if len(value.Names) > 0 {
						id := value.Names[0]
						kind := "constant"
						if node.Tok == token.VAR {
							kind = "variable"
						}
						// Preserve the complete group, including blank initializers,
						// implicit const expressions and iota declaration order.
						add(node, m.ImportPath+"."+id.Name, kind, nil, id)
					}
				}
			}
			return true
		})
		// Compute lexical parents by position first, then assign identities in
		// AST preorder. Go permits repeated init declarations and local type
		// names in different functions: neither may overwrite another symbol.
		parents := make([]int, len(local))
		for i := range local {
			parents[i] = -1
			length := len(source) + 1
			for j := range local {
				if i != j && local[j].Start <= local[i].Start && local[i].End <= local[j].End && local[j].End-local[j].Start < length {
					parents[i] = j
					length = local[j].End - local[j].Start
				}
			}
		}
		ordinals := map[string]int{}
		duplicates := map[string]int{}
		for i := range local {
			if parent := parents[i]; parent >= 0 {
				local[i].Parent = local[parent].Key
			}
			parent := local[i].Parent
			if local[i].Kind == "closure" {
				ordinal := ordinals[parent]
				ordinals[parent]++
				local[i].Name = fmt.Sprintf("<closure#%d>", ordinal)
				local[i].Key = m.ImportPath + "|" + relative + "|" + parent + "|" + local[i].Name
			} else if parent != "" {
				local[i].Key += "|in:" + parent
			}
			key := local[i].Key
			ordinal := duplicates[key]
			duplicates[key]++
			if ordinal > 0 {
				local[i].Key = fmt.Sprintf("%s#%d", key, ordinal)
			}
		}
		// A declaration containing a closure depends on that callable even
		// when it returns/stores it instead of invoking it at this call site.
		for _, symbol := range local {
			if symbol.Kind == "closure" && symbol.Parent != "" {
				r.world.Calls = append(r.world.Calls, reviewCall{Caller: symbol.Parent, TargetFile: relative, TargetOffset: symbol.Start, TargetPackage: m.ImportPath, File: relative, Start: symbol.Start, End: symbol.End, Line: symbol.Line, Spelling: symbol.Name, Relation: "references_callable"})
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
		directReferences := map[token.Pos]bool{}
		discarded := discardedErrors(f, info)
		ast.Inspect(f, func(n ast.Node) bool {
			call, ok := n.(*ast.CallExpr)
			if !ok {
				return true
			}
			original := unparen(call.Fun)
			if index, ok := original.(*ast.IndexExpr); ok {
				original = unparen(index.X)
			}
			if index, ok := original.(*ast.IndexListExpr); ok {
				original = unparen(index.X)
			}
			switch expr := original.(type) {
			case *ast.Ident:
				directReferences[expr.Pos()] = true
			case *ast.SelectorExpr:
				directReferences[expr.Sel.Pos()] = true
			}
			position := l.fset.PositionFor(call.Pos(), false)
			site := reviewCall{Caller: owner(call.Pos()), File: relative, Start: position.Offset, End: l.fset.PositionFor(call.End(), false).Offset, Line: position.Line, Relation: "calls", Reason: "unresolved_dynamic_target"}
			site.DiscardedError = valid && discarded[call]
			fun := boundExpression(call.Fun, bindings, info)
			if index, ok := fun.(*ast.IndexExpr); ok {
				fun = unparen(index.X)
			}
			if index, ok := fun.(*ast.IndexListExpr); ok {
				fun = unparen(index.X)
			}
			site.Spelling = string(formatted(l.fset, call.Fun))
			var obj types.Object
			dynamic := false
			var contract *types.Interface
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
					contract, dynamic = recv.Underlying().(*types.Interface)
					if promoted, ok := interfaceMethod(obj); ok {
						contract, dynamic = promoted, true
					}
					if dynamic {
						actual := receivers.receiver(f)
						if actual != nil {
							if _, uncertain := actual.Underlying().(*types.Interface); !uncertain {
								resolved, _, _ := types.LookupFieldOrMethod(actual, true, obj.Pkg(), f.Sel.Name)
								if resolved != nil {
									obj, dynamic = resolved, false
									contract = nil
								}
							}
						}
					}
				} else {
					obj = info.Uses[f.Sel]
				}
			}
			// Resolve the standard-library object, not the spelling: import aliases
			// work and a local object named os cannot invent an environment read.
			if target, ok := obj.(*types.Func); ok && valid && !dynamic && target.Pkg() != nil && target.Pkg().Path() == "os" {
				name := target.Name()
				if name == "Getenv" || name == "LookupEnv" || name == "Environ" || name == "Setenv" || name == "Unsetenv" || name == "Clearenv" {
					env := site
					env.Relation, env.Reason, env.Spelling = "reads_environment", "", ""
					env.Evidence = "go/types:os." + name
					if name == "Setenv" || name == "Unsetenv" || name == "Clearenv" {
						env.Relation = "writes_environment"
					}
					if len(call.Args) > 0 {
						if literal, ok := unparen(call.Args[0]).(*ast.BasicLit); ok && literal.Kind == token.STRING {
							env.Spelling, _ = strconv.Unquote(literal.Value)
						}
					}
					r.world.Calls = append(r.world.Calls, env)
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
					contract, dynamic = recv.Type().Underlying().(*types.Interface)
				}
				if !dynamic {
					r.bind(l, target.Origin(), &site)
				}
			}
			if literal, ok := fun.(*ast.FuncLit); ok && valid {
				site.TargetPackage, site.TargetFile = m.ImportPath, relative
				site.TargetOffset = l.fset.PositionFor(literal.Pos(), false).Offset
				site.Reason = ""
			}
			if !valid {
				site.Reason = "incomplete_package"
			}
			if site.Caller == "" {
				site.Reason = "package_initialization"
			}
			if valid && site.Caller != "" && dynamic && contract != nil {
				if method, ok := obj.(*types.Func); ok {
					site.Reason = "interface_dispatch"
					r.interfaceCalls = append(r.interfaceCalls, interfaceCall{len(r.world.Calls), contract, method})
				}
			} else if valid && site.Caller != "" && site.TargetFile == "" && site.Boundary == nil {
				candidates, limited := flow.candidates(call.Fun)
				if limited {
					site.Reason = "function_value_budget"
				}
				seen := map[token.Pos]bool{}
				seenFunctions := map[*types.Func]bool{}
				for _, candidate := range candidates {
					var pos token.Pos
					pkg := m.ImportPath
					fn := callableTarget(candidate, info)
					if fn != nil {
						pos = fn.Origin().Pos()
						if fn.Pkg() != nil {
							pkg = fn.Pkg().Path()
						}
					} else if literal, ok := candidate.(*ast.FuncLit); ok {
						pos = literal.Pos()
					}
					p := l.fset.PositionFor(pos, false)
					if fn != nil && r.dependencyObject(l, fn) {
						if seenFunctions[fn.Origin()] {
							continue
						}
						seenFunctions[fn.Origin()] = true
					} else {
						if !p.IsValid() || !r.allowed(p.Filename) || seen[pos] {
							continue
						}
						seen[pos] = true
					}
					possible := site
					possible.Relation = "may_call"
					possible.Reason = ""
					possible.TargetFile, possible.TargetOffset, possible.TargetPackage = r.relative(p.Filename), p.Offset, pkg
					if fn != nil {
						r.bind(l, fn.Origin(), &possible)
					}
					r.world.Calls = append(r.world.Calls, possible)
				}
			}
			r.world.Calls = append(r.world.Calls, site)
			return true
		})
		// Package values and their initializers are declaration dependencies.
		// Keep a distinct relation: a value reference is not a function call.
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
			site := reviewCall{Caller: owner(id.Pos()), File: relative, Start: a.Offset, End: l.fset.PositionFor(id.End(), false).Offset, Line: a.Line, Spelling: id.Name, Relation: "uses_value"}
			if !valid {
				site.Reason = "incomplete_package"
			} else {
				r.bind(l, obj, &site)
			}
			if site.Caller == "" {
				site.Reason = "package_initialization"
			}
			r.world.Calls = append(r.world.Calls, site)
		}

		// Passing or storing a function is a dependency without claiming a
		// runtime call. This connects callbacks and global function values to
		// their declarations even when invocation remains dynamic.
		for id, obj := range info.Uses {
			fn, ok := obj.(*types.Func)
			if !ok || directReferences[id.Pos()] {
				continue
			}
			a := l.fset.PositionFor(id.Pos(), false)
			if a.Filename != filename {
				continue
			}
			caller := owner(id.Pos())
			if caller == "" {
				continue
			}
			site := reviewCall{Caller: caller, File: relative, Start: a.Offset, End: l.fset.PositionFor(id.End(), false).Offset, Line: a.Line, Spelling: id.Name, Relation: "references_callable"}
			if !valid {
				site.Reason = "incomplete_package"
			} else {
				r.bind(l, fn.Origin(), &site)
			}
			r.world.Calls = append(r.world.Calls, site)
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
			if (!target.IsValid() || !r.allowed(target.Filename)) && !r.dependencyObject(l, obj) {
				continue
			}
			caller := owner(id.Pos())
			if caller == "" {
				continue
			}
			site := reviewCall{Caller: caller, File: relative, Start: a.Offset, End: l.fset.PositionFor(id.End(), false).Offset, Line: a.Line, Spelling: id.Name, Relation: "uses_type"}
			r.bind(l, obj, &site)
			r.world.Calls = append(r.world.Calls, site)
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

// Dependency failures describe the dependency that failed, not a discovery
// failure in every importer. Unknown/local failures remain conservative; the
// independent go/types result must still succeed before a package is valid.
func reviewLocalPackage(path string, all map[string]*metadata, allowed func(string) bool) bool {
	// ./... can return an error-only root (for example an unrendered template)
	// without a directory or module. It is a local discovery failure, not a
	// missing external dependency. DepOnly distinguishes imported packages.
	if m := all[path]; m != nil && !m.DepOnly {
		return true
	}
	for _, m := range all {
		if m.Module != nil && allowed(m.Module.Dir) && (path == m.Module.Path || strings.HasPrefix(path, m.Module.Path+"/")) {
			return true
		}
	}
	return false
}

// go list copies a dependency's Error into importers' DepsErrors. ImportStack
// can end at the importer, so resolve the complete diagnostic to its owner;
// never infer the missing package by parsing error prose.
func reviewErrorKey(problem packageError) string {
	value, _ := json.Marshal(problem)
	return string(value)
}
func reviewExternalErrors(all map[string]*metadata, allowed func(string) bool) map[string]bool {
	errors := map[string]bool{}
	for path, m := range all {
		if m.Error == nil {
			continue
		}
		key := reviewErrorKey(*m.Error)
		external := !m.Standard && !allowed(m.Dir) && !reviewLocalPackage(path, all, allowed)
		previous, seen := errors[key]
		errors[key] = external && (!seen || previous)
	}
	return errors
}
func reviewDiscoveryIncomplete(m *metadata, externalErrors map[string]bool) bool {
	if m.Error != nil || (m.Incomplete && len(m.DepsErrors) == 0) {
		return true
	}
	for _, problem := range m.DepsErrors {
		if !externalErrors[reviewErrorKey(problem)] {
			return true
		}
	}
	return false
}

func runReview(req reviewRequest) {
	if value := os.Getenv("GRAPHICLE_PHASE_TIMINGS"); value != "" && value != "0" {
		nativeReviewTimings = &reviewTimings{totals: map[string]time.Duration{}}
		done := reviewPhase("other")
		defer func() {
			done()
			names := make([]string, 0, len(nativeReviewTimings.totals))
			for name := range nativeReviewTimings.totals {
				names = append(names, name)
			}
			sort.Strings(names)
			for _, name := range names {
				fmt.Fprintf(os.Stderr, "go_phase=%s ns=%d\n", name, nativeReviewTimings.totals[name].Nanoseconds())
			}
			nativeReviewTimings = nil
		}()
	}
	root, _ := os.Getwd()
	root, _ = filepath.EvalSymlinks(root)
	result := struct {
		Version   string        `json:"analyzer"`
		Toolchain string        `json:"toolchain"`
		Worlds    []reviewWorld `json:"worlds"`
	}{Version: "graphicle-go-review-v15"}
	version, _ := exec.Command("go", "version").Output()
	result.Toolchain = strings.TrimSpace(string(version))
	var allowed []string
	seenRoots := map[string]bool{}
	for _, item := range req.Roots {
		directory := filepath.Join(root, item.Directory)
		if !seenRoots[directory] {
			allowed = append(allowed, directory)
			seenRoots[directory] = true
		}
	}
	env := append(os.Environ(), "GOOS="+req.GOOS, "GOARCH="+req.GOARCH)
	if req.Workspace != "" {
		env = append(env, "GOWORK="+filepath.Join(root, req.Workspace))
	}
	// All compiler worlds share this immutable materialized tree and environment.
	// Check the complete pin set once, retaining the same fail-closed behavior for
	// every world. A multi-module repository must not rescan every pin per module.
	donePreflight := reviewPhase("preflight")
	preflight := (&reviewCollector{root: root, allowedRoots: allowed}).preflight(env)
	cacheRequest := req
	if preflight != nil {
		cacheRequest.CacheIdentity = ""
	}
	packageCache := newReviewPackageCache(cacheRequest, root)
	donePreflight()
	for _, item := range req.Roots {
		world := reviewWorld{Root: filepath.ToSlash(filepath.Join(item.Directory, item.Module)), Symbols: []reviewSymbol{}, Calls: []reviewCall{}, Packages: []reviewPackage{}, Issues: []string{}, Inputs: []reviewInput{}}
		collector := &reviewCollector{root: root, allowedRoots: allowed, world: &world}
		if preflight != nil {
			world.Issues = append(world.Issues, "unsafe_or_invalid_module_configuration")
			result.Worlds = append(result.Worlds, world)
			continue
		}
		// Snapshot provenance comes from the authorized pins, never ambient Git
		// metadata. VCS stamping is a binary-build concern, not a type input.
		args := []string{"list", "-buildvcs=false", "-e", "-deps", "-json", moduleMode(filepath.Join(root, item.Directory, item.Module), func() string {
			if req.Workspace != "" {
				return filepath.Join(root, req.Workspace)
			}
			return "off"
		}())}
		if req.Tests {
			args = append(args, "-test")
		}
		if len(req.Tags) > 0 {
			args = append(args, "-tags="+strings.Join(req.Tags, ","))
		}
		args = append(args, "./...")
		doneDiscovery := reviewPhase("discovery")
		data, err := reviewCommand(filepath.Join(root, item.Directory, item.Module), env, args...)
		doneDiscovery()
		if err != nil {
			world.Issues = append(world.Issues, "package_discovery_failed")
		}
		doneMetadata := reviewPhase("metadata")
		packageCache.world()
		l := &loader{fset: token.NewFileSet(), meta: map[string]*metadata{}, packages: map[string]*types.Package{}, busy: map[string]bool{}, wanted: map[string]bool{}, sizes: types.SizesFor("gc", req.GOARCH), review: collector, cache: packageCache}
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
			if !m.Standard && collector.allowed(m.Dir) {
				addInputs := func(names []string, role string) {
					for _, name := range names {
						world.Inputs = append(world.Inputs, reviewInput{File: collector.relative(filepath.Join(m.Dir, name)), Package: m.ImportPath, Role: role})
					}
				}
				addInputs(m.GoFiles, "selected")
				addInputs(m.IgnoredGoFiles, "excluded_by_profile")
				addInputs(m.CgoFiles, "cgo")
				addInputs(m.EmbedFiles, "embed")
				if req.Tests {
					addInputs(m.TestGoFiles, "selected")
					addInputs(m.XTestGoFiles, "selected")
					addInputs(m.TestEmbedFiles, "embed")
					addInputs(m.XTestEmbedFiles, "embed")
				} else {
					addInputs(m.TestGoFiles, "excluded_test")
					addInputs(m.XTestGoFiles, "excluded_test")
				}
			}
			l.meta[m.ImportPath] = &m
			// Error-only go list records have no source to type-check. Preserve
			// their discovery diagnostic below instead of inventing another failure.
			if !m.DepOnly && !(m.Error != nil && m.Dir == "") {
				roots[m.ImportPath] = true
			}
		}
		collector.externalErrors = reviewExternalErrors(l.meta, collector.allowed)
		metadataPaths := make([]string, 0, len(l.meta))
		for path := range l.meta {
			metadataPaths = append(metadataPaths, path)
		}
		sort.Strings(metadataPaths)
		for _, path := range metadataPaths {
			m := l.meta[path]
			if m.Error != nil {
				collector.diagnostic(path, "discovery_error", "", 0, m.Error.Err)
			}
			if len(m.CgoFiles) > 0 {
				collector.diagnostic(path, "unsupported_cgo", "", 0, "Selected package requires cgo")
			}
			if !m.Standard && !collector.allowed(m.Dir) && !reviewLocalPackage(path, l.meta, collector.allowed) {
				world.Issues = append(world.Issues, "unavailable_external_package:"+path)
			} else if reviewDiscoveryIncomplete(m, collector.externalErrors) {
				world.Issues = append(world.Issues, "incomplete_discovery:"+path)
			}
		}
		doneMetadata()
		if l.sizes == nil {
			world.Issues = append(world.Issues, "unsupported_architecture")
		} else {
			for _, p := range sortedKeys(roots) {
				if _, err := l.Import(p); err != nil {
					world.Issues = append(world.Issues, "type_check_failed:"+p)
				}
			}
		}
		doneLink := reviewPhase("link")
		collector.expandInterfaces(l)
		// Index declaration intervals per compiler package/file. Looking through
		// every symbol for every call is quadratic on generated dependencies.
		declarations := map[string][]reviewSymbol{}
		for _, symbol := range world.Symbols {
			key := symbol.Package + "\x00" + symbol.File
			declarations[key] = append(declarations[key], symbol)
		}
		for i := range world.Calls {
			site := &world.Calls[i]
			// Environment operations bind to configuration inputs in the review
			// layer, not to a Go declaration in the admitted source tree.
			if site.Reason != "" || site.Boundary != nil || site.Relation == "reads_environment" || site.Relation == "writes_environment" {
				continue
			}
			var targets []string
			width := int(^uint(0) >> 1)
			for _, s := range declarations[site.TargetPackage+"\x00"+site.TargetFile] {
				if s.Start <= site.TargetOffset && site.TargetOffset < s.BodyStart {
					if s.End-s.Start < width {
						targets = nil
						width = s.End - s.Start
					}
					if s.End-s.Start == width {
						targets = append(targets, s.Key)
					}
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
			if a.Relation != b.Relation {
				return a.Relation < b.Relation
			}
			if a.TargetFile != b.TargetFile {
				return a.TargetFile < b.TargetFile
			}
			if a.TargetOffset != b.TargetOffset {
				return a.TargetOffset < b.TargetOffset
			}
			// Nested unresolved calls can share all the fields above. Give the
			// diagnostic examples a total order across fresh and cached runs.
			x, _ := json.Marshal(a)
			y, _ := json.Marshal(b)
			return string(x) < string(y)
		})
		sort.Slice(world.Packages, func(i, j int) bool { return world.Packages[i].Path < world.Packages[j].Path })
		sort.Strings(world.Issues)
		result.Worlds = append(result.Worlds, world)
		doneLink()
	}
	// Stream compressed native facts: source and call-site names repeat heavily.
	doneEncode := reviewPhase("encode")
	defer doneEncode()
	output := &countingWriter{writer: os.Stdout}
	fmt.Fprint(output, `{"encoding":"gzip-base64-v1","facts":"`)
	encoded := base64.NewEncoder(base64.StdEncoding, output)
	compressed := gzip.NewWriter(encoded)
	if err := json.NewEncoder(compressed).Encode(result); err != nil {
		panic(err)
	}
	if err := compressed.Close(); err != nil {
		panic(err)
	}
	if err := encoded.Close(); err != nil {
		panic(err)
	}
	fmt.Fprint(output, `"`)
	packageCache.writeResult(output, 64<<20)
	fmt.Fprintln(output, `}`)
}

// Function-value constraints are a bounded, context-insensitive may analysis.
// Unknown callers, writes and escaping storage are never treated as absent:
// candidate edges supplement (and do not erase) the unresolved invocation.
type callableFlow struct {
	values        map[types.Object][]ast.Expr
	unsafeStorage map[types.Object]bool
	info          *types.Info
	limited       bool
}

func functionValueFlow(files []*ast.File, info *types.Info, bindings map[types.Object]ast.Expr) *callableFlow {
	flow := &callableFlow{values: map[types.Object][]ast.Expr{}, unsafeStorage: map[types.Object]bool{}, info: info}
	object := func(id *ast.Ident) types.Object {
		if obj := info.Defs[id]; obj != nil {
			return obj
		}
		return info.Uses[id]
	}
	var rootObject func(ast.Expr) types.Object
	rootObject = func(expr ast.Expr) types.Object {
		switch e := unparen(expr).(type) {
		case *ast.Ident:
			return object(e)
		case *ast.SelectorExpr:
			return rootObject(e.X)
		case *ast.IndexExpr:
			return rootObject(e.X)
		case *ast.StarExpr:
			return rootObject(e.X)
		case *ast.UnaryExpr:
			return rootObject(e.X)
		}
		return nil
	}
	escape := func(expr ast.Expr) {
		if obj := rootObject(expr); obj != nil {
			flow.unsafeStorage[obj] = true
		}
	}
	putObject := func(obj types.Object, expr ast.Expr) {
		if obj == nil || expr == nil {
			return
		}
		if len(flow.values[obj]) >= 128 {
			flow.limited = true
			return
		}
		flow.values[obj] = append(flow.values[obj], expr)
	}
	put := func(id *ast.Ident, expr ast.Expr) { putObject(object(id), expr) }
	for _, file := range files {
		ast.Inspect(file, func(n ast.Node) bool {
			switch n := n.(type) {
			case *ast.ValueSpec:
				if len(n.Names) == len(n.Values) {
					for i, id := range n.Names {
						put(id, n.Values[i])
					}
				}
			case *ast.AssignStmt:
				for _, lhs := range n.Lhs {
					if _, plain := unparen(lhs).(*ast.Ident); !plain {
						escape(lhs)
					}
				}
				if len(n.Lhs) == len(n.Rhs) {
					for i, lhs := range n.Lhs {
						if id, ok := unparen(lhs).(*ast.Ident); ok {
							put(id, n.Rhs[i])
						}
					}
				}
			case *ast.UnaryExpr:
				if n.Op == token.AND {
					escape(n.X)
				}
			case *ast.ReturnStmt:
				for _, value := range n.Results {
					escape(value)
				}
			case *ast.CallExpr:
				for _, arg := range n.Args {
					escape(arg)
				}
				if sel, ok := unparen(n.Fun).(*ast.SelectorExpr); ok {
					if selection := info.Selections[sel]; selection != nil {
						if _, method := selection.Obj().(*types.Func); method {
							escape(sel.X)
						}
					}
				}
				var target *types.Func
				offset := 0
				fun := boundExpression(n.Fun, bindings, info)
				if index, ok := fun.(*ast.IndexExpr); ok {
					if t := info.TypeOf(index.X); t != nil {
						if _, ok := t.Underlying().(*types.Signature); ok {
							fun = unparen(index.X)
						}
					}
				}
				if index, ok := fun.(*ast.IndexListExpr); ok {
					fun = unparen(index.X)
				}
				switch e := fun.(type) {
				case *ast.Ident:
					target, _ = info.Uses[e].(*types.Func)
				case *ast.SelectorExpr:
					if sel := info.Selections[e]; sel != nil {
						target, _ = sel.Obj().(*types.Func)
						if sel.Kind() == types.MethodExpr {
							offset = 1
						}
					} else {
						target, _ = info.Uses[e.Sel].(*types.Func)
					}
				}
				if target == nil {
					break
				}
				sig, ok := target.Type().(*types.Signature)
				if !ok || sig.Variadic() {
					break
				}
				if sig.Params().Len()+offset != len(n.Args) {
					break
				}
				for i := 0; i < sig.Params().Len(); i++ {
					p := sig.Params().At(i)
					putObject(p, n.Args[i+offset])
				}
			}
			return true
		})
	}
	// Visit each alias once; a depth cutoff here could retain a stale target.
	queue := make([]types.Object, 0, len(flow.unsafeStorage))
	for obj := range flow.unsafeStorage {
		queue = append(queue, obj)
	}
	for i := 0; i < len(queue); i++ {
		for _, value := range flow.values[queue[i]] {
			if root := rootObject(value); root != nil && !flow.unsafeStorage[root] {
				flow.unsafeStorage[root] = true
				queue = append(queue, root)
			}
		}
	}
	return flow
}
func (flow *callableFlow) candidates(expr ast.Expr) ([]ast.Expr, bool) {
	var out []ast.Expr
	seen := map[ast.Expr]bool{}
	visits := 0
	limited := flow.limited
	var walk func(ast.Expr, int)
	walk = func(expr ast.Expr, depth int) {
		if expr == nil || seen[expr] {
			return
		}
		seen[expr] = true
		visits++
		if depth > 16 || visits > 512 || len(out) >= 64 {
			limited = true
			return
		}
		expr = unparen(expr)
		switch e := expr.(type) {
		case *ast.Ident:
			obj := flow.info.Uses[e]
			if _, ok := obj.(*types.Func); ok {
				out = append(out, e)
				return
			}
			for _, v := range flow.values[obj] {
				walk(v, depth+1)
			}
		case *ast.FuncLit:
			out = append(out, e)
		case *ast.SelectorExpr:
			var obj types.Object
			if sel := flow.info.Selections[e]; sel != nil {
				obj = sel.Obj()
				if _, ok := obj.(*types.Var); ok {
					flow.storage(e.X, e.Sel.Name, depth, walk, &visits, &limited)
					return
				}
			} else {
				obj = flow.info.Uses[e.Sel]
			}
			if fn, ok := obj.(*types.Func); ok {
				sig := fn.Type().(*types.Signature)
				if sig.Recv() != nil {
					if _, dynamic := sig.Recv().Type().Underlying().(*types.Interface); dynamic {
						return
					}
				}
				if sel := flow.info.Selections[e]; sel != nil {
					t := sel.Recv()
					if p, ok := t.(*types.Pointer); ok {
						t = p.Elem()
					}
					if _, dynamic := t.Underlying().(*types.Interface); dynamic {
						return
					}
				}
				out = append(out, e)
			}
		case *ast.IndexExpr:
			if t := flow.info.TypeOf(e.X); t != nil {
				if _, ok := t.Underlying().(*types.Signature); ok {
					walk(e.X, depth+1)
					return
				}
			}
			if v := flow.info.Types[e.Index].Value; v != nil {
				flow.storage(e.X, v.ExactString(), depth, walk, &visits, &limited)
			}
		case *ast.IndexListExpr:
			walk(e.X, depth+1)
		}
	}
	walk(expr, 0)
	return out, limited
}

// Keyed local literals: fields and constant map/array keys. No global
// field-name pooling and no unknown-index guesses. Mutations remain unresolved.
func (flow *callableFlow) storage(expr ast.Expr, key string, depth int, walk func(ast.Expr, int), visits *int, limited *bool) {
	*visits++
	// Truncated constraints could hide an alias escape or write. Do not reuse
	// literal storage when the mutation analysis could not see every binding.
	if flow.limited || depth > 16 || *visits > 512 {
		*limited = true
		return
	}
	switch e := unparen(expr).(type) {
	case *ast.Ident:
		if flow.unsafeStorage[flow.info.Uses[e]] {
			return
		}
		for _, v := range flow.values[flow.info.Uses[e]] {
			flow.storage(v, key, depth+1, walk, visits, limited)
		}
	case *ast.UnaryExpr:
		if e.Op == token.AND {
			flow.storage(e.X, key, depth+1, walk, visits, limited)
		}
	case *ast.CompositeLit:
		next := 0
		for _, item := range e.Elts {
			if pair, ok := item.(*ast.KeyValueExpr); ok {
				k := ""
				if id, ok := pair.Key.(*ast.Ident); ok {
					k = id.Name
				}
				if value := flow.info.Types[pair.Key].Value; value != nil {
					k = value.ExactString()
				}
				if k == key {
					walk(pair.Value, depth+1)
				}
				if index, err := strconv.Atoi(k); err == nil {
					next = index + 1
				}
			} else {
				if fmt.Sprint(next) == key {
					walk(item, depth+1)
				}
				next++
			}
		}
	}
}
func callableTarget(expr ast.Expr, info *types.Info) *types.Func {
	switch e := unparen(expr).(type) {
	case *ast.Ident:
		f, _ := info.Uses[e].(*types.Func)
		return f
	case *ast.SelectorExpr:
		if sel := info.Selections[e]; sel != nil {
			f, _ := sel.Obj().(*types.Func)
			return f
		}
		f, _ := info.Uses[e.Sel].(*types.Func)
		return f
	}
	return nil
}

// Count streamed facts without retaining a second copy in memory.
type countingWriter struct {
	writer io.Writer
	bytes  int64
}

func (w *countingWriter) Write(p []byte) (int, error) {
	n, err := w.writer.Write(p)
	w.bytes += int64(n)
	return n, err
}
