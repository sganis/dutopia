#!/usr/bin/env python3
# script/trash_fsops.py
"""Filesystem side of the Dutopia trash and archive movers (trash_mover.py,
archive_mover.py), run as root.

Every step works on file descriptors opened with O_NOFOLLOW from the
filesystem root, so a user swapping a path component for a symlink while the
cron runs cannot redirect a check, a rename or a delete:

- precheck / open_target: is this item one owner's, on this filesystem, not
  reached through a symlink, not inside .trash/.archive/.stats? (raise Skip
  if not)
- move_item: rename it into <fs>/.trash/<date>/<path>, pack folders as
  .tar.zst
- archive_item: pack it into <fs>/.archive/<path>.<date>.tar.gz
- purge: delete <fs>/.trash/<date> folders past the keep window
"""
import datetime
import errno
import os
import pwd
import re
import shutil
import stat
import subprocess

KEEP_DAYS = 30
# Owners below this uid are system accounts: never moved.
MIN_UID = 1000
RESERVED = (".trash", ".archive", ".stats")
DATE_DIR = re.compile(r"^\d{4}-\d{2}-\d{2}$")


def fs_of_item(item):
    return (item.get("fs") or "").rstrip("/") or "/"


class Skip(Exception):
    """The item stays where it is, for this reason."""


def _open_dir(name, dir_fd):
    """Open a folder below dir_fd without following a symlink."""
    return os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=dir_fd)


def precheck(item, check_mount=True, allowed_fs=None, min_uid=None):
    """Checks that need no walking. Returns (fs, rel, owner uid) or raises Skip."""
    path, owner = item.get("path") or "", item.get("owner") or ""
    if not path.startswith("/") or not item.get("fs") or not owner:
        raise Skip("bad item")
    fs = fs_of_item(item)
    rel = os.path.relpath(os.path.normpath(path), fs)
    if rel == ".":
        raise Skip("is the filesystem root")
    if rel == ".." or rel.startswith("../"):
        raise Skip(f"outside its filesystem {fs}")
    top = rel.split("/")[0]
    if top in RESERVED:
        raise Skip(f"inside {top}")
    if allowed_fs is not None and fs not in allowed_fs:
        raise Skip(f"{fs} is not a filesystem given with --fs")
    if check_mount and not os.path.ismount(fs):
        raise Skip(f"{fs} is not a mount point on this node")
    try:
        uid = pwd.getpwnam(owner).pw_uid
    except KeyError:
        raise Skip(f"unknown owner {owner}")
    floor = MIN_UID if min_uid is None else min_uid
    if uid < floor:
        raise Skip(f"owner {owner} is a system account (uid {uid} < {floor})")
    return fs, rel, uid


def check_tree(parent_fd, name, st, uid, dev, owner):
    """Raise Skip unless every entry under folder `name` belongs to `uid` and
    to the filesystem `dev`: a folder holding other users' files, or another
    mount, is not one user's to trash."""
    dfd = _open_dir(name, parent_fd)
    try:
        top = os.fstat(dfd)
        if (top.st_dev, top.st_ino) != (st.st_dev, st.st_ino):
            raise Skip("changed while being checked")
        for _root, dirs, files, rfd in os.fwalk(".", dir_fd=dfd, follow_symlinks=False):
            for n in dirs + files:
                e = os.stat(n, dir_fd=rfd, follow_symlinks=False)
                if e.st_dev != dev:
                    raise Skip("contains another filesystem")
                if e.st_uid != uid:
                    raise Skip(f"contains files not owned by {owner}")
    finally:
        os.close(dfd)


class Target:
    """A checked item, pinned by the fd of its parent folder: renames go
    through that fd, so swapping a path component for a symlink after the
    check cannot redirect them."""

    def __init__(self, fs, rel, uid, fds, name, st):
        self.fs, self.rel, self.uid = fs, rel, uid
        self.fds, self.name, self.st = fds, name, st
        self.fsfd, self.pfd = fds[0], fds[-1]
        self.dev = st.st_dev

    def close(self):
        for fd in reversed(self.fds):
            os.close(fd)


