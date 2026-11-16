import tempfile
import unittest
from pathlib import Path

from host_map import build

AGENT = """applicationName: alert-review-agent
namespace: siren
ingress:
  enabled: true
  hosts:
    - host: alert-review-agent.sharechat.internal
      paths:
        - path: /*
"""
POST = """applicationName: post-service
namespace: post-service
serviceDuplication:
  enabled: true
  paths:
    - path: /v1/get
ingress:
  enabled: true
  hosts:
    - host: post-service.sharechat.internal
      paths:
        - path: /*
        - path: /v1/batch
          serviceName: post-service-go
    - host: "{{ .Values.host }}"
      paths:
        - path: /*
"""
OFF = """applicationName: quiet
namespace: quiet
ingress:
  enabled: false
  hosts:
    - host: quiet.sharechat.internal
      paths: [{path: /*}]
"""
META = {"hosts": {
    "auth-service.sharechat.internal": ["sharechat/auth-service"],
    "alert-review-agent.sharechat.internal": ["platform/alert-review-agent"],
    "*.sharechat.com": ["moj/mum-nexus-gateway"],
}}


class HostMapTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        root = Path(self.dir.name)
        for repo, rel, text in [
            ("siren", "alert-review/infra/production/values.yaml", AGENT),
            ("post-service", "node/infra/production/values.yaml", POST),
            ("quiet", "infra/production/values.yaml", OFF),
            ("broken", "infra/production/values.yaml", "a: [unclosed\n"),
            ("siren", "docs/values.yaml", AGENT.replace("alert-review-agent.", "not-infra.")),
            ("dora", "infra/production/values.yaml", "---\n" + AGENT.replace("alert-review-agent", "dora") + "---\nother: 1\n"),
            ("tpl", "{{cookiecutter.name}}/infra/production/values.yaml", "a: [unclosed\n"),
        ]:
            path = root / repo / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        self.map = build(META, root)

    def tearDown(self):
        self.dir.cleanup()

    def test_repository_host_resolves_to_its_application(self):
        host = self.map["hosts"]["alert-review-agent.sharechat.internal"]
        self.assertEqual(host["status"], "resolved")
        self.assertEqual(host["applications"], ["alert-review-agent"])
        repo = [r for r in host["routes"] if r["source"] == "repository"]
        self.assertEqual(repo[0]["namespace"], "siren")
        self.assertEqual(repo[0]["file"], "alert-review/infra/production/values.yaml")
        self.assertEqual(repo[0]["environment"], "production")
        self.assertEqual({r["source"] for r in host["routes"]}, {"weaver", "repository"})
        self.assertEqual(host["observed_applications"], ["alert-review-agent"])

    def test_multi_document_values_are_read(self):
        self.assertEqual(self.map["hosts"]["dora.sharechat.internal"]["applications"], ["dora"])

    def test_the_covered_suffix_is_declared(self):
        self.assertEqual(self.map["suffixes"], [".sharechat.internal"])
        self.assertEqual(self.map["format"], "graphicle.host-map.v1")

    def test_weaver_only_host_is_kept(self):
        host = self.map["hosts"]["auth-service.sharechat.internal"]
        self.assertEqual(host["applications"], ["auth-service"])
        self.assertEqual(host["routes"][0]["entity"], "sharechat")

    def test_path_split_host_is_ambiguous_and_dupe_is_the_same_application(self):
        host = self.map["hosts"]["post-service.sharechat.internal"]
        self.assertEqual(host["status"], "ambiguous")
        self.assertEqual(host["applications"], ["post-service", "post-service-go"])
        services = {(r["path"], r["service"]) for r in host["routes"]}
        self.assertIn(("/v1/get", "post-service-dupe"), services)
        self.assertIn(("/v1/batch", "post-service-go"), services)

    def test_disabled_templated_external_and_non_infra_hosts_are_left_out(self):
        self.assertNotIn("quiet.sharechat.internal", self.map["hosts"])
        self.assertNotIn("not-infra.sharechat.internal", self.map["hosts"])
        self.assertFalse(any("{{" in h or h.endswith(".com") for h in self.map["hosts"]))

    def test_unreadable_values_are_reported(self):
        self.assertEqual(self.map["sources"]["unreadable_values"], ["broken/infra/production/values.yaml"])


if __name__ == "__main__":
    unittest.main()
