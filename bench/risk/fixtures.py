"""Authored regression cases; labels describe evidence, never production risk accuracy."""
import hashlib
import json


def sha(text):
    return hashlib.sha256(text.encode()).hexdigest()


def suite():
    def repo(name, before, after, **extra):
        module = f"module example/{name}\ngo 1.23\n"
        return {"name": name, "base": {"go.mod": module, **before},
                "head": {"go.mod": module, **after}, **extra}

    def source(repository, file, quote, side="head"):
        return {"kind": "source", "repository": repository, "side": side,
                "file": file, "quote": quote}

    guard = ("package access\nfunc Check(tenant, owner string) bool { return tenant == owner }\n"
             "func Authorize(tenant, owner string) bool { return Check(tenant, owner) }\n"
             'func Unrelated() string { return "hello" }\n')
    broken = guard.replace("tenant == owner", "tenant != owner")
    error = ("package payment\ntype Failure struct{}\nfunc (Failure) Error() string { return \"failed\" }\n"
             "func Persist() error { return Failure{} }\nfunc Submit() error { return Persist() }\n")
    ignored = error.replace("return Persist()", "Persist(); return nil")
    dynamic = "package dispatch\nfunc Invoke(callback func() int) int { return callback() }\n"
    large = "package batch\n" + "".join(
        f"func F{i}(v int) int {{\n" + "".join(
            f" if v == {j} {{ v += {j + 1} }}\n" for j in range(12)) + " return v\n}\n"
        for i in range(12))
    cases = [
        {"id": "tenant-guard", "repositories": [repo("access", {"access.go": guard},
          {"access.go": broken}, base_subject="Fix tenant isolation: preserve owner check")],
         "labels": [source("access", "access.go", "return tenant != owner"),
                    source("access", "access.go", "return Check(tenant, owner)"),
                    {"kind": "history", "repository": "access", "side": "base", "file": "access.go",
                     "subject": "Fix tenant isolation: preserve owner check"},
                    {"kind": "observation", "repository": "access", "side": "head", "file": "access.go"}],
         "observation": {"repository": "access", "path": "access.go", "line_start": 2, "line_end": 2},
         "forbidden": [source("access", "access.go", 'return "hello"')]},
        {"id": "discarded-error", "repositories": [repo("payment", {"payment.go": error},
          {"payment.go": ignored})], "labels": [source("payment", "payment.go", "Persist(); return nil"),
          source("payment", "payment.go", "return Failure{}")], "forbidden": []},
        {"id": "dynamic-dispatch", "repositories": [repo("dispatch", {"dispatch.go": dynamic},
          {"dispatch.go": dynamic.replace("callback()", "callback() + 1")})],
         "labels": [source("dispatch", "dispatch.go", "return callback() + 1")],
         "must_be_incomplete": True, "forbidden": []},
        {"id": "documentation-only", "repositories": [repo("docs", {"main.go": "package docs\nfunc Value() int { return 1 }\n",
          "README.md": "Old description\n"}, {"main.go": "package docs\nfunc Value() int { return 1 }\n",
          "README.md": "Updated description\n"})], "labels": [],
         "forbidden": [source("docs", "main.go", "return 1")]},
        {"id": "budget-pressure", "repositories": [repo("batch", {"batch.go": large},
          {"batch.go": large.replace("return v", "return v + 1")})],
         "labels": [source("batch", "batch.go", f"func F{i}(v int) int") for i in range(12)],
         "forbidden": []},
    ]

    # Exact configuration assertions, deliberately not observed runtime calls.
    code = "package service\nfunc Handle() int { return 1 }\n"
    config, contract = "deployment=pilot\n", "operation contract v1\n"
    http = {"protocol": "http", "authority": "https://orders.example", "method": "POST", "path": "/orders"}
    grpc = {"protocol": "grpc", "authority": "grpcs://pricing.example", "service": "pricing.v1.Price", "method": "Quote"}
    event = {"protocol": "event", "broker": "pilot/pubsub", "topic": "orders", "schema": "Order/v1", "group": None}

    def files(name, body, bindings):
        def cite(file, text, start, end):
            return {"file": file, "sha256": sha(text), "byte_start": start, "byte_end": end}
        rows = []
        for identity, direction, transport in bindings:
            rows.append({"id": identity, "direction": direction, "transport": transport,
                "service": {"namespace": "pilot", "name": name, "environment": "fixture", "configuration": "v1"},
                "owner": cite("service.go", body, body.index("func"), len(body.rstrip())),
                "site": cite("service.go", body, body.index("return"), len(body.rstrip()) - 2),
                "configuration": [cite("deployment.txt", config, 0, len(config))],
                "contract": cite("contract.txt", contract, 0, len(contract))})
        return {"service.go": body, "deployment.txt": config, "contract.txt": contract,
                "graphicle-services.json": json.dumps({"format": "graphicle.service-boundaries.v1",
                    "inventory_complete": True, "bindings": rows})}

    wiring = {"checkout": [("send", "outbound", http)],
              "orders": [("serve", "inbound", http), ("quote", "outbound", grpc)],
              "pricing": [("quote", "inbound", grpc), ("publish", "outbound", event)],
              "billing": [("consume", "inbound", {**event, "group": "billing"})]}
    repositories = [repo(name, files(name, code, bindings), files(name,
        code.replace("return 1", "return 2") if name == "pricing" else code, bindings))
        for name, bindings in wiring.items()]
    cases.append({"id": "service-chain", "anchor": "pricing", "repositories": repositories,
        "labels": [source(name, "service.go", "return 2" if name == "pricing" else "return 1") for name in wiring]
            + [{"kind": "relationship", "relation": relation} for relation in ("http_calls", "grpc_calls", "event_delivers")],
        "forbidden": []})
    return {"format": "graphicle.risk-pilot-fixtures.v1", "provenance": "authored_synthetic_regression",
            "cases": cases}