def open_target(item, check_mount=True, allowed_fs=None, min_uid=None):
    """precheck, then walk from the filesystem root with O_NOFOLLOW and check
    the item's owner (and, for a folder, every entry below it)."""
    fs, rel, uid = precheck(item, check_mount, allowed_fs, min_uid)
    owner = item["owner"]
    fds = [os.open(fs, os.O_RDONLY | os.O_DIRECTORY)]
    try:
        dev = os.fstat(fds[0]).st_dev
        parts = rel.split("/")
        for comp in parts[:-1]:
            try:
                fds.append(_open_dir(comp, fds[-1]))
            except FileNotFoundError:
                raise Skip("gone")
            except OSError as e:
                if e.errno in (errno.ELOOP, errno.ENOTDIR):
                    raise Skip("reached through a symlink or a non-folder")
                raise
            if os.fstat(fds[-1]).st_dev != dev:
                raise Skip("reached through another filesystem")
        name = parts[-1]
        try:
            st = os.stat(name, dir_fd=fds[-1], follow_symlinks=False)
        except FileNotFoundError:
            raise Skip("gone")
        if st.st_dev != dev:
            raise Skip("is another filesystem (a mount point)")
        if st.st_uid != uid:
            raise Skip(f"no longer owned by owner {owner}")
        if stat.S_ISDIR(st.st_mode):
            check_tree(fds[-1], name, st, uid, dev, owner)
        return Target(fs, rel, uid, fds, name, st)
    except BaseException:
        for fd in reversed(fds):
            os.close(fd)
        raise


def skip_reason(item, check_mount=True, allowed_fs=None, min_uid=None):
    """Why `item` must not be moved, or None when it can be."""
    try:
        open_target(item, check_mount, allowed_fs, min_uid).close()
    except Skip as e:
        return str(e)
    return None


def _check_reserved(st, dev, name=".trash"):
    if not stat.S_ISDIR(st.st_mode) or st.st_uid != os.geteuid() or st.st_dev != dev:
        raise RuntimeError(
            f"unsafe {name} (a symlink, not a folder, or not owned by this user): fix it by hand"
        )


def open_reserved(fsfd, dev, name):
    """fd of <fs>/<name> (.trash or .archive), created 0700; refuses a
    planted symlink or a folder that is not ours (root's, when running as
    root)."""
    try:
        os.mkdir(name, 0o700, dir_fd=fsfd)
    except FileExistsError:
        pass
    st = os.stat(name, dir_fd=fsfd, follow_symlinks=False)
    _check_reserved(st, dev, name)
    rfd = _open_dir(name, fsfd)
    if os.fstat(rfd).st_ino != st.st_ino:
        os.close(rfd)
        raise RuntimeError(f"{name} changed while being opened")
    os.fchmod(rfd, 0o700)
    return rfd


def open_trash(fsfd, dev):
    return open_reserved(fsfd, dev, ".trash")


def _exists(name, dir_fd):
    try:
        os.stat(name, dir_fd=dir_fd, follow_symlinks=False)
        return True
    except FileNotFoundError:
        return False


def make_dest(tfd, fs, today, rel):
    """Create .trash/<date>/<rel's folders>; return (parent fd, free name,
    path). The name gets .1, .2… when already taken today."""
    return _make_dest(tfd, os.path.join(fs, ".trash"), [today.isoformat()] + rel.split("/"), ".tar.zst")


def _make_dest(rfd, top, parts, ext):
    """Create the folders parts[:-1] below rfd (the reserved folder at path
    `top`), 0700; return (their fd, a free name for parts[-1], its path).
    The name gets .1, .2… when it or name+ext is taken."""
    cur = os.dup(rfd)
    try:
        for comp in parts[:-1]:
            try:
                os.mkdir(comp, 0o700, dir_fd=cur)
            except FileExistsError:
                pass
            nxt = _open_dir(comp, cur)
            os.close(cur)
            cur = nxt
        base = name = parts[-1]
        n = 0
        while _exists(name, cur) or _exists(name + ext, cur):
            n += 1
            name = f"{base}.{n}"
        return cur, name, os.path.join(top, *parts[:-1], name)
    except BaseException:
        os.close(cur)
        raise


