"""Local evaluation diagnostics; no telemetry and no changes to served context."""
import re


def diagnostics(packet, stderr, required=()):
    # A case/variant's artifact path is its correlation ID. Preserve repeated
    # phases; these are nested timings and must never be summed as total latency.
    phases = []
    for line in stderr.splitlines():
        match = re.fullmatch(r"phase=(\S+) ms=(\d+) rss_mb=([\d.]+) peak_rss_mb=([\d.]+)", line)
        if match:
            name, ms, rss, peak = match.groups()
            phases.append({"phase": name, "ms": int(ms), "rss_mb": float(rss),
                           "peak_rss_mb": float(peak)})
    result = {"phases": phases}
    for key in ("route", "mechanisms", "omitted", "source_state", "source_age",
                "local_reranker", "symbol_links", "code_graph", "retrieval_defaults"):
        if key in packet:
            result[key] = packet[key]
    if required:
        # Debug output is bounded to the requested top candidates. Absence here
        # is NOT proof that a source was unindexed or absent from every arm.
        candidates = packet.get("candidates_debug")
        result["required_spans"] = []
        for gold in required:
            visible = None if candidates is None else any(
                c.get("id") == "file:" + gold["file"]
                and isinstance(c.get("byte_start"), int)
                and isinstance(c.get("byte_end"), int)
                and c["byte_start"] <= gold["byte_start"]
                and c["byte_end"] >= gold["byte_end"] for c in candidates)
            result["required_spans"].append({"file": gold["file"],
                "byte_start": gold["byte_start"], "byte_end": gold["byte_end"],
                "contained_in_observed_candidate": visible})
    return result
