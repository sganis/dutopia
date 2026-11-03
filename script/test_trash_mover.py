# script/test_trash_mover.py
# Run: python3 -m unittest script/test_trash_mover.py  (needs tar + zstd)
import datetime
import http.server
import json
import os
import pwd
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import trash_mover as tm  # noqa: E402

ME = pwd.getpwuid(os.geteuid()).pw_name
# Tests run as a normal user, whose uid may be below the production floor.
tm.MIN_UID = 0
TODAY = datetime.date(2026, 11, 4)


class Fs(unittest.TestCase):
    """A temp dir standing in for a mounted filesystem."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.fs = os.path.realpath(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def mk(self, rel, data=b"x"):
        p = os.path.join(self.fs, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data)
        return p

    def item(self, path, owner=ME):
        return {"path": path, "owner": owner, "fs": self.fs, "size": 1,
                "actor": owner, "queued_at": 0}


class SkipReason(Fs):
    def test_ok_item_has_no_reason(self):
        p = self.mk("a/file")
        self.assertIsNone(tm.skip_reason(self.item(p), check_mount=False))

    def test_gone(self):
        self.assertIn("gone", tm.skip_reason(self.item(self.fs + "/nope"), check_mount=False))

    def test_inside_trash_or_stats_or_root(self):
        for rel in [".trash/2026-11-01/x", ".stats/scan.csv"]:
            p = self.mk(rel)
            self.assertIsNotNone(tm.skip_reason(self.item(p), check_mount=False), rel)
        self.assertIsNotNone(tm.skip_reason(self.item(self.fs), check_mount=False))

    def test_outside_fs(self):
        with tempfile.NamedTemporaryFile() as f:
            self.assertIsNotNone(tm.skip_reason(self.item(f.name), check_mount=False))

    def test_not_owned_and_unknown_owner(self):
        p = self.mk("a/file")
        if os.geteuid() != 0:
            self.assertIn("owner", tm.skip_reason(self.item(p, owner="root"), check_mount=False))
        self.assertIn("owner", tm.skip_reason(self.item(p, owner="no-such-user-xyz"), check_mount=False))

    def test_path_through_a_symlinked_parent(self):
        self.mk("real/file")
        os.symlink(os.path.join(self.fs, "real"), os.path.join(self.fs, "link"))
        p = os.path.join(self.fs, "link/file")
        self.assertIn("symlink", tm.skip_reason(self.item(p), check_mount=False))

    def test_not_a_mount_point(self):
        p = self.mk("a/file")
        self.assertIn("mount", tm.skip_reason(self.item(p), check_mount=True))


class Move(Fs):
    def test_file_keeps_its_path_under_dated_trash(self):
        p = self.mk("a/b/file.dat")
        dest = tm.move_item(self.item(p), TODAY, check_mount=False)
        self.assertEqual(dest, os.path.join(self.fs, ".trash/2026-11-04/a/b/file.dat"))
        self.assertTrue(os.path.isfile(dest))
        self.assertFalse(os.path.exists(p))
        self.assertEqual(os.stat(os.path.join(self.fs, ".trash")).st_mode & 0o777, 0o700)

    def test_folder_becomes_tar_zst(self):
        self.mk("a/run1/x.txt", b"hello")
        self.mk("a/run1/sub/y.txt")
        dest = tm.move_item(self.item(os.path.join(self.fs, "a/run1")), TODAY, check_mount=False)
        self.assertEqual(dest, os.path.join(self.fs, ".trash/2026-11-04/a/run1.tar.zst"))
        self.assertFalse(os.path.exists(os.path.join(self.fs, ".trash/2026-11-04/a/run1")))
        names = subprocess.run(
            ["tar", "--use-compress-program", "zstd -d", "-tf", dest],
            check=True, capture_output=True, text=True,
        ).stdout.split()
        self.assertIn("run1/x.txt", names)
        self.assertIn("run1/sub/y.txt", names)

    def test_same_path_twice_a_day_gets_a_suffix(self):
        first = tm.move_item(self.item(self.mk("a/f")), TODAY, check_mount=False)
        second = tm.move_item(self.item(self.mk("a/f")), TODAY, check_mount=False)
        self.assertNotEqual(first, second)
        self.assertTrue(os.path.isfile(first) and os.path.isfile(second))


class Purge(Fs):
    def test_removes_only_dated_dirs_older_than_keep_days(self):
        # TODAY is 2026-11-04: 10-04 is 31 days old (purged), 10-05 is 30 (kept).
        for d in ["2026-09-01", "2026-10-04", "2026-10-05", "keep-me"]:
            os.makedirs(os.path.join(self.fs, ".trash", d))
        removed = tm.purge(self.fs, TODAY, keep_days=30)
        self.assertEqual(sorted(os.path.basename(r) for r in removed), ["2026-09-01", "2026-10-04"])
        self.assertEqual(sorted(os.listdir(os.path.join(self.fs, ".trash"))), ["2026-10-05", "keep-me"])

    def test_no_trash_dir_is_fine(self):
        self.assertEqual(tm.purge(self.fs, TODAY, keep_days=30), [])


class State(unittest.TestCase):
    def test_batches_are_saved_and_their_filesystems_remembered(self):
        with tempfile.TemporaryDirectory() as d:
            tm.save_batch(d, TODAY, [{"path": "/red/nas1/a", "fs": "/red/nas1"}])
            tm.save_batch(d, TODAY, [{"path": "/red/nas2/b", "fs": "/red/nas2"}])
            with open(os.path.join(d, "2026-11-04.json")) as f:
                self.assertEqual(len(json.load(f)), 2)  # same day: appended
            self.assertEqual(tm.seen_filesystems(d), {"/red/nas1", "/red/nas2"})


class Claim(unittest.TestCase):
    def test_posts_with_bearer_token_and_parses_items(self):
        seen = {}

        class H(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                seen["path"] = self.path
                seen["auth"] = self.headers.get("Authorization")
                body = json.dumps([{"path": "/red/nas1/a"}]).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *a):
                pass

        srv = http.server.HTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        try:
            items = tm.claim(f"http://127.0.0.1:{srv.server_port}/", "tok", allow_http=True)
        finally:
            srv.shutdown()
        self.assertEqual(items, [{"path": "/red/nas1/a"}])
        self.assertEqual(seen, {"path": "/api/trash/claim", "auth": "Bearer tok"})


class Hardening(Fs):
    """Review findings C1, C2, I1, I3: the cron runs as root on user trees."""

    def test_parent_swapped_for_symlink_after_check_does_not_move_the_target(self):
        # alice queues a folder she owns, then swaps its parent for a link
        # to the filesystem root between the check and the move.
        self.mk("projects/bob/data", b"victim")
        decoy = self.mk("alice/x/projects/mine", b"decoy")
        item = self.item(os.path.join(self.fs, "alice/x/projects"))
        self.assertIsNone(tm.skip_reason(item, check_mount=False))
        os.rename(os.path.join(self.fs, "alice/x"), os.path.join(self.fs, "alice/x.real"))
        os.symlink(self.fs, os.path.join(self.fs, "alice/x"))
        try:
            tm.move_item(item, TODAY, check_mount=False)
        except tm.Skip:
            pass
        self.assertTrue(os.path.isfile(os.path.join(self.fs, "projects/bob/data")))
        del decoy

    def test_swap_inside_move_is_pinned_to_the_checked_folder(self):
        self.mk("projects/bob/data", b"victim")
        self.mk("alice/x/projects/mine", b"decoy")
        item = self.item(os.path.join(self.fs, "alice/x/projects"))
        real_open_trash = tm.open_trash

        def swap_then_open(*a):
            os.rename(os.path.join(self.fs, "alice/x"), os.path.join(self.fs, "alice/x.real"))
            os.symlink(self.fs, os.path.join(self.fs, "alice/x"))
            return real_open_trash(*a)

        with mock.patch.object(tm, "open_trash", swap_then_open):
            dest = tm.move_item(item, TODAY, check_mount=False)
        self.assertTrue(os.path.isfile(os.path.join(self.fs, "projects/bob/data")))
        self.assertFalse(os.path.exists(os.path.join(self.fs, "alice/x.real/projects")))
        self.assertTrue(dest.endswith("alice/x/projects.tar.zst"))

    def test_planted_trash_symlink_is_refused(self):
        victim = os.path.join(self.fs, "bob")
        os.makedirs(os.path.join(victim, "2020-01-01"))
        os.chmod(victim, 0o755)
        os.symlink(victim, os.path.join(self.fs, ".trash"))
        f = self.mk("a/file")
        with self.assertRaises(Exception):
            tm.move_item(self.item(f), TODAY, check_mount=False)
        self.assertTrue(os.path.isfile(f))
        with self.assertRaises(Exception):
            tm.purge(self.fs, TODAY, keep_days=30)
        self.assertTrue(os.path.isdir(os.path.join(victim, "2020-01-01")))
        self.assertEqual(os.stat(victim).st_mode & 0o777, 0o755)

    def test_tree_walk_rejects_entries_of_another_owner(self):
        self.mk("a/run1/x")
        fsfd = os.open(self.fs, os.O_RDONLY | os.O_DIRECTORY)
        try:
            pfd = os.open("a", os.O_RDONLY | os.O_DIRECTORY, dir_fd=fsfd)
            st = os.stat("run1", dir_fd=pfd, follow_symlinks=False)
            with self.assertRaises(tm.Skip) as cm:
                tm.check_tree(pfd, "run1", st, os.geteuid() + 1, st.st_dev, "someone")
            self.assertIn("not owned", str(cm.exception))
            tm.check_tree(pfd, "run1", st, os.geteuid(), st.st_dev, ME)  # own tree: fine
            os.close(pfd)
        finally:
            os.close(fsfd)

    def test_filesystem_allowlist(self):
        p = self.mk("a/file")
        self.assertIn("--fs", tm.skip_reason(self.item(p), check_mount=False, allowed_fs={"/elsewhere"}))
        self.assertIsNone(tm.skip_reason(self.item(p), check_mount=False, allowed_fs={self.fs}))

    def test_system_account_owners_are_refused(self):
        p = self.mk("a/file")
        reason = tm.skip_reason(self.item(p), check_mount=False, min_uid=os.geteuid() + 1)
        self.assertIn("system account", reason)

    def test_plain_http_needs_an_explicit_flag(self):
        with self.assertRaises(ValueError):
            tm.claim("http://127.0.0.1:9/", "tok")


class EndToEnd(Fs):
    def test_run_moves_batch_and_purges(self):
        os.makedirs(os.path.join(self.fs, ".trash", "2026-01-01"))
        f = self.mk("p/file")
        self.mk("p/dir/x")
        gone = self.item(self.fs + "/p/missing")
        with tempfile.TemporaryDirectory() as state:
            errors = tm.run(
                [self.item(f), self.item(self.fs + "/p/dir"), gone],
                today=TODAY, state_dir=state, keep_days=30,
                purge_fs=[], check_mount=False,
            )
            self.assertEqual(errors, 0)
            t = os.path.join(self.fs, ".trash/2026-11-04/p")
            self.assertTrue(os.path.isfile(os.path.join(t, "file")))
            self.assertTrue(os.path.isfile(os.path.join(t, "dir.tar.zst")))
            self.assertFalse(os.path.exists(os.path.join(self.fs, ".trash/2026-01-01")))


if __name__ == "__main__":
    unittest.main()


class Main(unittest.TestCase):
    def test_dev_mode_needs_only_the_api_and_the_token_env(self):
        seen = {}

        class H(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                seen["auth"] = self.headers.get("Authorization")
                self.send_response(200)
                self.send_header("Content-Length", "2")
                self.end_headers()
                self.wfile.write(b"[]")

            def log_message(self, *a):
                pass

        srv = http.server.HTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        try:
            with tempfile.TemporaryDirectory() as home, \
                    mock.patch.dict(os.environ, {"TRASH_TOKEN": "tok", "HOME": home}):
                rc = tm.main(["--dev", "--api", f"http://127.0.0.1:{srv.server_port}"])
                saved = os.listdir(os.path.join(home, ".dutopia-trash"))
        finally:
            srv.shutdown()
        self.assertEqual(rc, 0)
        self.assertEqual(seen["auth"], "Bearer tok")
        self.assertEqual(len(saved), 1)
