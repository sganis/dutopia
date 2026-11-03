#!/usr/bin/env python3
# script/trash_mover.py
"""Dutopia nightly trash mover (Python 3 stdlib only).

Runs as root from cron on a node that mounts the filesystems:

  0 2 * * * python3 /opt/dutopia/trash_mover.py \\
      --api https://dutopia.apps.ocp.corp.local --token-file /etc/dutopia/trash.token

Local testing (duapi started with TRASH_UNDO_HOURS=0 TRASH_TOKEN=x):

  TRASH_TOKEN=x python3 script/trash_mover.py --dev --api http://localhost:8080

1. POST /api/trash/claim with the token: duapi returns the items queued over
   24h (TRASH_UNDO_HOURS) ago and drops them from its queue. The batch is saved first to
   <state-dir>/<date>.json (rerun it with --batch if a run dies halfway).
2. Each item is skipped if it is gone, inside .trash/.stats, reached through
   a symlink, no longer owned by its owner, a folder holding anything of
   another owner or another mount, owned by a system account (--min-uid), on
   a filesystem not given with --fs, or on one not mounted on this node.
3. Otherwise it is renamed to <fs>/.trash/<date>/<path relative to fs>
   (instant, same filesystem); a folder is then packed into <name>.tar.zst
   and removed. <fs>/.trash is 0700 root, next to <fs>/.stats; a .trash that
   is a symlink or not root's is refused. Every step runs on file descriptors
   opened with O_NOFOLLOW, so swapping a path component for a symlink while
   the cron runs cannot redirect it.
4. <fs>/.trash/<date> folders older than --keep-days are deleted, for every
   filesystem seen in this or earlier batches plus each --purge FS.

Exit status 1 when any item or purge failed (cron mails the output).
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
# Results per POST /api/trash/report (~300 B each, under duapi's 64 KiB).
REPORT_CHUNK = 100


def log(tag, msg):
    print(f"[{tag}] {msg}", flush=True)


def call_api(api, endpoint, token=None, body=None, cafile=None, timeout=120,
             allow_http=False, with_headers=False):
    """GET (no body) or POST (JSON body) <api>/api/<endpoint>; returns the JSON
    reply (and the reply headers with with_headers). Plain http is refused
    unless allow_http: replies tell root what to move."""
    url = api.rstrip("/") + "/api/" + endpoint
    if not url.startswith("https://") and not allow_http:
        raise ValueError(f"refusing non-https API {api} (pass --allow-http to override)")
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


def api_filesystems(api, cafile=None, allow_http=False):
    """The server's FILESYSTEMS list (GET /api/health)."""
    fss = call_api(api, "health", cafile=cafile, allow_http=allow_http).get("filesystems")
    return [f.rstrip("/") or "/" for f in fss or []]


def claim(api, token, cafile=None, timeout=120, allow_http=False, dry_run=False,
          filesystems=None, confirmations=None):
    """POST /api/trash/claim; returns the claimed items (a list). Only items
    of `filesystems` when given (the ones this node mounts: the rest stays
    queued for another node). dry_run asks for the same list without
    removing it from the queue. `confirmations` (a dict) gets whether the
    server applied each option: an older duapi ignores both."""
    params = {}
    if dry_run:
        params["dry_run"] = "1"
    if filesystems is not None:
        params["fs"] = ",".join(filesystems)
    endpoint = "trash/claim" + ("?" + urllib.parse.urlencode(params) if params else "")
    items, headers = call_api(api, endpoint, token, body={}, cafile=cafile, timeout=timeout,
                              allow_http=allow_http, with_headers=True)
    if confirmations is not None:
        confirmations["dry_run"] = headers.get("X-Trash-Dry-Run") == "1"
        confirmations["fs"] = headers.get("X-Trash-Fs-Filter") == "1"
    if not isinstance(items, list):
        raise ValueError(f"claim returned {type(items).__name__}, expected a list")
    return items


