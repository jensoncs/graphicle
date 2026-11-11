"""The Core image's build stage receives every compile input.

deploy/Dockerfile copies an explicit list of paths, and
deploy/Dockerfile.dockerignore admits an explicit allowlist. A build
script or an include_str!/include_bytes! target outside both lists
breaks the image build only inside Docker, so this checks them here.
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
CRATES = ("graphicle-graph", "graphicle-memory", "graphicle")
INCLUDE = re.compile(r'include_(?:str|bytes)!\("([^"]+)"\)')


def build_copies():
    text = (ROOT / "deploy/Dockerfile").read_text()
    stage = text.split("\nFROM ", 2)[1]
    sources = []
    for line in stage.splitlines():
        if line.startswith("COPY ") and "--from" not in line:
            sources += line.split()[1:-1]
    return sources


def admitted():
    lines = (ROOT / "deploy/Dockerfile.dockerignore").read_text().splitlines()
    return [line[1:].rstrip("/") for line in lines if line.startswith("!")]


def covered(path, roots):
    return any(path == root or path.startswith(root + "/") for root in roots)


def inputs():
    found = set()
    for crate in CRATES:
        base = ROOT / "crates" / crate
        if (base / "build.rs").is_file():
            found.add(f"crates/{crate}/build.rs")
        for source in (base / "src").rglob("*.rs"):
            for target in INCLUDE.findall(source.read_text(errors="ignore")):
                resolved = (source.parent / target).resolve()
                if resolved.is_file():
                    found.add(resolved.relative_to(ROOT).as_posix())
    return found


class DockerContextTest(unittest.TestCase):
    def test_every_compile_input_is_copied_and_admitted(self):
        copies, allow = build_copies(), admitted()
        missing = sorted(p for p in inputs() if not covered(p, copies))
        self.assertEqual(missing, [], "not copied by deploy/Dockerfile")
        ignored = sorted(p for p in copies if not covered(p, allow))
        self.assertEqual(ignored, [], "copied but excluded by the dockerignore")


if __name__ == "__main__":
    unittest.main()
