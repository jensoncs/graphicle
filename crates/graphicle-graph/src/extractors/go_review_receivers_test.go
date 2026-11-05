package main

import (
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"go/types"
	"strings"
	"testing"
)

func receiverFactoryChain() string {
	var out strings.Builder
	out.WriteString("func Factory0() Value { return A{} }; ")
	for i := 1; i < 300; i++ {
		fmt.Fprintf(&out, "func Factory%d() Value { return Factory%d() }; ", i, i-1)
	}
	return out.String()
}

func TestExactReceiversRequireImmutableStorageAndAllReturnPaths(t *testing.T) {
	const declarations = `package app
type Value interface { Get() int }
type A struct{}
func (A) Get() int { return 1 }
type B struct{}
func (B) Get() int { return 2 }
type Holder struct { value Value }
type Nested struct { inner *Holder }
func (h *Holder) Reset(v Value) { h.value = v }
func Replace(h *Holder, v Value) { h.value = v }
`
	for _, tc := range []struct{ name, decl, body, want string }{
		{"single_return", `func New() Value { return A{} }`, `return New().Get()`, "A"},
		{"forwarding", `func Inner() Value { return A{} }; func New() Value { return Inner() }`, `v := New(); return v.Get()`, "A"},
		{"same_type_branches", `func New(b bool) Value { if b {return A{}}; return A{} }`, `return New(flag).Get()`, "A"},
		{"concrete_parameter", `func New(a A) Value { return a }`, `return New(A{}).Get()`, "A"},
		{"bound_return", `func New() Value { v := Value(A{}); return v }`, `return New().Get()`, "A"},
		{"changed_return", `func New() Value { return B{} }`, `return New().Get()`, "B"},
		{"mixed_returns", `func New(b bool) Value { if b { return A{} }; return B{} }`, `return New(flag).Get()`, ""},
		{"nil_return", `func New() Value { return nil }`, `return New().Get()`, ""},
		{"typed_nil", `func New() Value { return (*A)(nil) }`, `return New().Get()`, "A"},
		{"named_defer", `func New() (v Value) { defer func(){v=B{}}(); return A{} }`, `return New().Get()`, ""},
		{"return_reassigned", `func New(v Value) Value { x:=Value(A{}); x=v; return x }`, `return New(input).Get()`, ""},
		{"recursive_return", `func New(b bool) Value { if b { return A{} }; return New(b) }`, `return New(flag).Get()`, ""},
		{"generic_return", `func New[T Value](v T) Value { return v }`, `return New[A](A{}).Get()`, ""},
		{"function_variable", `func New() Value { return A{} }`, `factory:=New; return factory().Get()`, ""},
		{"summary_budget", receiverFactoryChain(), `return Factory299().Get()`, ""},
		{"keyed", ``, `h:=Holder{value:A{}}; return h.value.Get()`, "A"},
		{"positional", ``, `h:=Holder{B{}}; return h.value.Get()`, "B"},
		{"pointer", ``, `h:=&Holder{value:A{}}; return h.value.Get()`, "A"},
		{"immediate", ``, `return (Holder{value:A{}}).value.Get()`, "A"},
		{"interface_copy", ``, `h:=Holder{value:A{}}; v:=h.value; return v.Get()`, "A"},
		{"nested_literal", ``, `h:=Nested{inner:&Holder{value:A{}}}; return h.inner.value.Get()`, "A"},
		{"promoted", `type Embedded struct { Value }`, `h:=Embedded{Value:A{}}; return h.Get()`, "A"},
		{"promoted_nested", `type Embedded struct { Value }; type Outer struct { *Embedded }`, `h:=Outer{&Embedded{A{}}}; return h.Get()`, "A"},
		{"field_reassigned", ``, `h:=Holder{value:A{}}; h.value=input; return h.value.Get()`, ""},
		{"holder_reassigned", ``, `h:=Holder{value:A{}}; h=Holder{input}; return h.value.Get()`, ""},
		{"pointer_alias", ``, `h:=&Holder{value:A{}}; alias:=h; alias.value=input; return h.value.Get()`, ""},
		{"implicit_address", ``, `h:=Holder{value:A{}}; h.Reset(input); return h.value.Get()`, ""},
		{"address_escape", ``, `h:=Holder{value:A{}}; Replace(&h,input); return h.value.Get()`, ""},
		{"field_address", ``, `h:=Holder{value:A{}}; p:=&h.value; *p=input; return h.value.Get()`, ""},
		{"closure_write", ``, `h:=Holder{value:A{}}; func(){h.value=input}(); return h.value.Get()`, ""},
		{"range_write", ``, `h:=Holder{value:A{}}; for _,h.value=range []Value{input} {}; return h.value.Get()`, ""},
		{"nested_escape", ``, `h:=Nested{inner:&Holder{value:A{}}}; Replace(h.inner,input); return h.inner.value.Get()`, ""},
		{"shared_nested", ``, `inner:=&Holder{value:A{}}; h:=Nested{inner:inner}; Replace(inner,input); return h.inner.value.Get()`, ""},
		{"nil_field", ``, `var h Holder; return h.value.Get()`, ""},
		{"parameter_holder", ``, `h:=external; return h.value.Get()`, ""},
		{"local_reassigned", ``, `v:=Value(A{}); v=input; return v.Get()`, ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			fs := token.NewFileSet()
			file, err := parser.ParseFile(fs, "app.go", declarations+tc.decl+`
func Use(input Value, flag bool, external Holder) int {`+tc.body+`}`, 0)
			if err != nil {
				t.Fatal(err)
			}
			info := &types.Info{Defs: map[*ast.Ident]types.Object{}, Uses: map[*ast.Ident]types.Object{},
				Types: map[ast.Expr]types.TypeAndValue{}, Selections: map[*ast.SelectorExpr]*types.Selection{}}
			if _, err := (&types.Config{}).Check("example/app", fs, []*ast.File{file}, info); err != nil {
				t.Fatal(err)
			}
			flow := exactReceiverFlow([]*ast.File{file}, info, localValueBindings([]*ast.File{file}, info))
			var selector *ast.SelectorExpr
			ast.Inspect(file, func(n ast.Node) bool {
				if s, ok := n.(*ast.SelectorExpr); ok && s.Sel.Name == "Get" {
					selector = s
				}
				return true
			})
			actual := flow.receiver(selector)
			if tc.want == "" {
				if actual != nil {
					t.Fatalf("unsafe exact receiver: %s", actual)
				}
			} else if actual == nil || strings.TrimPrefix(actual.String(), "*") != "example/app."+tc.want {
				t.Fatalf("receiver %v, want %s", actual, tc.want)
			}
		})
	}
}
