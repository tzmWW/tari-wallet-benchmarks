#!/usr/bin/env python3
"""Build an evidence-bound correction manifest for the Mode 1 S1 timing bug."""

import argparse
import copy
import hashlib
import json
import re
import sqlite3
import urllib.request
from datetime import datetime, timedelta, timezone
from pathlib import Path


LOG_TIMESTAMP = r"(?P<timestamp>\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d+)"
HEIGHT_RE = re.compile(LOG_TIMESTAMP + r" .*Base node height:(?P<height>\d+)")
ACCEPTED_RE = re.compile(
    LOG_TIMESTAMP
    + r" .*Transaction \(TxId: (?P<tx_id>\d+)\) submission response from Base Node: "
    + r"TxSubmissionResponse \{ accepted: true,"
)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def parse_log_time(value: str, utc_offset_hours: int) -> datetime:
    whole, fraction = value.split(".", 1)
    local = datetime.strptime(f"{whole}.{fraction[:6]}", "%Y-%m-%d %H:%M:%S.%f")
    return local.replace(tzinfo=timezone(timedelta(hours=utc_offset_hours))).astimezone(timezone.utc)


def parse_db_time(value: str) -> datetime:
    return datetime.strptime(value, "%Y-%m-%d %H:%M:%S.%f").replace(tzinfo=timezone.utc)


def read_wallet_log_evidence(log_dir: Path, utc_offset_hours: int):
    heights = set()
    accepted = {}
    for path in sorted(log_dir.glob("*.log")):
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            if match := HEIGHT_RE.search(line):
                heights.add(
                    (
                        parse_log_time(match["timestamp"], utc_offset_hours),
                        int(match["height"]),
                    )
                )
            if match := ACCEPTED_RE.search(line):
                timestamp = parse_log_time(match["timestamp"], utc_offset_hours)
                accepted[match["tx_id"]] = min(
                    timestamp, accepted.get(match["tx_id"], timestamp)
                )
    if not heights:
        raise ValueError("wallet logs contain no timestamped base-node heights")
    return sorted(heights), accepted


def db_transaction(conn: sqlite3.Connection, tx_id: str):
    row = conn.execute(
        """
        SELECT timestamp, mined_height, status, amount, fee
        FROM completed_transactions
        WHERE printf('%u', tx_id) = ?
        """,
        (tx_id,),
    ).fetchone()
    if row is None:
        raise ValueError(f"console DB lacks completed transaction {tx_id}")
    return row


def db_shape(conn: sqlite3.Connection, tx_id: str):
    inputs = conn.execute(
        "SELECT COUNT(*) FROM outputs WHERE printf('%u', spent_in_tx_id) = ?",
        (tx_id,),
    ).fetchone()[0]
    outputs = conn.execute(
        """
        SELECT commitment
        FROM outputs
        WHERE printf('%u', received_in_tx_id) = ?
        ORDER BY id
        """,
        (tx_id,),
    ).fetchall()
    return inputs, [bytes(row[0]).hex() for row in outputs]


def confirmation_evidence(created_at: datetime, tip_height: int, heights):
    observed = next(
        ((timestamp, height) for timestamp, height in heights if timestamp >= created_at and height >= tip_height),
        None,
    )
    if observed is None:
        raise ValueError(f"wallet logs never observe required tip height {tip_height}")
    observed_at, observed_height = observed
    elapsed = int((observed_at - created_at).total_seconds() * 1000)
    if elapsed < 0:
        raise ValueError("derived confirmation duration is negative")
    return elapsed, observed_at, observed_height


