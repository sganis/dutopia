# script/test_mover.py
# Run: python3 -m unittest script/test_mover.py
import contextlib
import datetime
import http.server
import io
import json
import os
import pwd
import sys
import tempfile
import threading
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mover as tm  # noqa: E402
import trash_fsops as fsops  # noqa: E402

ME = pwd.getpwuid(os.geteuid()).pw_name
# Tests run as a normal user, whose uid may be below the production floor.
fsops.MIN_UID = 0
TODAY = datetime.date(2026, 11, 4)


class FakeApi:
    """A tiny duapi stand-in: GET /api/health lists `filesystems`, POST
    /api/trash/claim returns `items`. Every request is kept in `requests` as
    (method, path, body)."""

    def __init__(self, filesystems=(), items=(), honor=True):
        self.filesystems, self.items, self.requests = list(filesystems), list(items), []
        # honor=False plays an older duapi that ignores ?dry_run, ?fs and
        # ?action (and has no archive queue).
        self.honor = honor

    def __enter__(self):
        api = self

        class H(http.server.BaseHTTPRequestHandler):
            def _reply(self, obj):
                body = json.dumps(obj).encode()
                self.send_response(200)
                if api.honor and "dry_run=1" in self.path:
                    self.send_header("X-Trash-Dry-Run", "1")
                if api.honor and "fs=" in self.path:
                    self.send_header("X-Trash-Fs-Filter", "1")
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                api.requests.append(("GET", self.path, None, self.headers.get("Authorization")))
                # An older duapi (honor=False) has no archive queue either.
                self._reply({"filesystems": api.filesystems, "archive": api.honor})

            def do_POST(self):
                n = int(self.headers.get("Content-Length") or 0)
                body = json.loads(self.rfile.read(n) or b"null")
                api.requests.append(("POST", self.path, body, self.headers.get("Authorization")))
                if not self.path.startswith("/api/trash/claim"):
                    return self._reply({"recorded": 1})
                want = "archive" if "action=archive" in self.path else "trash"
                self._reply([i for i in api.items
                             if not api.honor or (i.get("action") or "trash") == want])

            def log_message(self, *a):
                pass

        self.srv = http.server.HTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.srv.server_port}"
        return self

    def __exit__(self, *a):
        self.srv.shutdown()
        self.srv.server_close()

    def posts(self, prefix):
        return [r for r in self.requests if r[0] == "POST" and r[1].startswith(prefix)]


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
        self.assertIsNone(fsops.skip_reason(self.item(p), check_mount=False))

    def test_gone(self):
        self.assertIn("gone", fsops.skip_reason(self.item(self.fs + "/nope"), check_mount=False))

    def test_inside_trash_or_stats_or_root(self):
        for rel in [".trash/2026-11-01/x", ".stats/scan.csv"]:
            p = self.mk(rel)
            self.assertIsNotNone(fsops.skip_reason(self.item(p), check_mount=False), rel)
        self.assertIsNotNone(fsops.skip_reason(self.item(self.fs), check_mount=False))

    def test_outside_fs(self):
        with tempfile.NamedTemporaryFile() as f:
            self.assertIsNotNone(fsops.skip_reason(self.item(f.name), check_mount=False))

    def test_not_owned_and_unknown_owner(self):
        p = self.mk("a/file")
        if os.geteuid() != 0:
            self.assertIn("owner", fsops.skip_reason(self.item(p, owner="root"), check_mount=False))
        self.assertIn("owner", fsops.skip_reason(self.item(p, owner="no-such-user-xyz"), check_mount=False))

    def test_path_through_a_symlinked_parent(self):
        self.mk("real/file")
        os.symlink(os.path.join(self.fs, "real"), os.path.join(self.fs, "link"))
        p = os.path.join(self.fs, "link/file")
        self.assertIn("symlink", fsops.skip_reason(self.item(p), check_mount=False))

    def test_not_a_mount_point(self):
        p = self.mk("a/file")
        self.assertIn("mount", fsops.skip_reason(self.item(p), check_mount=True))


