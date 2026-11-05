#!/usr/bin/env python3
# script/trash_mover.py
"""Dutopia nightly trash mover (Python 3 stdlib only), run as root by cron.

  trash_mover.py [move|purge] --api URL --token-file F --fs FS [--fs FS] [--dry-run]

move: claim the items queued over TRASH_UNDO_HOURS ago on the filesystems
this node mounts, save the batch, move each to <fs>/.trash/<date>/<path>
(a rename: folders stay folders), log each result. Items that are gone, not their
owner's, a folder holding other owners' files, or reached through a symlink
are skipped (see trash_fsops.py).
purge: delete <fs>/.trash/<date> folders older than TRASH_KEEP_DAYS.
No command does both. Exit status 1 when anything failed. Runbook:
deploy/TRASH.md.
"""
import argparse
import datetime
import json
import os
import ssl
import sys
import urllib.parse
import urllib.request

import trash_fsops as fsops
from trash_fsops import (  # noqa: F401  (re-exported for callers and tests)
    MIN_UID, Skip, check_tree, move_item, open_target, open_trash, precheck, purge,
    skip_reason,
)

STATE_DIR = "/var/lib/dutopia-trash"
KEEP_DAYS = 30


def log(tag, msg):
    print(f"[{tag}] {msg}", flush=True)


def call_api(api, endpoint, token=None, body=None, cafile=None, timeout=120,
             allow_http=False, with_headers=False):
    """GET (no body) or POST (JSON body) <api>/api/<endpoint>; returns the JSON
    reply (and the reply headers with with_headers). Plain http is refused
    unless allow_http: replies tell root what to move."""
    url = api.rstrip("/") + "/api/" + endpoint
    if not url.startswith("https://") and not allow_http:
        raise ValueError(f"refusing non-https API {api} (use https; --dev allows http for local tests)")
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, headers=headers,
                                 method="GET" if body is None else "POST")
    ctx = ssl.create_default_context(cafile=cafile) if url.startswith("https") else None
    with urllib.request.urlopen(req, timeout=timeout, context=ctx) as resp:
        data = json.load(resp)
        return (data, resp.headers) if with_headers else data


def api_health(api, cafile=None, allow_http=False):
    """GET /api/health (no token): {"filesystems": [...], "keep_days": N or None}."""
    h = call_api(api, "health", cafile=cafile, allow_http=allow_http)
    days = h.get("trash_keep_days")
    return {
        "filesystems": [f.rstrip("/") or "/" for f in h.get("filesystems") or []],
        "keep_days": days if isinstance(days, int) and days >= 0 else None,
    }


def claim(api, token, cafile=None, timeout=120, allow_http=False, dry_run=False,
          filesystems=None, confirmations=None, action=None):
    """POST /api/trash/claim; returns the claimed items (a list). Only items
    of `filesystems` when given (the ones this node mounts: the rest stays
    queued for another node). dry_run asks for the same list without
    removing it from the queue. `action` "archive" asks for the archive's
    items (default: the trash's). `confirmations` (a dict) gets whether the
    server applied each option: an older duapi ignores them."""
    params = {}
    if dry_run:
        params["dry_run"] = "1"
    if filesystems is not None:
        params["fs"] = ",".join(filesystems)
    if action:
        params["action"] = action
    endpoint = "trash/claim" + ("?" + urllib.parse.urlencode(params) if params else "")
    items, headers = call_api(api, endpoint, token, body={}, cafile=cafile, timeout=timeout,
                              allow_http=allow_http, with_headers=True)
    if confirmations is not None:
        confirmations["dry_run"] = headers.get("X-Trash-Dry-Run") == "1"
        confirmations["fs"] = headers.get("X-Trash-Fs-Filter") == "1"
    if not isinstance(items, list):
        raise ValueError(f"claim returned {type(items).__name__}, expected a list")
    return items


def acquire_lock(state_dir):
    """Hold <state_dir>/.lock for this run; BlockingIOError if another run
    holds it (a run with huge folders to pack can outlast a day)."""
    import fcntl
    os.makedirs(state_dir, mode=0o700, exist_ok=True)
    f = open(os.path.join(state_dir, ".lock"), "w")
    try:
        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        f.close()
        raise
    return f