def rmtree_fd(parent_fd, name):
    """rm -rf `name` below parent_fd, never following symlinks."""
    dfd = _open_dir(name, parent_fd)
    try:
        for _root, dirs, files, rfd in os.fwalk(".", dir_fd=dfd, topdown=False,
                                               follow_symlinks=False):
            for n in files:
                os.unlink(n, dir_fd=rfd)
            for n in dirs:
                try:
                    os.rmdir(n, dir_fd=rfd)
                except NotADirectoryError:  # a symlink to a folder
                    os.unlink(n, dir_fd=rfd)
    finally:
        os.close(dfd)
    os.rmdir(name, dir_fd=parent_fd)


def _tar(cwd_fd, archive, member, program, chdir=None, owner=None):
    """Run tar inside cwd_fd: pack `member` (below `chdir`, when given) into
    `archive` with `program`. It is written as `archive`.part, read back
    whole (`program -t`), fsynced, given to `owner` (uid, gid: the dashboard
    then counts it as theirs after the next scan) and only then renamed, so
    a crash never leaves a truncated file under the final name. The .part
    is removed when tar or the test fails (CalledProcessError)."""
    part = archive + ".part"
    cmd = ["tar", "--use-compress-program", program, "-cf", part]
    cmd += (["-C", chdir] if chdir else []) + [member]
    try:
        for c in (cmd, [program.split()[0], "-t", "-q", part]):
            subprocess.run(c, check=True, capture_output=True, text=True,
                           preexec_fn=lambda: os.fchdir(cwd_fd), pass_fds=(cwd_fd,))
    except subprocess.CalledProcessError:
        if _exists(part, cwd_fd):
            os.unlink(part, dir_fd=cwd_fd)
        raise
    fd = os.open(part, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=cwd_fd)
    try:
        os.fsync(fd)
        if owner:
            os.fchown(fd, *owner)
    finally:
        os.close(fd)
    os.rename(part, archive, src_dir_fd=cwd_fd, dst_dir_fd=cwd_fd)


def compress(parent_fd, name, path, owner=None):
    """Pack folder `name` (below parent_fd) into <name>.tar.zst, owned by
    `owner`, and remove it; return the archive's path. tar runs inside
    parent_fd."""
    try:
        _tar(parent_fd, name + ".tar.zst", name, "zstd -T0 -q", owner=owner)
    except subprocess.CalledProcessError as e:
        raise RuntimeError(f"tar failed, left uncompressed at {path}: {e.stderr.strip()}")
    rmtree_fd(parent_fd, name)
    return path + ".tar.zst"


def _rename_checked(t, dfd, dname, dest):
    """Rename the checked item `t` to dfd/dname; return its stat. When what
    was renamed is not what was checked, put it back (Skip) or stop
    (RuntimeError, it stays at `dest`)."""
    os.rename(t.name, dname, src_dir_fd=t.pfd, dst_dir_fd=dfd)
    moved = os.stat(dname, dir_fd=dfd, follow_symlinks=False)
    if (moved.st_dev, moved.st_ino) != (t.st.st_dev, t.st.st_ino):
        # Replaced between the check and the rename: put it back.
        if not _exists(t.name, t.pfd):
            os.rename(dname, t.name, src_dir_fd=dfd, dst_dir_fd=t.pfd)
            raise Skip("replaced while being moved; put back")
        raise RuntimeError(f"replaced while being moved; left at {dest}")
    return moved