class Move(Fs):
    def test_file_keeps_its_path_under_dated_trash(self):
        p = self.mk("a/b/file.dat")
        dest = fsops.move_item(self.item(p), TODAY, check_mount=False)
        self.assertEqual(dest, os.path.join(self.fs, ".trash/2026-11-04/a/b/file.dat"))
        self.assertTrue(os.path.isfile(dest))
        self.assertFalse(os.path.exists(p))
        self.assertEqual(os.stat(os.path.join(self.fs, ".trash")).st_mode & 0o777, 0o700)

    def test_folder_is_moved_as_is(self):
        self.mk("a/run1/x.txt", b"hello")
        self.mk("a/run1/sub/y.txt")
        before = os.stat(os.path.join(self.fs, "a/run1"))
        dest = fsops.move_item(self.item(os.path.join(self.fs, "a/run1")), TODAY, check_mount=False)
        self.assertEqual(dest, os.path.join(self.fs, ".trash/2026-11-04/a/run1"))
        self.assertFalse(os.path.exists(os.path.join(self.fs, "a/run1")))
        self.assertEqual(os.stat(dest).st_ino, before.st_ino)  # renamed, not copied
        with open(os.path.join(dest, "x.txt"), "rb") as f:
            self.assertEqual(f.read(), b"hello")
        self.assertTrue(os.path.isfile(os.path.join(dest, "sub/y.txt")))

    def test_same_path_twice_a_day_gets_a_suffix(self):
        first = fsops.move_item(self.item(self.mk("a/f")), TODAY, check_mount=False)
        second = fsops.move_item(self.item(self.mk("a/f")), TODAY, check_mount=False)
        self.assertNotEqual(first, second)
        self.assertTrue(os.path.isfile(first) and os.path.isfile(second))


class State(unittest.TestCase):
    def test_batches_of_a_day_are_appended(self):
        with tempfile.TemporaryDirectory() as d:
            tm.save_batch(d, TODAY, [{"path": "/red/nas1/a", "fs": "/red/nas1"}])
            tm.save_batch(d, TODAY, [{"path": "/red/nas2/b", "fs": "/red/nas2", "action": "archive"}])
            with open(os.path.join(d, "2026-11-04.json")) as f:
                self.assertEqual(len(json.load(f)), 2)


class Claim(unittest.TestCase):
    def test_posts_with_bearer_token_and_parses_items(self):
        with FakeApi(items=[{"path": "/red/nas1/a"}]) as api:
            items = tm.claim(api.url, "tok", allow_http=True)
        self.assertEqual(items, [{"path": "/red/nas1/a"}])
        self.assertEqual([(r[1], r[3]) for r in api.requests], [("/api/trash/claim", "Bearer tok")])

    def test_claims_only_the_given_filesystems(self):
        with FakeApi() as api:
            tm.claim(api.url, "tok", allow_http=True, filesystems=["/red/nas1", "/red/nas2"])
        self.assertEqual(api.requests[0][1], "/api/trash/claim?fs=%2Fred%2Fnas1%2C%2Fred%2Fnas2")