def save_batch(state_dir, today, items):
    """Append `items` to <state_dir>/<date>.json (tmp + rename); return its path."""
    os.makedirs(state_dir, mode=0o700, exist_ok=True)
    path = os.path.join(state_dir, f"{today.isoformat()}.json")
    old = []
    if os.path.exists(path):
        with open(path) as f:
            old = json.load(f)
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(old + list(items), f, indent=1)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)
    return path


def seen_filesystems(state_dir):
    """Every `fs` named in a saved batch."""
    out = set()
    try:
        names = os.listdir(state_dir)
    except FileNotFoundError:
        return out
    for name in names:
        if not name.endswith(".json"):
            continue
        try:
            with open(os.path.join(state_dir, name)) as f:
                out.update(i["fs"] for i in json.load(f) if i.get("fs"))
        except (OSError, ValueError, TypeError, KeyError, AttributeError):
            continue
    return out


def run(items, today, state_dir, keep_days, purge_fs, check_mount=True,
        allowed_fs=None, min_uid=None, dry_run=False, do_purge=True, mover=None):
    """Move every item (with `mover`, default move_item), then purge; return
    the number of failures. dry_run runs every check but moves and deletes
    nothing. Each item's result is one log line: ok, skip or err."""
    errors = 0
    seen = set()

    for item in items:
        if item.get("fs"):
            seen.add(fsops.fs_of_item(item))
        path = item.get("path")
        try:
            if dry_run:
                open_target(item, check_mount, allowed_fs, min_uid).close()
                log("would move", path)
            else:
                dest = (mover or move_item)(item, today, check_mount, allowed_fs, min_uid)
                log("ok", f"{path} -> {dest}")
        except Skip as e:
            log("skip", f"{path}: {e}")
        except Exception as e:  # keep going: one bad item must not stop the batch
            errors += 1
            log("err", f"{path}: {e}")
    if not do_purge:
        return errors
    targets = seen | seen_filesystems(state_dir) | {p.rstrip("/") or "/" for p in purge_fs}
    if allowed_fs is not None:
        targets &= allowed_fs
    if targets:
        log("purge", f"trash older than {keep_days} day(s) on {', '.join(sorted(targets))}")
    for fs in sorted(targets):
        if check_mount and not os.path.ismount(fs):
            continue
        try:
            for p in purge(fs, today, keep_days, dry_run):
                log("would purge" if dry_run else "purged", p)
        except (OSError, RuntimeError) as e:
            errors += 1
            log("err", f"purge {fs}: {e}")
    return errors


def main(argv=None):
    ap = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        epilog="Example: trash_mover.py --api https://dutopia.example --token-file "
               "/etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2 [--dry-run]",
    )
    ap.add_argument("command", nargs="?", choices=["move", "purge"],
                    help="only move, or only purge (default: both)")
    ap.add_argument("--api", help="duapi URL (the Route)")
    ap.add_argument("--fs", action="append", metavar="FS",
                    help="filesystem this node handles (repeat)")
    ap.add_argument("--dry-run", action="store_true", help="show what would happen, change nothing")
    ap.add_argument("--token-file", help="file with TRASH_TOKEN (default: $TRASH_TOKEN)")
    ap.add_argument("--keep-days", type=int, help="purge age (default: the server's TRASH_KEEP_DAYS)")
    ap.add_argument("--batch", help="process a saved batch instead of claiming")
    ap.add_argument("--cafile", help="CA bundle for a private-CA Route")
    ap.add_argument("--dev", action="store_true",
                    help="local test: plain http, no mount points, any uid, state in ~/.dutopia-trash")
    ap.add_argument("--state-dir", help=argparse.SUPPRESS)  # tests and odd setups
    args = ap.parse_args(argv)
    args.purge_only, args.move_only = args.command == "purge", args.command == "move"
    args.allow_http = args.no_mount_check = args.dev
    args.min_uid = 0 if args.dev else MIN_UID
    if not args.state_dir:
        args.state_dir = os.path.expanduser("~/.dutopia-trash") if args.dev else STATE_DIR
    today = datetime.date.today()
    allowed = {f.rstrip("/") or "/" for f in args.fs} if args.fs else None
    purge_fs = sorted(allowed or [])

    lock = None
    if not args.dry_run:
        try:
            lock = acquire_lock(args.state_dir)
        except BlockingIOError:
            log("err", f"another run holds {os.path.join(args.state_dir, '.lock')}: not starting")
            return 1
        except OSError as e:
            # Before claiming: without a state dir the batch could not be saved.
            log("err", f"cannot use state dir {args.state_dir}: {e}")
            return 1
    try:
        return work(args, ap, today, allowed, purge_fs)
    finally:
        if lock:
            lock.close()