def move_item(item, today, check_mount=True, allowed_fs=None, min_uid=None):
    """Check and move one item into the trash; return where it ended up.
    Raises Skip when it must stay, another exception on failure."""
    t = open_target(item, check_mount, allowed_fs, min_uid)
    try:
        tfd = open_trash(t.fsfd, t.dev)
        try:
            dpfd, dname, dest = make_dest(tfd, t.fs, today, t.rel)
            try:
                moved = _rename_checked(t, dpfd, dname, dest)
                if stat.S_ISDIR(moved.st_mode):
                    return compress(dpfd, dname, dest, (moved.st_uid, moved.st_gid))
                return dest
            finally:
                os.close(dpfd)
        finally:
            os.close(tfd)
    finally:
        t.close()


def purge(fs, today, keep_days=KEEP_DAYS, dry_run=False):
    """Delete <fs>/.trash/<date> folders older than keep_days; return them
    (dry_run: only list them). A filesystem that is not there has nothing."""
    try:
        fsfd = os.open(fs, os.O_RDONLY | os.O_DIRECTORY)
    except FileNotFoundError:
        return []
    try:
        try:
            st = os.stat(".trash", dir_fd=fsfd, follow_symlinks=False)
        except FileNotFoundError:
            return []
        _check_reserved(st, os.fstat(fsfd).st_dev)
        tfd = _open_dir(".trash", fsfd)
        try:
            removed = []
            for name in sorted(os.listdir(tfd)):
                if not DATE_DIR.match(name):
                    continue
                try:
                    day = datetime.date.fromisoformat(name)
                except ValueError:
                    continue
                if (today - day).days > keep_days:
                    if not dry_run:
                        rmtree_fd(tfd, name)
                    removed.append(os.path.join(fs, ".trash", name))
            return removed
        finally:
            os.close(tfd)
    finally:
        os.close(fsfd)


def gzip_program():
    """pigz (gzip on every core) when installed, else gzip."""
    return "pigz" if shutil.which("pigz") else "gzip"


def archive_item(item, today, check_mount=True, allowed_fs=None, min_uid=None):
    """Check one item and pack it into <fs>/.archive/<path>.<date>.tar.gz;
    return the archive's path. The item is first renamed into a root-only
    staging folder next to it (same filesystem: nothing changes it while tar
    runs) and packed under its own name, so `tar -xpzf` as root in its old
    folder restores it with owners and modes. When tar fails it is put back.
    Raises Skip when it must stay, another exception on failure."""
    if item.get("action") != "archive":
        raise Skip("not queued for the archive")
    t = open_target(item, check_mount, allowed_fs, min_uid)
    try:
        afd = open_reserved(t.fsfd, t.dev, ".archive")
        try:
            *parents, last = t.rel.split("/")
            parts = parents + [f"{last}.{today.isoformat()}"]
            dpfd, stage, dest = _make_dest(afd, os.path.join(t.fs, ".archive"), parts, ".tar.gz")
            try:
                return _pack_into(t, dpfd, stage, dest)
            finally:
                os.close(dpfd)
        finally:
            os.close(afd)
    finally:
        t.close()


def _pack_into(t, dpfd, stage, dest):
    """archive_item's work below dpfd: stage/<name> → stage.tar.gz."""
    os.mkdir(stage, 0o700, dir_fd=dpfd)
    sfd = _open_dir(stage, dpfd)
    try:
        try:
            _rename_checked(t, sfd, t.name, os.path.join(dest, t.name))
        except Skip:
            os.rmdir(stage, dir_fd=dpfd)
            raise
        try:
            _tar(dpfd, stage + ".tar.gz", t.name, gzip_program(), chdir=stage,
                 owner=(t.st.st_uid, t.st.st_gid))
        except subprocess.CalledProcessError as e:
            why = e.stderr.strip()
            if _exists(t.name, t.pfd):
                raise RuntimeError(f"tar failed ({why}); its old path is taken again: left at {dest}")
            os.rename(t.name, t.name, src_dir_fd=sfd, dst_dir_fd=t.pfd)
            os.rmdir(stage, dir_fd=dpfd)
            raise RuntimeError(f"tar failed, put back: {why}")
    finally:
        os.close(sfd)
    rmtree_fd(dpfd, stage)
    return dest + ".tar.gz"