def corrected_mode1_s1(profile: dict, conn: sqlite3.Connection, heights, accepted):
    repetition = profile["modes"]["old_wallet"]["scenarios"]["S1"]["repetitions"][0]
    metrics = repetition["metrics"]
    tx_ids = metrics["tx_ids"]
    verified = metrics["verified_transactions"]
    timings = copy.deepcopy(metrics["tx_timings"])
    observations = copy.deepcopy(metrics["transaction_observations"])
    evidence_rows = []
    if not (len(tx_ids) == len(verified) == len(timings) == len(observations) == 127):
        raise ValueError("Mode 1 S1 does not contain exactly 127 aligned evidence rows")

    for index, (tx_id, transaction, timing, observation) in enumerate(
        zip(tx_ids, verified, timings, observations, strict=True)
    ):
        if transaction["tx_id"] != tx_id or not transaction["confirmed"]:
            raise ValueError(f"verification row is not aligned and confirmed for {tx_id}")
        if tx_id not in accepted:
            raise ValueError(f"raw wallet logs do not prove base-node acceptance for {tx_id}")
        created_raw, mined_height, status, amount, fee = db_transaction(conn, tx_id)
        if (
            status != 6
            or mined_height != transaction["mined_height"]
            or amount != transaction["amount_microtari"]
            or fee != transaction["fee_microtari"]
        ):
            raise ValueError(f"console DB terminal evidence differs for {tx_id}")
        inputs, commitments = db_shape(conn, tx_id)
        expected_outputs = 2 if index < 63 else 8
        if inputs != 1 or len(commitments) != expected_outputs:
            raise ValueError(
                f"console DB shape differs for {tx_id}: inputs={inputs} outputs={len(commitments)}"
            )
        created_at = parse_db_time(created_raw)
        duration, observed_at, observed_height = confirmation_evidence(
            created_at, transaction["tip_height"], heights
        )
        if not created_at <= accepted[tx_id] <= observed_at:
            raise ValueError(f"raw acceptance timestamp is out of order for {tx_id}")
        shape = {
            "input_count": inputs,
            "total_output_count": expected_outputs,
            "payment_output_count": expected_outputs,
            "change_output_count": 0,
            "output_commitments": commitments,
        }
        timing.update(
            {
                "tx_id": tx_id,
                "api_accepted": True,
                "dispatch_to_confirmed_at_c_min_ms": duration,
                **shape,
            }
        )
        for key in ("api_error", "error", "failure_class"):
            timing.pop(key, None)
        observation.update(
            {
                "transaction_id": tx_id,
                "api_accepted": True,
                "api_error": None,
                "terminal_outcome": "confirmed",
                "error": None,
                "confirmation_ms": duration,
                "confirmation_timing_origin": "grpc_dispatch_to_independent_c_min",
                "confirmation_timing_reason": None,
                "amount_microtari": transaction["amount_microtari"],
                "fee_microtari": transaction["fee_microtari"],
                "fee_unavailable_reason": None,
                "fee_disposition": "confirmed_paid",
                "mined_height": transaction["mined_height"],
                "confirmations": transaction["confirmations"],
                "min_confirmations": transaction["min_confirmations"],
                "tip_end_height": transaction["tip_height"],
                **shape,
            }
        )
        observation.pop("failure_class", None)
        evidence_rows.append(
            {
                "transaction_id": tx_id,
                "console_db_created_at": created_at.isoformat().replace("+00:00", "Z"),
                "base_node_accepted_at": accepted[tx_id].isoformat().replace("+00:00", "Z"),
                "c_min_observed_at": observed_at.isoformat().replace("+00:00", "Z"),
                "c_min_observed_height": observed_height,
                "confirmation_ms": duration,
                "status": status,
                "amount_microtari": amount,
                "fee_microtari": fee,
                "mined_height": mined_height,
                "tip_height": transaction["tip_height"],
                **shape,
            }
        )

    return timings, observations, evidence_rows


def source_hashes(paths):
    return {str(path): sha256(path.read_bytes()) for path in paths}


def verify_end_anchor(db_path: Path, height: int, expected_hash: str):
    with sqlite3.connect(db_path) as conn:
        row = conn.execute(
            "SELECT hex(hash) FROM scanned_tip_blocks WHERE height = ? ORDER BY id DESC LIMIT 1",
            (height,),
        ).fetchone()
    if row is None or row[0].lower() != expected_hash:
        raise ValueError("fresh-scan DB does not contain the expected end anchor")


