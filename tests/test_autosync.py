import json
import subprocess
import unittest
from unittest import mock

import helpers

from cubi.argocd_apps import PREVIOUS_SYNC_ANNOTATION, normalize
from cubi.modules import autosync


def application(name="demo", automated=None, annotations=None, owners=None, sources=None):
    metadata = {"name": name, "annotations": annotations or {}}
    if owners:
        metadata["ownerReferences"] = owners
    spec = {"project": "default", "syncPolicy": {}}
    if automated is not None:
        spec["syncPolicy"]["automated"] = automated
    if sources:
        spec["sources"] = sources
    else:
        spec["source"] = {"repoURL": "https://github.com/pkr-lab/capulus-core.git", "path": "apps/demo", "targetRevision": "main"}
    return normalize({"metadata": metadata, "spec": spec, "status": {}})


class DisableTest(unittest.TestCase):
    def test_disable_removes_automated_and_remembers_flags(self):
        app = application(automated={"prune": True, "selfHeal": True})
        change = autosync.plan(app, autosync.DISABLE)
        self.assertEqual(change.patch["spec"]["syncPolicy"], {"automated": None})
        stored = json.loads(change.patch["metadata"]["annotations"][PREVIOUS_SYNC_ANNOTATION])
        self.assertEqual(stored, {"prune": True, "selfHeal": True})
        self.assertEqual(change.before, "auto (prune, selfHeal)")
        self.assertEqual(change.after, "manual")

    def test_disable_remembers_empty_automated_block(self):
        app = application(automated={})
        change = autosync.plan(app, autosync.DISABLE)
        self.assertEqual(json.loads(change.patch["metadata"]["annotations"][PREVIOUS_SYNC_ANNOTATION]), {})

    def test_disable_skips_manual_apps(self):
        self.assertEqual(autosync.plan(application(), autosync.DISABLE), "bereits manual")


class EnableTest(unittest.TestCase):
    def test_enable_restores_remembered_flags_and_clears_annotation(self):
        remembered = json.dumps({"prune": True, "selfHeal": False, "allowEmpty": True})
        app = application(annotations={PREVIOUS_SYNC_ANNOTATION: remembered})
        change = autosync.plan(app, autosync.ENABLE)
        self.assertEqual(change.patch["spec"]["syncPolicy"]["automated"], {"prune": True, "selfHeal": False, "allowEmpty": True})
        self.assertIsNone(change.patch["metadata"]["annotations"][PREVIOUS_SYNC_ANNOTATION])
        self.assertEqual(change.after, "auto (prune, allowEmpty)")

    def test_enable_without_memory_uses_plain_automated(self):
        change = autosync.plan(application(), autosync.ENABLE)
        self.assertEqual(change.patch["spec"]["syncPolicy"]["automated"], {})
        self.assertTrue(any("keine gemerkten" in note for note in change.notes))

    def test_enable_ignores_corrupt_annotation(self):
        app = application(annotations={PREVIOUS_SYNC_ANNOTATION: "{kaputt"})
        change = autosync.plan(app, autosync.ENABLE)
        self.assertEqual(change.patch["spec"]["syncPolicy"]["automated"], {})

    def test_enable_skips_auto_apps(self):
        self.assertEqual(autosync.plan(application(automated={}), autosync.ENABLE), "bereits auto")

    def test_roundtrip_restores_original_settings(self):
        original = {"prune": True, "selfHeal": True}
        disabled = autosync.plan(application(automated=original), autosync.DISABLE)
        annotations = {PREVIOUS_SYNC_ANNOTATION: disabled.patch["metadata"]["annotations"][PREVIOUS_SYNC_ANNOTATION]}
        enabled = autosync.plan(application(annotations=annotations), autosync.ENABLE)
        self.assertEqual(enabled.patch["spec"]["syncPolicy"]["automated"], original)


class WarningTest(unittest.TestCase):
    def test_applicationset_owner_is_flagged(self):
        app = application(automated={}, owners=[{"kind": "ApplicationSet", "name": "apps"}])
        change = autosync.plan(app, autosync.DISABLE)
        self.assertTrue(any("ApplicationSet apps" in note for note in change.notes))

    def test_parent_app_tracking_is_flagged(self):
        app = application(automated={}, annotations={"argocd.argoproj.io/tracking-id": "root:argoproj.io/Application:argocd/demo"})
        self.assertTrue(any("übergeordneten App" in note for note in autosync.plan(app, autosync.DISABLE).notes))


class ApplyTest(unittest.TestCase):
    def test_apply_sends_merge_patch_via_kubectl(self):
        change = autosync.plan(application(automated={"prune": True}), autosync.DISABLE)
        done = subprocess.CompletedProcess([], 0, "", "")
        with mock.patch("cubi.argocd_apps.shell.capture", return_value=done) as capture, mock.patch.object(autosync.ui, "console"):
            autosync.apply([change])
        command = capture.call_args.args[0]
        self.assertEqual(command[:4], ["kubectl", "patch", "applications.argoproj.io", "demo"])
        self.assertIn("--type=merge", command)
        self.assertEqual(json.loads(command[-1]), change.patch)

    def test_failed_patch_is_reported_and_does_not_stop_others(self):
        first = autosync.plan(application("one", automated={}), autosync.DISABLE)
        second = autosync.plan(application("two", automated={}), autosync.DISABLE)
        failed = subprocess.CompletedProcess([], 1, "", "forbidden")
        fine = subprocess.CompletedProcess([], 0, "", "")
        with mock.patch("cubi.argocd_apps.shell.capture", side_effect=[failed, fine]) as capture, mock.patch.object(autosync.ui, "console") as console:
            autosync.apply([first, second])
        self.assertEqual(capture.call_count, 2)
        printed = " ".join(call.args[0].plain for call in console.print.call_args_list)
        self.assertIn("✖ one", printed)
        self.assertIn("✔ two", printed)


if __name__ == "__main__":
    unittest.main()
