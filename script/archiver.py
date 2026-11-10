#!/usr/bin/env python3
# script/archiver.py
"""Dutopia archiver (Python 3 stdlib only), run by hand as root.

  archiver.py --fs FS [--fs FS] [--dry-run]

Packs what mover.py left in <fs>/.archive: each <path>.<date>/<name>
becomes <path>.<date>.tar.gz (owned by the item's owner), read back whole
before the staged copy is removed. Works on the filesystem only (no API,
no token), whenever you run it. If tar fails (e.g. the filesystem is full)
the item stays staged for the next run. One log line per item; exit status
1 when anything failed. Runbook: deploy/TRASH.md.

Needs trash_fsops.py next to it.
"""
import argparse
import os
import sys

import trash_fsops as fsops

STATE_DIR = "/var/lib/dutopia-mover"


def log(tag, msg):
    print(f"[{tag}] {msg}", flush=True)


def acquire_lock(state_dir):
    """Hold <state_dir>/.archiver.lock: two archivers would pack the same items."""
    import fcntl
    os.makedirs(state_dir, mode=0o700, exist_ok=True)
    f = open(os.path.join(state_dir, ".archiver.lock"), "w")
    try:
        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        f.close()
        raise
    return f


def pack(filesystems, dry_run=False, check_mount=True):
    """Pack every staged item on `filesystems`; return the number of failures."""
    errors = 0
    for fs in filesystems:
        if check_mount and not os.path.ismount(fs):
            errors += 1
            log("err", f"{fs} is not a mount point on this node")
            continue
        try:
            for staged, archive, err in fsops.pack_archive(fs, dry_run):
                if err is not None:
                    errors += 1
                    log("err", f"{staged}: {err}")
                elif dry_run:
                    log("would pack", staged)
                else:
                    log("ok", f"{staged} -> {archive}")
        except (OSError, RuntimeError) as e:
            errors += 1
            log("err", f"{fs}: {e}")
    return errors


def main(argv=None):
    ap = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        epilog="Example: archiver.py --fs /red/nas1 --fs /red/nas2 [--dry-run]",
    )
    ap.add_argument("--fs", action="append", required=True, metavar="FS",
                    help="filesystem whose .archive to pack (repeat)")
    ap.add_argument("--dry-run", action="store_true", help="list what would be packed, change nothing")
    ap.add_argument("--dev", action="store_true",
                    help="local test: no mount points, lock in ~/.dutopia-mover")
    ap.add_argument("--state-dir", help=argparse.SUPPRESS)  # tests and odd setups
    args = ap.parse_args(argv)
    if not args.state_dir:
        args.state_dir = os.path.expanduser("~/.dutopia-mover") if args.dev else STATE_DIR
    filesystems = sorted({f.rstrip("/") or "/" for f in args.fs})

    lock = None
    if not args.dry_run:
        try:
            lock = acquire_lock(args.state_dir)
        except BlockingIOError:
            log("err", f"another archiver holds {os.path.join(args.state_dir, '.archiver.lock')}")
            return 1
        except OSError as e:
            log("err", f"cannot use state dir {args.state_dir}: {e}")
            return 1
    try:
        return 1 if pack(filesystems, args.dry_run, check_mount=not args.dev) else 0
    finally:
        if lock:
            lock.close()


if __name__ == "__main__":
    sys.exit(main())
