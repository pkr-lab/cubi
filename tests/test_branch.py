import json
import subprocess
import unittest
from unittest import mock

import helpers

from cubi.errors import CubiError
from cubi.modules import branch
from test_autosync import APPSET, application

SINGLE = {"repoURL": "https://github.com/pkr-lab/capulus-core.git", "path": "apps/demo", "targetRevision": "main"}
MULTI = [
    {"repoURL": "https://github.com/pkr-lab/charts.git", "path": "chart", "targetRevision": "v1"},
    {"repoURL": "https://github.com/pkr-lab/values.git", "ref": "values", "targetRevision": "main"},
]


def completed(code=0, stdout="", stderr=""):
    return subprocess.CompletedProcess([], code, stdout, stderr)


class PatchTest(unittest.TestCase):
    def test_single_source_patch_targets_spec_source(self):
        app = application()
        patch = branch.build_patch(app, 0, "feature/x")
        self.assertEqual(patch[1], {"op": "add", "path": "/spec/source/targetRevision", "value": "feature/x"})
        self.assertEqual(patch[0], {"op": "test", "path": "/spec/source/repoURL", "value": SINGLE["repoURL"]})

    def test_multi_source_patch_targets_chosen_index(self):
        app = application(sources=MULTI)
        patch = branch.build_patch(app, 1, "dev")
        self.assertEqual(patch[1]["path"], "/spec/sources/1/targetRevision")
        self.assertEqual(patch[0]["value"], MULTI[1]["repoURL"])

    def test_single_entry_sources_list_still_uses_sources_path(self):
        app = application(sources=MULTI[:1])
        self.assertEqual(branch.build_patch(app, 0, "dev")[1]["path"], "/spec/sources/0/targetRevision")

    def test_plan_skips_app_already_on_target(self):
        self.assertIn("main", branch.plan(application(), 0, "main"))

    def test_plan_records_previous_branch(self):
        change = branch.plan(application(), 0, "dev")
        self.assertEqual((change.previous, change.target), ("main", "dev"))

    def test_missing_target_revision_is_head(self):
        app = application(sources=[{"repoURL": "https://example.com/r.git"}])
        self.assertEqual(branch.plan(app, 0, "main").previous, "HEAD")


class NotesTest(unittest.TestCase):
    def test_auto_sync_note_mentions_immediate_sync(self):
        change = branch.plan(application(automated={"selfHeal": True}), 0, "dev")
        self.assertTrue(any("synct sofort" in note for note in change.notes))

    def test_prune_gets_extra_warning(self):
        change = branch.plan(application(automated={"prune": True}), 0, "dev")
        self.assertTrue(any("prune" in note and "gelöscht" in note for note in change.notes))

    def test_manual_app_has_no_notes(self):
        self.assertEqual(branch.plan(application(), 0, "dev").notes, [])


class DisplayTest(unittest.TestCase):
    def test_off_target_detects_other_branch(self):
        self.assertFalse(branch.off_target(application()))
        self.assertTrue(branch.off_target(application(sources=MULTI)))

    def test_cells_show_branch_and_policy(self):
        cells = branch.cells(application(automated={"prune": True}))
        self.assertEqual([cell.plain for cell in cells], ["demo", "main", "auto (prune)", "pkr-lab/capulus-core"])

    def test_multi_source_branches_joined(self):
        self.assertEqual(branch.cells(application(sources=MULTI))[1].plain, "v1 | main")


class RemoteCheckTest(unittest.TestCase):
    def test_github_branch_found(self):
        with mock.patch.object(branch.shell, "capture_json", return_value=[{"ref": "refs/heads/dev"}]):
            self.assertTrue(branch.ref_exists("https://github.com/pkr-lab/x.git", "dev"))

    def test_github_branch_missing(self):
        with mock.patch.object(branch.shell, "capture_json", return_value=[{"ref": "refs/heads/dev-2"}]):
            self.assertFalse(branch.ref_exists("https://github.com/pkr-lab/x.git", "dev"))

    def test_github_tag_counts(self):
        answers = [[], [{"ref": "refs/tags/v1"}]]
        with mock.patch.object(branch.shell, "capture_json", side_effect=answers):
            self.assertTrue(branch.ref_exists("git@github.com:pkr-lab/x.git", "v1"))

    def test_github_error_is_unknown(self):
        with mock.patch.object(branch.shell, "capture_json", side_effect=CubiError("404")):
            self.assertIsNone(branch.ref_exists("https://github.com/pkr-lab/x.git", "dev"))

    def test_commit_id_is_not_reported_missing(self):
        with mock.patch.object(branch.shell, "capture_json", return_value=[]):
            self.assertIsNone(branch.ref_exists("https://github.com/pkr-lab/x.git", "abc1234"))

    def test_generic_remote_uses_ls_remote(self):
        with mock.patch.object(branch.shell, "capture", return_value=completed(0)) as capture:
            self.assertTrue(branch.ref_exists("https://git.example.com/r.git", "dev"))
        command = capture.call_args.args[0]
        self.assertEqual(command[:3], ["git", "ls-remote", "--exit-code"])
        self.assertEqual(capture.call_args.kwargs["env"]["GIT_TERMINAL_PROMPT"], "0")

    def test_generic_remote_missing_and_unreachable(self):
        with mock.patch.object(branch.shell, "capture", return_value=completed(2)):
            self.assertFalse(branch.ref_exists("https://git.example.com/r.git", "dev"))
        with mock.patch.object(branch.shell, "capture", return_value=completed(128, stderr="fatal")):
            self.assertIsNone(branch.ref_exists("https://git.example.com/r.git", "dev"))

    def test_remote_checked_once_per_repo_and_branch(self):
        first = branch.plan(application("a"), 0, "dev")
        second = branch.plan(application("b"), 0, "dev")
        with mock.patch.object(branch, "ref_exists", return_value=True) as check:
            result = branch.check_remote([first, second])
        self.assertEqual(check.call_count, 1)
        self.assertEqual(list(result.values()), [True])