class Hardening(Fs):
    """Review findings C1, C2, I1, I3: the cron runs as root on user trees."""

    def test_parent_swapped_for_symlink_after_check_does_not_move_the_target(self):
        # alice queues a folder she owns, then swaps its parent for a link
        # to the filesystem root between the check and the move.
        self.mk("projects/bob/data", b"victim")
        decoy = self.mk("alice/x/projects/mine", b"decoy")
        item = self.item(os.path.join(self.fs, "alice/x/projects"))
        self.assertIsNone(fsops.skip_reason(item, check_mount=False))
        os.rename(os.path.join(self.fs, "alice/x"), os.path.join(self.fs, "alice/x.real"))
        os.symlink(self.fs, os.path.join(self.fs, "alice/x"))
        try:
            fsops.move_item(item, TODAY, check_mount=False)
        except fsops.Skip:
            pass
        self.assertTrue(os.path.isfile(os.path.join(self.fs, "projects/bob/data")))
        del decoy

    def test_swap_inside_move_is_pinned_to_the_checked_folder(self):
        self.mk("projects/bob/data", b"victim")
        self.mk("alice/x/projects/mine", b"decoy")
        item = self.item(os.path.join(self.fs, "alice/x/projects"))
        real_open_trash = fsops.open_trash

        def swap_then_open(*a):
            os.rename(os.path.join(self.fs, "alice/x"), os.path.join(self.fs, "alice/x.real"))
            os.symlink(self.fs, os.path.join(self.fs, "alice/x"))
            return real_open_trash(*a)

        with mock.patch.object(fsops, "open_trash", swap_then_open):
            dest = fsops.move_item(item, TODAY, check_mount=False)
        self.assertTrue(os.path.isfile(os.path.join(self.fs, "projects/bob/data")))
        self.assertFalse(os.path.exists(os.path.join(self.fs, "alice/x.real/projects")))
        self.assertTrue(dest.endswith("alice/x/projects"))

    def test_planted_trash_symlink_is_refused(self):
        victim = os.path.join(self.fs, "bob")
        os.makedirs(os.path.join(victim, "2020-01-01"))
        os.chmod(victim, 0o755)
        os.symlink(victim, os.path.join(self.fs, ".trash"))
        f = self.mk("a/file")
        with self.assertRaises(Exception):
            fsops.move_item(self.item(f), TODAY, check_mount=False)
        self.assertTrue(os.path.isfile(f))
        self.assertTrue(os.path.isdir(os.path.join(victim, "2020-01-01")))
        self.assertEqual(os.stat(victim).st_mode & 0o777, 0o755)

    def test_tree_walk_rejects_entries_of_another_owner(self):
        self.mk("a/run1/x")
        fsfd = os.open(self.fs, os.O_RDONLY | os.O_DIRECTORY)
        try:
            pfd = os.open("a", os.O_RDONLY | os.O_DIRECTORY, dir_fd=fsfd)
            st = os.stat("run1", dir_fd=pfd, follow_symlinks=False)
            with self.assertRaises(fsops.Skip) as cm:
                fsops.check_tree(pfd, "run1", st, os.geteuid() + 1, st.st_dev, "someone")
            self.assertIn("not owned", str(cm.exception))
            fsops.check_tree(pfd, "run1", st, os.geteuid(), st.st_dev, ME)  # own tree: fine
            os.close(pfd)
        finally:
            os.close(fsfd)

    def test_filesystem_allowlist(self):
        p = self.mk("a/file")
        self.assertIn("--fs", fsops.skip_reason(self.item(p), check_mount=False, allowed_fs={"/elsewhere"}))
        self.assertIsNone(fsops.skip_reason(self.item(p), check_mount=False, allowed_fs={self.fs}))

    def test_system_account_owners_are_refused(self):
        p = self.mk("a/file")
        reason = fsops.skip_reason(self.item(p), check_mount=False, min_uid=os.geteuid() + 1)
        self.assertIn("system account", reason)

    def test_plain_http_needs_an_explicit_flag(self):
        with self.assertRaises(ValueError):
            tm.claim("http://127.0.0.1:9/", "tok")


class EndToEnd(Fs):
    def test_run_moves_each_item_where_its_action_says(self):
        f = self.mk("p/file")
        self.mk("p/dir/x")
        self.mk("p/old/y")
        gone = self.item(self.fs + "/p/missing")
        archive = {**self.item(self.fs + "/p/old"), "action": "archive"}
        errors = tm.run([self.item(f), self.item(self.fs + "/p/dir"), gone, archive],
                        today=TODAY, check_mount=False)
        self.assertEqual(errors, 0)
        t = os.path.join(self.fs, ".trash/2026-11-04/p")
        self.assertTrue(os.path.isfile(os.path.join(t, "file")))
        self.assertTrue(os.path.isfile(os.path.join(t, "dir/x")))
        self.assertTrue(os.path.isfile(os.path.join(self.fs, ".archive/p/old.2026-11-04/old/y")))
        self.assertFalse(os.path.exists(self.fs + "/p/old"))

    def test_unknown_action_is_an_error_and_stays(self):
        f = self.mk("p/file")
        self.assertEqual(tm.run([{**self.item(f), "action": "shred"}], TODAY, check_mount=False), 1)
        self.assertTrue(os.path.isfile(f))

    def test_output_moves_the_reserved_folders_out_of_the_fs(self):
        f = self.mk("p/file")
        self.mk("p/dir/x")
        archive = {**self.item(self.fs + "/p/dir"), "action": "archive"}
        with tempfile.TemporaryDirectory() as out:
            self.assertEqual(tm.run([self.item(f), archive], TODAY, check_mount=False,
                                   output=out), 0)
            top = os.path.join(out, os.path.basename(self.fs))
            self.assertTrue(os.path.isfile(os.path.join(top, ".trash", "2026-11-04", "p", "file")))
            self.assertTrue(os.path.isdir(os.path.join(top, ".archive", "p", "dir.2026-11-04", "dir")))
        self.assertFalse(os.path.exists(os.path.join(self.fs, ".trash")))
        self.assertFalse(os.path.exists(os.path.join(self.fs, ".archive")))
        self.assertFalse(os.path.exists(f))


