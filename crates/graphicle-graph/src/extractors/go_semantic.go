// Bounded source semantics. Standard-library parser and go/types only; no user code runs.
package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"go/ast"
	"go/build"
	"go/importer"
	"go/parser"
	"go/token"
	"go/types"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
)

type semanticRequest struct {
	Mode    string            `json:"mode"`
	Files   map[string]string `json:"files"`
	File    string            `json:"file"`
	Offset  int               `json:"offset"`
	NewName string            `json:"new_name"`
	GOOS    string            `json:"goos"`
	GOARCH  string            `json:"goarch"`
	Tags    []string          `json:"tags"`
}
type semanticPackage struct {
	fset  *token.FileSet
	files []*ast.File
	info  *types.Info
	pkg   *types.Package
	err   error
}

type semanticImporter struct {
	cross bool
	base  types.Importer
}

func (i semanticImporter) Import(path string) (*types.Package, error) {
	if path == "unsafe" {
		return types.Unsafe, nil
	}
	if i.cross {
		return nil, fmt.Errorf("cross-target import metadata unavailable")
	}
	pkg, err := build.Default.Import(path, "", build.FindOnly)
	if err != nil || !pkg.Goroot {
		return nil, fmt.Errorf("dependency source is outside semantic input set")
	}
	return i.base.Import(path)
}

func semanticParse(req semanticRequest) (*semanticPackage, error) {
	p := &semanticPackage{fset: token.NewFileSet(), info: &types.Info{Defs: map[*ast.Ident]types.Object{}, Uses: map[*ast.Ident]types.Object{}, Types: map[ast.Expr]types.TypeAndValue{}, Selections: map[*ast.SelectorExpr]*types.Selection{}}}
	if len(req.Files) == 0 || len(req.Files) > 128 {
		return nil, fmt.Errorf("source file limit")
	}
	ctx := build.Default
	if req.GOOS != "" {
		ctx.GOOS = req.GOOS
	}
	if req.GOARCH != "" {
		ctx.GOARCH = req.GOARCH
	}
	ctx.BuildTags = req.Tags
	ctx.CgoEnabled = false
	ctx.OpenFile = func(path string) (io.ReadCloser, error) {
		s, ok := req.Files[filepath.ToSlash(filepath.Clean(path))]
		if !ok {
			return nil, os.ErrNotExist
		}
		return io.NopCloser(strings.NewReader(s)), nil
	}
	names := []string{}
	size := 0
	for name, s := range req.Files {
		names = append(names, name)
		size += len(s)
	}
	sort.Strings(names)
	if size > 2*1024*1024 {
		return nil, fmt.Errorf("source byte limit")
	}
	targetSelected := false
	for _, name := range names {
		if filepath.Dir(name) != filepath.Dir(req.File) {
			return nil, fmt.Errorf("one package directory required")
		}
		if strings.HasSuffix(name, "_test.go") {
			continue
		}
		active, err := ctx.MatchFile(filepath.Dir(name), filepath.Base(name))
		if err != nil {
			return nil, err
		}
		if !active {
			continue
		}
		targetSelected = targetSelected || name == req.File
		f, err := parser.ParseFile(p.fset, name, req.Files[name], parser.AllErrors)
		if err != nil {
			return nil, fmt.Errorf("syntax errors in selected source")
		}
		p.files = append(p.files, f)
	}
	if !targetSelected {
		return nil, fmt.Errorf("target outside selected build")
	}
	conf := types.Config{Importer: semanticImporter{cross: ctx.GOOS != runtime.GOOS || ctx.GOARCH != runtime.GOARCH, base: importer.Default()}, Sizes: types.SizesFor("gc", ctx.GOARCH), Error: func(error) {}}
	if conf.Sizes == nil {
		return nil, fmt.Errorf("unsupported architecture")
	}
	p.pkg, p.err = conf.Check("graphicle/semantic", p.fset, p.files, p.info)
	return p, nil
}
func (p *semanticPackage) position(pos token.Pos) (string, int) {
	v := p.fset.PositionFor(pos, false)
	return filepath.ToSlash(v.Filename), v.Offset
}
func (p *semanticPackage) objectKey(obj types.Object) string {
	if obj == nil {
		return ""
	}
	if !obj.Pos().IsValid() {
		return "builtin:" + obj.Name()
	}
	file, offset := p.position(obj.Pos())
	if file == "" {
		if obj.Pkg() != nil {
			return obj.Pkg().Path() + ":" + obj.Name()
		}
		return obj.Name()
	}
	return fmt.Sprintf("%s:%d", file, offset)
}
func runSemantic(req semanticRequest) {
	var value interface{}
	p, err := semanticParse(req)
	if err != nil {
		value = map[string]interface{}{"error": err.Error(), "complete": false}
	} else if req.Mode == "flow" {
		value = statementFlow(req, p)
	} else if req.Mode == "rename" {
		value = semanticRename(req, p)
	} else {
		value = map[string]interface{}{"error": "unknown semantic operation"}
	}
	if result, ok := value.(map[string]interface{}); ok {
		goos, goarch := req.GOOS, req.GOARCH
		if goos == "" {
			goos = build.Default.GOOS
		}
		if goarch == "" {
			goarch = build.Default.GOARCH
		}
		result["analyzer"] = "go-semantic-v1"
		result["go_version"] = runtime.Version()
		result["profile"] = map[string]interface{}{"goos": goos, "goarch": goarch, "tags": req.Tags, "cgo": false, "tests": false}
	}
	if err := json.NewEncoder(os.Stdout).Encode(value); err != nil {
		panic(err)
	}
}

