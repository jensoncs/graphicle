import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("store_load", Path(__file__).with_name("run.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


def checkout(path):
    path.mkdir()
    (path / "hello world.py").write_text("def main():\n    return 1\n")
    subprocess.run(["git", "init", "-q", str(path)], check=True)
    subprocess.run(["git", "-C", str(path), "add", "."], check=True)


class HarnessTests(unittest.TestCase):
    def test_existing_data_and_symlinks_are_refused_without_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            seed, index = root / "seed", root / "index"
            seed.mkdir()
            index.mkdir()
            data = root / "data"
            data.mkdir()
            sentinel = data / "keep"
            sentinel.write_text("existing data")
            with self.assertRaises(ValueError):
                runner.fresh_paths(data, root / "output", seed, index)
            self.assertEqual(sentinel.read_text(), "existing data")
            alias = root / "alias"
            alias.symlink_to(root / "not-created")
            with self.assertRaises(ValueError):
                runner.fresh_paths(alias, root / "output", seed, index)
            self.assertFalse((root / "not-created").exists())
            self.assertFalse((root / "output").exists())

    def test_output_and_data_cannot_overlap_each_other_or_checkouts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            seed, index = root / "seed", root / "index"
            seed.mkdir()
            index.mkdir()
            for data, output in [(root / "data", root / "data" / "out"),
                                 (seed / "data", root / "out"),
                                 (root / "data", index / "out")]:
                with self.subTest(data=data, output=output), self.assertRaises(ValueError):
                    runner.fresh_paths(data, output, seed, index)
                self.assertFalse(data.exists())
                self.assertFalse(output.exists())

    def test_empty_and_failed_phases_have_null_latency_and_strict_json(self):
        phases, _, failures = runner.summarize_calls([(10, .2, "graph_stats", 503, False, "indexing")])
        self.assertIsNone(phases["warmup"]["p50_ms"])
        self.assertIsNone(phases["indexing"]["p99_ms"])
        self.assertEqual(failures, {"graph_stats:503": 1})
        json.dumps(phases, allow_nan=False)

    def test_latency_uses_successes_and_counts_every_failure(self):
        phases, tools, failures = runner.summarize_calls([
            (1, .1, "graph_stats", 200, True, "indexing"),
            (2, .3, "graph_stats", 200, True, "indexing"),
            (3, 4, "graph_stats", 500, False, "indexing"),
        ])
        self.assertEqual(phases["indexing"]["p50_ms"], 200)
        self.assertEqual(tools["graph_stats"]["calls"], 3)
        self.assertEqual(failures, {"graph_stats:500": 1})

    def test_call_is_attributed_to_its_starting_phase(self):
        class Server:
            timeout = 1

            def mcp(self, repository, method, params, session, ident):
                if method == "tools/call":
                    readers.phase = "after"
                    readers.stop.set()
                return 200, {"result": {}}, {}

        corpus = types.SimpleNamespace(args=lambda *args: {})
        readers = runner.Readers(Server(), "project", corpus,
                                 [("graph_stats", 1, "none", {})], 1, 20, 1)
        readers.client(0)
        self.assertEqual(readers.calls[0][5], "warmup")

    def test_corpus_keeps_paths_with_spaces(self):
        with tempfile.TemporaryDirectory() as temporary:
            seed = Path(temporary) / "seed"
            checkout(seed)
            corpus = runner.Corpus(seed, random.Random(1))
            self.assertEqual(corpus.files, ["hello world.py"])
            self.assertEqual(corpus.symbols, ["main"])

    def test_failed_seed_run_keeps_a_failure_report(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            seed, index = root / "seed", root / "index"
            checkout(seed)
            checkout(index)
            binary = root / "fake-graphicle"
            binary.write_text(f"#!{sys.executable}\nimport sys\n"
                              "if '--version' in sys.argv: print('test-binary')\n"
                              "else: sys.exit(7)\n")
            binary.chmod(0o700)
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                code = runner.main(["--bin", str(binary), "--seed", str(seed), "--index", str(index),
                                    "--data-dir", str(root / "data"), "--out", str(root / "out")])
            report = json.loads((root / "out" / "run.json").read_text())
            self.assertEqual(code, 1)
            self.assertFalse(report["completed"])
            self.assertIn("seed sync exited 7", report["error"])
            self.assertTrue((root / "out" / "seed-sync.log").exists())

    def test_shutdown_reaps_a_started_process(self):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
                                 start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            runner.stop_process(child)
            self.assertIsNotNone(child.poll())
        finally:
            if child.poll() is None:
                os.killpg(child.pid, 9)
                child.wait()

    def test_macos_memory_is_read_from_ps(self):
        with patch.object(runner.sys, "platform", "darwin"), patch.object(runner.subprocess, "run") as command:
            command.return_value.stdout = " 12345\n"
            self.assertEqual(runner.rss_kib(123), (12345, 12345))
            self.assertEqual(command.call_args.args[0], ["ps", "-p", "123", "-o", "rss="])


if __name__ == "__main__":
    unittest.main()
