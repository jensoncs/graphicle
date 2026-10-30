"""Optional, revision-bound evidence. Never grants review coverage or changes primary text."""
import hashlib
import json
import re


def compose(primary, packet, repository_id, snapshot, max_bytes=12000):
    """Accept authenticated structured excerpts only; ignore server-supplied prompt prose."""
    result = {"primary": primary, "secondary": {"status": "empty", "evidence": []},
              "policy": {"authority": "secondary_source", "may_reduce_review_scope": False,
                         "may_reuse_review_coverage": False, "source_text_is_instructions": False}}
    secondary = result["secondary"]
    if not isinstance(packet, dict) or packet.get("repository_id") != repository_id:
        secondary["status"] = "invalid_repository"
        return result
    if packet.get("snapshot") != snapshot:
        secondary["status"] = "stale_snapshot"
        return result
    if not isinstance(packet.get("evidence"), list):
        secondary["status"] = "invalid_evidence"
        return result
    kept, seen, omitted = [], set(), 0
    for source in packet["evidence"]:
        if not isinstance(source, dict):
            secondary["status"] = "invalid_evidence"
            return result
        identifier = source.get("id", "")
        file_id = isinstance(identifier, str) and identifier.startswith("file:")
        if source.get("source_kind", "repository_file" if file_id else None) != "repository_file":
            omitted += 1
            continue
        start, end, text = source.get("byte_start"), source.get("byte_end"), source.get("text")
        sha, path = source.get("sha256"), source.get("path", identifier[5:] if file_id else None)
        if (source.get("repository_id", repository_id) != repository_id
                or not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{64}", sha)
                or not isinstance(path, str) or not path or path.startswith(("/", "\\"))
                or ".." in path.replace("\\", "/").split("/")
                or type(start) is not int or type(end) is not int or start < 0 or end <= start
                or not isinstance(text, str) or len(text.encode("utf-8")) != end - start):
            secondary["status"] = "invalid_evidence"
            return result
        identity = (path, sha, start, end)
        if identity in seen:
            continue
        seen.add(identity)
        item = {"repository_id": repository_id, "snapshot": snapshot, "path": path,
                "sha256": sha, "byte_start": start, "byte_end": end, "text": text,
                "text_sha256": hashlib.sha256(text.encode()).hexdigest(), "authority": "quoted_source"}
        candidate = kept + [item]
        if len(json.dumps(candidate, ensure_ascii=False, separators=(",", ":")).encode()) > max_bytes - 512 or len(kept) >= 40:
            omitted += 1
            continue
        kept.append(item)
    secondary.update(status="included" if kept else "empty", evidence=kept, omitted=omitted,
                     repository_id=repository_id, snapshot=snapshot,
                     complete=False)
    secondary["content_id"] = hashlib.sha256(json.dumps(secondary, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()
    return result
