#!/usr/bin/env python3
# script/trash_fsops.py
"""Filesystem side of the Dutopia mover (mover.py, cron) and archiver
(archiver.py, by hand), run as root.

Every step works on file descriptors opened with O_NOFOLLOW from the
filesystem root, so a user swapping a path component for a symlink while the
cron runs cannot redirect a check, a rename or a delete:

- precheck / open_target: is this item one owner's, on this filesystem, not
  reached through a symlink, not inside .trash/.archive/.stats? (raise Skip
  if not)
- move_item: rename it into <fs>/.trash/<date>/<path>, as is
- stage_item: rename it into <fs>/.archive/<path>.<date>/<name>, as is
- pack_archive: pack each staged item into <fs>/.archive/<path>.<date>.tar.gz

Every reserved folder lives under the item's filesystem, or under `output`
(a folder of one's own, from --output on mover.py/archiver.py) to test the
whole flow without root.

Moves are renames: instant at any size; owners, modes and ACLs untouched.
Nothing here deletes from .trash: old trash is removed by hand
(deploy/TRASH.md).
"""
import errno
import os
import pwd
import re
import shutil
import stat
import subprocess

# Owners below this uid are system accounts: never moved.
MIN_UID = 1000
RESERVED = (".trash", ".archive", ".stats")
# A staged item's folder is <name>.<date>, or <name>.<date>.N.
STAGE_SUFFIX = re.compile(r"^\d{4}-\d{2}-\d{2}(\.\d+)?$")


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
    mount, is not one user's to move."""
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


def _check_reserved(st, dev, name):
    if not stat.S_ISDIR(st.st_mode) or st.st_uid != os.geteuid() or st.st_dev != dev:
        raise RuntimeError(
            f"unsafe {name} (a symlink, not a folder, or not owned by this user): fix it by hand"
        )


def open_existing_reserved(fsfd, dev, name):
    """fd of <fs>/<name> (.trash or .archive), or None when there is none;
    refuses a planted symlink or a folder that is not ours (root's, when
    running as root)."""
    try:
        st = os.stat(name, dir_fd=fsfd, follow_symlinks=False)
    except FileNotFoundError:
        return None
    _check_reserved(st, dev, name)
    rfd = _open_dir(name, fsfd)
    if os.fstat(rfd).st_ino != st.st_ino:
        os.close(rfd)
        raise RuntimeError(f"{name} changed while being opened")
    return rfd


def open_reserved(fsfd, dev, name):
    """open_existing_reserved, creating <fs>/<name> first; always 0700."""
    try:
        os.mkdir(name, 0o700, dir_fd=fsfd)
    except FileExistsError:
        pass
    rfd = open_existing_reserved(fsfd, dev, name)
    if rfd is None:
        raise RuntimeError(f"{name} vanished while being opened")
    os.fchmod(rfd, 0o700)
    return rfd


def open_out_reserved(t, output, name, opener):
    """The reserved folder `name` on the filesystem of target `t`, or under
    `output` (an absolute path) when given: (its fd, the path its contents
    are reported under). The output tree is a normal folder of one's own,
    opened without the fs-root walk: with --output the reserved folders live
    in it, so a user cannot plant them on the filesystem itself."""
    if not output:
        return opener(t.fsfd, t.dev), t.fs
    os.makedirs(output, exist_ok=True)
    otop = os.path.join(output, os.path.basename(t.fs.rstrip("/")) or "root")
    os.makedirs(otop, exist_ok=True)
    ofd = os.open(otop, os.O_RDONLY | os.O_DIRECTORY)
    try:
        rfd = open_reserved(ofd, os.fstat(ofd).st_dev, name)
    except BaseException:
        os.close(ofd)
        raise
    return rfd, otop


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
    return _make_dest(tfd, os.path.join(fs, ".trash"), [today.isoformat()] + rel.split("/"), "")


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


def move_item(item, today, check_mount=True, allowed_fs=None, min_uid=None,
              output=None):
    """Check and move one item into the trash, as is; return where it ended
    up. Restore is a rename back. `output` moves the trash folder out of the
    filesystem (a folder of one's own, to test without root). Raises Skip
    when it must stay, another exception on failure."""
    t = open_target(item, check_mount, allowed_fs, min_uid)
    try:
        tfd, ttop = open_out_reserved(t, output, ".trash", open_trash)
        try:
            dpfd, dname, dest = make_dest(tfd, ttop, today, t.rel)
            try:
                _rename_checked(t, dpfd, dname, dest)
                return dest
            finally:
                os.close(dpfd)
        finally:
            os.close(tfd)
    finally:
        t.close()


