import unittest
from unittest import mock

import helpers

from cubi import main
from cubi.errors import CubiError, Finished
from cubi.modules import autosync, feature_testing
from test_autosync import application


class LaunchTest(unittest.TestCase):
    def test_finished_module_ends_cubi(self):
        with mock.patch.dict(main.MODULES, {"scale": (mock.Mock(side_effect=Finished), "Pods", "")}):
            self.assertTrue(main.launch("scale"))

    def test_normal_return_stays_in_cubi(self):
        with mock.patch.dict(main.MODULES, {"scale": (mock.Mock(), "Pods", "")}):
            self.assertFalse(main.launch("scale"))

    def test_error_is_shown_and_stays_in_cubi(self):
        with mock.patch.dict(main.MODULES, {"scale": (mock.Mock(side_effect=CubiError("kaputt")), "Pods", "")}), mock.patch.object(main.ui, "error") as error:
            self.assertFalse(main.launch("scale"))
        error.assert_called_once_with("kaputt")

    def test_menu_loop_exits_after_finished_module(self):
        with mock.patch.dict(main.MODULES, {"scale": (mock.Mock(side_effect=Finished), "Pods", "")}, clear=True), mock.patch.object(main.ui, "select", return_value="scale") as select, mock.patch("sys.stdin.isatty", return_value=True), mock.patch("sys.stdout.isatty", return_value=True):
            self.assertEqual(main.main([]), 0)
        self.assertEqual(select.call_count, 1)

    def test_menu_loop_continues_after_normal_module(self):
        with mock.patch.dict(main.MODULES, {"scale": (mock.Mock(), "Pods", "")}, clear=True), mock.patch.object(main.ui, "select", side_effect=["scale", None]) as select, mock.patch("sys.stdin.isatty", return_value=True), mock.patch("sys.stdout.isatty", return_value=True):
            self.assertEqual(main.main([]), 0)
        self.assertEqual(select.call_count, 2)


class AutosyncRunTest(unittest.TestCase):
    def run_flow(self, confirm, applied):
        patches = [
            mock.patch.object(autosync.ui, "select", return_value=autosync.DISABLE),
            mock.patch.object(autosync.argocd_apps, "choose_apps", return_value=[application(automated={})]),
            mock.patch.object(autosync.ui, "begin"),
            mock.patch.object(autosync.ui, "console"),
            mock.patch.object(autosync.ui, "confirm", return_value=confirm),
            mock.patch.object(autosync, "apply", side_effect=lambda changes: changes if applied else []),
            mock.patch.object(autosync, "verify"),
        ]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)
        autosync.run()

    def test_finishes_after_change(self):
        with self.assertRaises(Finished):
            self.run_flow(True, True)

    def test_declined_stays(self):
        self.run_flow(False, True)

    def test_failed_apply_stays(self):
        self.run_flow(True, False)


class FeatureTestingRunTest(unittest.TestCase):
    def run_flow(self, confirm):
        patches = [
            mock.patch.object(feature_testing.ui, "select", return_value=feature_testing.SWITCH),
            mock.patch.object(feature_testing.argocd_apps, "choose_apps", return_value=[application()]),
            mock.patch.object(feature_testing.ui, "prompt", return_value="dev"),
            mock.patch.object(feature_testing, "check_remote", return_value={}),
            mock.patch.object(feature_testing.ui, "begin"),
            mock.patch.object(feature_testing.ui, "console"),
            mock.patch.object(feature_testing.ui, "confirm", return_value=confirm),
            mock.patch.object(feature_testing, "apply", side_effect=lambda changes: changes),
            mock.patch.object(feature_testing, "verify"),
        ]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)
        feature_testing.run()

    def test_finishes_after_change(self):
        with self.assertRaises(Finished):
            self.run_flow(True)

    def test_declined_stays(self):
        self.run_flow(False)


if __name__ == "__main__":
    unittest.main()
