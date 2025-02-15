import unittest
from unittest.mock import MagicMock, patch

from dbt.clients.git import list_tags


class Git(unittest.TestCase):
    @patch("dbt_common.clients.system.run_cmd")
    @patch("subprocess.Popen")
    def test_list_tags(self, mock_popen, mock_run_cmd):
        mock_process = MagicMock()
        mock_process.returncode = 0
        mock_process.communicate.return_value = (b"v1.0.0\nv1.1.0\nv2.0.0\n", b"")
        mock_popen.return_value = mock_process

        cwd = "/path/to/repo"
        tags = list_tags(cwd)

        self.assertEqual(tags, ["v1.0.0", "v1.1.0", "v2.0.0"])
