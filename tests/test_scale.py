import json
import subprocess
import unittest
from unittest import mock

import helpers

from cubi.argocd_apps import SCALE_ANNOTATION, normalize
from cubi.modules import scale


def application(name="demo", automated=None, annotations=None, resources=None, owners=None):
    metadata = {"name": name, "annotations": annotations or {}}
    if owners:
        metadata["ownerReferences"] = owners
    spec = {
        "project": "default",
        "destination": {"namespace": "web"},
        "source": {"repoURL": "https://github.com/pkr-lab/capulus-core.git", "path": "apps/demo", "targetRevision": "main"},
        "syncPolicy": {} if automated is None else {"automated": automated},
    }
    if resources is None:
        resources = [
            {"group": "apps", "kind": "Deployment", "name": "web", "namespace": "web"},
            {"group": "apps", "kind": "StatefulSet", "name": "db"},
            {"group": "", "kind": "Service", "name": "web", "namespace": "web"},
            {"group": "batch", "kind": "CronJob", "name": "job", "namespace": "web"},
        ]
    return normalize({"metadata": metadata, "spec": spec, "status": {"resources": resources}})


LIVE = {"Deployment/web/web": 3, "StatefulSet/web/db": 2}


def memory(replicas=None, auto=None):
    return {SCALE_ANNOTATION: json.dumps({"replicas": replicas or {"Deployment/web/web": 3}, "autoSync": auto})}


class WorkloadTest(unittest.TestCase):
    def test_only_deployments_and_statefulsets_are_found(self):
        keys = [item.key for item in scale.workloads(application())]
        self.assertEqual(keys, ["Deployment/web/web", "StatefulSet/web/db"])

    def test_namespace_falls_back_to_destination(self):
        self.assertEqual(scale.workloads(application())[1].namespace, "web")

    def test_duplicates_are_ignored(self):
        resource = {"group": "apps", "kind": "Deployment", "name": "web", "namespace": "web"}
        self.assertEqual(len(scale.workloads(application(resources=[resource, resource]))), 1)

    def test_target_uses_fully_qualified_resource(self):
        self.assertEqual(scale.from_key("StatefulSet/web/db").target, "statefulsets.apps/db")


class ScaleDownTest(unittest.TestCase):
    def test_remembers_replicas_and_scales_to_zero(self):
        change = scale.plan_down(application(), LIVE)
        stored = json.loads(change.patch["metadata"]["annotations"][SCALE_ANNOTATION])
        self.assertEqual(stored["replicas"], LIVE)
        self.assertIsNone(stored["autoSync"])
        self.assertEqual([count for _, count in change.ops], [0, 0])
        self.assertEqual((change.before, change.after), ("5 Pods", "0 Pods"))

    def test_manual_app_keeps_sync_policy(self):
        self.assertNotIn("spec", scale.plan_down(application(), LIVE).patch)

    def test_auto_sync_is_disabled_and_remembered(self):
        change = scale.plan_down(application(automated={"prune": True, "selfHeal": True}), LIVE)
        self.assertEqual(change.patch["spec"], {"syncPolicy": {"automated": None}})
        stored = json.loads(change.patch["metadata"]["annotations"][SCALE_ANNOTATION])
        self.assertEqual(stored["autoSync"], {"prune": True, "selfHeal": True})
        self.assertTrue(any("Auto-Sync wird deaktiviert" in note for note in change.notes))

    def test_already_zero_is_skipped(self):
        self.assertEqual(scale.plan_down(application(), {"Deployment/web/web": 0, "StatefulSet/web/db": 0}), "bereits 0 Pods")

    def test_app_without_workloads_is_skipped(self):
        self.assertIn("keine", scale.plan_down(application(resources=[]), {}))

    def test_partially_scaled_app_keeps_earlier_memory(self):
        app = application(annotations=memory({"Deployment/web/web": 3}, {"prune": True}))
        change = scale.plan_down(app, {"Deployment/web/web": 0, "StatefulSet/web/db": 2})
        stored = json.loads(change.patch["metadata"]["annotations"][SCALE_ANNOTATION])
        self.assertEqual(stored["replicas"], {"Deployment/web/web": 3, "StatefulSet/web/db": 2})
        self.assertEqual(stored["autoSync"], {"prune": True})
        self.assertEqual([workload.key for workload, _ in change.ops], ["StatefulSet/web/db"])

    def test_applicationset_owner_is_flagged(self):
        app = application(owners=[{"kind": "ApplicationSet", "name": "apps"}])
        self.assertTrue(any("ApplicationSet apps" in note for note in scale.plan_down(app, LIVE).notes))


