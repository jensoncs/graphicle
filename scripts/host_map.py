"""Build the internal host -> Kubernetes Service map that review input uses.

Two sources, merged per host:
  1. weaver (`/api/meta` `hosts`): hosts of services in the Nexus mesh, observed.
  2. local clones: k8s-application chart values (`infra/<env>/values*.yaml`)
     that declare `ingress.hosts` / `ingressGrpc.hosts`, as the chart renders
     them (a path's `serviceName`, else `applicationName`; duplication paths
     reach `<app>-dupe`, the same application's canary copy).

A host is `resolved` when every route names one application, else
`ambiguous` (e.g. one host split by path between two services). Nothing here
is a runtime proof: weaver shows observed traffic, values show configuration.

Run once a day (a launchd agent, ~/Library/LaunchAgents/local.graphicle.host-map.plist).
A failed weaver fetch fails the run and keeps the previous map: a map without
the mesh would look complete. Clones are read as checked out, never pulled;
each repository route carries the commit it was read at.

    python3 scripts/host_map.py --repos ~/sharechat --out ~/.local/share/graphicle/host-map.json
"""

import argparse
import datetime
import json
import os
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

FORMAT = "graphicle.host-map.v1"
WEAVER = "http://weaver.sharechat.internal"
HOST = re.compile(r"(?=.{1,253}$)([a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?\.)+[a-z][-a-z0-9]{0,62}")
NAME = re.compile(r"[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?")
VALUES = re.compile(r"values[-.\w]*\.ya?ml")
SKIP = {".git", "node_modules", "vendor", "target", ".venv", "venv", "dist", "build"}
SUFFIX = ".sharechat.internal"


def application(service):
    """The application a chart Service belongs to (`<app>-dupe`, `shadow-<app>`)."""
    service = service.removeprefix("shadow-")
    return service.removesuffix("-dupe")


def weaver_routes(meta):
    """`hosts` of weaver's /api/meta: host -> ["entity/service", ...]."""
    routes = []
    for host, services in (meta.get("hosts") or {}).items():
        if not host.endswith(SUFFIX) or not HOST.fullmatch(host):
            continue
        for qualified in services:
            entity, _, service = qualified.rpartition("/")
            if NAME.fullmatch(service):
                routes.append({"host": host, "path": None, "service": service,
                               "entity": entity or None, "source": "weaver"})
    return routes


def values_routes(values, where):
    """Routes one chart values document declares; `where` is its provenance."""
    app = values.get("applicationName")
    namespace = values.get("namespace")
    if not isinstance(app, str) or not NAME.fullmatch(app):
        return []
    routes = []
    for key in ("ingress", "ingressGrpc"):
        block = values.get(key)
        if not isinstance(block, dict) or block.get("enabled") is not True:
            continue
        duplication = values.get("serviceDuplication")
        duplication = duplication if isinstance(duplication, dict) else {}
        extra = (duplication.get("paths") or []) if duplication.get("enabled") is True else []
        for entry in block.get("hosts") or []:
            host = entry.get("host") if isinstance(entry, dict) else None
            if not isinstance(host, str) or not host.endswith(SUFFIX) or not HOST.fullmatch(host):
                continue
            paths = [(p, app) for p in entry.get("paths") or []] + [(p, f"{app}-dupe") for p in extra]
            for path, default in paths:
                if not isinstance(path, dict):
                    continue
                service = path.get("serviceName") or default
                if isinstance(service, str) and NAME.fullmatch(service):
                    routes.append({"host": host, "path": path.get("path"), "service": service,
                                   "namespace": namespace, "application": app,
                                   "source": "repository", **where})
    return routes


def values_files(root):
    for directory, subdirs, files in os.walk(root):
        # A `{{...}}` directory is a project template (cookiecutter), not a service.
        subdirs[:] = [d for d in subdirs if d not in SKIP and "{{" not in d]
        parent = Path(directory)
        if parent.parent.name == "infra":
            for name in files:
                if VALUES.fullmatch(name):
                    yield parent / name


def load_yaml(path):
    """Every document of the file, one JSON line each; None when unreadable."""
    out = subprocess.run(["yq", "-o=json", "-I=0", ".", str(path)], capture_output=True, text=True, timeout=30)
    if out.returncode != 0:
        return None
    try:
        return [json.loads(line) for line in out.stdout.splitlines() if line.strip()]
    except json.JSONDecodeError:
        return None


def head(repo):
    out = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"], capture_output=True, text=True)
    return out.stdout.strip() or None


def repository_routes(root):
    routes, unreadable, commits = [], [], {}
    for path in sorted(values_files(root)):
        rel = path.relative_to(root)
        repo = rel.parts[0]
        if repo not in commits:
            commits[repo] = head(root / repo)
        documents = load_yaml(path)
        if documents is None:
            unreadable.append(str(rel))
            continue
        where = {"repository": repo, "file": str(Path(*rel.parts[1:])),
                 "environment": path.parent.name, "commit": commits[repo]}
        for values in documents:
            if isinstance(values, dict):
                routes.extend(values_routes(values, where))
    return routes, unreadable


def merge(routes):
    hosts = {}
    for route in routes:
        hosts.setdefault(route["host"], []).append({k: v for k, v in route.items() if k != "host"})
    out = {}
    for host in sorted(hosts):
        entries = sorted(hosts[host], key=lambda r: json.dumps(r, sort_keys=True))
        apps = sorted({application(r["service"]) for r in entries})
        observed = sorted({application(r["service"]) for r in entries if r["source"] == "weaver"})
        out[host] = {"status": "resolved" if len(apps) == 1 else "ambiguous",
                     "applications": apps, "observed_applications": observed, "routes": entries}
    return out


def build(meta, root):
    repo_routes, unreadable = repository_routes(root)
    hosts = merge(weaver_routes(meta) + repo_routes)
    return {
        "format": FORMAT,
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "sources": {"weaver": WEAVER, "repositories": str(root), "unreadable_values": unreadable},
        # Every host the map can know ends with one of these; another host is not
        # "unmapped", only out of scope.
        "suffixes": [SUFFIX],
        "hosts": hosts,
    }


def fetch_meta(base):
    with urllib.request.urlopen(f"{base}/api/meta", timeout=30) as response:
        meta = json.load(response)
    if not isinstance(meta.get("hosts"), dict) or not meta["hosts"]:
        raise ValueError("weaver /api/meta has no hosts")
    return meta


def write(path, document):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(document, indent=1, sort_keys=True) + "\n")
    tmp.replace(path)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repos", required=True, type=Path, help="directory of local clones")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--weaver", default=WEAVER)
    args = parser.parse_args(argv)
    document = build(fetch_meta(args.weaver), args.repos.expanduser().resolve())
    write(args.out.expanduser(), document)
    hosts = document["hosts"].values()
    print(f"{len(document['hosts'])} hosts, {sum(h['status'] == 'ambiguous' for h in hosts)} ambiguous, "
          f"{len(document['sources']['unreadable_values'])} unreadable values files -> {args.out}")


if __name__ == "__main__":
    main(sys.argv[1:])