def verify_authority_anchor(url: str, height: int, expected_hash: str):
    separator = "&" if "?" in url else "?"
    request = urllib.request.Request(
        f"{url}{separator}height={height}",
        headers={"User-Agent": "tari-wallet-benchmarks/1"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        document = json.load(response)
    actual = bytes(document["hash"]).hex()
    if document["height"] != height or actual != expected_hash:
        raise ValueError("authority endpoint does not match the expected end anchor")
    return {"url": url, "height": height, "hash": actual, "timestamp": document["timestamp"]}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw", type=Path, required=True)
    parser.add_argument("--raw-out", type=Path, required=True)
    parser.add_argument("--console-db", type=Path, required=True)
    parser.add_argument("--wallet-log-dir", type=Path, required=True)
    parser.add_argument("--manifest-out", type=Path, required=True)
    parser.add_argument("--manifest-path", required=True)
    parser.add_argument("--evidence-out", type=Path, required=True)
    parser.add_argument("--evidence-path", required=True)
    parser.add_argument("--end-anchor-db", type=Path, required=True)
    parser.add_argument("--authority-header-url", required=True)
    parser.add_argument("--export-commit", required=True)
    parser.add_argument("--corrected-at", required=True)
    parser.add_argument("--end-height", type=int, required=True)
    parser.add_argument("--end-hash", required=True)
    parser.add_argument("--log-utc-offset-hours", type=int, required=True)
    args = parser.parse_args()

    raw_bytes = args.raw.read_bytes()
    profile = json.loads(raw_bytes)
    heights, accepted = read_wallet_log_evidence(
        args.wallet_log_dir, args.log_utc_offset_hours
    )
    with sqlite3.connect(args.console_db) as conn:
        timings, observations, evidence_rows = corrected_mode1_s1(
            profile, conn, heights, accepted
        )
    verify_end_anchor(args.end_anchor_db, args.end_height, args.end_hash)
    authority_anchor = verify_authority_anchor(
        args.authority_header_url, args.end_height, args.end_hash
    )
    log_paths = sorted(args.wallet_log_dir.glob("*.log"))
    evidence = {
        "schema_version": 1,
        "description": "Sanitized evidence used to reconstruct Mode 1 S1 reporting rows; no seed words, private keys, or serialized transactions are included.",
        "source_sha256": {
            "console_db": sha256(args.console_db.read_bytes()),
            "end_anchor_db": sha256(args.end_anchor_db.read_bytes()),
            "wallet_logs": source_hashes(log_paths),
        },
        "end_anchor": {
            "fresh_scan_db": {"height": args.end_height, "hash": args.end_hash},
            "authority": authority_anchor,
        },
        "mode1_s1_transactions": evidence_rows,
    }
    args.evidence_out.write_text(
        json.dumps(evidence, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    substantive = [
        {"pointer": "/profile_kind", "value": "final"},
        {"pointer": "/run_complete", "value": True},
        {"pointer": "/provenance/export_commit", "value": args.export_commit},
        {"pointer": "/base_node/tip_end_height", "value": args.end_height},
        {"pointer": "/base_node/tip_end_hash", "value": args.end_hash},
        {"pointer": "/base_node/authority_tip_end_height", "value": args.end_height},
        {"pointer": "/base_node/authority_tip_end_hash", "value": args.end_hash},
        {
            "pointer": "/modes/old_wallet/scenarios/S1/repetitions/0/metrics/tx_timings",
            "value": timings,
        },
        {
            "pointer": "/modes/old_wallet/scenarios/S1/repetitions/0/metrics/transaction_observations",
            "value": observations,
        },
        {
            "pointer": "/modes/old_wallet/scenarios/S1/repetitions/0/metrics/outcome_counts",
            "value": {
                "accepted": 127,
                "attempted": 127,
                "confirmed": 127,
                "rejected": 0,
                "stalled": 0,
                "timed_out": 0,
            },
        },
    ]
    correction = {
        "manifest_schema_version": 1,
        "manifest_path": args.manifest_path,
        "tool": "scripts/correct-profile.py",
        "tool_version": "1",
        "corrected_at": args.corrected_at,
        "raw_profile_sha256": sha256(raw_bytes),
        "raw_profile_size": len(raw_bytes),
        "transformations": copy.deepcopy(substantive),
    }
    manifest = {
        "manifest_schema_version": 1,
        "description": "Reconstruct Mode 1 S1 reporting rows lost by the CoinSplit timing-linkage bug and finalize the completed run.",
        "raw_profile_sha256": sha256(raw_bytes),
        "evidence": {
            "path": args.evidence_path,
            "sha256": sha256(args.evidence_out.read_bytes()),
            "mode1_s1_transactions": 127,
            "end_anchor": {"height": args.end_height, "hash": args.end_hash},
            "log_utc_offset_hours": args.log_utc_offset_hours,
        },
        "transformations": substantive
        + [{"pointer": "/provenance/correction", "value": correction}],
    }
    args.raw_out.write_bytes(raw_bytes)
    args.manifest_out.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(
        json.dumps(
            {
                "accepted_log_ids": len(
                    set(accepted).intersection(
                        profile["modes"]["old_wallet"]["scenarios"]["S1"][
                            "repetitions"
                        ][0]["metrics"]["tx_ids"]
                    )
                ),
                "evidence_sha256": sha256(args.evidence_out.read_bytes()),
                "manifest_sha256": sha256(args.manifest_out.read_bytes()),
                "raw_profile_sha256": sha256(raw_bytes),
                "transactions": len(observations),
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
