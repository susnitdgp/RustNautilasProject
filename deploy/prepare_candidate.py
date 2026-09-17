#!/usr/bin/env python3
"""Package an optimized binary and its exact source as an inactive candidate."""
import datetime, hashlib, json, shutil, subprocess, tarfile
from pathlib import Path
root=Path(__file__).resolve().parents[1]
def git(*args):
    return subprocess.check_output(["git","-C",str(root),*args])
def digest(path):
    h=hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda:f.read(1024*1024),b""):h.update(chunk)
    return h.hexdigest()
binary=root/"target/release/kite-node"
selection=root/"config/production-supertrend.json"
assert binary.is_file(),"Build the release binary first"
settings=json.loads(selection.read_text())
assert settings["interval"]=="5minute" and settings["contracts"]==1
assert not settings["live_orders_enabled"] and not settings["atr_stop_enabled"]
head=git("rev-parse","HEAD").decode().strip()
stamp=datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
destination=Path.home()/"kite-deploy/releases"/(stamp+"-"+head[:7])
destination.mkdir(parents=True,exist_ok=False)
for directory in ["bin","config","doc"]:(destination/directory).mkdir()
shutil.copy2(binary,destination/"bin/kite-node")
shutil.copy2(selection,destination/"config/production-supertrend.json")
for name in ["RedisReference.md","RedisCredentials.md","SlackAlerts.md"]:
    shutil.copy2(root/"doc"/name,destination/"doc"/name)
names=git("ls-files","--cached","--others","--exclude-standard","-z").decode().split("\0")
with tarfile.open(destination/"source.tar.gz","w:gz") as archive:
    for name in sorted(set(names)):
        if name and (root/name).is_file():archive.add(root/name,arcname=name,recursive=False)
status=git("status","--porcelain").decode()
(destination/"source-status.txt").write_text(status)
(destination/"source-changes.patch").write_bytes(git("diff","HEAD"))
manifest={"status":"CANDIDATE_NOT_ACTIVATED","created_utc":stamp,"git_head":head,
    "source_has_uncommitted_changes":bool(status),
    "source_archive_sha256":digest(destination/"source.tar.gz"),
    "binary_sha256":digest(destination/"bin/kite-node"),
    "configuration_sha256":digest(destination/"config/production-supertrend.json"),
    "selection":settings,"build_command":"cargo build --locked --release -p kite-node -j 3",
    "live_service_installed":False,"live_orders_enabled":False,
    "blocker":"Real orders disabled; session paper operation, revision recovery and protected-market native dispatch implemented; manual review, full-session qualification and controlled broker validation remain"}
(destination/"manifest.json").write_text(json.dumps(manifest,indent=2)+"\n")
print(destination)