type renameEdit struct {
	File   string `json:"file"`
	Start  int    `json:"byte_start"`
	End    int    `json:"byte_end"`
	Before string `json:"before"`
	After  string `json:"after"`
}

func semanticRename(req semanticRequest, p *semanticPackage) interface{} {
	refuse := func(reason string) interface{} { return map[string]interface{}{"validated": false, "error": reason} }
	if p.err != nil {
		return refuse("type_information_incomplete")
	}
	if !token.IsIdentifier(req.NewName) || req.NewName == "_" {
		return refuse("invalid_new_identifier")
	}
	var target types.Object
	for _, table := range []map[*ast.Ident]types.Object{p.info.Defs, p.info.Uses} {
		for id, obj := range table {
			file, at := p.position(id.Pos())
			if file == req.File && req.Offset >= at && req.Offset < at+len(id.Name) {
				target = obj
			}
		}
	}
	v, ok := target.(*types.Var)
	if !ok || v.Name() == "_" || v.IsField() || v.Parent() == nil || v.Parent() == p.pkg.Scope() {
		return refuse("only_function_local_variables_and_parameters_are_supported")
	}
	file, _ := p.position(target.Pos())
	if file != req.File {
		return refuse("target_not_local_to_file")
	}
	if req.NewName == target.Name() {
		return refuse("name_unchanged")
	}
	edits := []renameEdit{}
	for _, table := range []map[*ast.Ident]types.Object{p.info.Defs, p.info.Uses} {
		for id, obj := range table {
			if obj == target {
				file, at := p.position(id.Pos())
				if file != req.File {
					return refuse("reference_outside_target_file")
				}
				edits = append(edits, renameEdit{file, at, at + len(id.Name), id.Name, req.NewName})
			}
		}
	}
	sort.Slice(edits, func(i, j int) bool { return edits[i].Start < edits[j].Start })
	original := req.Files[req.File]
	var out bytes.Buffer
	cursor := 0
	for _, e := range edits {
		if e.Start < cursor || original[e.Start:e.End] != e.Before {
			return refuse("overlapping_or_stale_edit")
		}
		out.WriteString(original[cursor:e.Start])
		out.WriteString(e.After)
		cursor = e.End
	}
	out.WriteString(original[cursor:])
	changed := req
	changed.Files = map[string]string{}
	for f, s := range req.Files {
		changed.Files[f] = s
	}
	changed.Files[req.File] = out.String()
	after, err := semanticParse(changed)
	if err != nil || after.err != nil {
		return refuse("rename_does_not_typecheck_or_collides")
	}
	mapOffset := func(file string, offset int) int {
		if file != req.File {
			return offset
		}
		shift := 0
		for _, e := range edits {
			if e.Start >= offset {
				break
			}
			shift += len(e.After) - (e.End - e.Start)
		}
		return offset + shift
	}
	afterBindings := map[string]string{}
	for _, table := range []map[*ast.Ident]types.Object{after.info.Defs, after.info.Uses} {
		for id, obj := range table {
			if obj != nil {
				file, at := after.position(id.Pos())
				afterBindings[fmt.Sprintf("%s:%d", file, at)] = after.objectKey(obj)
			}
		}
	}
	checked := 0
	for _, table := range []map[*ast.Ident]types.Object{p.info.Defs, p.info.Uses} {
		for id, obj := range table {
			if obj == nil {
				continue
			}
			file, at := p.position(id.Pos())
			key := p.objectKey(obj)
			of, op := p.position(obj.Pos())
			if of != "" {
				key = fmt.Sprintf("%s:%d", of, mapOffset(of, op))
			}
			if afterBindings[fmt.Sprintf("%s:%d", file, mapOffset(file, at))] != key {
				return refuse("rename_changes_a_binding")
			}
			checked++
		}
	}
	return map[string]interface{}{"validated": true, "scope": "function_local_go_binding", "old_name": target.Name(), "new_name": req.NewName, "edits": edits, "bindings_checked": checked, "limitations": []string{"reflection_and_string_references_not_renamed", "test_build_variants_not_supported", "standalone_package_typecheck_not_native_module_build"}}
}

