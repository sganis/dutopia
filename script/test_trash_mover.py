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
tm.fsops.MIN_UID = 0
TODAY = datetime.date(2026, 11, 4)


class FakeApi:
    """A tiny duapi stand-in: GET /api/health lists `filesystems`, POST
    /api/trash/claim returns `items`, POST /api/trash/report records the
    body. Every request is kept in `requests` as (method, path, body)."""

    def __init__(self, filesystems=(), items=(), honor=True):
        self.filesystems, self.items, self.requests = list(filesystems), list(items), []
        # honor=False plays an older duapi that ignores ?dry_run and ?fs.
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
                self._reply({"filesystems": api.filesystems})

            def do_POST(self):
                n = int(self.headers.get("Content-Length") or 0)
                body = json.loads(self.rfile.read(n) or b"null")
                api.requests.append(("POST", self.path, body, self.headers.get("Authorization")))
                self._reply(api.items if self.path.startswith("/api/trash/claim") else {"recorded": 1})

            def log_message(self, *a):
                pass

        self.srv = http.server.HTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.srv.server_port}"
        return self

    def __exit__(self, *a):
        self.srv.shutdown()

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

        with mock.patch.object(tm.fsops, "open_trash", swap_then_open):
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



def run_main(api, *args, env=None):
    """main() against `api` with a temp HOME and TRASH_TOKEN=tok."""
    with tempfile.TemporaryDirectory() as home, \
            mock.patch.dict(os.environ, {"TRASH_TOKEN": "tok", "HOME": home, **(env or {})}):
        rc = tm.main(["--api", api.url, *args])
        state = os.path.join(home, ".dutopia-trash")
        saved = sorted(os.listdir(state)) if os.path.isdir(state) else []
    return rc, saved


class Main(unittest.TestCase):
    def test_dev_mode_needs_only_the_api_and_the_token_env(self):
        with FakeApi(filesystems=["/red/nas1"]) as api:
            rc, saved = run_main(api, "--dev")
        self.assertEqual(rc, 0)
        self.assertEqual(api.posts("/api/trash/claim")[0][3], "Bearer tok")
        self.assertEqual(len([n for n in saved if n.endswith(".json")]), 1)

    def test_claims_only_filesystems_mounted_here(self):
        # Without --dev the mount check applies: these are not mount points.
        with FakeApi(filesystems=["/red/nas1", "/red/nas2"]) as api, \
                tempfile.TemporaryDirectory() as state:
            rc, _ = run_main(api, "--allow-http", "--state-dir", state)
        self.assertEqual(rc, 0)
        self.assertEqual(api.posts("/api/trash/claim"), [])  # nothing mounted: claim nothing

    def test_fs_flag_narrows_the_claim(self):
        with FakeApi(filesystems=["/red/nas1", "/red/nas2"]) as api:
            run_main(api, "--dev", "--fs", "/red/nas2")
        self.assertIn("fs=%2Fred%2Fnas2", api.posts("/api/trash/claim")[0][1])
        self.assertNotIn("nas1", api.posts("/api/trash/claim")[0][1])