def report(api, token, results, cafile=None, allow_http=False):
    """POST the results in chunks: duapi caps request bodies (64 KiB by
    default). Returns the number of chunks that failed."""
    failed = 0
    for i in range(0, len(results), REPORT_CHUNK):
        chunk = results[i:i + REPORT_CHUNK]
        try:
            call_api(api, "trash/report", token, body=chunk, cafile=cafile, allow_http=allow_http)
        except Exception as e:
            failed += 1
            log("err", f"report of {len(chunk)} result(s): {e}")
    log("report", f"{len(results)} result(s) sent" + (f", {failed} chunk(s) failed" if failed else ""))
    return failed


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
        allowed_fs=None, min_uid=None, dry_run=False, results=None):
    """Move every item, then purge; return the number of failures.
    dry_run runs every check but moves and deletes nothing. Each item's
    outcome (status moved/skipped/error, detail, dest) is appended to
    `results` when given, for POST /api/trash/report."""
    errors = 0
    seen = set()

    def outcome(item, status, detail="", dest=None):
        if results is not None:
            results.append({**item, "status": status, "detail": detail, "dest": dest})

    for item in items:
        if item.get("fs"):
            seen.add(fsops.fs_of_item(item))
        path = item.get("path")
        try:
            if dry_run:
                open_target(item, check_mount, allowed_fs, min_uid).close()
                log("would move", path)
            else:
                dest = move_item(item, today, check_mount, allowed_fs, min_uid)
                log("ok", f"{path} -> {dest}")
                outcome(item, "moved", dest=dest)
        except Skip as e:
            log("skip", f"{path}: {e}")
            outcome(item, "skipped", str(e))
        except Exception as e:  # keep going: one bad item must not stop the batch
            errors += 1
            log("err", f"{path}: {e}")
            outcome(item, "error", str(e))
    targets = seen | seen_filesystems(state_dir) | {p.rstrip("/") or "/" for p in purge_fs}
    if allowed_fs is not None:
        targets &= allowed_fs
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
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--api", help="duapi base URL, e.g. https://dutopia.example")
    ap.add_argument("--token-file", help="file holding TRASH_TOKEN (default: the TRASH_TOKEN env var)")
    ap.add_argument("--cafile", help="CA bundle for a private-CA duapi certificate")
    ap.add_argument("--state-dir", help=f"saved batches (default {STATE_DIR}; ~/.dutopia-trash with --dev)")
    ap.add_argument("--keep-days", type=int, default=KEEP_DAYS, help="purge trash older than this")
    ap.add_argument("--purge", action="append", default=[], metavar="FS",
                    help="also purge FS/.trash (repeatable)")
    ap.add_argument("--batch", help="process this saved batch instead of claiming")
    ap.add_argument("--fs", action="append", metavar="FS",
                    help="only act on these filesystems (repeatable; recommended)")
    ap.add_argument("--min-uid", type=int, default=MIN_UID,
                    help=f"never move items of owners below this uid (default {MIN_UID})")
    ap.add_argument("--allow-http", action="store_true",
                    help="accept a plain-http --api (testing only)")
    ap.add_argument("--no-mount-check", action="store_true",
                    help="accept filesystems that are not mount points (testing only)")
    ap.add_argument("--dry-run", action="store_true",
                    help="check and report what would move or be purged; change nothing "
                         "(the server keeps the items queued)")
    ap.add_argument("--dev", action="store_true",
                    help="local testing: --allow-http --no-mount-check --min-uid 0, "
                         "batches in ~/.dutopia-trash")
    args = ap.parse_args(argv)
    if args.dev:
        args.allow_http = args.no_mount_check = True
        args.min_uid = 0
    if not args.state_dir:
        args.state_dir = os.path.expanduser("~/.dutopia-trash") if args.dev else STATE_DIR
    today = datetime.date.today()
    allowed = {f.rstrip("/") or "/" for f in args.fs} if args.fs else None
    purge_fs = list(args.purge)

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


def work(args, ap, today, allowed, purge_fs):
    """Claim (or load --batch), move, purge, report; return the exit code."""
    errors = 0
    token = read_token(args)
    items = []
    if args.batch:
        with open(args.batch) as f:
            items = json.load(f)
    else:
        if not args.api or not token:
            ap.error("--api and a token (--token-file or TRASH_TOKEN) are required "
                     "unless --batch is given")
        try:
            server_fs = api_filesystems(args.api, args.cafile, args.allow_http)
        except Exception as e:
            # Still purge what earlier batches knew about, below.
            errors += 1
            server_fs = None
            log("err", f"cannot read {args.api}/api/health: {e}")
        if server_fs is not None:
            local = []
            if allowed is not None and not allowed & set(server_fs):
                errors += 1
                log("err", f"--fs {', '.join(sorted(allowed))} matches none of the server's "
                           f"FILESYSTEMS ({', '.join(server_fs) or 'none'})")
            else:
                local = [f for f in server_fs if (allowed is None or f in allowed)
                         and (args.no_mount_check or os.path.ismount(f))]
                if not local:
                    log("claim", f"none of the server's FILESYSTEMS ({', '.join(server_fs) or 'none'}) "
                                 "is mounted on this node: nothing to claim")
            purge_fs += local
            if local:
                got = claim_items(args, token, local, today)
                if got is None:
                    return 1
                items, failed = got
                errors += failed

    results = []
    errors += run(items, today, args.state_dir, args.keep_days, purge_fs,
                  check_mount=not args.no_mount_check, allowed_fs=allowed,
                  min_uid=args.min_uid, dry_run=args.dry_run, results=results)
    if results and not args.dry_run and args.api and token:
        errors += report(args.api, token, results, args.cafile, args.allow_http)
    return 1 if errors else 0


def claim_items(args, token, local, today):
    """Claim this node's items and save the batch. Returns (items, errors),
    or None when the run must stop (a dry run the server did not honor)."""
    confirmed = {}
    try:
        items = claim(args.api, token, args.cafile, allow_http=args.allow_http,
                      dry_run=args.dry_run, filesystems=local, confirmations=confirmed)
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
