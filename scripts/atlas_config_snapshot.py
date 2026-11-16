"""Turn Atlas shard ConfigMaps into Kubernetes ConfigMap YAML for review input.

Input: the JSON that Atlas `get_shard_config` returns for a ConfigMap (one file
per ConfigMap). Output: `<out>/<name>.yaml`, the form graphicle's review path
reads (`extractors::environment::config_map`).

This script holds no credentials: fetch the JSON with an authenticated Atlas
client (the Atlas MCP tool). It never writes a Secret, and it refuses anything
it cannot represent exactly, so a snapshot is the deployed version or nothing.

    python3 scripts/atlas_config_snapshot.py --namespace siren --out snap/ cfg.json
"""

import argparse
import json
import re
import sys
from pathlib import Path

DATA_KEY = re.compile(r"[-._a-zA-Z0-9]{1,253}")


def quoted(value):
    # A JSON string is a valid YAML double-quoted scalar; keep non-ASCII
    # literal so no surrogate-pair escapes reach the YAML parser.
    return json.dumps(value, ensure_ascii=False)


def snapshot(config, namespace):
    if not namespace:
        raise ValueError("namespace is required")
    if config.get("type") != "configMap":
        raise ValueError(f"{config.get('name')}: only ConfigMaps are snapshotted")
    if not config.get("is_deployed") or config.get("version") != config.get("deployed_version"):
        raise ValueError(f"{config.get('name')}: the latest version is not the deployed one")
    lines = [
        "apiVersion: v1",
        "kind: ConfigMap",
        "metadata:",
        f"  name: {config['name']}",
        f"  namespace: {namespace}",
        "  annotations:",
        f"    atlas-config-version: {quoted(str(config['version']))}",
        "data:",
    ]
    seen = set()
    for item in config.get("data") or []:
        key = item.get("key", "")
        # Both are one string entry of the ConfigMap's `data`.
        if item.get("format") not in ("key-value", "plaintext") or not isinstance(item.get("value"), str):
            raise ValueError(f"{config['name']}/{key}: unsupported format {item.get('format')}")
        if not DATA_KEY.fullmatch(key) or key in seen:
            raise ValueError(f"{config['name']}: invalid or repeated key {key!r}")
        seen.add(key)
        lines.append(f"  {key}: {quoted(item['value'])}")
    return "\n".join(lines) + "\n"


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--namespace", required=True)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("configs", nargs="+", type=Path)
    args = parser.parse_args(argv)
    texts = {}
    for path in args.configs:
        config = json.loads(path.read_text())
        texts[config["name"]] = snapshot(config, args.namespace)
    args.out.mkdir(parents=True, exist_ok=True)
    for name, text in texts.items():
        (args.out / f"{name}.yaml").write_text(text)


if __name__ == "__main__":
    main(sys.argv[1:])
