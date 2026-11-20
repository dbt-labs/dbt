import os
import unittest
import uuid
from unittest import mock
from unittest.mock import MagicMock, call, patch

from dbt.constants import (
    MANIFEST_FILE_NAME,
    RUN_RESULTS_FILE_NAME,
    SEMANTIC_MANIFEST_FILE_NAME,
)
from dbt.exceptions import DbtProjectError
from dbt.parser.manifest import write_semantic_manifest
from dbt.utils.artifact_upload import (
    PRODUCED_ARTIFACTS_PATHS,
    ArtifactUploadConfig,
    _format_error,
    _retry_with_backoff,
    _upload_headers,
    add_artifact_produced,
    upload_artifacts,
)
from dbt_common.exceptions import DbtBaseException

# Service SAS query string; sv and sig are what mark a URL as Azure.
AZURE_SAS = "sv=2024-11-04&ss=b&srt=o&sp=cw&se=2026-08-15T00:00:00Z&sig=cGxhY2Vob2xkZXI%3D"


class TestArtifactUploadConfig(unittest.TestCase):
    def setUp(self):
        self.config = ArtifactUploadConfig(
            tenant_hostname="test-tenant.dbt.com",
            DBT_CLOUD_TOKEN="test-token",
            DBT_CLOUD_ACCOUNT_ID="1234",
            DBT_CLOUD_ENVIRONMENT_ID="5678",
        )
        self.test_invocation_id = str(uuid.uuid4())

    def test_get_ingest_url(self):
        expected_url = (
            "https://test-tenant.dbt.com/api/private/accounts/1234/environments/5678/ingests/"
        )
        self.assertEqual(self.config.get_ingest_url(), expected_url)

    def test_get_complete_url(self):
        ingest_id = "9012"
        expected_url = (
            "https://test-tenant.dbt.com/api/private/accounts/1234/environments/5678/ingests/9012/"
        )
        self.assertEqual(self.config.get_complete_url(ingest_id), expected_url)

    def test_get_headers_with_invocation_id(self):
        expected_headers = {
            "Accept": "application/json",
            "X-Invocation-Id": self.test_invocation_id,
            "Authorization": "Token test-token",
        }
        self.assertEqual(
            self.config.get_headers(invocation_id=self.test_invocation_id),
            expected_headers,
        )

    def test_get_headers_without_invocation_id(self):
        with mock.patch("uuid.uuid4") as mock_uuid:
            mock_uuid.return_value = uuid.UUID("12345678-1234-1234-1234-123456789012")
            expected_headers = {
                "Accept": "application/json",
                "X-Invocation-Id": "12345678-1234-1234-1234-123456789012",
                "Authorization": "Token test-token",
            }
            self.assertEqual(self.config.get_headers(), expected_headers)


