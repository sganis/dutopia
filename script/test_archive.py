# script/test_archive.py
# Run: python3 -m unittest script/test_archive.py  (needs tar + gzip)
import contextlib
import io
import os
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import archiver  # noqa: E402
import trash_fsops as fsops  # noqa: E402
from test_mover import ME, TODAY, Fs  # noqa: E402

fsops.MIN_UID = 0
DAY = TODAY.isoformat()


def members(archive):
    return subprocess.run(["tar", "-tzf", archive], check=True, capture_output=True,
                          text=True).stdout.split()


class ArchiveFs(Fs):
    def setUp(self):
        super().setUp()
        self._st = tempfile.TemporaryDirectory()
        self.state = self._st.name

    def tearDown(self):
        self._st.cleanup()
        super().tearDown()

    def item(self, path, owner=ME):
        return {**super().item(path, owner), "action": "archive"}

    def stage(self, rel):
        return fsops.stage_item(self.item(os.path.join(self.fs, rel)), TODAY, check_mount=False)

    def archiver(self, *args):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = archiver.main(["--fs", self.fs, "--dev", "--state-dir", self.state, *args])
        return rc, out.getvalue()


class Stage(ArchiveFs):
    """The mover's half: a rename into .archive, nothing packed."""

    def test_folder_is_moved_as_is_next_to_its_future_archive(self):
        self.mk("a/run1/x.txt", b"hello")
        before = os.stat(os.path.join(self.fs, "a/run1"))
        staged = self.stage("a/run1")
        self.assertEqual(staged, os.path.join(self.fs, f".archive/a/run1.{DAY}/run1"))
        self.assertEqual(os.stat(staged).st_ino, before.st_ino)  # renamed, not copied
        self.assertFalse(os.path.exists(os.path.join(self.fs, "a/run1")))
        self.assertEqual(os.stat(os.path.join(self.fs, ".archive")).st_mode & 0o777, 0o700)

    def test_same_path_twice_a_day_gets_a_suffix(self):
        self.mk("a/f")
        first = self.stage("a/f")
        self.mk("a/f")
        second = self.stage("a/f")
        self.assertEqual(second, os.path.join(self.fs, f".archive/a/f.{DAY}.1/f"))
        self.assertTrue(os.path.isfile(first) and os.path.isfile(second))

    def test_only_items_queued_for_the_archive(self):
        p = self.mk("a/f")
        with self.assertRaises(fsops.Skip):
            fsops.stage_item(Fs.item(self, p), TODAY, check_mount=False)
        self.assertTrue(os.path.isfile(p))

    def test_nothing_inside_the_archive_is_archived_again(self):
        self.mk("a/f")
        with self.assertRaises(fsops.Skip):
            fsops.stage_item(self.item(self.stage("a/f")), TODAY, check_mount=False)

    def test_a_skipped_item_leaves_no_stage_folder(self):
        self.mk("a/other")
        with self.assertRaises(fsops.Skip):
            self.stage("a/gone")
        self.assertFalse(os.path.exists(os.path.join(self.fs, ".archive/a", f"gone.{DAY}")))


