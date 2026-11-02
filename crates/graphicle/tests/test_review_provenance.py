"""Pure SDK contract tests: no server or model required."""
import copy
import hashlib
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "sdk/python"))
from graphicle_sdk import apply_review_delta, materialize_service_provenance


def reference(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


class ProvenanceTests(unittest.TestCase):
    def test_impact_forwards_explicit_capability_and_preserves_default(self):
        from graphicle_sdk.client import Project
        class Client:
            def command(self, operation, **arguments):
                return arguments
        project = Project(Client(), "repo")
        default = project.impact([], [], profile={"goos": "linux", "goarch": "amd64"})
        compact = project.impact([], [], profile={"goos": "linux", "goarch": "amd64"}, compact_provenance=True)
        self.assertNotIn("compact_provenance", default)
        self.assertTrue(compact["compact_provenance"])

    def packet(self):
        leaf = {"authority": "pinned_configuration_assertion", "runtime_observed": False,
                "manifest": {"file": "graphicle-services.json", "sha256": "b" * 64},
                "citations": [{"file": "服务.go", "byte_start": 1, "byte_end": 20, "sha256": "a" * 64}]}
        leaf_id = reference(leaf)
        edge = {"source_ref": leaf_id, "target_ref": leaf_id}
        edge_id = reference(edge)
        packet = {"packet_id": "one", "previous_packet_id": None, "base": [], "profile": {},
            "entries": [{"id": "entry", "reasons": [{"steps": [{"evidence": "configured_static", "provenance_ref": edge_id}]}]}],
            "diffs": [], "unchanged": [], "invalidated": [], "context_order": ["entry"], "tokens": 100,
            "service_provenance": {"format": "graphicle.service-provenance.v1", "definitions": {leaf_id: leaf, edge_id: edge}}}
        packet["entries"][0]["service_boundary"] = {"authority": leaf["authority"], "runtime_observed": False,
            "resolution": {"status": "unresolved"}, "provenance_ref": leaf_id}
        return packet, leaf, edge_id

    def test_full_and_unchanged_delta_reexpand_from_current_table(self):
        packet, leaf, edge_id = self.packet()
        original = copy.deepcopy(packet)
        full = apply_review_delta(None, packet)
        self.assertEqual(packet, original)
        step = full["entries"][0]["reasons"][0]["steps"][0]
        self.assertEqual(step["provenance"], {"source": leaf, "target": leaf})
        self.assertEqual(step["provenance_ref"], edge_id)
        self.assertNotIn("tokens", full)
        self.assertEqual(full["entries"][0]["service_boundary"]["citations"], leaf["citations"])
        self.assertEqual(full["entries"][0]["service_boundary"]["resolution"], {"status": "unresolved"})
        delta = {**packet, "packet_id": "two", "previous_packet_id": "one", "entries": [], "unchanged": ["entry"]}
        step["provenance"]["source"]["runtime_observed"] = True
        delta["empty_reason"] = "unchanged_from_previous_packet"
        result = apply_review_delta(full, delta)
        self.assertNotIn("empty_reason", result)
        self.assertEqual(delta["empty_reason"], "unchanged_from_previous_packet")
        self.assertFalse(result["entries"][0]["reasons"][0]["steps"][0]["provenance"]["source"]["runtime_observed"])
        delta.pop("service_provenance")
        with self.assertRaisesRegex(ValueError, "Invalid service provenance"):
            apply_review_delta(full, delta)

    def test_corrupt_missing_wrong_format_and_overdeep_references_fail(self):
        packet, leaf, edge_id = self.packet()
        leaf_id = reference(leaf)
        for mutate in (
            lambda p: p.pop("service_provenance"),
            lambda p: p["service_provenance"].update(format="unknown"),
            lambda p: p["service_provenance"]["definitions"].pop(leaf_id),
            lambda p: p["service_provenance"]["definitions"][leaf_id].update(runtime_observed=True),
            lambda p: p["entries"][0]["reasons"][0]["steps"][0].update(provenance_ref="missing"),
            lambda p: p["entries"][0]["service_boundary"].update(runtime_observed=True),
        ):
            bad = copy.deepcopy(packet)
            mutate(bad)
            with self.assertRaises(ValueError):
                materialize_service_provenance(bad)
        too_deep = {"source_ref": edge_id}
        packet["service_provenance"]["definitions"][reference(too_deep)] = too_deep
        with self.assertRaises(ValueError):
            materialize_service_provenance(packet)


if __name__ == "__main__":
    unittest.main()