class TestRetryWithBackoff(unittest.TestCase):
    def setUp(self):
        self.time_sleep_patcher = patch("dbt.utils.artifact_upload.time.sleep")
        self.mock_sleep = self.time_sleep_patcher.start()

    def tearDown(self):
        self.time_sleep_patcher.stop()

    def test_successful_first_try(self):
        """Test that function returns immediately on success."""
        func = MagicMock(return_value=(True, "success"))
        result = _retry_with_backoff("operation", func)
        self.assertEqual(result, "success")
        func.assert_called_once()
        self.mock_sleep.assert_not_called()

    def test_successful_after_retry(self):
        """Test that function retries and succeeds."""
        mock_response = MagicMock()
        mock_response.status_code = 503
        func = MagicMock(side_effect=[(False, mock_response), (True, "success")])
        result = _retry_with_backoff("operation", func)
        self.assertEqual(result, "success")
        self.assertEqual(func.call_count, 2)
        self.mock_sleep.assert_called_once_with(1)  # First retry delay

    def test_failure_after_max_retries(self):
        """Test that function raises exception after max retries."""
        mock_response = MagicMock()
        mock_response.status_code = 503
        func = MagicMock(return_value=(False, mock_response))
        with self.assertRaises(DbtBaseException) as context:
            _retry_with_backoff("operation", func, max_retries=3)
        self.assertIn("Error operation", str(context.exception))
        self.assertEqual(func.call_count, 4)
        # Sleep should be called twice (after first and second attempts)
        self.assertEqual(self.mock_sleep.call_count, 3)
        self.assertEqual(
            self.mock_sleep.call_args_list, [call(1), call(2), call(4)]
        )  # Exponential backoff

    def test_non_retryable_status_code(self):
        """Test that non-retryable status codes raise immediately."""
        mock_response = MagicMock()
        mock_response.status_code = 400  # Not in retry_codes
        func = MagicMock(return_value=(False, mock_response))
        with self.assertRaises(DbtBaseException) as context:
            _retry_with_backoff("operation", func)
        self.assertIn("Error operation", str(context.exception))
        self.assertEqual(func.call_count, 1)
        self.mock_sleep.assert_not_called()

    def test_request_exception_handling(self):
        """Test that RequestException is caught and retried."""
        import requests

        func = MagicMock(
            side_effect=[requests.RequestException("Network error"), (True, "success")]
        )
        result = _retry_with_backoff("operation", func)
        self.assertEqual(result, "success")
        self.assertEqual(func.call_count, 2)
        self.mock_sleep.assert_called_once_with(1)

    def test_request_exception_max_retries(self):
        """Test that RequestException raises after max retries."""
        import requests

        func = MagicMock(side_effect=requests.RequestException("Network error"))
        with self.assertRaises(DbtBaseException) as context:
            _retry_with_backoff("operation", func, max_retries=3)
        self.assertIn("Error operation: Network error", str(context.exception))
        self.assertEqual(func.call_count, 4)
        self.assertEqual(self.mock_sleep.call_count, 3)

    def test_custom_retry_codes(self):
        """Test that custom retry codes are respected."""
        mock_response = MagicMock()
        mock_response.status_code = 429  # Too Many Requests
        func = MagicMock(side_effect=[(False, mock_response), (True, "success")])
        result = _retry_with_backoff("operation", func, retry_codes=[429, 503])
        self.assertEqual(result, "success")
        self.assertEqual(func.call_count, 2)
        self.mock_sleep.assert_called_once_with(1)

    def test_error_message_includes_provider_error_code(self):
        """Test that the provider's error code is surfaced, not just the status code."""
        mock_response = MagicMock()
        mock_response.status_code = 400
        mock_response.headers = {}
        mock_response.text = (
            "<Error><Code>MissingRequiredHeader</Code>"
            "<HeaderName>x-ms-blob-type</HeaderName></Error>"
        )
        func = MagicMock(return_value=(False, mock_response))
        with self.assertRaises(DbtBaseException) as context:
            _retry_with_backoff("uploading artifacts", func)
        self.assertIn("MissingRequiredHeader", str(context.exception))


class TestUploadHeaders(unittest.TestCase):
    def test_azure_blob_storage_requires_blob_type(self):
        """Azure Blob Storage rejects Put Blob without x-ms-blob-type."""
        headers = _upload_headers(f"https://prodeu2.blob.core.windows.net/c/a.zip?{AZURE_SAS}")
        self.assertEqual(headers["x-ms-blob-type"], "BlockBlob")
        self.assertEqual(headers["Content-Type"], "application/zip")

    def test_azure_endpoints_without_blob_core_hostname(self):
        """Detection is on the SAS grammar, so non-".blob.core.*" endpoints are covered too."""
        for label, url in (
            ("sovereign gov", f"https://acct.blob.core.usgovcloudapi.net/c/a.zip?{AZURE_SAS}"),
            ("sovereign china", f"https://acct.blob.core.chinacloudapi.cn/c/a.zip?{AZURE_SAS}"),
            ("dns zone endpoint", f"https://acct.z14.blob.storage.azure.net/c/a.zip?{AZURE_SAS}"),
            ("custom domain", f"https://artifacts.example.com/c/a.zip?{AZURE_SAS}"),
            ("azurite", f"http://127.0.0.1:10000/devstoreaccount1/c/a.zip?{AZURE_SAS}"),
        ):
            with self.subTest(label):
                self.assertEqual(_upload_headers(url)["x-ms-blob-type"], "BlockBlob")

    def test_user_delegation_sas_is_detected(self):
        """A user-delegation SAS carries extra skoid/sktid params but still has sv and sig."""
        url = (
            "https://acct.blob.core.windows.net/c/a.zip?sv=2024-11-04&sr=b&sp=cw"
            "&skoid=1111-2222&sktid=3333-4444&sig=cGxhY2Vob2xkZXI%3D"
        )
        self.assertEqual(_upload_headers(url)["x-ms-blob-type"], "BlockBlob")

    def test_s3_and_gcs_get_no_extra_headers(self):
        """Presigned S3/GCS URLs must be left untouched."""
        for url in (
            "https://bucket.s3.us-east-1.amazonaws.com/artifacts.zip?X-Amz-Signature=abc",
            "https://storage.googleapis.com/bucket/artifacts.zip?X-Goog-Signature=abc",
        ):
            with self.subTest(url=url):
                self.assertEqual(_upload_headers(url), {})

    def test_partial_sas_params_get_no_extra_headers(self):
        """Both sv and sig are required; a lone one is not an Azure SAS."""
        for url in (
            "https://acct.blob.core.windows.net/c/a.zip?sv=2024-11-04",
            "https://acct.blob.core.windows.net/c/a.zip?sig=cGxhY2Vob2xkZXI%3D",
            "https://acct.blob.core.windows.net/c/a.zip",
        ):
            with self.subTest(url=url):
                self.assertEqual(_upload_headers(url), {})

    def test_malformed_url_gets_no_extra_headers(self):
        """A URL without a hostname must not raise."""
        self.assertEqual(_upload_headers("not-a-url"), {})