def run_main(api, *args, env=None):
    """main() against `api` with a temp HOME and TRASH_TOKEN=tok."""
    with tempfile.TemporaryDirectory() as home, \
            mock.patch.dict(os.environ, {"TRASH_TOKEN": "tok", "HOME": home, **(env or {})}):
        rc = tm.main(["--api", api.url, *args])
        state = os.path.join(home, ".dutopia-mover")
        saved = sorted(os.listdir(state)) if os.path.isdir(state) else []
    return rc, saved


class Main(unittest.TestCase):
    def test_dev_mode_needs_only_the_api_and_the_token_env(self):
        with FakeApi(filesystems=["/red/nas1"]) as api:
            rc, saved = run_main(api, "--dev")
        self.assertEqual(rc, 0)
        self.assertEqual(api.posts("/api/trash/claim")[0][3], "Bearer tok")
        self.assertEqual(len([n for n in saved if n.endswith(".json")]), 1)

    def test_local_filesystems_are_the_mounted_allowed_ones(self):
        self.assertEqual(tm.local_filesystems(["/", "/no/such/mount"], None, check_mount=True), ["/"])
        self.assertEqual(tm.local_filesystems(["/a", "/b"], {"/b"}, check_mount=False), ["/b"])

    def test_fs_flag_narrows_both_claims(self):
        with FakeApi(filesystems=["/red/nas1", "/red/nas2"]) as api:
            run_main(api, "--dev", "--fs", "/red/nas2")
        paths = [r[1] for r in api.posts("/api/trash/claim")]
        self.assertEqual(len(paths), 2)
        self.assertIn("action=archive", paths[1])
        for p in paths:
            self.assertIn("fs=%2Fred%2Fnas2", p)
            self.assertNotIn("nas1", p)


class DryRun(Fs):
    def test_dry_run_checks_but_moves_nothing(self):
        f = self.mk("p/file")
        self.mk("p/dir/x")
        archive = {**self.item(self.fs + "/p/dir"), "action": "archive"}
        errors = tm.run([self.item(f), archive, self.item(self.fs + "/p/gone")],
                        today=TODAY, check_mount=False, dry_run=True)
        self.assertEqual(errors, 0)
        self.assertTrue(os.path.isfile(f))
        self.assertTrue(os.path.isfile(self.fs + "/p/dir/x"))
        self.assertEqual(sorted(os.listdir(self.fs)), ["p"])

    def test_dry_run_claim_asks_the_server_not_to_remove(self):
        with FakeApi(filesystems=["/red/nas1"]) as api:
            rc, saved = run_main(api, "--dev", "--dry-run")
        self.assertEqual(rc, 0)
        self.assertTrue(all("dry_run=1" in r[1] for r in api.posts("/api/trash/claim")))
        self.assertEqual(saved, [])  # nothing claimed, nothing to save


class StateDir(unittest.TestCase):
    def test_unusable_state_dir_stops_before_claiming(self):
        with FakeApi(filesystems=["/red/nas1"]) as api:
            rc, _ = run_main(api, "--dev", "--state-dir", "/proc/nope/never" if sys.platform != "darwin" else "/System/nope")
        self.assertEqual(rc, 1)
        self.assertEqual(api.posts("/api/trash/claim"), [])


