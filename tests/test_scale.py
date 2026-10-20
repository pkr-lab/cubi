import json
import subprocess
import unittest
from unittest import mock

import helpers

from cubi.argocd_apps import normalize
from cubi.modules import scale


def application(name="demo", automated=None, state=None, resources=None, owners=None):
    metadata = {"name": name, "annotations": {}}
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
    return normalize({"metadata": metadata, "spec": spec, "status": {"resources": resources}}, state)


LIVE = {"Deployment/web/web": 3, "StatefulSet/web/db": 2}


def memory(replicas=None, auto=None, name="demo"):
    return {f"{name}.scale": json.dumps({"replicas": replicas or {"Deployment/web/web": 3}, "autoSync": auto})}


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
        stored = json.loads(change.state_value)
        self.assertEqual(change.state_key, "demo.scale")
        self.assertEqual(stored["replicas"], LIVE)
        self.assertIsNone(stored["autoSync"])
        self.assertEqual([count for _, count in change.ops], [0, 0])
        self.assertEqual((change.before, change.after), ("5 Pods", "0 Pods"))

    def test_manual_app_keeps_sync_policy(self):
        self.assertFalse(scale.plan_down(application(), LIVE).pause)

    def test_auto_sync_is_disabled_and_remembered(self):
        change = scale.plan_down(application(automated={"prune": True, "selfHeal": True}), LIVE)
        self.assertTrue(change.pause)
        self.assertEqual(json.loads(change.state_value)["autoSync"], {"prune": True, "selfHeal": True})
        self.assertTrue(any("Auto-Sync wird deaktiviert" in note for note in change.notes))

    def test_already_zero_is_skipped(self):
        self.assertEqual(scale.plan_down(application(), {"Deployment/web/web": 0, "StatefulSet/web/db": 0}), "bereits 0 Pods")

    def test_app_without_workloads_is_skipped(self):
        self.assertIn("keine", scale.plan_down(application(resources=[]), {}))

    def test_partially_scaled_app_keeps_earlier_memory(self):
        app = application(state=memory({"Deployment/web/web": 3}, {"prune": True}))
        change = scale.plan_down(app, {"Deployment/web/web": 0, "StatefulSet/web/db": 2})
        stored = json.loads(change.state_value)
        self.assertEqual(stored["replicas"], {"Deployment/web/web": 3, "StatefulSet/web/db": 2})
        self.assertEqual(stored["autoSync"], {"prune": True})
        self.assertEqual([workload.key for workload, _ in change.ops], ["StatefulSet/web/db"])

    def test_applicationset_app_is_held_by_rule(self):
        app = application(automated={"selfHeal": True}, owners=[{"kind": "ApplicationSet", "name": "apps"}])
        self.assertTrue(any("ApplicationSet apps" in note and "ignoreApplicationDifferences" in note for note in scale.plan_down(app, LIVE).notes))


