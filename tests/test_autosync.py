import json
import unittest
from unittest import mock

import helpers

from cubi.argocd_apps import normalize
from cubi.modules import autosync


def application(name="demo", automated=None, state=None, owners=None, sources=None, tracking=False):
    metadata = {"name": name, "annotations": {"argocd.argoproj.io/tracking-id": "root:x"} if tracking else {}}
    if owners:
        metadata["ownerReferences"] = owners
    spec = {"project": "default", "syncPolicy": {}}
    if automated is not None:
        spec["syncPolicy"]["automated"] = automated
    if sources:
        spec["sources"] = sources
    else:
        spec["source"] = {"repoURL": "https://github.com/pkr-lab/capulus-core.git", "path": "apps/demo", "targetRevision": "main"}
    return normalize({"metadata": metadata, "spec": spec, "status": {}}, state)


def remembered(value, name="demo"):
    return {f"{name}.sync": json.dumps(value)}


APPSET = [{"kind": "ApplicationSet", "name": "apps"}]


class DisableTest(unittest.TestCase):
    def test_disable_remembers_flags_in_state(self):
        change = autosync.plan(application(automated={"prune": True, "selfHeal": True}), autosync.DISABLE)
        self.assertEqual(change.state_key, "demo.sync")
        self.assertEqual(json.loads(change.state_value), {"prune": True, "selfHeal": True})
        self.assertEqual((change.before, change.after), ("auto (prune, selfHeal)", "manual"))

    def test_disable_remembers_empty_automated_block(self):
        change = autosync.plan(application(automated={}), autosync.DISABLE)
        self.assertEqual(json.loads(change.state_value), {})

    def test_disable_skips_manual_apps(self):
        self.assertEqual(autosync.plan(application(), autosync.DISABLE), "bereits manual")


class EnableTest(unittest.TestCase):
    def test_enable_restores_remembered_flags(self):
        app = application(state=remembered({"prune": True, "selfHeal": False, "allowEmpty": True}))
        change = autosync.plan(app, autosync.ENABLE)
        self.assertEqual(change.restore, {"prune": True, "selfHeal": False, "allowEmpty": True})
        self.assertEqual(change.after, "auto (prune, allowEmpty)")

    def test_enable_without_memory_uses_plain_automated(self):
        change = autosync.plan(application(), autosync.ENABLE)
        self.assertEqual(change.restore, {})
        self.assertTrue(any("keine gemerkten" in note for note in change.notes))

    def test_enable_ignores_corrupt_state(self):
        change = autosync.plan(application(state={"demo.sync": "{kaputt"}), autosync.ENABLE)
        self.assertEqual(change.restore, {})

    def test_enable_skips_auto_apps(self):
        self.assertEqual(autosync.plan(application(automated={}), autosync.ENABLE), "bereits auto")

    def test_state_of_other_app_is_not_used(self):
        app = application(state=remembered({"prune": True}, name="other"))
        self.assertEqual(autosync.plan(app, autosync.ENABLE).restore, {})


class OwnerTest(unittest.TestCase):
    def test_applicationset_app_is_held_by_rule(self):
        change = autosync.plan(application(automated={}, owners=APPSET), autosync.DISABLE)
        self.assertTrue(any("ApplicationSet apps" in note and "ignoreApplicationDifferences" in note for note in change.notes))

    def test_enable_mentions_rule_removal(self):
        change = autosync.plan(application(owners=APPSET), autosync.ENABLE)
        self.assertTrue(any("Regel wird entfernt" in note for note in change.notes))

    def test_parent_app_tracking_is_flagged(self):
        change = autosync.plan(application(automated={}, tracking=True), autosync.DISABLE)
        self.assertTrue(any("übergeordneten App" in note for note in change.notes))

    def test_owner_cell(self):
        self.assertEqual(autosync.owner_cell(application(owners=APPSET)).plain, "AppSet apps")
        self.assertEqual(autosync.owner_cell(application()).plain, "-")


class ApplyTest(unittest.TestCase):
    def manager(self):
        manager = mock.Mock()
        patches = [
            mock.patch.object(autosync.argocd_apps, "save_state", manager.save_state),
            mock.patch.object(autosync.argocd_apps, "pause_auto_sync", manager.pause),
            mock.patch.object(autosync.argocd_apps, "resume_auto_sync", manager.resume),
            mock.patch.object(autosync.ui, "console"),
        ]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)
        return manager

    def test_disable_saves_state_before_pausing(self):
        manager = self.manager()
        change = autosync.plan(application(automated={"prune": True}), autosync.DISABLE)
        self.assertEqual(autosync.apply([change]), [change])
        self.assertEqual([call[0] for call in manager.mock_calls], ["save_state", "pause"])
        self.assertEqual(manager.save_state.call_args.args[0], "demo.sync")

    def test_enable_resumes_before_dropping_state(self):
        manager = self.manager()
        change = autosync.plan(application(state=remembered({"prune": True})), autosync.ENABLE)
        autosync.apply([change])
        self.assertEqual([call[0] for call in manager.mock_calls], ["resume", "save_state"])
        self.assertEqual(manager.resume.call_args.args[1], {"prune": True})
        self.assertEqual(manager.save_state.call_args.args, ("demo.sync", None))

    def test_failure_is_reported_and_does_not_stop_others(self):
        manager = self.manager()
        manager.pause.side_effect = [autosync.CubiError("forbidden"), None]
        first = autosync.plan(application("one", automated={}), autosync.DISABLE)
        second = autosync.plan(application("two", automated={}), autosync.DISABLE)
        self.assertEqual(autosync.apply([first, second]), [second])
        printed = " ".join(call.args[0].plain for call in autosync.ui.console.print.call_args_list)
        self.assertIn("✖ one", printed)
        self.assertIn("✔ two", printed)


class VerifyTest(unittest.TestCase):
    def run_verify(self, change, current):
        with mock.patch.object(autosync.argocd_apps, "recheck", return_value=current), mock.patch.object(autosync.argocd_apps, "report_checks") as report:
            autosync.verify([change])
        return report.call_args.args[0]

    def test_disable_holds_when_still_manual(self):
        change = autosync.plan(application(automated={}), autosync.DISABLE)
        self.assertEqual(self.run_verify(change, {"demo": application()}), [("demo", True)])

    def test_disable_reverted_by_controller_is_reported(self):
        change = autosync.plan(application(automated={}), autosync.DISABLE)
        self.assertEqual(self.run_verify(change, {"demo": application(automated={"selfHeal": True})}), [("demo", False)])

    def test_enable_checks_for_automated(self):
        change = autosync.plan(application(), autosync.ENABLE)
        self.assertEqual(self.run_verify(change, {"demo": application(automated={})}), [("demo", True)])

    def test_missing_app_counts_as_failure(self):
        change = autosync.plan(application(automated={}), autosync.DISABLE)
        self.assertEqual(self.run_verify(change, {}), [("demo", False)])


if __name__ == "__main__":
    unittest.main()