class TestFormatError(unittest.TestCase):
    def _response(self, text="", headers=None):
        response = MagicMock()
        response.headers = headers or {}
        response.text = text
        response.__repr__ = lambda _: "<Response [403]>"
        return response

    def test_signed_request_is_not_leaked(self):
        """An S3 SignatureDoesNotMatch body echoes live credentials; only the code may escape."""
        body = (
            "<Error><Code>SignatureDoesNotMatch</Code>"
            "<Message>The request signature we calculated does not match</Message>"
            "<StringToSign>AWS4-HMAC-SHA256\n20260814T120000Z\n"
            "X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20260814%2Fus-east-1%2Fs3%2Faws4_request&amp;"
            "X-Amz-Security-Token=FwoGZXIvYXdzEBYaDF9SECRETSESSIONTOKEN</StringToSign>"
            "<CanonicalRequest>PUT\n/artifacts.zip</CanonicalRequest></Error>"
        )
        message = _format_error(self._response(text=body))
        self.assertEqual(message, "<Response [403]> SignatureDoesNotMatch")
        self.assertNotIn("X-Amz-Security-Token", message)
        self.assertNotIn("SECRETSESSIONTOKEN", message)
        self.assertNotIn("AKIAIOSFODNN7EXAMPLE", message)

    def test_azure_authentication_error_detail_is_not_leaked(self):
        """Azure echoes the string-to-sign in <AuthenticationErrorDetail>."""
        body = (
            "<Error><Code>AuthenticationFailed</Code>"
            "<AuthenticationErrorDetail>Signature did not match. String to sign used was "
            "PUT\n\n\nsv=2024-11-04&amp;sig=REDACTMEabc123"
            "</AuthenticationErrorDetail></Error>"
        )
        message = _format_error(self._response(text=body))
        self.assertEqual(message, "<Response [403]> AuthenticationFailed")
        self.assertNotIn("REDACTMEabc123", message)

    def test_error_code_response_header_is_preferred(self):
        """Azure sets a machine-readable code in x-ms-error-code; the body is then untouched."""
        message = _format_error(
            self._response(
                text="<Error><Code>Ignored</Code><StringToSign>secret</StringToSign></Error>",
                headers={"x-ms-error-code": "MissingRequiredHeader"},
            )
        )
        self.assertEqual(message, "<Response [403]> MissingRequiredHeader")
        self.assertNotIn("secret", message)

    def test_body_without_error_code_is_dropped_entirely(self):
        """An unrecognized body shape must not be echoed on the chance it holds a secret."""
        message = _format_error(self._response(text="sig=abc123 was rejected"))
        self.assertEqual(message, "<Response [403]>")

    def test_non_string_body_is_ignored(self):
        """A stubbed/streamed response whose .text is not a str must not raise."""
        response = MagicMock()
        response.headers = {}
        response.__repr__ = lambda _: "<Response [400]>"
        self.assertEqual(_format_error(response), "<Response [400]>")


