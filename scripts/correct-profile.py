#!/usr/bin/env python3
"""Apply an explicit, hash-bound JSON correction manifest."""

import argparse
import copy
import hashlib
import json
from pathlib import Path


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def set_pointer(document: object, pointer: str, value: object) -> None:
    if not pointer.startswith("/"):
        raise ValueError(f"invalid JSON Pointer: {pointer}")
    parts = [part.replace("~1", "/").replace("~0", "~") for part in pointer[1:].split("/")]
    if not parts:
        raise ValueError("root replacement is not supported")
    target = document
    for part in parts[:-1]:
        target = target[int(part)] if isinstance(target, list) else target[part]
    leaf = parts[-1]
    if isinstance(target, list):
        target[int(leaf)] = value
    else:
        target[leaf] = value


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    raw_bytes = args.raw.read_bytes()
    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    actual = digest(raw_bytes)
    if actual != manifest["raw_profile_sha256"]:
        raise SystemExit(f"raw profile SHA-256 mismatch: expected {manifest['raw_profile_sha256']}, got {actual}")
    document = json.loads(raw_bytes)
    for transformation in manifest["transformations"]:
        set_pointer(document, transformation["pointer"], copy.deepcopy(transformation["value"]))
    output = (json.dumps(document, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode("utf-8")
    args.out.write_bytes(output)
    print(json.dumps({
        "raw_profile_sha256": actual,
        "corrected_profile_sha256": digest(output),
        "transformations": [item["pointer"] for item in manifest["transformations"]],
    }, sort_keys=True))


if __name__ == "__main__":
    main()