class DryRun(Fs):
    def test_dry_run_checks_but_moves_and_purges_nothing(self):
        old = os.path.join(self.fs, ".trash", "2026-01-01")
        os.makedirs(old)
        f = self.mk("p/file")
        self.mk("p/dir/x")
        with tempfile.TemporaryDirectory() as state:
            errors = tm.run(
                [self.item(f), self.item(self.fs + "/p/dir"), self.item(self.fs + "/p/gone")],
                today=TODAY, state_dir=state, keep_days=30, purge_fs=[],
                check_mount=False, dry_run=True,
            )
        self.assertEqual(errors, 0)
        self.assertTrue(os.path.isfile(f))
        self.assertTrue(os.path.isfile(self.fs + "/p/dir/x"))
        self.assertTrue(os.path.isdir(old))
        self.assertEqual(os.listdir(os.path.join(self.fs, ".trash")), ["2026-01-01"])

    def test_purge_dry_run_lists_without_deleting(self):
        os.makedirs(os.path.join(self.fs, ".trash", "2026-01-01"))
        listed = tm.purge(self.fs, TODAY, keep_days=30, dry_run=True)
        self.assertEqual([os.path.basename(p) for p in listed], ["2026-01-01"])
        self.assertTrue(os.path.isdir(os.path.join(self.fs, ".trash", "2026-01-01")))

    def test_dry_run_claim_asks_the_server_not_to_remove(self):
        with FakeApi(filesystems=["/red/nas1"]) as api:
            rc, saved = run_main(api, "--dev", "--dry-run")
        self.assertEqual(rc, 0)
        self.assertIn("dry_run=1", api.posts("/api/trash/claim")[0][1])
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

    def test_results_are_reported_to_the_server(self):
        f = self.mk("p/file")
        items = [self.item(f), self.item(self.fs + "/p/gone")]
        with FakeApi(filesystems=[self.fs], items=items) as api:
            rc = self._main(api)
        self.assertEqual(rc, 0)
        (_, _, body, auth), = api.posts("/api/trash/report")
        self.assertEqual(auth, "Bearer tok")
        by_path = {o["path"]: o for o in body}
        moved = by_path[f]
        self.assertEqual(moved["status"], "moved")
        self.assertTrue(moved["dest"].endswith("/p/file"))
        self.assertEqual(moved["owner"], ME)
        self.assertEqual(by_path[self.fs + "/p/gone"]["status"], "skipped")
        self.assertIn("gone", by_path[self.fs + "/p/gone"]["detail"])

    def test_dry_run_reports_nothing(self):
        items = [self.item(self.mk("p/file")), self.item(self.fs + "/p/gone")]
        with FakeApi(filesystems=[self.fs], items=items) as api:
            self._main(api, "--dry-run")
        self.assertEqual(api.posts("/api/trash/report"), [])

    def test_report_goes_in_chunks_under_the_body_limit(self):
        items = [self.item(f"{self.fs}/gone/{i}") for i in range(250)]
        with FakeApi(filesystems=[self.fs], items=items) as api:
            self.assertEqual(self._main(api), 0)
        sizes = [len(r[2]) for r in api.posts("/api/trash/report")]
        self.assertEqual(sizes, [100, 100, 50])

    def test_batch_rerun_reports_too(self):
        p = self.mk("p/file")
        batch = os.path.join(self.state, "b.json")
        with open(batch, "w") as f:
            json.dump([self.item(p)], f)
        with FakeApi(filesystems=[self.fs]) as api:
            self.assertEqual(self._main(api, "--batch", batch), 0)
        self.assertEqual(api.posts("/api/trash/claim"), [])
        (_, _, body, _), = api.posts("/api/trash/report")
        self.assertEqual(body[0]["status"], "moved")

    def test_old_server_ignoring_dry_run_is_caught(self):
        p = self.mk("p/file")
        with FakeApi(filesystems=[self.fs], items=[self.item(p)], honor=False) as api:
            rc = self._main(api, "--dry-run")
        self.assertEqual(rc, 1)
        self.assertTrue(os.path.isfile(p))  # nothing moved
        saved = [n for n in os.listdir(self.state) if n.endswith(".json")]
        self.assertEqual(len(saved), 1, "the items it really claimed are saved for --batch")

    def test_old_server_ignoring_the_fs_filter_still_runs(self):
        p = self.mk("p/file")
        with FakeApi(filesystems=[self.fs], items=[self.item(p)], honor=False) as api:
            self.assertEqual(self._main(api), 0)
        self.assertFalse(os.path.exists(p))

    def test_unreachable_api_still_purges_known_filesystems(self):
        old = os.path.join(self.fs, ".trash", "2020-01-01")
        os.makedirs(old)
        tm.save_batch(self.state, datetime.date(2020, 1, 1), [self.item(self.fs + "/x")])
        with mock.patch.dict(os.environ, {"TRASH_TOKEN": "tok"}):
            rc = tm.main(["--api", "http://127.0.0.1:9", "--dev", "--state-dir", self.state])
        self.assertEqual(rc, 1)
        self.assertFalse(os.path.exists(old))

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

    def test_corrupt_saved_batch_still_processes_and_reports(self):
        with open(os.path.join(self.state, f"{datetime.date.today().isoformat()}.json"), "w") as f:
            f.write("{not json")
        p = self.mk("p/file")
        with FakeApi(filesystems=[self.fs], items=[self.item(p)]) as api:
            rc = self._main(api)
        self.assertEqual(rc, 1)  # the save failed: flagged for the cron mail
        self.assertFalse(os.path.exists(p))  # but the claimed item was moved
        self.assertEqual(len(api.posts("/api/trash/report")), 1)


if __name__ == "__main__":
    unittest.main()
