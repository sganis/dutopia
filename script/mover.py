#!/usr/bin/env python3
# script/mover.py
"""Dutopia nightly mover (Python 3 stdlib only), run as root by cron.

  mover.py --api URL --token-file F --fs FS [--fs FS] [--dry-run]
          [--output DIR]

Claims what was queued over TRASH_UNDO_HOURS ago on the filesystems this
node mounts, saves the batch, and moves each item as is (a rename: instant
at any size; owners, modes and ACLs untouched), one log line per item:
  trash:   <fs>/.trash/<date>/<path>
  archive: <fs>/.archive/<path>.<date>/<name>  (archiver.py packs it later)
With --output DIR both folders live under it instead (one per filesystem,
to test the whole flow without root); the items still move on the real
filesystems.
Items that are gone, not their owner's, a folder holding other owners'
files, or reached through a symlink are skipped (see trash_fsops.py).
Nothing is deleted. Exit status 1 when anything failed. Runbook:
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


STATE_DIR = os.path.expanduser("~/.dutopia-mover")
# What is done with an item, by its queue action. Both movers take the same
# options, so --output flows to both untouched.
MOVERS = {"trash": fsops.move_item, "archive": fsops.stage_item}


def log(tag, msg):
    print(f"[{tag}] {msg}", flush=True)


def action_of(item):
    return item.get("action") or "trash"


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
    """GET /api/health (no token): {"filesystems": [...], "archive": bool}."""
    h = call_api(api, "health", cafile=cafile, allow_http=allow_http)
    return {
        "filesystems": [f.rstrip("/") or "/" for f in h.get("filesystems") or []],
        # An older duapi has no archive queue and would ignore ?action=archive.
        "archive": h.get("archive") is True,
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
    holds it."""
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


def run(items, today, check_mount=True, allowed_fs=None, min_uid=None, dry_run=False,
        output=None):
    """Move every item where its action says; return the number of failures.
    dry_run runs every check but moves nothing. `output` is the --output
    folder: the trash and archive live under it instead of each filesystem
    (to test without root). Each item's result is one log line: ok, skip or
    err."""
    errors = 0
    for item in items:
        path, action = item.get("path"), action_of(item)
        try:
            mover = MOVERS.get(action)
            if mover is None:
                raise ValueError(f"unknown action {action!r}")
            if dry_run:
                fsops.open_target(item, check_mount, allowed_fs, min_uid).close()
                log(f"would {action}", path)
            else:
                dest = mover(item, today, check_mount, allowed_fs, min_uid, output)
                log("ok", f"{path} -> {dest}")
        except fsops.Skip as e:
            log("skip", f"{path}: {e}")
        except Exception as e:  # keep going: one bad item must not stop the batch
            errors += 1
            log("err", f"{path}: {e}")
    return errors


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


def main(argv=None):
    ap = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        epilog="Example: mover.py --api https://dutopia.example --token-file "
               "/etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2 [--dry-run]",
    )
    ap.add_argument("--api", help="duapi URL (the Route)")
    ap.add_argument("--fs", action="append", metavar="FS",
                    help="filesystem this node handles (repeat)")
    ap.add_argument("--dry-run", action="store_true", help="show what would happen, change nothing")
    ap.add_argument("--output", metavar="DIR",
                    help="folder to hold .trash/.archive instead of each filesystem (testing; as a normal user)")
    ap.add_argument("--token-file", help="file with TRASH_TOKEN (default: $TRASH_TOKEN)")
    ap.add_argument("--batch", help="redo a saved batch instead of claiming")
    ap.add_argument("--cafile", help="CA bundle for a private-CA Route")
    ap.add_argument("--dev", action="store_true",
                    help="local test: plain http, no mount points, any uid, state in ~/.dutopia-mover")
    ap.add_argument("--state-dir", help=argparse.SUPPRESS)  # tests and odd setups
    args = ap.parse_args(argv)
    args.allow_http = args.dev
    if not args.state_dir:
        args.state_dir = os.path.expanduser("~/.dutopia-mover") if args.dev else STATE_DIR
    token = read_token(args)
    if not args.batch and (not args.api or not token):
        ap.error("needs --api and a token (--token-file or TRASH_TOKEN); --batch needs neither")

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
        return work(args, token, datetime.date.today())
    finally:
        if lock:
            lock.close()


def work(args, token, today):
    """Claim (or load --batch) and move; return the exit code."""
    allowed = {f.rstrip("/") or "/" for f in args.fs} if args.fs else None
    opts = dict(check_mount=not args.dev, allowed_fs=allowed,
                min_uid=0 if args.dev else fsops.MIN_UID, dry_run=args.dry_run,
                output=args.output)
    if args.batch:
        with open(args.batch) as f:
            return 1 if run(json.load(f), today, **opts) else 0

    try:
        server = api_health(args.api, args.cafile, args.allow_http)
    except Exception as e:
        log("err", f"cannot read {args.api}/api/health: {e}")
        return 1
    server_fs = server["filesystems"]
    if allowed is not None and not allowed & set(server_fs):
        log("err", f"--fs {', '.join(sorted(allowed))} matches none of the server's "
                   f"FILESYSTEMS ({', '.join(server_fs) or 'none'})")
        return 1
    local = local_filesystems(server_fs, allowed, opts["check_mount"])
    if not local:
        log("claim", f"none of the server's FILESYSTEMS ({', '.join(server_fs) or 'none'}) "
                     "is mounted on this node: nothing to claim")
        return 0

    errors = 0
    for action in MOVERS:
        if action == "archive" and not server["archive"]:
            errors += 1
            log("err", f"{args.api} has no archive queue (older duapi?): archive items stay queued")
            continue
        got = claim_items(args, token, local, today, action)
        if got is None:
            return 1
        items, failed = got
        errors += failed + run(items, today, **opts)
    return 1 if errors else 0


def claim_items(args, token, local, today, action):
    """Claim this node's items of `action` and save the batch. Returns
    (items, errors), or None when the run must stop (a dry run the server
    did not honor)."""
    confirmed = {}
    try:
        items = claim(args.api, token, args.cafile, allow_http=args.allow_http,
                      dry_run=args.dry_run, filesystems=local, confirmations=confirmed,
                      action=action)
    except Exception as e:
        log("err", f"claim {action}: {e}")
        return [], 1
    log("claim", f"{len(items)} {action} item(s) on {', '.join(local)}"
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
