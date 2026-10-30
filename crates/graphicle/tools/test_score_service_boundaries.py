import unittest
from score_service_boundaries import score


class Accuracy(unittest.TestCase):
    def test_wrong_service_is_a_false_positive_and_a_missed_target(self):
        labels = {"head": [{"snapshot": "pinned"}], "callers": [
            {"repository_id": "a", "binding": "send", "protocol": "http", "targets": [["b", "serve"]]},
            {"repository_id": "a", "binding": "omitted", "protocol": "grpc", "targets": [["b", "rpc"]]},
            {"repository_id": "a", "binding": "dynamic", "protocol": "event", "targets": []}]}
        def entry(binding, protocol, targets):
            return {"side": "head", "repository_id": "a", "service_boundary": {
                "binding": binding, "direction": "outbound", "transport": {"protocol": protocol},
                "resolution": {"targets": [{"owner": {"repository_id": repo}, "binding": id}
                                           for repo, id in targets]}}}
        packet = {"head": labels["head"], "coverage": {"complete": False}, "entries": [
            entry("send", "http", [("wrong", "serve")]), entry("dynamic", "event", [])]}
        result = score(packet, labels)["overall"]
        self.assertEqual((result["true_positives"], result["false_positives"], result["false_negatives"]), (0, 1, 2))
        self.assertEqual(result["omitted_callers"], 1)
        self.assertEqual(result["correct_negative_callers"], 1)
        self.assertEqual(result["recall"], 0)
        packet["head"] = [{"snapshot": "different"}]
        with self.assertRaises(ValueError):
            score(packet, labels)


if __name__ == "__main__":
    unittest.main()
