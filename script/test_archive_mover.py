# script/test_archive_mover.py
# Run: python3 -m unittest script/test_archive_mover.py  (needs tar + gzip)
import datetime
import os
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import archive_mover as am  # noqa: E402
import trash_fsops as fsops  # noqa: E402
from test_trash_mover import ME, TODAY, FakeApi, Fs  # noqa: E402

fsops.MIN_UID = 0


def members(archive):
    return subprocess.run(["tar", "-tzf", archive], check=True, capture_output=True,
                          text=True).stdout.split()


class ArchiveFs(Fs):
    def item(self, path, owner=ME):
        return {**super().item(path, owner), "action": "archive"}


class Archive(ArchiveFs):
    def archive(self, rel):
        return fsops.archive_item(self.item(os.path.join(self.fs, rel)), TODAY, check_mount=False)

    def test_folder_becomes_dated_tar_gz_at_its_path(self):
        self.mk("a/run1/x.txt", b"hello")
        self.mk("a/run1/sub/y.txt")
        dest = self.archive("a/run1")
        self.assertEqual(dest, os.path.join(self.fs, ".archive/a/run1.2026-11-04.tar.gz"))
        self.assertFalse(os.path.exists(os.path.join(self.fs, "a/run1")))
        self.assertEqual(os.listdir(os.path.join(self.fs, ".archive/a")), ["run1.2026-11-04.tar.gz"])
        self.assertIn("run1/sub/y.txt", members(dest))
        self.assertEqual(os.stat(os.path.join(self.fs, ".archive")).st_mode & 0o777, 0o700)

    def test_file_is_packed_under_its_own_name_and_restores(self):
        self.mk("a/f.dat", b"data")
        dest = self.archive("a/f.dat")
        self.assertEqual(dest, os.path.join(self.fs, ".archive/a/f.dat.2026-11-04.tar.gz"))
        self.assertEqual(members(dest), ["f.dat"])
        subprocess.run(["tar", "-xpzf", dest], cwd=os.path.join(self.fs, "a"), check=True)
        with open(os.path.join(self.fs, "a/f.dat"), "rb") as f:
            self.assertEqual(f.read(), b"data")

    def test_same_path_twice_a_day_gets_a_suffix(self):
        self.mk("a/f")
        first = self.archive("a/f")
        self.mk("a/f")
        second = self.archive("a/f")
        self.assertEqual(second, os.path.join(self.fs, ".archive/a/f.2026-11-04.1.tar.gz"))
        self.assertTrue(os.path.isfile(first) and os.path.isfile(second))

    def test_only_items_queued_for_the_archive(self):
        p = self.mk("a/f")
        with self.assertRaises(fsops.Skip):
            fsops.archive_item(Fs.item(self, p), TODAY, check_mount=False)
        self.assertTrue(os.path.isfile(p))

    def test_nothing_inside_the_archive_is_archived_again(self):
        self.mk("a/f")
        dest = self.archive("a/f")
        with self.assertRaises(fsops.Skip):
            fsops.archive_item(self.item(dest), TODAY, check_mount=False)

    def test_tar_failure_puts_it_back(self):
        self.mk("a/run1/x.txt", b"hello")
        with mock.patch.object(fsops, "gzip_program", return_value="false"):
            with self.assertRaisesRegex(RuntimeError, "put back"):
                self.archive("a/run1")
        with open(os.path.join(self.fs, "a/run1/x.txt"), "rb") as f:
            self.assertEqual(f.read(), b"hello")
        self.assertEqual(os.listdir(os.path.join(self.fs, ".archive/a")), [])


class Main(ArchiveFs):
    def setUp(self):
        super().setUp()
        self._st = tempfile.TemporaryDirectory()
        self.state = self._st.name

    def tearDown(self):
        self._st.cleanup()
        super().tearDown()

    def _main(self, api, *args):
        with mock.patch.dict(os.environ, {"TRASH_TOKEN": "tok"}):
            return am.main(["--api", api.url, "--dev", "--state-dir", self.state, *args])

    def test_claims_the_archive_and_reports(self):
        self.mk("p/dir/x")
        items = [self.item(self.fs + "/p/dir"), self.item(self.fs + "/p/gone")]
        with FakeApi(filesystems=[self.fs], items=items) as api:
            self.assertEqual(self._main(api), 0)
        (_, path, _, auth), = api.posts("/api/trash/claim")
        self.assertIn("action=archive", path)
        self.assertEqual(auth, "Bearer tok")
        (_, _, body, _), = api.posts("/api/trash/report")
        by_path = {o["path"]: o for o in body}
        done = by_path[self.fs + "/p/dir"]
        self.assertEqual(done["status"], "moved")
        self.assertTrue(done["dest"].endswith(f"/.archive/p/dir.{datetime.date.today()}.tar.gz"))
        self.assertEqual(by_path[self.fs + "/p/gone"]["status"], "skipped")

    def test_older_server_is_refused_before_claiming(self):
        with FakeApi(filesystems=[self.fs], items=[self.item(self.mk("p/f"))], honor=False) as api:
            self.assertEqual(self._main(api), 1)
        self.assertEqual(api.posts("/api/trash/claim"), [])

    def test_dry_run_changes_and_reports_nothing(self):
        p = self.mk("p/f")
        with FakeApi(filesystems=[self.fs], items=[self.item(p)]) as api:
            self.assertEqual(self._main(api, "--dry-run"), 0)
        self.assertIn("dry_run=1", api.posts("/api/trash/claim")[0][1])
        self.assertTrue(os.path.isfile(p))
        self.assertEqual(api.posts("/api/trash/report"), [])


if __name__ == "__main__":
    unittest.main()