class TestUploadArtifacts(unittest.TestCase):
    def setUp(self):
        self.project_dir = "/fake/project/dir"
        self.target_path = "/fake/project/dir/target"
        self.command = "run"

        # Module-level state, only cleared on successful upload; reset for test isolation
        PRODUCED_ARTIFACTS_PATHS.clear()

        # Create patchers
        self.load_project_patcher = patch("dbt.utils.artifact_upload.load_project")
        self.zipfile_patcher = patch("dbt.utils.artifact_upload.zipfile.ZipFile")
        self.requests_post_patcher = patch("dbt.utils.artifact_upload.requests.post")
        self.requests_put_patcher = patch("dbt.utils.artifact_upload.requests.put")
        self.requests_patch_patcher = patch("dbt.utils.artifact_upload.requests.patch")
        self.open_patcher = patch("builtins.open", mock.mock_open(read_data=b"test data"))
        self.fire_event_patcher = patch("dbt.utils.artifact_upload.fire_event")
        self.retry_patcher = patch("dbt.utils.artifact_upload._retry_with_backoff")
        self.time_sleep_patcher = patch("dbt.utils.artifact_upload.time.sleep")

        # Start patchers
        self.mock_load_project = self.load_project_patcher.start()
        self.mock_zipfile = self.zipfile_patcher.start()
        self.mock_requests_post = self.requests_post_patcher.start()
        self.mock_requests_put = self.requests_put_patcher.start()
        self.mock_requests_patch = self.requests_patch_patcher.start()
        self.mock_open = self.open_patcher.start()
        self.mock_fire_event = self.fire_event_patcher.start()
        self.mock_retry = self.retry_patcher.start()
        self.mock_sleep = self.time_sleep_patcher.start()

        # Configure mocks
        self.mock_project = MagicMock()
        self.mock_project.dbt_cloud = {"tenant_hostname": "test-tenant"}
        self.mock_load_project.return_value = self.mock_project

        # Mock response for POST request (create ingest)
        self.mock_post_response = MagicMock()
        self.mock_post_response.status_code = 200
        self.mock_post_response.json.return_value = {
            "data": {"id": "ingest123", "upload_url": "https://test-upload-url.com"}
        }
        self.mock_requests_post.return_value = self.mock_post_response

        # Mock response for PUT request (upload artifacts)
        self.mock_put_response = MagicMock()
        self.mock_put_response.status_code = 200
        self.mock_requests_put.return_value = self.mock_put_response

        # Mock response for PATCH request (complete ingest)
        self.mock_patch_response = MagicMock()
        self.mock_patch_response.status_code = 204
        self.mock_requests_patch.return_value = self.mock_patch_response

        # Mock retry to pass through to the first result
        self.mock_retry.side_effect = lambda operation_name, func, max_retries=3: func()[1]

        # Setup the env var for the test
        self.original_token = os.environ.get("DBT_CLOUD_TOKEN")
        self.original_account_id = os.environ.get("DBT_CLOUD_ACCOUNT_ID")
        self.original_environment_id = os.environ.get("DBT_CLOUD_ENVIRONMENT_ID")

        os.environ["DBT_CLOUD_TOKEN"] = "test-token"
        os.environ["DBT_CLOUD_ACCOUNT_ID"] = "1234"
        os.environ["DBT_CLOUD_ENVIRONMENT_ID"] = "5678"

    def tearDown(self):
        self.load_project_patcher.stop()
        self.zipfile_patcher.stop()
        self.requests_post_patcher.stop()
        self.requests_put_patcher.stop()
        self.requests_patch_patcher.stop()
        self.open_patcher.stop()
        self.fire_event_patcher.stop()
        self.retry_patcher.stop()
        self.time_sleep_patcher.stop()
        if self.original_token:
            os.environ["DBT_CLOUD_TOKEN"] = self.original_token
        if self.original_account_id:
            os.environ["DBT_CLOUD_ACCOUNT_ID"] = self.original_account_id
        if self.original_environment_id:
            os.environ["DBT_CLOUD_ENVIRONMENT_ID"] = self.original_environment_id

    def test_upload_artifacts_successful_upload(self):
        # Set up mock for ZipFile context manager
        mock_zipfile_instance = MagicMock()
        self.mock_zipfile.return_value.__enter__.return_value = mock_zipfile_instance
        add_artifact_produced(os.path.join(self.target_path, MANIFEST_FILE_NAME))
        add_artifact_produced(os.path.join(self.target_path, RUN_RESULTS_FILE_NAME))

        # Call the function
        upload_artifacts(self.project_dir, self.target_path, self.command)

        # Verify the project was loaded
        self.mock_load_project.assert_called_once_with(
            self.project_dir, version_check=False, profile=mock.ANY, cli_vars=None
        )

        # Verify zip file was created and artifacts were added
        self.mock_zipfile.assert_called_once_with("target.zip", "w")
        expected_artifact_calls = [
            call(f"{self.target_path}/{RUN_RESULTS_FILE_NAME}", RUN_RESULTS_FILE_NAME),
            call(f"{self.target_path}/{MANIFEST_FILE_NAME}", MANIFEST_FILE_NAME),
        ]
        mock_zipfile_instance.write.assert_has_calls(expected_artifact_calls, any_order=True)

        # Verify retry was called for each step
        self.assertEqual(self.mock_retry.call_count, 3)
        retry_calls = [call_args[0][0] for call_args in self.mock_retry.call_args_list]
        self.assertIn("creating ingest request", retry_calls)
        self.assertIn("uploading artifacts", retry_calls)
        self.assertIn("completing ingest", retry_calls)

        # Verify fire_event was called with ArtifactUploadSuccess
        success_event_call = [
            call
            for call in self.mock_fire_event.call_args_list
            if "completed successfully" in call[0][0].msg
        ]
        self.assertTrue(len(success_event_call) > 0)

    def _set_upload_url(self, upload_url):
        self.mock_post_response.json.return_value = {
            "data": {"id": "ingest123", "upload_url": upload_url}
        }

    def test_upload_sends_blob_type_header_for_azure_url(self):
        """The PUT to an Azure Blob SAS URL must carry x-ms-blob-type."""
        self.mock_zipfile.return_value.__enter__.return_value = MagicMock()
        self._set_upload_url(f"https://prodeu2.blob.core.windows.net/c/artifacts.zip?{AZURE_SAS}")
        add_artifact_produced(os.path.join(self.target_path, MANIFEST_FILE_NAME))

        upload_artifacts(self.project_dir, self.target_path, self.command)

        headers = self.mock_requests_put.call_args.kwargs["headers"]
        self.assertEqual(headers["x-ms-blob-type"], "BlockBlob")

    def test_upload_sends_no_extra_headers_for_s3_url(self):
        """Presigned S3 uploads must not gain Azure-specific headers."""
        self.mock_zipfile.return_value.__enter__.return_value = MagicMock()
        self._set_upload_url(
            "https://bucket.s3.us-east-1.amazonaws.com/artifacts.zip"
            "?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Expires=3600&X-Amz-Signature=abc"
        )
        add_artifact_produced(os.path.join(self.target_path, MANIFEST_FILE_NAME))

        upload_artifacts(self.project_dir, self.target_path, self.command)

        self.assertEqual(self.mock_requests_put.call_args.kwargs["headers"], {})

    def test_upload_treats_201_created_as_success(self):
        """Azure returns 201 on a successful Put Blob; it must not be an error."""
        # Delegate to the real retry logic so success/failure classification is exercised
        self.mock_retry.side_effect = _retry_with_backoff

        self.mock_zipfile.return_value.__enter__.return_value = MagicMock()
        self._set_upload_url(f"https://prodeu2.blob.core.windows.net/c/artifacts.zip?{AZURE_SAS}")
        self.mock_put_response.status_code = 201
        add_artifact_produced(os.path.join(self.target_path, MANIFEST_FILE_NAME))

        upload_artifacts(self.project_dir, self.target_path, self.command)

        self.mock_requests_put.assert_called_once()
        success_event_call = [
            call
            for call in self.mock_fire_event.call_args_list
            if "completed successfully" in call[0][0].msg
        ]
        self.assertTrue(len(success_event_call) > 0)

    def test_upload_artifacts_default_target_path(self):
        # Call the function with target_path=None
        mock_zipfile_instance = MagicMock()
        self.mock_zipfile.return_value.__enter__.return_value = mock_zipfile_instance
        add_artifact_produced(os.path.join(self.target_path, MANIFEST_FILE_NAME))
        add_artifact_produced(os.path.join(self.target_path, RUN_RESULTS_FILE_NAME))

        upload_artifacts(self.project_dir, None, self.command)

        # Verify the default target path was used
        expected_artifact_calls = [
            call(f"{self.target_path}/{RUN_RESULTS_FILE_NAME}", RUN_RESULTS_FILE_NAME),
            call(f"{self.target_path}/{MANIFEST_FILE_NAME}", MANIFEST_FILE_NAME),
        ]
        mock_zipfile_instance.write.assert_has_calls(expected_artifact_calls, any_order=True)

    def test_upload_artifacts_missing_tenant_config(self):
        # Set up project without dbt_cloud config
        self.mock_project.dbt_cloud = {}
        add_artifact_produced(os.path.join(self.target_path, MANIFEST_FILE_NAME))

        # Verify that the function raises an exception
        with self.assertRaises(DbtProjectError) as context:
            upload_artifacts(self.project_dir, self.target_path, self.command)

        self.assertIn("tenant_hostname not found", str(context.exception))
        self.mock_retry.assert_not_called()

    def test_upload_artifacts_with_retry_failures(self):
        # Set up mock for ZipFile context manager
        mock_zipfile_instance = MagicMock()
        self.mock_zipfile.return_value.__enter__.return_value = mock_zipfile_instance

        # Make retry raise exceptions for each step
        self.mock_retry.side_effect = [
            DbtBaseException("Error creating ingest request: Mock failure"),
            DbtBaseException("Error uploading artifacts: Mock failure"),
            DbtBaseException("Error completing ingest: Mock failure"),
        ]
        add_artifact_produced(os.path.join(self.target_path, MANIFEST_FILE_NAME))

        # Test each step failing
        # 1. Create ingest failure
        with self.assertRaises(DbtBaseException) as context:
            upload_artifacts(self.project_dir, self.target_path, self.command)
        self.assertIn("Error creating ingest request", str(context.exception))
        self.mock_retry.reset_mock()

        # Reset the side effect for the next test
        self.mock_retry.side_effect = [
            self.mock_post_response,  # First call succeeds
            DbtBaseException("Error uploading artifacts: Mock failure"),
            DbtBaseException("Error completing ingest: Mock failure"),
        ]

        # 2. Upload failure
        with self.assertRaises(DbtBaseException) as context:
            upload_artifacts(self.project_dir, self.target_path, self.command)
        self.assertIn("Error uploading artifacts", str(context.exception))
        self.mock_retry.reset_mock()

        # Reset the side effect for the next test
        self.mock_retry.side_effect = [
            self.mock_post_response,  # First call succeeds
            self.mock_put_response,  # Second call succeeds
            DbtBaseException("Error completing ingest: Mock failure"),
        ]

        # 3. Complete failure
        with self.assertRaises(DbtBaseException) as context:
            upload_artifacts(self.project_dir, self.target_path, self.command)
        self.assertIn("Error completing ingest", str(context.exception))


class TestWriteSemanticManifestRegistersArtifact(unittest.TestCase):
    def setUp(self):
        self.target_path = "/fake/project/dir/target"
        PRODUCED_ARTIFACTS_PATHS.clear()

    def tearDown(self):
        PRODUCED_ARTIFACTS_PATHS.clear()

    @patch("dbt.parser.manifest.SemanticManifest")
    def test_write_semantic_manifest_registers_artifact(self, mock_semantic_manifest):
        write_semantic_manifest(manifest=MagicMock(), target_path=self.target_path)

        expected_path = os.path.join(self.target_path, SEMANTIC_MANIFEST_FILE_NAME)
        mock_semantic_manifest.return_value.write_json_to_file.assert_called_once_with(
            expected_path
        )
        self.assertIn(expected_path, PRODUCED_ARTIFACTS_PATHS)


if __name__ == "__main__":
    unittest.main()