class AppsetPlanTest(unittest.TestCase):
    def test_other_branch_is_held_by_rule(self):
        change = branch.plan(application(owners=APPSET), 0, "dev", "main")
        self.assertTrue(change.hold)
        self.assertEqual(change.pointer, "/spec/source/targetRevision")
        self.assertTrue(any("ignoreApplicationDifferences" in note for note in change.notes))

    def test_template_branch_releases_rule(self):
        change = branch.plan(application(owners=APPSET), 0, "main", "main")
        self.assertIsInstance(change, str)
        change = branch.plan(application(owners=APPSET, sources=[{"repoURL": "https://x/r.git", "targetRevision": "dev"}]), 0, "main", "main")
        self.assertFalse(change.hold)
        self.assertTrue(any("Regel wird entfernt" in note for note in change.notes))

    def test_unknown_template_holds(self):
        self.assertTrue(branch.plan(application(owners=APPSET), 0, "main-x", None).hold)

    def test_plain_app_has_no_hold(self):
        self.assertIsNone(branch.plan(application(), 0, "dev").hold)

    def test_multi_source_pointer(self):
        change = branch.plan(application(owners=APPSET, sources=MULTI), 1, "dev", "main")
        self.assertEqual(change.pointer, "/spec/sources/1/targetRevision")

    def test_template_lookup_is_cached_per_appset(self):
        app = application(owners=APPSET)
        cache = {}
        with mock.patch.object(branch.argocd_apps, "template_revision", return_value="main") as lookup:
            self.assertEqual(branch.template_for(app, 0, cache), "main")
            branch.template_for(app, 0, cache)
        self.assertEqual(lookup.call_count, 1)
        self.assertIsNone(branch.template_for(application(), 0, cache))


class ApplyTest(unittest.TestCase):
    def manager(self):
        manager = mock.Mock()
        patches = [
            mock.patch.object(branch.argocd_apps, "set_hold", manager.hold),
            mock.patch.object(branch.argocd_apps, "patch_app", manager.patch),
            mock.patch.object(branch.ui, "console"),
        ]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)
        return manager

    def test_plain_app_is_patched_with_json_patch(self):
        manager = self.manager()
        change = branch.plan(application(), 0, "dev")
        self.assertEqual(branch.apply([change]), [change])
        self.assertEqual(manager.patch.call_args.args, ("demo", change.patch, "json"))
        manager.hold.assert_not_called()

    def test_held_branch_sets_rule_before_patching(self):
        manager = self.manager()
        change = branch.plan(application(owners=APPSET), 0, "dev", "main")
        branch.apply([change])
        self.assertEqual([call[0] for call in manager.mock_calls], ["hold", "patch"])
        self.assertEqual(manager.hold.call_args.args, ("apps", "demo", "/spec/source/targetRevision", True))

    def test_reset_to_template_patches_then_releases_rule(self):
        manager = self.manager()
        change = branch.plan(application(owners=APPSET, sources=[{"repoURL": "https://x/r.git", "targetRevision": "dev"}]), 0, "main", "main")
        branch.apply([change])
        self.assertEqual([call[0] for call in manager.mock_calls], ["patch", "hold"])
        self.assertEqual(manager.hold.call_args.args[3], False)

    def test_failed_rule_prevents_patch(self):
        manager = self.manager()
        manager.hold.side_effect = branch.CubiError("conflict")
        change = branch.plan(application(owners=APPSET), 0, "dev", "main")
        self.assertEqual(branch.apply([change]), [])
        manager.patch.assert_not_called()


class VerifyTest(unittest.TestCase):
    def run_verify(self, change, current):
        with mock.patch.object(branch.argocd_apps, "recheck", return_value=current), mock.patch.object(branch.argocd_apps, "report_checks") as report:
            branch.verify([change])
        return report.call_args.args[0]

    def test_branch_holds(self):
        change = branch.plan(application(), 0, "dev")
        moved = application(sources=[{"repoURL": "https://x/r.git", "targetRevision": "dev"}])
        self.assertEqual(self.run_verify(change, {"demo": moved}), [("demo", True)])

    def test_reverted_branch_is_reported(self):
        change = branch.plan(application(), 0, "dev")
        self.assertEqual(self.run_verify(change, {"demo": application()}), [("demo", False)])


if __name__ == "__main__":
    unittest.main()