class SetCountTest(unittest.TestCase):
    def test_scaling_up_to_custom_count(self):
        change = scale.plan_set(application(), {"Deployment/web/web": 1, "StatefulSet/web/db": 1}, 4)
        self.assertEqual(change.action, scale.SET)
        self.assertEqual([(workload.key, count) for workload, count in change.ops], [("Deployment/web/web", 4), ("StatefulSet/web/db", 4)])
        self.assertEqual((change.before, change.after), ("2 Pods", "8 Pods"))

    def test_zero_is_the_down_action(self):
        self.assertEqual(scale.plan_set(application(), LIVE, 0).action, scale.DOWN)

    def test_previous_values_are_remembered_including_zero(self):
        change = scale.plan_set(application(), {"Deployment/web/web": 0, "StatefulSet/web/db": 2}, 3)
        self.assertEqual(json.loads(change.state_value)["replicas"], {"Deployment/web/web": 0, "StatefulSet/web/db": 2})

    def test_workloads_already_at_target_are_left_alone(self):
        change = scale.plan_set(application(), {"Deployment/web/web": 2, "StatefulSet/web/db": 5}, 2)
        self.assertEqual([workload.key for workload, _ in change.ops], ["StatefulSet/web/db"])

    def test_all_at_target_is_skipped(self):
        self.assertEqual(scale.plan_set(application(), {"Deployment/web/web": 2, "StatefulSet/web/db": 2}, 2), "bereits 2 Pods")

    def test_only_selected_workloads_are_changed(self):
        change = scale.plan_set(application(), LIVE, 1, {"StatefulSet/web/db"})
        self.assertEqual([workload.key for workload, _ in change.ops], ["StatefulSet/web/db"])
        self.assertEqual(list(json.loads(change.state_value)["replicas"]), ["StatefulSet/web/db"])

    def test_no_selected_workload_is_skipped(self):
        self.assertEqual(scale.plan_set(application(), LIVE, 1, set()), "kein Workload gewählt")

    def test_earlier_memory_wins_over_current_live_value(self):
        app = application(state=memory({"Deployment/web/web": 3}, {"prune": True}))
        change = scale.plan_set(app, {"Deployment/web/web": 5, "StatefulSet/web/db": 2}, 1)
        stored = json.loads(change.state_value)
        self.assertEqual(stored["replicas"]["Deployment/web/web"], 3)
        self.assertEqual(stored["autoSync"], {"prune": True})

    def test_auto_sync_is_paused_for_any_custom_count(self):
        change = scale.plan_set(application(automated={"selfHeal": True}), LIVE, 7)
        self.assertTrue(change.pause)

    def test_custom_count_roundtrip_restores_original(self):
        change = scale.plan_set(application(), LIVE, 9)
        up = scale.plan_up(application(state={change.state_key: change.state_value}))
        self.assertEqual({workload.key: count for workload, count in up.ops}, LIVE)


class ScaleUpTest(unittest.TestCase):
    def test_restores_replicas_and_clears_annotation(self):
        app = application(state=memory({"Deployment/web/web": 3, "StatefulSet/web/db": 2}))
        change = scale.plan_up(app)
        self.assertEqual([(workload.key, count) for workload, count in change.ops], [("Deployment/web/web", 3), ("StatefulSet/web/db", 2)])
        self.assertIsNone(change.state_value)
        self.assertEqual(change.after, "5 Pods")

    def test_reenables_remembered_auto_sync(self):
        app = application(state=memory(auto={"prune": True, "selfHeal": True}))
        change = scale.plan_up(app)
        self.assertEqual(change.resume, {"prune": True, "selfHeal": True})

    def test_does_not_touch_auto_sync_that_is_already_on(self):
        app = application(automated={}, state=memory(auto={"prune": True}))
        self.assertIsNone(scale.plan_up(app).resume)

    def test_without_memory_is_skipped(self):
        self.assertEqual(scale.plan_up(application()), "keine gemerkten Replikas")

    def test_corrupt_memory_is_ignored(self):
        self.assertEqual(scale.plan_up(application(state={"demo.scale": "{kaputt"})), "keine gemerkten Replikas")

    def test_roundtrip(self):
        auto = {"prune": True, "selfHeal": True}
        down = scale.plan_down(application(automated=auto), LIVE)
        up = scale.plan_up(application(state={down.state_key: down.state_value}))
        self.assertEqual({workload.key: count for workload, count in up.ops}, LIVE)
        self.assertEqual(up.resume, auto)


def completed(code=0, stderr=""):
    return subprocess.CompletedProcess([], code, "", stderr)


