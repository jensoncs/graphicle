import copy
import unittest

from fixtures import suite
from pilot import compare, diagnose, digest, phases, rollup, verify_packet


class MeasurementTests(unittest.TestCase):
    def fixture(self):
        text = "package p\nfunc Check() bool { return true }\n"
        entry = {"repository_id": "repo", "side": "head", "file": "p.go", "byte_start": 10,
                 "byte_end": len(text) - 1, "file_sha256": digest(text.encode()),
                 "text": text[10:-1], "text_sha256": digest(text[10:-1].encode()), "reasons": []}
        request = {"base": [{"snapshot": "base"}], "head": [{"snapshot": "head"}],
                   "profile": {}, "max_tokens": 1000}
        packet = {**request, "entries": [entry], "tokens": 100, "coverage": {"complete": False}}
        return packet, request, {("head", "repo", "p.go"): text}

    def test_exact_source_and_pin_validation(self):
        packet, request, sources = self.fixture()
        verify_packet(packet, request, sources)
        for mutate in (lambda p: p.update(head=[]), lambda p: p.update(tokens=1001),
                       lambda p: p.update(previous_packet_id="old"),
                       lambda p: p["entries"][0].update(text="fabricated"),
                       lambda p: p["entries"][0].update(file_sha256="wrong"),
                       lambda p: p["entries"][0].update(byte_end=1000)):
            changed = copy.deepcopy(packet)
            mutate(changed)
            with self.assertRaises(ValueError):
                verify_packet(changed, request, sources)

    def test_missing_evidence_never_invents_a_stage_or_low_risk(self):
        packet, _, _ = self.fixture()
        labels = [{"kind": "source", "repository": "r", "side": "head", "file": "p.go", "quote": "return true"},
                  {"kind": "history", "repository": "r", "side": "head", "file": "p.go", "subject": "fix"}]
        packet["coverage"]["budget_omitted"] = 2
        result = diagnose(packet, labels, {"r": "repo"}, False)
        self.assertEqual(result["recall"], 0.5)
        self.assertEqual(result["labels"][1]["status"], "not_requested")
        result = diagnose(packet, labels, {"r": "repo"}, True)
        self.assertEqual(result["labels"][1]["status"], "not_delivered_stage_unknown")
        self.assertFalse(result["may_lower_sentinel_risk"])
        self.assertEqual(result["risk_verdict"], "unmeasured")
        self.assertIsNone(diagnose(packet, [], {"r": "repo"}, False)["recall"])

    def test_label_repository_and_side_cannot_match_wrong_evidence(self):
        packet, _, _ = self.fixture()
        labels = [{"kind": "source", "repository": "r", "side": "base", "file": "p.go", "quote": "return true"}]
        self.assertEqual(diagnose(packet, labels, {"r": "repo"}, False)["delivered"], 0)
        labels[0]["side"] = "head"
        self.assertEqual(diagnose(packet, labels, {"r": "other"}, False)["delivered"], 0)

    def test_failures_count_as_misses_not_successful_empty_cases(self):
        row = {"variant": "static", "budget": 4000, "sample": "first", "required": 2, "status": "error"}
        ok = {**row, "status": "ok", "delivered_labels": [0, 1], "forbidden_hits": 0,
              "elapsed_ms": 10, "tokens": 100, "phase_ms": {"review_pack": 3}}
        result = rollup([row, ok])["static/4000/first"]
        self.assertEqual((result["planned"], result["errors"], result["recall"]), (2, 1, 0.5))
        self.assertEqual(result["latency_ms_completed"], {"samples": 1, "median": 10})

    def test_comparison_catches_losses_errors_and_missing_runs(self):
        old = {"contract": {"fixtures": "same"}, "runs": [{"id": "a", "status": "ok",
               "delivered_labels": [0, 1], "forbidden_hits": 0}]}
        new = copy.deepcopy(old)
        new["runs"][0]["delivered_labels"] = [0]
        self.assertEqual(compare(new, old)[0]["labels"], [1])
        new["runs"][0]["status"] = "error"
        self.assertEqual(compare(new, old)[0]["reason"], "missing_or_failed_run")
        new["runs"] = []
        self.assertTrue(compare(new, old))
        new["contract"] = {}
        with self.assertRaises(ValueError):
            compare(new, old)

    def test_phase_trace_preserves_samples_without_summing_nested_wall_time(self):
        parsed = phases("phase=review_pack ms=15 rss_mb=10.2 peak_rss_mb=12.0\n"
                        "phase=review_pack ms=2 rss_mb=11.0 peak_rss_mb=12.0\n"
                        "phase=marker ms=0 count=3\nunrelated output\n")
        self.assertEqual([p["ms"] for p in parsed], [15, 2])
        self.assertEqual(parsed[0]["process_peak_rss_mb"], 12.0)

    def test_source_labels_bind_to_fixture_bytes_and_negative_controls_exist(self):
        data = suite()
        self.assertEqual(data["provenance"], "authored_synthetic_regression")
        self.assertEqual(len({c["id"] for c in data["cases"]}), 6)
        for case in data["cases"]:
            repos = {r["name"]: r for r in case["repositories"]}
            for label in case["labels"] + case["forbidden"]:
                if label["kind"] == "source":
                    self.assertIn(label["quote"], repos[label["repository"]][label["side"]][label["file"]])


if __name__ == "__main__":
    unittest.main()