def stage_item(item, today, check_mount=True, allowed_fs=None, min_uid=None,
               output=None):
    """Check one item queued for the archive and move it, as is, into the
    root-only <fs>/.archive/<path>.<date>/<name>; archiver.py packs it later
    into <path>.<date>.tar.gz. `output` moves the archive folder out of the
    filesystem (a folder of one's own, to test without root). Returns where
    it is now. Restore is a rename back. Raises Skip when it must stay,
    another exception on failure."""
    if item.get("action") != "archive":
        raise Skip("not queued for the archive")
    t = open_target(item, check_mount, allowed_fs, min_uid)
    try:
        afd, atop = open_out_reserved(t, output, ".archive",
                                      lambda fsfd, dev: open_reserved(fsfd, dev, ".archive"))
        try:
            *parents, last = t.rel.split("/")
            parts = parents + [f"{last}.{today.isoformat()}"]
            dpfd, stage, dest = _make_dest(afd, os.path.join(atop, ".archive"), parts, ".tar.gz")
            try:
                os.mkdir(stage, 0o700, dir_fd=dpfd)
                sfd = _open_dir(stage, dpfd)
                try:
                    _rename_checked(t, sfd, t.name, os.path.join(dest, t.name))
                except Skip:
                    os.rmdir(stage, dir_fd=dpfd)
                    raise
                finally:
                    os.close(sfd)
                return os.path.join(dest, t.name)
            finally:
                os.close(dpfd)
        finally:
            os.close(afd)
    finally:
        t.close()


def gzip_program():
    """pigz (gzip on every core) when installed, else gzip."""
    return "pigz" if shutil.which("pigz") else "gzip"


def _tar(cwd_fd, archive, member, program, chdir=None, owner=None):
    """Run tar inside cwd_fd: pack `member` (below `chdir`, when given) into
    `archive` with `program`. It is written as `archive`.part, read back
    whole (`program -t`), fsynced, given to `owner` (uid, gid) and only then
    renamed, so a crash never leaves a truncated file under the final name.
    The .part is removed when tar or the test fails (CalledProcessError)."""
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


def _staged_name(folder, inside):
    """The item staged in `folder` (holding the entries `inside`), or None:
    a staged folder is <name>.<date>[.N] holding just <name>."""
    if len(inside) != 1:
        return None
    name = inside[0]
    rest = folder[len(name) + 1:] if folder.startswith(name + ".") else ""
    return name if STAGE_SUFFIX.match(rest) else None


def find_stages(afd):
    """Walk .archive (fd afd); yield (parent fd, parent path relative to
    .archive, stage folder, item name) for each staged item. Only this
    user's (root's) folders are walked, never a staged item: what is inside
    belongs to its owner and is never looked at."""
    me = os.geteuid()
    for root, dirs, _files, rfd in os.fwalk(".", dir_fd=afd, follow_symlinks=False):
        walk = []
        for d in dirs:
            st = os.stat(d, dir_fd=rfd, follow_symlinks=False)
            if not stat.S_ISDIR(st.st_mode) or st.st_uid != me:
                continue
            sfd = _open_dir(d, rfd)
            try:
                name = _staged_name(d, os.listdir(sfd))
            finally:
                os.close(sfd)
            if name is None:
                walk.append(d)
            else:
                yield rfd, os.path.normpath(root), d, name
        dirs[:] = walk


def pack_stage(pfd, stage, name, dest):
    """Pack stage/<name> (below pfd; `dest` is the stage's path) into
    <stage>.tar.gz, given to the item's owner (the dashboard then counts it
    as theirs), then remove the staged copy; return the archive's path.
    When tar fails (e.g. the filesystem is full) the staged item stays as is
    (RuntimeError)."""
    archive = stage + ".tar.gz"
    if _exists(archive, pfd):
        raise RuntimeError(f"{dest}.tar.gz already exists: check by hand")
    sfd = _open_dir(stage, pfd)
    try:
        st = os.stat(name, dir_fd=sfd, follow_symlinks=False)
    finally:
        os.close(sfd)
    try:
        _tar(pfd, archive, name, gzip_program(), chdir=stage, owner=(st.st_uid, st.st_gid))
    except subprocess.CalledProcessError as e:
        raise RuntimeError(f"tar failed, left staged: {e.stderr.strip()}")
    rmtree_fd(pfd, stage)
    return dest + ".tar.gz"


def pack_archive(fs, dry_run=False, output=None):
    """Pack every item staged in <fs>/.archive; yield (staged path, archive
    path or None, error or None) per item (dry_run: only find them). A
    filesystem with no .archive has nothing. `output` packs what was staged
    under it instead (from --output, to test without root)."""
    top = fs
    if output:
        os.makedirs(output, exist_ok=True)
        top = os.path.join(output, os.path.basename(fs.rstrip("/")) or "root")
        os.makedirs(top, exist_ok=True)
    fsfd = os.open(top, os.O_RDONLY | os.O_DIRECTORY)
    try:
        afd = open_existing_reserved(fsfd, os.fstat(fsfd).st_dev, ".archive")
        if afd is None:
            return
        try:
            for pfd, rel, stage, name in find_stages(afd):
                dest = os.path.normpath(os.path.join(top, ".archive", rel, stage))
                staged = os.path.join(dest, name)
                if dry_run:
                    yield staged, None, None
                    continue
                try:
                    yield staged, pack_stage(pfd, stage, name, dest), None
                except (OSError, RuntimeError) as e:
                    yield staged, None, e
        finally:
            os.close(afd)
    finally:
        os.close(fsfd)
