package main

import (
	"go/ast"
	"go/importer"
	"go/parser"
	"go/token"
	"go/types"
	"testing"
)

// Each call site says what it does with the callee's error, so a review can
// show where sites of one callee disagree (nexus #963: one builder returns a
// failed config read, a sibling builds routes without the configs).
func TestEachCallSiteSaysWhatItDoesWithTheError(t *testing.T) {
	const src = `package app
import (
	"errors"
	"fmt"
)
func Load() (int, error) { return 0, errors.New("x") }
func log(error) {}
func returns() (int, error) { v, err := Load(); if err != nil { return 0, err }; return v, nil }
func forwards() (int, error) { return Load() }
func proceeds() int { n := 0; if v, err := Load(); err == nil { n = v }; return n }
func continues() { for i := 0; i < 2; i++ { _, err := Load(); if err != nil { log(err); continue } } }
func logs() { _, err := Load(); if err != nil { log(err) } }
func drops() { v, _ := Load(); _ = v }
func ignores() { Load() }
func elsewhere() { _, err := Load(); _ = 1; _ = err }
func wraps() (int, error) { v, err := Load(); if err != nil { return 0, fmt.Errorf("load: %w", err) }; return v, nil }
func replaces() (int, error) { v, err := Load(); if err != nil { return 0, errors.New("no load") }; return v, nil }
func fallsBack() string { _, err := Load(); if err != nil { log(err); return "" }; return "x" }
func stops() { _, err := Load(); if err != nil { return } }
`
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, "app.go", src, 0)
	if err != nil {
		t.Fatal(err)
	}
	info := &types.Info{Types: map[ast.Expr]types.TypeAndValue{}, Defs: map[*ast.Ident]types.Object{}, Uses: map[*ast.Ident]types.Object{}}
	if _, err := (&types.Config{Importer: importer.Default()}).Check("app", fset, []*ast.File{file}, info); err != nil {
		t.Fatal(err)
	}
	got := map[string]string{}
	handling := errorHandling(file, info)
	for _, d := range file.Decls {
		fn, ok := d.(*ast.FuncDecl)
		if !ok || fn.Body == nil {
			continue
		}
		ast.Inspect(fn.Body, func(n ast.Node) bool {
			if c, ok := n.(*ast.CallExpr); ok {
				if id, ok := c.Fun.(*ast.Ident); ok && id.Name == "Load" {
					got[fn.Name.Name] = handling[c]
				}
			}
			return true
		})
	}
	want := map[string]string{"returns": "returns_on_error", "forwards": "returned", "proceeds": "proceeds_on_error",
		"continues": "continues_on_error", "logs": "logs_on_error", "drops": "discarded", "ignores": "discarded", "elsewhere": "",
		"wraps": "returns_on_error", "replaces": "returns_on_error", "fallsBack": "falls_back_on_error", "stops": "stops_on_error"}
	for name, w := range want {
		if got[name] != w {
			t.Errorf("%s: got %q, want %q", name, got[name], w)
		}
	}
}
