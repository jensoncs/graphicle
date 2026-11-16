import json
import tempfile
import unittest
from pathlib import Path

from atlas_config_snapshot import snapshot

CONFIG = {
    "name": "siren-alert-review-config",
    "type": "configMap",
    "version": 2,
    "deployed_version": 2,
    "is_deployed": True,
    "data": [
        {"format": "key-value", "key": "ALERT_REVIEW_URL", "value": "http://a.internal/v1/review"},
        {"format": "key-value", "key": "NOTE", "value": "say \"hi\"\nbye"},
        {"format": "plaintext", "key": "SCHEDULE", "value": "@every 5m"},
    ],
}


class SnapshotTest(unittest.TestCase):
    def test_writes_the_deployed_config_map(self):
        text = snapshot(CONFIG, "siren")
        self.assertIn("kind: ConfigMap\n", text)
        self.assertIn("  name: siren-alert-review-config\n  namespace: siren\n", text)
        self.assertIn('  atlas-config-version: "2"\n', text)
        self.assertIn('  ALERT_REVIEW_URL: "http://a.internal/v1/review"\n', text)
        self.assertIn('  NOTE: "say \\"hi\\"\\nbye"\n', text)
        self.assertIn('  SCHEDULE: "@every 5m"\n', text)
        self.assertTrue(text.endswith("\n"))

    def test_refuses_what_it_cannot_represent_faithfully(self):
        for change in [
            {"type": "secret"},
            {"is_deployed": False},
            {"version": 3},
            {"data": [{"format": "file", "key": "a.json", "value": "{}"}]},
            {"data": [{"format": "key-value", "key": "bad key", "value": "x"}]},
            {"data": CONFIG["data"] + CONFIG["data"][:1]},
        ]:
            with self.assertRaises(ValueError, msg=change):
                snapshot({**CONFIG, **change}, "siren")
        with self.assertRaises(ValueError):
            snapshot(CONFIG, "")

    def test_cli_writes_one_file_per_config(self):
        from atlas_config_snapshot import main

        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp, "in.json")
            source.write_text(json.dumps(CONFIG))
            out = Path(tmp, "out")
            main(["--namespace", "siren", "--out", str(out), str(source)])
            self.assertEqual(
                (out / "siren-alert-review-config.yaml").read_text(),
                snapshot(CONFIG, "siren"),
            )


if __name__ == "__main__":
    unittest.main()
