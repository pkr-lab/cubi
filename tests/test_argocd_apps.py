import json
import subprocess
import unittest
from unittest import mock

import helpers

from cubi import argocd_apps
from cubi.errors import CubiError


def done(code=0, stdout="", stderr=""):
    return subprocess.CompletedProcess([], code, stdout, stderr)


class FakeCluster:
    def __init__(self, appset=None, configmap=None):
        self.appset = appset or {"metadata": {"resourceVersion": "7"}, "spec": {}}
        self.configmap = configmap
        self.calls = []

    def capture(self, cmd, timeout=30, cwd=None, env=None):
        self.calls.append(cmd)
        verb, resource = cmd[1], cmd[2]
        if resource == "configmap":
            return self.configmap_call(verb, cmd)
        if resource == argocd_apps.APPSET_RESOURCE or (verb == "get" and cmd[2] == argocd_apps.APPSET_RESOURCE):
            return self.appset_call(verb, cmd)
        return done()

    def configmap_call(self, verb, cmd):
        if verb == "get":
            if self.configmap is None:
                return done(1, stderr='Error from server (NotFound): configmaps "cubi-state" not found')
            return done(stdout=json.dumps({"data": self.configmap}))
        if verb == "create":
            if self.configmap is not None:
                return done(1, stderr="Error from server (AlreadyExists)")
            self.configmap = {}
            return done()
        if self.configmap is None:
            return done(1, stderr="Error from server (NotFound)")
        for key, value in json.loads(cmd[-1])["data"].items():
            if value is None:
                self.configmap.pop(key, None)
            else:
                self.configmap[key] = value
        return done()

    def appset_call(self, verb, cmd):
        if verb == "get":
            return done(stdout=json.dumps(self.appset))
        operations = json.loads(cmd[-1])
        assert operations[0] == {"op": "test", "path": "/metadata/resourceVersion", "value": self.appset["metadata"]["resourceVersion"]}
        rules = self.appset["spec"].get("ignoreApplicationDifferences")
        for operation in operations[1:]:
            if operation["op"] == "add":
                self.appset["spec"]["ignoreApplicationDifferences"] = operation["value"]
            else:
                self.appset["spec"].pop("ignoreApplicationDifferences", None)
        return done()


def patched(cluster):
    return mock.patch.object(argocd_apps.shell, "capture", cluster.capture)


class StateTest(unittest.TestCase):
    def test_missing_configmap_is_empty_state(self):
        with patched(FakeCluster()):
            self.assertEqual(argocd_apps.load_state(), {})

    def test_save_creates_configmap_and_stores_value(self):
        cluster = FakeCluster()
        with patched(cluster):
            argocd_apps.save_state("demo.sync", "{}")
            self.assertEqual(argocd_apps.load_state(), {"demo.sync": "{}"})

    def test_save_keeps_other_keys(self):
        cluster = FakeCluster(configmap={"other.sync": "{}"})
        with patched(cluster):
            argocd_apps.save_state("demo.sync", "{\"prune\": true}")
        self.assertEqual(set(cluster.configmap), {"other.sync", "demo.sync"})

    def test_delete_removes_only_that_key(self):
        cluster = FakeCluster(configmap={"a.sync": "{}", "b.sync": "{}"})
        with patched(cluster):
            argocd_apps.save_state("a.sync", None)
        self.assertEqual(cluster.configmap, {"b.sync": "{}"})

    def test_delete_without_configmap_is_fine(self):
        with patched(FakeCluster()):
            argocd_apps.save_state("a.sync", None)

    def test_unreadable_state_raises(self):
        with mock.patch.object(argocd_apps.shell, "capture", return_value=done(1, stderr="forbidden")):
            with self.assertRaises(CubiError):
                argocd_apps.load_state()

    def test_normalize_attaches_state(self):
        state = {"demo.sync": json.dumps({"prune": True}), "demo.scale": json.dumps({"replicas": {"Deployment/ns/web": 2}, "autoSync": None})}
        app = argocd_apps.normalize({"metadata": {"name": "demo"}, "spec": {}}, state)
        self.assertEqual(app["stored"], {"prune": True})
        self.assertEqual(app["scale"], {"replicas": {"Deployment/ns/web": 2}, "autoSync": None})

    def test_scale_state_without_replicas_is_ignored(self):
        self.assertIsNone(argocd_apps.parse_scale(json.dumps({"replicas": {}, "autoSync": None})))
        self.assertIsNone(argocd_apps.parse_scale(json.dumps({"replicas": {"k": "x"}})))


