#!/usr/bin/env python3
# script/archive_mover.py
"""Dutopia nightly archiver (Python 3 stdlib only), run as root by cron.

  archive_mover.py --api URL --token-file F --fs FS [--fs FS] [--dry-run]

Claims the items queued for the archive over TRASH_UNDO_HOURS ago on the
filesystems this node mounts, saves the batch, packs each into
<fs>/.archive/<path>.<date>.tar.gz, removes the original and reports the
results. Same checks as the trash (trash_fsops.py): items that are gone, not
their owner's, a folder holding other owners' files, or reached through a
symlink are skipped. Archives are never purged. Restore one as root from its
old folder: tar -xpzf <archive> (owners and modes are kept). Exit status 1
when anything failed. Runbook: deploy/TRASH.md.

Needs trash_mover.py and trash_fsops.py next to it.
"""
import argparse
import datetime
import json
import os
import sys

import trash_mover as tm
from trash_fsops import MIN_UID, archive_item

STATE_DIR = "/var/lib/dutopia-archive"
log = tm.log


def main(argv=None):
    ap = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        epilog="Example: archive_mover.py --api https://dutopia.example --token-file "
               "/etc/dutopia/trash.token --fs /red/nas1 --fs /red/nas2 [--dry-run]",
    )
    ap.add_argument("--api", help="duapi URL (the Route)")
    ap.add_argument("--fs", action="append", metavar="FS",
                    help="filesystem this node handles (repeat)")
    ap.add_argument("--dry-run", action="store_true", help="show what would happen, change nothing")
    ap.add_argument("--token-file", help="file with TRASH_TOKEN (default: $TRASH_TOKEN)")
    ap.add_argument("--batch", help="process a saved batch instead of claiming")
    ap.add_argument("--cafile", help="CA bundle for a private-CA Route")
    ap.add_argument("--dev", action="store_true",
                    help="local test: plain http, no mount points, any uid, state in ~/.dutopia-archive")
    ap.add_argument("--state-dir", help=argparse.SUPPRESS)  # tests and odd setups
    args = ap.parse_args(argv)
    args.allow_http = args.dev
    if not args.state_dir:
        args.state_dir = os.path.expanduser("~/.dutopia-archive") if args.dev else STATE_DIR
    token = tm.read_token(args)
    if not args.batch and (not args.api or not token):
        ap.error("needs --api and a token (--token-file or TRASH_TOKEN)")

    lock = None
    if not args.dry_run:
        try:
            lock = tm.acquire_lock(args.state_dir)
        except BlockingIOError:
            log("err", f"another run holds {os.path.join(args.state_dir, '.lock')}: not starting")
            return 1
        except OSError as e:
            log("err", f"cannot use state dir {args.state_dir}: {e}")
            return 1
    try:
        return work(args, token, datetime.date.today())
    finally:
        if lock:
            lock.close()


def claim_archive_items(args, token, allowed, today, check_mount):
    """Claim this node's archive items and save the batch: (items, errors),
    or None when the run must stop."""
    try:
        health = tm.call_api(args.api, "health", cafile=args.cafile, allow_http=args.allow_http)
    except Exception as e:
        log("err", f"cannot read {args.api}/api/health: {e}")
        return None
    if health.get("archive") is not True:
        # An older duapi ignores ?action=archive and would hand over the trash.
        log("err", f"{args.api} has no archive queue (older duapi?): upgrade it first; nothing claimed")
        return None
    server_fs = [f.rstrip("/") or "/" for f in health.get("filesystems") or []]
    if allowed is not None and not allowed & set(server_fs):
        log("err", f"--fs {', '.join(sorted(allowed))} matches none of the server's "
                   f"FILESYSTEMS ({', '.join(server_fs) or 'none'})")
        return None
    local = tm.local_filesystems(server_fs, allowed, check_mount)
    if not local:
        log("claim", f"none of the server's FILESYSTEMS ({', '.join(server_fs) or 'none'}) "
                     "is mounted on this node: nothing to claim")
        return [], 0
    return tm.claim_items(args, token, local, today, action="archive")


def work(args, token, today):
    """Claim (or load --batch), archive, report; return the exit code."""
    check_mount = not args.dev
    allowed = {f.rstrip("/") or "/" for f in args.fs} if args.fs else None
    if args.batch:
        with open(args.batch) as f:
            items, errors = json.load(f), 0
    else:
        got = claim_archive_items(args, token, allowed, today, check_mount)
        if got is None:
            return 1
        items, errors = got
    results = []
    errors += tm.run(items, today, args.state_dir, 0, [], check_mount=check_mount,
                     allowed_fs=allowed, min_uid=0 if args.dev else MIN_UID,
                     dry_run=args.dry_run, results=results, do_purge=False, mover=archive_item)
    if results and not args.dry_run and args.api and token:
        errors += tm.report(args.api, token, results, args.cafile, args.allow_http)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
