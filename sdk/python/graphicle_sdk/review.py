"""Materialize one bounded review view without requesting a second full packet."""
import copy
import hashlib
import json


def materialize_service_provenance(packet):
    """Expand verified packet-local service references without changing the input.

    Keep provenance_ref so an unchanged delta entry is revalidated against the
    new packet's complete table. Expanded prompt size differs from wire tokens.
    """
    def refuse():
        raise ValueError("Invalid service provenance; request a full packet")

    table = packet.get("service_provenance")
    definitions = {}
    if table is not None:
        if not isinstance(table, dict) or table.get("format") != "graphicle.service-provenance.v1":
            refuse()
        definitions = table.get("definitions")
        if not isinstance(definitions, dict):
            refuse()
        for key, value in definitions.items():
            if (not isinstance(value, dict) or hashlib.sha256(json.dumps(value, sort_keys=True,
                    separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode()).hexdigest() != key):
                refuse()
    expanded = {}

    def expand(key, depth=0):
        if not isinstance(key, str) or key not in definitions or depth > 1:
            refuse()
        if depth and any(field in definitions[key] for field in ("source_ref", "target_ref")):
            refuse()
        if key in expanded:
            return expanded[key]
        value = copy.deepcopy(definitions[key])
        for field in ("source", "target"):
            if field + "_ref" in value:
                ref = value.pop(field + "_ref")
                if field in value:
                    refuse()
                value[field] = expand(ref, depth + 1)
        expanded[key] = value
        return value

    for key in definitions:
        expand(key)
    result = copy.deepcopy(packet)
    for entry in result.get("entries", []):
        boundary = entry.get("service_boundary")
        if isinstance(boundary, dict) and "provenance_ref" in boundary:
            assertion = expand(boundary["provenance_ref"])
            if "citations" not in assertion or "manifest" not in assertion:
                refuse()
            for key, value in assertion.items():
                if key not in ("citations", "manifest") and boundary.get(key) != value:
                    refuse()
            boundary["citations"] = copy.deepcopy(assertion["citations"])
            boundary["manifest"] = copy.deepcopy(assertion["manifest"])
        for reason in entry.get("reasons") or []:
            for step in reason.get("steps") or []:
                if "provenance_ref" in step:
                    if step.get("evidence") != "configured_static":
                        refuse()
                    step["provenance"] = copy.deepcopy(expand(step["provenance_ref"]))
    result.pop("tokens", None)
    return result


def apply_review_delta(previous, packet):
    """Verify receipt/body references and apply every invalidation atomically.

    `previous` is the last materialized view, or None for a full packet. Source
    bindings come from the new envelope; matching bytes never preserve old pins.
    """
    def refuse():
        raise ValueError("Invalid review delta; request a full packet")

    prior = {}
    if packet.get("previous_packet_id") is not None:
        if (not previous or previous.get("packet_id") != packet["previous_packet_id"]
                or previous.get("base") != packet.get("base")
                or previous.get("profile") != packet.get("profile")):
            refuse()
        prior = {e["id"]: (c, e) for c in ("entries", "diffs") for e in previous[c]}
        if len(prior) != len(previous["entries"]) + len(previous["diffs"]):
            refuse()
    current = {}
    for key in packet.get("unchanged", []):
        if key not in prior or key in current:
            refuse()
        current[key] = prior[key]
    invalidated = {e["id"] for e in packet.get("invalidated", [])}
    for collection in ("entries", "diffs"):
        for raw in packet[collection]:
            entry = copy.deepcopy(raw)
            key = entry["id"]
            if key in current or key in invalidated:
                refuse()
            reference = entry.pop("text_ref", None)
            if reference is not None:
                if reference is True:
                    reference = {"packet_id": packet.get("previous_packet_id"),
                                 "entry_id": key, "sha256": entry.get("text_sha256")}
                if not isinstance(reference, dict):
                    refuse()
                old = prior.get(reference.get("entry_id"))
                if (not old or old[0] != "entries" or collection != "entries"
                        or reference.get("packet_id") != packet.get("previous_packet_id")
                        or old[1].get("repository_id") != entry.get("repository_id")
                        or "text" in entry):
                    refuse()
                entry["text"] = old[1].get("text", "")
                sha = hashlib.sha256(entry["text"].encode()).hexdigest()
                if sha != reference.get("sha256") or sha != entry.get("text_sha256"):
                    refuse()
            if collection == "entries" and "text_sha256" in entry:
                if hashlib.sha256(entry.get("text", "").encode()).hexdigest() != entry["text_sha256"]:
                    refuse()
            current[key] = (collection, entry)
    order = packet.get("context_order")
    for collection, entry in current.values():
        if collection == "entries" and "text_sha256" in entry:
            if hashlib.sha256(entry.get("text", "").encode()).hexdigest() != entry["text_sha256"]:
                refuse()
    if (not isinstance(order, list) or len(set(order)) != len(order)
            or set(order) != set(current) or set(prior) - set(current) != invalidated):
        refuse()
    result = copy.deepcopy(packet)
    for collection in ("entries", "diffs"):
        result[collection] = [copy.deepcopy(current[key][1]) for key in order if current[key][0] == collection]
    result["unchanged"], result["invalidated"] = [], []
    result.pop("tokens", None)  # Wire token count is not the reconstructed prompt's cost.
    return materialize_service_provenance(result)