class HoldTest(unittest.TestCase):
    def test_hold_adds_rule_for_exactly_one_app(self):
        cluster = FakeCluster()
        with patched(cluster):
            self.assertTrue(argocd_apps.set_hold("apps", "demo", argocd_apps.SYNC_POINTER, True))
        self.assertEqual(cluster.appset["spec"]["ignoreApplicationDifferences"], [{"name": "demo", "jsonPointers": ["/spec/syncPolicy"]}])

    def test_hold_is_idempotent(self):
        cluster = FakeCluster()
        with patched(cluster):
            argocd_apps.set_hold("apps", "demo", argocd_apps.SYNC_POINTER, True)
            self.assertFalse(argocd_apps.set_hold("apps", "demo", argocd_apps.SYNC_POINTER, True))
        self.assertEqual(len(cluster.appset["spec"]["ignoreApplicationDifferences"]), 1)

    def test_release_removes_only_own_rule(self):
        foreign = {"jsonPointers": ["/status"], "name": "other"}
        cluster = FakeCluster({"metadata": {"resourceVersion": "7"}, "spec": {"ignoreApplicationDifferences": [foreign]}})
        with patched(cluster):
            argocd_apps.set_hold("apps", "demo", argocd_apps.SYNC_POINTER, True)
            argocd_apps.set_hold("apps", "demo", argocd_apps.SYNC_POINTER, False)
        self.assertEqual(cluster.appset["spec"]["ignoreApplicationDifferences"], [foreign])

    def test_release_of_last_rule_removes_the_field(self):
        cluster = FakeCluster()
        with patched(cluster):
            argocd_apps.set_hold("apps", "demo", argocd_apps.SYNC_POINTER, True)
            argocd_apps.set_hold("apps", "demo", argocd_apps.SYNC_POINTER, False)
        self.assertNotIn("ignoreApplicationDifferences", cluster.appset["spec"])

    def test_release_without_rule_does_nothing(self):
        cluster = FakeCluster()
        with patched(cluster):
            self.assertFalse(argocd_apps.set_hold("apps", "demo", argocd_apps.SYNC_POINTER, False))
        self.assertEqual(len(cluster.calls), 1)

    def test_different_pointers_are_different_rules(self):
        cluster = FakeCluster()
        with patched(cluster):
            argocd_apps.set_hold("apps", "demo", "/spec/syncPolicy", True)
            argocd_apps.set_hold("apps", "demo", "/spec/source/targetRevision", True)
        self.assertEqual(len(cluster.appset["spec"]["ignoreApplicationDifferences"]), 2)

    def test_concurrent_change_fails_loudly(self):
        calls = iter([done(stdout=json.dumps({"metadata": {"resourceVersion": "1"}, "spec": {}})), done(1, stderr="test operation failed")])
        with mock.patch.object(argocd_apps.shell, "capture", side_effect=lambda *a, **k: next(calls)):
            with self.assertRaises(CubiError):
                argocd_apps.set_hold("apps", "demo", argocd_apps.SYNC_POINTER, True)


class PauseResumeTest(unittest.TestCase):
    def app(self, appset="apps"):
        return {"name": "demo", "appset": appset}

    def test_pause_sets_rule_before_patching_application(self):
        cluster = FakeCluster()
        with patched(cluster):
            argocd_apps.pause_auto_sync(self.app())
        kinds = [call[2] for call in cluster.calls if call[1] in ("patch",)]
        self.assertEqual(kinds, [argocd_apps.APPSET_RESOURCE, argocd_apps.APP_RESOURCE])
        self.assertIn('"automated": null', cluster.calls[-1][-1])

    def test_pause_without_appset_only_patches_application(self):
        cluster = FakeCluster()
        with patched(cluster):
            argocd_apps.pause_auto_sync(self.app(None))
        self.assertEqual([call[2] for call in cluster.calls], [argocd_apps.APP_RESOURCE])

    def test_resume_patches_application_then_drops_rule(self):
        cluster = FakeCluster()
        with patched(cluster):
            argocd_apps.set_hold("apps", "demo", argocd_apps.SYNC_POINTER, True)
            cluster.calls.clear()
            argocd_apps.resume_auto_sync(self.app(), {"prune": True})
        patches = [call[2] for call in cluster.calls if call[1] == "patch"]
        self.assertEqual(patches, [argocd_apps.APP_RESOURCE, argocd_apps.APPSET_RESOURCE])
        self.assertNotIn("ignoreApplicationDifferences", cluster.appset["spec"])


class TemplateTest(unittest.TestCase):
    def appset(self, **template):
        return {"metadata": {"resourceVersion": "1"}, "spec": {"template": {"spec": template}}}

    def test_literal_revision_of_single_source(self):
        with patched(FakeCluster(self.appset(source={"targetRevision": "main"}))):
            self.assertEqual(argocd_apps.template_revision("apps", False, 0), "main")

    def test_literal_revision_of_chosen_source(self):
        with patched(FakeCluster(self.appset(sources=[{"targetRevision": "v1"}, {"targetRevision": "dev"}]))):
            self.assertEqual(argocd_apps.template_revision("apps", True, 1), "dev")

    def test_templated_revision_is_unknown(self):
        with patched(FakeCluster(self.appset(source={"targetRevision": "{{.revision}}"}))):
            self.assertIsNone(argocd_apps.template_revision("apps", False, 0))

    def test_missing_source_is_unknown(self):
        with patched(FakeCluster(self.appset())):
            self.assertIsNone(argocd_apps.template_revision("apps", True, 3))


class CheckTest(unittest.TestCase):
    def test_report_marks_reverted_changes(self):
        with mock.patch.object(argocd_apps.ui, "console") as console:
            argocd_apps.report_checks([("a", True), ("b", False)], "zurückgesetzt")
        printed = [call.args[0].plain for call in console.print.call_args_list]
        self.assertEqual(printed, ["✔ a: Änderung hält", "⚠ b: zurückgesetzt"])

    def test_recheck_waits_then_reads_requested_apps_with_state(self):
        apps = [{"name": "a"}, {"name": "b"}]
        with mock.patch.object(argocd_apps.time, "sleep") as sleep, mock.patch.object(argocd_apps, "fetch_apps", return_value=apps) as fetch, mock.patch.object(argocd_apps.ui, "console"):
            result = argocd_apps.recheck(["a"])
        sleep.assert_called_once_with(argocd_apps.SETTLE_SECONDS)
        fetch.assert_called_once_with(with_state=True)
        self.assertEqual(list(result), ["a"])


if __name__ == "__main__":
    unittest.main()
