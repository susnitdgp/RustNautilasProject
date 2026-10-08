#!/usr/bin/env python3
"""Fail-closed independent ILRC evaluation preflight. Never sends orders."""
import argparse
import datetime as dt
import hashlib
import json
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[1]


def validate(manifest_path, candidate_path=None):
    manifest = json.loads(pathlib.Path(manifest_path).read_text())
    if manifest.get("version") != 1 or manifest.get("research_only") is not True or manifest.get("live_orders_enabled") is not False:
        raise ValueError("Manifest must be version 1, research-only with live orders disabled")
    expected = manifest["sha256_files"]
    if len(expected) != 4:
        raise ValueError("Four frozen research files required")
    for rel, digest in expected.items():
        if pathlib.Path(rel).is_absolute() or ".." in pathlib.Path(rel).parts:
            raise ValueError("Unsafe manifest file path")
        actual = hashlib.sha256((ROOT / rel).read_bytes()).hexdigest()
        if actual != digest:
            raise ValueError(f"Frozen baseline changed: {rel}")
    if candidate_path is None:
        return {"status": "frozen_baseline_verified", "candidate": "not_configured"}
    c = json.loads(pathlib.Path(candidate_path).read_text())
    required = {"symbol", "instrument_token", "expiry", "contract_month", "verified_from_kite_master", "approved_for_research", "data_start", "data_end", "live_orders_enabled"}
    if set(c) != required:
        raise ValueError("Candidate schema mismatch")
    if c["live_orders_enabled"] is not False or c["verified_from_kite_master"] is not True or c["approved_for_research"] is not True:
        raise ValueError("Candidate is not verified and approved for read-only research")
    start, end, expiry = [dt.date.fromisoformat(c[k]) for k in ("data_start", "data_end", "expiry")]
    cutoff = dt.date.fromisoformat(manifest["research_cutoff_date"])
    if not (cutoff < start <= end <= expiry):
        raise ValueError("Future data must postdate frozen research and predate contract expiry")
    if not isinstance(c["instrument_token"], int) or isinstance(c["instrument_token"], bool) or c["instrument_token"] <= 0:
        raise ValueError("Invalid verified Kite instrument token")
    if c["contract_month"] != expiry.strftime("%Y-%m"):
        raise ValueError("Contract month/expiry mismatch")
    if not re.fullmatch(r"CRUDEOIL\d{2}[A-Z]{3}FUT\.MCX", c["symbol"]):
        raise ValueError("Only standard CRUDEOIL futures are supported")
    if c["symbol"] != f"CRUDEOIL{expiry.strftime('%y%b').upper()}FUT.MCX":
        raise ValueError("Contract symbol and expiry mismatch")
    return {"status": "candidate_admitted_for_read_only_research", "instrument": c["symbol"], "data_start": str(start), "data_end": str(end)}


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--manifest", default=str(ROOT / "config/ilrc-frozen-research.json"))
    p.add_argument("--candidate")
    args = p.parse_args()
    print(json.dumps(validate(args.manifest, args.candidate), sort_keys=True))


if __name__ == "__main__":
    main()