class ScaleUpTest(unittest.TestCase):
    def test_restores_replicas_and_clears_annotation(self):
        app = application(annotations=memory({"Deployment/web/web": 3, "StatefulSet/web/db": 2}))
        change = scale.plan_up(app)
        self.assertEqual([(workload.key, count) for workload, count in change.ops], [("Deployment/web/web", 3), ("StatefulSet/web/db", 2)])
        self.assertIsNone(change.patch["metadata"]["annotations"][SCALE_ANNOTATION])
        self.assertEqual(change.after, "5 Pods")

    def test_reenables_remembered_auto_sync(self):
        app = application(annotations=memory(auto={"prune": True, "selfHeal": True}))
        change = scale.plan_up(app)
        self.assertEqual(change.patch["spec"]["syncPolicy"]["automated"], {"prune": True, "selfHeal": True})

    def test_does_not_touch_auto_sync_that_is_already_on(self):
        app = application(automated={}, annotations=memory(auto={"prune": True}))
        self.assertNotIn("spec", scale.plan_up(app).patch)

    def test_without_memory_is_skipped(self):
        self.assertEqual(scale.plan_up(application()), "keine gemerkten Replikas")

    def test_corrupt_memory_is_ignored(self):
        self.assertEqual(scale.plan_up(application(annotations={SCALE_ANNOTATION: "{kaputt"})), "keine gemerkten Replikas")

    def test_roundtrip(self):
        auto = {"prune": True, "selfHeal": True}
        down = scale.plan_down(application(automated=auto), LIVE)
        annotations = {SCALE_ANNOTATION: down.patch["metadata"]["annotations"][SCALE_ANNOTATION]}
        up = scale.plan_up(application(annotations=annotations))
        self.assertEqual({workload.key: count for workload, count in up.ops}, LIVE)
        self.assertEqual(up.patch["spec"]["syncPolicy"]["automated"], auto)


def completed(code=0, stderr=""):
    return subprocess.CompletedProcess([], code, "", stderr)


class ApplyTest(unittest.TestCase):
    def run_apply(self, change, results):
        with mock.patch("cubi.argocd_apps.shell.capture", side_effect=results) as capture, mock.patch("cubi.modules.scale.shell.capture", capture), mock.patch.object(scale.ui, "console") as console:
            scale.apply([change])
        return [call.args[0] for call in capture.call_args_list], " ".join(call.args[0].plain for call in console.print.call_args_list)

    def test_down_patches_application_before_scaling(self):
        change = scale.plan_down(application(automated={}), LIVE)
        commands, printed = self.run_apply(change, [completed()] * 3)
        self.assertEqual(commands[0][:2], ["kubectl", "patch"])
        self.assertEqual(commands[1], ["kubectl", "scale", "deployments.apps/web", "-n", "web", "--replicas=0"])
        self.assertEqual(commands[2][2], "statefulsets.apps/db")
        self.assertIn("✔ demo", printed)

    def test_down_does_not_scale_when_patch_fails(self):
        change = scale.plan_down(application(), LIVE)
        commands, printed = self.run_apply(change, [completed(1, "forbidden")])
        self.assertEqual(len(commands), 1)
        self.assertIn("✖ demo", printed)

    def test_up_scales_before_patching_application(self):
        change = scale.plan_up(application(annotations=memory()))
        commands, _ = self.run_apply(change, [completed()] * 2)
        self.assertEqual(commands[0][:2], ["kubectl", "scale"])
        self.assertIn("--replicas=3", commands[0])
        self.assertEqual(commands[1][:2], ["kubectl", "patch"])

    def test_up_keeps_memory_when_scaling_fails(self):
        change = scale.plan_up(application(annotations=memory()))
        commands, printed = self.run_apply(change, [completed(1, "nope")])
        self.assertEqual(len(commands), 1)
        self.assertIn("bleiben erhalten", printed)


class CellsTest(unittest.TestCase):
    def test_scaled_app_shows_previous_count(self):
        cells = scale.cells_for(scale.UP)(application(annotations=memory({"Deployment/web/web": 3, "StatefulSet/web/db": 2})))
        self.assertEqual([cell.plain for cell in cells], ["demo", "2", "auf 0 (vorher 5)", "manual"])

    def test_normal_app_status(self):
        self.assertEqual(scale.cells_for(scale.DOWN)(application())[2].plain, "normal")


if __name__ == "__main__":
    unittest.main()
