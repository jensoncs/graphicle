"""Materialize one bounded review view without requesting a second full packet."""
import copy
import hashlib


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
    return result