def read_token(args):
    if args.token_file:
        with open(args.token_file) as f:
            return f.read().strip()
    return os.environ.get("TRASH_TOKEN", "").strip()


def local_filesystems(server_fs, allowed, check_mount=True):
    """The server's filesystems this node handles: in --fs (when given) and
    mounted here."""
    return [f for f in server_fs if (allowed is None or f in allowed)
            and (not check_mount or os.path.ismount(f))]


def work(args, ap, today, allowed, purge_fs):
    """Claim (or load --batch) and move, then purge, as the command allows
    (move, purge, or both); return the exit code."""
    errors = 0
    do_move, do_purge = not args.purge_only, not args.move_only
    token = read_token(args)
    if do_move and not args.batch and (not args.api or not token):
        ap.error("moving needs --api and a token (--token-file or TRASH_TOKEN); "
                 "`purge` needs neither")
    server = None
    if args.api:
        try:
            server = api_health(args.api, args.cafile, args.allow_http)
        except Exception as e:
            # Still purge what earlier batches knew about, below.
            errors += 1
            log("err", f"cannot read {args.api}/api/health: {e}")
    keep_days = args.keep_days
    if keep_days is None:
        keep_days = server["keep_days"] if server and server["keep_days"] is not None else KEEP_DAYS

    local = []
    if server is not None:
        server_fs = server["filesystems"]
        if allowed is not None and not allowed & set(server_fs):
            errors += 1
            log("err", f"--fs {', '.join(sorted(allowed))} matches none of the server's "
                       f"FILESYSTEMS ({', '.join(server_fs) or 'none'})")
        else:
            local = local_filesystems(server_fs, allowed, not args.no_mount_check)
            if not local and do_move and not args.batch:
                log("claim", f"none of the server's FILESYSTEMS ({', '.join(server_fs) or 'none'}) "
                             "is mounted on this node: nothing to claim")
        purge_fs += local

    items = []
    if do_move and args.batch:
        with open(args.batch) as f:
            items = json.load(f)
    elif do_move and local:
        got = claim_items(args, token, local, today)
        if got is None:
            return 1
        items, failed = got
        errors += failed

    errors += run(items, today, args.state_dir, keep_days, purge_fs,
                  check_mount=not args.no_mount_check, allowed_fs=allowed,
                  min_uid=args.min_uid, dry_run=args.dry_run, do_purge=do_purge)
    return 1 if errors else 0


def claim_items(args, token, local, today, action=None):
    """Claim this node's items (of `action`, default the trash's) and save
    the batch. Returns (items, errors), or None when the run must stop (a
    dry run the server did not honor)."""
    confirmed = {}
    try:
        items = claim(args.api, token, args.cafile, allow_http=args.allow_http,
                      dry_run=args.dry_run, filesystems=local, confirmations=confirmed,
                      action=action)
    except Exception as e:
        log("err", f"claim: {e}")
        return [], 1
    log("claim", f"{len(items)} item(s) on {', '.join(local)}"
        + (" (dry run: left queued)" if args.dry_run else ""))
    ignored_dry_run = args.dry_run and not confirmed.get("dry_run")
    if not confirmed.get("fs"):
        log("warn", "the server ignored the filesystem filter (older duapi?): items of "
                    "filesystems not mounted here are skipped; upgrade duapi before the cron")
    if args.dry_run and not ignored_dry_run:
        return items, 0
    errors = 0
    try:
        log("saved", save_batch(args.state_dir, today, items))
    except Exception as e:
        # The server already dropped these: keep a copy in the cron mail.
        errors += 1
        log("err", f"saving batch: {e}; batch follows")
        print(json.dumps(items), flush=True)
    if ignored_dry_run:
        log("err", "the server ignored dry_run (older duapi?) and really claimed these "
                   "items; nothing was moved. Upgrade duapi, then process them with --batch.")
        return None
    return items, errors



if __name__ == "__main__":
    sys.exit(main())