type flowAccess struct {
	Symbol string `json:"symbol"`
	Name   string `json:"name"`
}
type flowNode struct {
	ID        int          `json:"id"`
	Kind      string       `json:"kind"`
	Start     int          `json:"byte_start"`
	End       int          `json:"byte_end"`
	Reads     []flowAccess `json:"reads"`
	Writes    []flowAccess `json:"writes"`
	Reachable bool         `json:"reachable"`
}
type flowEdge struct {
	From    int    `json:"from"`
	To      int    `json:"to"`
	Outcome string `json:"outcome"`
}
type flowBuilder struct {
	namedReturns []*ast.Ident
	loopBreak    []int
	loopContinue []int
	p            *semanticPackage
	file         string
	nodes        []flowNode
	edges        []flowEdge
	issues       map[string]bool
}

func (b *flowBuilder) node(n ast.Node, kind string, reads ast.Node, writes []*ast.Ident) int {
	id := len(b.nodes)
	_, start := b.p.position(n.Pos())
	_, end := b.p.position(n.End())
	node := flowNode{ID: id, Kind: kind, Start: start, End: end, Reads: []flowAccess{}, Writes: []flowAccess{}}
	seen := map[string]bool{}
	if reads != nil {
		ast.Inspect(reads, func(n ast.Node) bool {
			if _, ok := n.(*ast.FuncLit); ok {
				b.issues["closure_body_not_followed"] = true
				return false
			}
			if e, ok := n.(*ast.BinaryExpr); ok && (e.Op == token.LAND || e.Op == token.LOR) {
				b.issues["short_circuit_expression_not_expanded"] = true
			}
			if _, ok := n.(*ast.CallExpr); ok {
				b.issues["call_effects_not_expanded"] = true
			}
			if e, ok := n.(*ast.UnaryExpr); ok && (e.Op == token.AND || e.Op == token.MUL || e.Op == token.ARROW) {
				b.issues["heap_and_external_effects_not_followed"] = true
			}
			if _, ok := n.(*ast.StarExpr); ok {
				b.issues["heap_and_external_effects_not_followed"] = true
			}
			if _, ok := n.(*ast.SelectorExpr); ok {
				b.issues["heap_and_external_effects_not_followed"] = true
			}
			if _, ok := n.(*ast.IndexExpr); ok {
				b.issues["heap_and_external_effects_not_followed"] = true
			}
			if id, ok := n.(*ast.Ident); ok {
				if obj, ok := b.p.info.Uses[id].(*types.Var); ok {
					key := b.p.objectKey(obj)
					if !seen[key] {
						node.Reads = append(node.Reads, flowAccess{key, obj.Name()})
						seen[key] = true
					}
				}
			}
			return true
		})
	}
	for _, ident := range writes {
		obj := b.p.info.Defs[ident]
		if obj == nil {
			obj = b.p.info.Uses[ident]
		}
		if obj != nil {
			node.Writes = append(node.Writes, flowAccess{b.p.objectKey(obj), obj.Name()})
		}
	}
	b.nodes = append(b.nodes, node)
	return id
}
func (b *flowBuilder) edge(from, to int, outcome string) {
	b.edges = append(b.edges, flowEdge{from, to, outcome})
}
func (b *flowBuilder) block(stmts []ast.Stmt, next int) int {
	for i := len(stmts) - 1; i >= 0; i-- {
		next = b.statement(stmts[i], next)
	}
	return next
}
func (b *flowBuilder) statement(stmt ast.Stmt, next int) int {
	if len(b.nodes) >= 256 {
		b.issues["statement_limit"] = true
		return next
	}
	switch n := stmt.(type) {
	case *ast.BlockStmt:
		return b.block(n.List, next)
	case *ast.IfStmt:
		yes := b.block(n.Body.List, next)
		no := next
		if n.Else != nil {
			no = b.statement(n.Else, next)
		}
		id := b.node(n.Cond, "condition", n.Cond, nil)
		b.edge(id, yes, "true")
		b.edge(id, no, "false")
		if n.Init != nil {
			return b.statement(n.Init, id)
		}
		return id
	case *ast.ForStmt:
		var id int
		if n.Cond != nil {
			id = b.node(n.Cond, "condition", n.Cond, nil)
		} else {
			id = b.node(n, "loop", nil, nil)
			b.nodes[id].End = b.nodes[id].Start + 3
		}
		post := id
		if n.Post != nil {
			post = b.statement(n.Post, id)
		}
		b.loopBreak = append(b.loopBreak, next)
		b.loopContinue = append(b.loopContinue, post)
		body := b.block(n.Body.List, post)
		b.loopBreak = b.loopBreak[:len(b.loopBreak)-1]
		b.loopContinue = b.loopContinue[:len(b.loopContinue)-1]
		b.edge(id, body, "true")
		if n.Cond != nil {
			b.edge(id, next, "false")
		}
		if n.Init != nil {
			return b.statement(n.Init, id)
		}
		return id
	case *ast.RangeStmt:
		if t := b.p.info.TypeOf(n.X); t != nil {
			switch t.Underlying().(type) {
			case *types.Signature, *types.Chan:
				b.issues["range_effects_not_expanded"] = true
			}
		}
		writes := []*ast.Ident{}
		for _, expr := range []ast.Expr{n.Key, n.Value} {
			if ident, ok := expr.(*ast.Ident); ok {
				writes = append(writes, ident)
			} else if expr != nil {
				b.issues["heap_and_external_effects_not_followed"] = true
			}
		}
		// Go evaluates the range expression once. Keep its saved value separate
		// from variables that may be reassigned during iteration.
		input := b.node(n.X, "range_input", n.X, nil)
		saved := flowAccess{fmt.Sprintf("range-input:%d", b.nodes[input].Start), "<range input>"}
		b.nodes[input].Writes = []flowAccess{saved}
		id := b.node(n.X, "range", nil, nil)
		b.nodes[id].Reads = []flowAccess{saved}
		iteration := b.node(n.X, "iteration_values", nil, writes)
		b.nodes[iteration].Reads = []flowAccess{saved}
		b.loopBreak = append(b.loopBreak, next)
		b.loopContinue = append(b.loopContinue, id)
		body := b.block(n.Body.List, id)
		b.loopBreak = b.loopBreak[:len(b.loopBreak)-1]
		b.loopContinue = b.loopContinue[:len(b.loopContinue)-1]
		b.edge(id, iteration, "has_next")
		b.edge(iteration, body, "next")
		b.edge(id, next, "done")
		b.edge(input, id, "next")
		return input
	case *ast.BranchStmt:
		if n.Label == nil && len(b.loopBreak) > 0 && (n.Tok == token.BREAK || n.Tok == token.CONTINUE) {
			id := b.node(n, n.Tok.String(), nil, nil)
			to := b.loopBreak[len(b.loopBreak)-1]
			if n.Tok == token.CONTINUE {
				to = b.loopContinue[len(b.loopContinue)-1]
			}
			b.edge(id, to, n.Tok.String())
			return id
		}
		b.issues["unsupported_control_statement"] = true
		id := b.node(n, "unsupported", nil, nil)
		b.edge(id, next, "unknown")
		return id
	case *ast.ReturnStmt:
		id := b.node(n, "return", n, nil)
		if len(n.Results) == 0 {
			for _, ident := range b.namedReturns {
				if obj := b.p.info.Defs[ident]; obj != nil {
					b.nodes[id].Reads = append(b.nodes[id].Reads, flowAccess{b.p.objectKey(obj), obj.Name()})
				}
			}
		}
		b.edge(id, 0, "return")
		return id
	case *ast.AssignStmt:
		writes := []*ast.Ident{}
		for _, left := range n.Lhs {
			if id, ok := left.(*ast.Ident); ok {
				writes = append(writes, id)
			} else {
				b.issues["heap_and_external_effects_not_followed"] = true
			}
		}
		// RHS uses only for '='; compound assignments also read the prior LHS.
		id := b.node(n, "assignment", n, writes)
		if n.Tok == token.ASSIGN || n.Tok == token.DEFINE {
			b.nodes[id].Reads = []flowAccess{}
			seen := map[string]bool{}
			for _, right := range n.Rhs {
				temporary := b.node(right, "expression", right, nil)
				for _, r := range b.nodes[temporary].Reads {
					if !seen[r.Symbol] {
						seen[r.Symbol] = true
						b.nodes[id].Reads = append(b.nodes[id].Reads, r)
					}
				}
				b.nodes = b.nodes[:temporary]
			}
		}
		b.edge(id, next, "next")
		return id
	case *ast.DeclStmt:
		decl, ok := n.Decl.(*ast.GenDecl)
		if !ok || decl.Tok != token.VAR {
			// Types and constants do not define runtime scalar values.
			return next
		}
		for i := len(decl.Specs) - 1; i >= 0; i-- {
			spec := decl.Specs[i].(*ast.ValueSpec)
			id := b.node(spec, "declaration", spec, spec.Names)
			b.edge(id, next, "next")
			next = id
		}
		return next
	case *ast.IncDecStmt:
		writes := []*ast.Ident{}
		if id, ok := n.X.(*ast.Ident); ok {
			writes = append(writes, id)
		} else {
			b.issues["heap_and_external_effects_not_followed"] = true
		}
		id := b.node(n, "update", n, writes)
		b.edge(id, next, "next")
		return id
	case *ast.ExprStmt:
		id := b.node(n, "operation", n, nil)
		b.edge(id, next, "next")
		return id
	case *ast.EmptyStmt:
		return next
	default:
		b.issues["unsupported_control_statement"] = true
		id := b.node(n, "unsupported", nil, nil)
		b.edge(id, next, "unknown")
		return id
	}
}
func statementFlow(req semanticRequest, p *semanticPackage) interface{} {
	var function *ast.FuncDecl
	for _, f := range p.files {
		if file, _ := p.position(f.Pos()); file != req.File {
			continue
		}
		for _, decl := range f.Decls {
			if fn, ok := decl.(*ast.FuncDecl); ok && fn.Body != nil {
				_, start := p.position(fn.Pos())
				_, end := p.position(fn.End())
				if req.Offset >= start && req.Offset < end {
					function = fn
				}
			}
		}
	}
	if function == nil {
		return map[string]interface{}{"error": "function_declaration_not_found", "complete": false}
	}
	count := 0
	ast.Inspect(function, func(n ast.Node) bool {
		if n != nil {
			count++
		}
		return count <= 4096
	})
	statements := 0
	ast.Inspect(function, func(n ast.Node) bool {
		if _, ok := n.(ast.Stmt); ok {
			statements++
		}
		return statements <= 200
	})
	if count > 4096 || statements > 200 {
		return map[string]interface{}{"error": "statement_analysis_limit", "coverage": map[string]interface{}{"complete": false, "issues": []string{"statement_analysis_limit"}}}
	}
	b := &flowBuilder{p: p, file: req.File, issues: map[string]bool{}, nodes: []flowNode{}, edges: []flowEdge{}}
	if p.err != nil {
		b.issues["type_information_incomplete"] = true
	}
	if function.Type.Results != nil {
		for _, field := range function.Type.Results.List {
			b.namedReturns = append(b.namedReturns, field.Names...)
		}
	}
	b.node(function.Body, "exit", nil, nil)
	first := b.block(function.Body.List, 0)
	params := []*ast.Ident{}
	for _, list := range []*ast.FieldList{function.Recv, function.Type.Params, function.Type.Results} {
		if list != nil {
			for _, f := range list.List {
				params = append(params, f.Names...)
			}
		}
	}
	entry := b.node(function.Type, "entry", nil, params)
	b.edge(entry, first, "next")
	n := len(b.nodes)
	successors := make([][]flowEdge, n)
	pred := make([][]int, n)
	for _, e := range b.edges {
		successors[e.From] = append(successors[e.From], e)
		pred[e.To] = append(pred[e.To], e.From)
	}
	queue := []int{entry}
	for i := 0; i < len(queue); i++ {
		id := queue[i]
		if b.nodes[id].Reachable {
			continue
		}
		b.nodes[id].Reachable = true
		for _, e := range successors[id] {
			queue = append(queue, e.To)
		}
	}
	// Reaching definitions over the bounded CFG. Every possible reaching write is retained.
	states := make([]map[string]map[int]bool, n)
	for i := range states {
		states[i] = map[string]map[int]bool{}
	}
	for pass := 0; pass <= n; pass++ {
		changed := false
		for id, node := range b.nodes {
			if !node.Reachable {
				continue
			}
			merged := map[string]map[int]bool{}
			for _, pr := range pred[id] {
				for symbol, defs := range states[pr] {
					if merged[symbol] == nil {
						merged[symbol] = map[int]bool{}
					}
					for d := range defs {
						merged[symbol][d] = true
					}
				}
			}
			for _, w := range node.Writes {
				merged[w.Symbol] = map[int]bool{id: true}
			}
			before, _ := json.Marshal(states[id])
			after, _ := json.Marshal(merged)
			if !bytes.Equal(before, after) {
				states[id] = merged
				changed = true
			}
		}
		if !changed {
			break
		}
		if pass == n {
			b.issues["data_flow_iteration_limit"] = true
		}
	}
	data := []map[string]interface{}{}
	for id, node := range b.nodes {
		if !node.Reachable {
			continue
		}
		for _, r := range node.Reads {
			defs := map[int]bool{}
			for _, pr := range pred[id] {
				for d := range states[pr][r.Symbol] {
					defs[d] = true
				}
			}
			if len(defs) == 0 {
				b.issues["value_origin_outside_function"] = true
			}
			for d := range defs {
				data = append(data, map[string]interface{}{"from": d, "to": id, "symbol": r.Symbol, "name": r.Name})
			}
		}
	}
	sort.Slice(data, func(i, j int) bool {
		a, _ := json.Marshal(data[i])
		c, _ := json.Marshal(data[j])
		return string(a) < string(c)
	})
	// Postdominators distinguish lexical guards from early-return guards.
	post := make([]map[int]bool, n)
	for i := range post {
		post[i] = map[int]bool{}
		if i == 0 {
			post[i][0] = true
		} else if b.nodes[i].Reachable {
			for j := range b.nodes {
				if b.nodes[j].Reachable {
					post[i][j] = true
				}
			}
		}
	}
	for pass := 0; pass <= n; pass++ {
		changed := false
		for id := range b.nodes {
			if id == 0 || !b.nodes[id].Reachable {
				continue
			}
			set := map[int]bool{id: true}
			if len(successors[id]) > 0 {
				for k := range post[successors[id][0].To] {
					all := true
					for _, e := range successors[id][1:] {
						all = all && post[e.To][k]
					}
					if all {
						set[k] = true
					}
				}
			}
			if len(set) != len(post[id]) {
				post[id] = set
				changed = true
			}
		}
		if !changed {
			break
		}
	}
	controls := []map[string]interface{}{}
	for _, e := range b.edges {
		if b.nodes[e.From].Kind != "condition" && b.nodes[e.From].Kind != "range" {
			continue
		}
		for id := range b.nodes {
			if id != e.From && post[e.To][id] && !post[e.From][id] {
				controls = append(controls, map[string]interface{}{"condition": e.From, "statement": id, "outcome": e.Outcome})
			}
		}
	}
	// A graph path that cannot reach the exit has no finite postdominator proof.
	terminating := map[int]bool{}
	pending := []int{0}
	for i := 0; i < len(pending); i++ {
		id := pending[i]
		if terminating[id] {
			continue
		}
		terminating[id] = true
		pending = append(pending, pred[id]...)
	}
	for id, node := range b.nodes {
		if node.Reachable && !terminating[id] {
			b.issues["nonterminating_path"] = true
		}
	}
	if b.issues["unsupported_control_statement"] || b.issues["nonterminating_path"] {
		controls = []map[string]interface{}{}
	}
	// An opaque control transfer can skip a definition or introduce another one.
	// Keep the CFG gaps visible, but do not present its guessed reaching writes.
	if b.issues["unsupported_control_statement"] {
		data = []map[string]interface{}{}
	}
	issues := sortedKeys(b.issues)
	_, functionStart := p.position(function.Pos())
	return map[string]interface{}{"language": "go", "kind": "potential_static", "observed": false, "function": function.Name.Name, "function_byte_start": functionStart, "file": req.File, "entry": entry, "exit": 0, "statements": b.nodes, "control_edges": b.edges, "data_edges": data, "gates": controls, "gate_semantics": "direct_control_dependence; follow condition links transitively for enclosing gates", "coverage": map[string]interface{}{"complete": len(issues) == 0, "scope": "intraprocedural_scalar_values_and_structured_branches", "issues": issues, "interprocedural_effects": false, "taint_analysis": false, "exceptional_edges": false, "normal_termination_assumed": true, "module_dependency_resolution": false, "program_completeness_proven": false}}
}
