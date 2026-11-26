"""Deterministic checks for the hc transport, without a broker or live core."""
import io
import json
import subprocess
import unittest
from unittest.mock import patch

import crabbox

LEASE = "cbx_0123456789ab"
ROUTE = crabbox.Route(LEASE, "/repo with spaces", "/bin/hc", "/bin/crabbox")


def result(code=0, stdout="", stderr=""):
    return subprocess.CompletedProcess([], code, stdout, stderr)


@patch("crabbox.sys.stderr", new_callable=io.StringIO)
class CrabboxMachineTests(unittest.TestCase):
    @patch("crabbox.os.execvp")
    @patch("crabbox.run")
    def test_remote_shell_line_is_one_argument_and_ssh_uses_no_pty(self, run, execvp, stderr):
        run.return_value = result(stdout="'ssh' '-o' 'ProxyCommand=relay --id X' 'user@host'\n")
        line = "sh -c 'exec herdr-core connect'"
        crabbox.connect(ROUTE, line)
        self.assertEqual(run.call_args.kwargs["cwd"], ROUTE.repo)
        execvp.assert_called_once_with(
            "ssh", ["ssh", "-o", "ProxyCommand=relay --id X", "-T", "user@host", line]
        )

    @patch("crabbox.run")
    def test_broker_confirmed_end_forgets_only_this_registration(self, run, stderr):
        run.side_effect = [
            result(2, stderr="no SSH target"),
            result(stdout=f'{{"id":"{LEASE}","provider":"serve","state":"released"}}'),
            result(stdout=json.dumps({"machines": [
                {"name": ROUTE.name, "target": "another-transport", "via": True},
                {"name": "renamed-lease", "target": ROUTE.via, "via": True},
            ]})),
            result(),
        ]
        self.assertEqual(crabbox.connect(ROUTE, "connect"), 2)
        self.assertEqual(run.call_args.args[0], [ROUTE.hc, "machine", "rm", "renamed-lease"])

    @patch("crabbox.run")
    def test_broker_failures_and_non_matching_lease_states_do_not_forget(self, run, stderr):
        for status in [
            result(1, stderr="coordinator unavailable"),
            result(1, stderr='coordinator GET /v1/leases/cbx_different: http 404: {"error":"not_found"}'),
            result(0, stdout=f'{{"id":"{LEASE}","provider":"serve","state":"active"}}'),
            result(0, stdout='{"id":"cbx_different","provider":"serve","state":"released"}'),
        ]:
            with self.subTest(status=status):
                run.reset_mock(side_effect=True)
                run.side_effect = [result(1, stderr="no SSH target"), status]
                self.assertEqual(crabbox.connect(ROUTE, "connect"), 1)
                self.assertEqual(run.call_count, 2)


    @patch("crabbox.run")
    def test_exact_broker_not_found_is_end_after_inventory_retention(self, run, stderr):
        run.return_value = result(1, stderr=f'coordinator GET /v1/leases/{LEASE}: http 404: {{"error":"not_found"}}')
        self.assertTrue(crabbox.ended(ROUTE))


if __name__ == "__main__":
    unittest.main()