class ApplyTest(unittest.TestCase):
    def run_apply(self, change, scale_results):
        manager = mock.Mock()
        patches = [
            mock.patch.object(scale.argocd_apps, "save_state", manager.save_state),
            mock.patch.object(scale.argocd_apps, "pause_auto_sync", manager.pause),
            mock.patch.object(scale.argocd_apps, "resume_auto_sync", manager.resume),
            mock.patch.object(scale.shell, "capture", side_effect=scale_results),
            mock.patch.object(scale.ui, "console"),
        ]
        started = [patch.start() for patch in patches]
        self.addCleanup(lambda: [patch.stop() for patch in patches])
        capture = started[3]
        manager.attach_mock(capture, "scale")
        result = scale.apply([change])
        printed = " ".join(call.args[0].plain for call in scale.ui.console.print.call_args_list)
        return manager, capture, result, printed

    def test_down_saves_state_and_pauses_before_scaling(self):
        change = scale.plan_down(application(automated={"selfHeal": True}), LIVE)
        manager, capture, result, printed = self.run_apply(change, [completed()] * 2)
        self.assertEqual([call[0] for call in manager.mock_calls], ["save_state", "pause", "scale", "scale"])
        self.assertEqual(capture.call_args_list[0].args[0], ["kubectl", "scale", "deployments.apps/web", "-n", "web", "--replicas=0"])
        self.assertEqual(result, [change])
        self.assertIn("✔ demo", printed)

    def test_down_of_manual_app_does_not_pause(self):
        change = scale.plan_down(application(), LIVE)
        manager, _, _, _ = self.run_apply(change, [completed()] * 2)
        self.assertEqual([call[0] for call in manager.mock_calls], ["save_state", "scale", "scale"])

    def test_down_does_not_scale_when_pausing_fails(self):
        change = scale.plan_down(application(automated={}), LIVE)
        manager = mock.Mock()
        manager.pause.side_effect = scale.CubiError("forbidden")
        with mock.patch.object(scale.argocd_apps, "save_state"), mock.patch.object(scale.argocd_apps, "pause_auto_sync", manager.pause), mock.patch.object(scale.shell, "capture") as capture, mock.patch.object(scale.ui, "console") as console:
            self.assertEqual(scale.apply([change]), [])
        capture.assert_not_called()
        self.assertIn("✖ demo", console.print.call_args.args[0].plain)

    def test_up_scales_then_resumes_then_drops_state(self):
        change = scale.plan_up(application(state=memory(auto={"prune": True})))
        manager, capture, result, _ = self.run_apply(change, [completed()])
        self.assertEqual([call[0] for call in manager.mock_calls], ["scale", "resume", "save_state"])
        self.assertIn("--replicas=3", capture.call_args.args[0])
        self.assertEqual(manager.resume.call_args.args[1], {"prune": True})
        self.assertEqual(manager.save_state.call_args.args, ("demo.scale", None))
        self.assertEqual(result, [change])

    def test_up_keeps_memory_when_scaling_fails(self):
        change = scale.plan_up(application(state=memory()))
        manager, _, result, printed = self.run_apply(change, [completed(1, "nope")])
        self.assertEqual([call[0] for call in manager.mock_calls], ["scale"])
        self.assertEqual(result, [])
        self.assertIn("bleiben erhalten", printed)


class VerifyTest(unittest.TestCase):
    def run_verify(self, change, app, live):
        with mock.patch.object(scale.argocd_apps, "recheck", return_value={"demo": app}), mock.patch.object(scale, "fetch_live", return_value=live), mock.patch.object(scale.argocd_apps, "report_checks") as report:
            scale.verify([change])
        return report.call_args.args[0]

    def test_down_holds_when_replicas_stay_zero(self):
        change = scale.plan_down(application(), LIVE)
        self.assertEqual(self.run_verify(change, application(), {"Deployment/web/web": 0, "StatefulSet/web/db": 0}), [("demo", True)])

    def test_down_reverted_by_argocd_is_reported(self):
        change = scale.plan_down(application(), LIVE)
        self.assertEqual(self.run_verify(change, application(), {"Deployment/web/web": 3, "StatefulSet/web/db": 0}), [("demo", False)])

    def test_up_checks_restored_counts(self):
        change = scale.plan_up(application(state=memory()))
        self.assertEqual(self.run_verify(change, application(), {"Deployment/web/web": 3}), [("demo", True)])
        self.assertEqual(self.run_verify(change, application(), {"Deployment/web/web": 0}), [("demo", False)])


class AskCountTest(unittest.TestCase):
    def ask(self, value):
        with mock.patch.object(scale.ui, "prompt", return_value=value):
            return scale.ask_count()

    def test_number_is_parsed(self):
        self.assertEqual(self.ask(" 12 "), 12)
        self.assertEqual(self.ask("0"), 0)

    def test_cancel_and_empty_return_none(self):
        self.assertIsNone(self.ask(None))
        self.assertIsNone(self.ask("  "))

    def test_invalid_input_raises(self):
        for value in ("abc", "1.5", "-1"):
            with self.assertRaises(scale.CubiError):
                self.ask(value)