class Report(Fs):
    def _main(self, api, *args):
        with mock.patch.dict(os.environ, {"TRASH_TOKEN": "tok"}):
            return tm.main(["--api", api.url, "--dev", "--state-dir", self.state, *args])

    def setUp(self):
        super().setUp()
        self._st = tempfile.TemporaryDirectory()
        self.state = self._st.name

    def tearDown(self):
        self._st.cleanup()
        super().tearDown()

    def test_results_are_logged_and_nothing_is_posted_back(self):
        f = self.mk("p/file")
        items = [self.item(f), self.item(self.fs + "/p/gone")]
        out = io.StringIO()
        with FakeApi(filesystems=[self.fs], items=items) as api, contextlib.redirect_stdout(out):
            rc = self._main(api)
        self.assertEqual(rc, 0)
        log = out.getvalue()
        self.assertIn(f"[ok] {f} -> ", log)
        self.assertIn(f"[skip] {self.fs}/p/gone: ", log)
        self.assertEqual([r[1].split("?")[0] for r in api.requests if r[0] == "POST"],
                         ["/api/trash/claim"] * 2)  # trash, then archive; nothing else

    def test_batch_rerun_moves_without_claiming(self):
        p = self.mk("p/file")
        batch = os.path.join(self.state, "b.json")
        with open(batch, "w") as f:
            json.dump([self.item(p)], f)
        with FakeApi(filesystems=[self.fs]) as api:
            self.assertEqual(self._main(api, "--batch", batch), 0)
        self.assertEqual(api.posts("/api/trash/claim"), [])
        self.assertFalse(os.path.exists(p))

    def test_old_server_ignoring_dry_run_is_caught(self):
        p = self.mk("p/file")
        with FakeApi(filesystems=[self.fs], items=[self.item(p)], honor=False) as api:
            rc = self._main(api, "--dry-run")
        self.assertEqual(rc, 1)
        self.assertTrue(os.path.isfile(p))  # nothing moved
        saved = [n for n in os.listdir(self.state) if n.endswith(".json")]
        self.assertEqual(len(saved), 1, "the items it really claimed are saved for --batch")

    def test_old_server_still_moves_the_trash_but_never_claims_the_archive(self):
        p = self.mk("p/file")
        out = io.StringIO()
        with FakeApi(filesystems=[self.fs], items=[self.item(p)], honor=False) as api, \
                contextlib.redirect_stdout(out):
            self.assertEqual(self._main(api), 1)  # flagged: upgrade duapi
        self.assertFalse(os.path.exists(p))
        self.assertEqual(len(api.posts("/api/trash/claim")), 1)
        self.assertIn("no archive queue", out.getvalue())

    def test_claims_the_archive_and_moves_it_aside(self):
        self.mk("p/dir/x")
        items = [{**self.item(self.fs + "/p/dir"), "action": "archive"}]
        with FakeApi(filesystems=[self.fs], items=items) as api:
            self.assertEqual(self._main(api), 0)
        day = datetime.date.today().isoformat()
        self.assertTrue(os.path.isfile(os.path.join(self.fs, ".archive/p", f"dir.{day}", "dir/x")))
        self.assertFalse(os.path.exists(self.fs + "/p/dir"))

    def test_unreachable_api_is_an_error(self):
        with mock.patch.dict(os.environ, {"TRASH_TOKEN": "tok"}):
            rc = tm.main(["--api", "http://127.0.0.1:9", "--dev", "--state-dir", self.state])
        self.assertEqual(rc, 1)

    def test_fs_flag_matching_no_server_filesystem_is_an_error(self):
        with FakeApi(filesystems=[self.fs]) as api:
            rc = self._main(api, "--fs", "/typo")
        self.assertEqual(rc, 1)
        self.assertEqual(api.posts("/api/trash/claim"), [])

    def test_a_second_run_waits_for_nobody_and_claims_nothing(self):
        import fcntl
        os.makedirs(self.state, exist_ok=True)
        with open(os.path.join(self.state, ".lock"), "w") as held:
            fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with FakeApi(filesystems=[self.fs]) as api:
                rc = self._main(api)
        self.assertEqual(rc, 1)
        self.assertEqual(api.posts("/api/trash/claim"), [])

    def test_corrupt_saved_batch_still_processes(self):
        with open(os.path.join(self.state, f"{datetime.date.today().isoformat()}.json"), "w") as f:
            f.write("{not json")
        p = self.mk("p/file")
        with FakeApi(filesystems=[self.fs], items=[self.item(p)]) as api:
            rc = self._main(api)
        self.assertEqual(rc, 1)  # the save failed: flagged for the cron mail
        self.assertFalse(os.path.exists(p))  # but the claimed item was moved


class Flags(unittest.TestCase):
    def test_removed_commands_and_flags_are_refused(self):
        for args in (["move"], ["purge"], ["--keep-days", "7"], ["--allow-http"], ["--min-uid", "0"]):
            with self.assertRaises(SystemExit, msg=args), contextlib.redirect_stderr(io.StringIO()):
                tm.main(["--dev", "--api", "http://x", *args])


if __name__ == "__main__":
    unittest.main()