class Pack(ArchiveFs):
    """The archiver's half: staged items become .tar.gz."""

    def test_folder_becomes_dated_tar_gz_at_its_path(self):
        self.mk("a/run1/x.txt", b"hello")
        self.mk("a/run1/sub/y.txt")
        self.stage("a/run1")
        rc, log = self.archiver()
        self.assertEqual(rc, 0, log)
        dest = os.path.join(self.fs, f".archive/a/run1.{DAY}.tar.gz")
        self.assertEqual(os.listdir(os.path.join(self.fs, ".archive/a")), [f"run1.{DAY}.tar.gz"])
        self.assertIn("run1/sub/y.txt", members(dest))
        self.assertIn(f"-> {dest}", log)

    def test_file_is_packed_under_its_own_name_and_restores(self):
        self.mk("a/f.dat", b"data")
        self.stage("a/f.dat")
        self.assertEqual(self.archiver()[0], 0)
        dest = os.path.join(self.fs, f".archive/a/f.dat.{DAY}.tar.gz")
        self.assertEqual(members(dest), ["f.dat"])
        subprocess.run(["tar", "-xpzf", dest], cwd=os.path.join(self.fs, "a"), check=True)
        with open(os.path.join(self.fs, "a/f.dat"), "rb") as f:
            self.assertEqual(f.read(), b"data")

    def test_the_archive_belongs_to_the_item_owner(self):
        self.mk("a/f")
        self.stage("a/f")
        self.archiver()
        st = os.stat(os.path.join(self.fs, f".archive/a/f.{DAY}.tar.gz"))
        self.assertEqual((st.st_uid, st.st_gid), (os.getuid(), os.getgid()))

    def test_folders_inside_an_item_that_look_staged_are_left_alone(self):
        self.mk(f"a/run1/x.{DAY}/x", b"user data")
        self.mk(f"p.{DAY}/p/f")  # a parent named like a stage
        self.stage("a/run1")
        self.stage(f"p.{DAY}/p")
        self.assertEqual(self.archiver()[0], 0)
        self.assertEqual(os.listdir(os.path.join(self.fs, ".archive/a")), [f"run1.{DAY}.tar.gz"])
        self.assertIn(f"run1/x.{DAY}/x", members(os.path.join(self.fs, f".archive/a/run1.{DAY}.tar.gz")))
        self.assertTrue(os.path.isfile(os.path.join(self.fs, f".archive/p.{DAY}/p.{DAY}.tar.gz")))

    def test_tar_failure_leaves_it_staged(self):
        self.mk("a/run1/x.txt", b"hello")
        staged = self.stage("a/run1")
        with mock.patch.object(fsops, "gzip_program", return_value="false"):
            rc, log = self.archiver()
        self.assertEqual(rc, 1)
        self.assertIn("left staged", log)
        self.assertTrue(os.path.isfile(os.path.join(staged, "x.txt")))
        self.assertEqual(os.listdir(os.path.join(self.fs, ".archive/a")), [f"run1.{DAY}"])
        self.assertEqual(self.archiver()[0], 0)  # the next run packs it

    def test_an_archive_that_does_not_read_back_is_not_kept(self):
        self.mk("a/run1/x.txt", b"hello")
        self.stage("a/run1")
        # Compresses like gzip, but its integrity test (-t) fails.
        prog = os.path.join(self.state, "badgz")
        with open(prog, "w") as f:
            f.write('#!/bin/sh\n[ "$1" = "-t" ] && exit 1\nexec gzip "$@"\n')
        os.chmod(prog, 0o755)
        with mock.patch.object(fsops, "gzip_program", return_value=prog):
            self.assertEqual(self.archiver()[0], 1)
        self.assertEqual(os.listdir(os.path.join(self.fs, ".archive/a")), [f"run1.{DAY}"])  # no .part

    def test_an_archive_already_there_is_not_overwritten(self):
        self.mk("a/f")
        staged = self.stage("a/f")
        with open(os.path.join(self.fs, f".archive/a/f.{DAY}.tar.gz"), "w") as f:
            f.write("earlier run")
        rc, log = self.archiver()
        self.assertEqual(rc, 1)
        self.assertIn("check by hand", log)
        self.assertTrue(os.path.isfile(staged))

    def test_dry_run_lists_and_changes_nothing(self):
        self.mk("a/f")
        staged = self.stage("a/f")
        rc, log = self.archiver("--dry-run")
        self.assertEqual(rc, 0)
        self.assertIn(f"[would pack] {staged}", log)
        self.assertTrue(os.path.isfile(staged))

    def test_no_archive_folder_is_nothing_to_do(self):
        self.assertEqual(self.archiver(), (0, ""))

    def test_output_packs_the_archive_staged_there(self):
        self.mk("a/f")
        out = os.path.join(self.state, "out")
        staged = fsops.stage_item(self.item(os.path.join(self.fs, "a/f")), TODAY,
                                  check_mount=False, output=out)
        fs = os.path.basename(self.fs)
        self.assertEqual(staged, os.path.join(out, fs, f".archive/a/f.{DAY}/f"))
        self.assertFalse(os.path.exists(os.path.join(self.fs, ".archive")))
        rc, log = self.archiver("--output", out)
        self.assertEqual(rc, 0, log)
        self.assertEqual(os.listdir(os.path.join(out, fs, ".archive/a")), [f"f.{DAY}.tar.gz"])
        self.assertIn(f"-> {os.path.join(out, fs, '.archive/a', f'f.{DAY}.tar.gz')}", log)

    def test_needs_a_filesystem(self):
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            archiver.main(["--dev"])


if __name__ == "__main__":
    unittest.main()