class RunTest(unittest.TestCase):
    def setUp(self):
        self.app = application()
        self.patches = {
            "select": mock.patch.object(scale.ui, "select"),
            "prompt": mock.patch.object(scale.ui, "prompt", return_value="4"),
            "multiselect": mock.patch.object(scale.ui, "multiselect"),
            "confirm": mock.patch.object(scale.ui, "confirm", return_value=True),
            "choose": mock.patch.object(scale.argocd_apps, "choose_apps", return_value=[self.app]),
            "live": mock.patch.object(scale, "fetch_live", return_value={"Deployment/web/web": 1, "StatefulSet/web/db": 1}),
            "apply": mock.patch.object(scale, "apply"),
            "verify": mock.patch.object(scale, "verify"),
            "begin": mock.patch.object(scale.ui, "begin"),
            "console": mock.patch.object(scale.ui, "console"),
            "info": mock.patch.object(scale.ui, "info"),
        }
        self.mocks = {name: patch.start() for name, patch in self.patches.items()}
        self.addCleanup(lambda: [patch.stop() for patch in self.patches.values()])
        self.mocks["apply"].side_effect = lambda changes: changes

    def test_custom_count_asks_for_number_and_workloads_then_finishes(self):
        self.mocks["select"].return_value = scale.SET
        self.mocks["multiselect"].return_value = ["Deployment/web/web"]
        with self.assertRaises(scale.Finished):
            scale.run()
        changes = self.mocks["apply"].call_args.args[0]
        self.assertEqual([(workload.key, count) for workload, count in changes[0].ops], [("Deployment/web/web", 4)])
        self.assertIn("4 Pods", self.mocks["choose"].call_args.args[0])
        self.mocks["verify"].assert_called_once()

    def test_single_workload_app_skips_workload_question(self):
        self.app = application(resources=[{"group": "apps", "kind": "Deployment", "name": "web", "namespace": "web"}])
        self.mocks["choose"].return_value = [self.app]
        self.mocks["select"].return_value = scale.SET
        with self.assertRaises(scale.Finished):
            scale.run()
        self.mocks["multiselect"].assert_not_called()

    def test_zero_does_not_ask_for_number(self):
        self.mocks["select"].return_value = scale.DOWN
        with self.assertRaises(scale.Finished):
            scale.run()
        self.mocks["prompt"].assert_not_called()
        self.assertEqual(self.mocks["apply"].call_args.args[0][0].ops[0][1], 0)

    def test_declined_confirmation_stays_in_cubi(self):
        self.mocks["select"].return_value = scale.DOWN
        self.mocks["confirm"].return_value = False
        scale.run()
        self.mocks["apply"].assert_not_called()

    def test_cancel_at_any_step_stays_in_cubi(self):
        self.mocks["select"].return_value = None
        scale.run()
        self.mocks["select"].return_value = scale.SET
        self.mocks["prompt"].return_value = None
        scale.run()
        self.mocks["prompt"].return_value = "2"
        self.mocks["multiselect"].return_value = None
        scale.run()
        self.mocks["apply"].assert_not_called()

    def test_nothing_applied_does_not_finish(self):
        self.mocks["select"].return_value = scale.DOWN
        self.mocks["apply"].side_effect = lambda changes: []
        scale.run()

    def test_nothing_to_do_does_not_finish(self):
        self.mocks["select"].return_value = scale.SET
        self.mocks["live"].return_value = {"Deployment/web/web": 4, "StatefulSet/web/db": 4}
        self.mocks["multiselect"].return_value = ["Deployment/web/web", "StatefulSet/web/db"]
        scale.run()
        self.mocks["apply"].assert_not_called()


class CellsTest(unittest.TestCase):
    def test_scaled_app_shows_previous_count(self):
        cells = scale.cells_for(scale.UP)(application(state=memory({"Deployment/web/web": 3, "StatefulSet/web/db": 2})))
        self.assertEqual([cell.plain for cell in cells], ["demo", "2", "geändert (vorher 5)", "manual"])

    def test_normal_app_status(self):
        self.assertEqual(scale.cells_for(scale.DOWN)(application())[2].plain, "normal")

    def test_set_action_applies_to_every_app_with_workloads(self):
        self.assertEqual(scale.cells_for(scale.SET)(application())[0].style, "")
        self.assertEqual(scale.cells_for(scale.SET)(application(resources=[]))[0].style, "dim")


if __name__ == "__main__":
    unittest.main()
