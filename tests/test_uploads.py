from __future__ import annotations

from typing import cast

import pytest

from suunto_mcp.client import SuuntoClient
from suunto_mcp.tools.uploads import (
    _is_terminal_upload_status,
    _upload_status_value,
    _validate_signed_url,
)


def test_upload_status_value_checks_known_status_fields() -> None:
    assert _upload_status_value({"status": "COMPLETED"}) == "COMPLETED"
    assert _upload_status_value({"state": "DONE"}) == "DONE"
    assert _upload_status_value({"uploadStatus": "FAILED"}) == "FAILED"
    assert _upload_status_value({"processingStatus": "PROCESSING"}) == "PROCESSING"
    assert _upload_status_value({"other": "PROCESSING"}) is None


def test_terminal_upload_status_matching_is_case_insensitive() -> None:
    assert _is_terminal_upload_status({"status": "completed"}, ["COMPLETED"]) is True
    assert _is_terminal_upload_status({"status": "PROCESSING"}, ["COMPLETED"]) is False


async def test_wait_for_upload_status_rejects_invalid_attempt_count() -> None:
    from suunto_mcp.tools.uploads import _wait_for_upload_status

    with pytest.raises(ValueError, match="max_attempts"):
        await _wait_for_upload_status(cast(SuuntoClient, object()), "upload-1", max_attempts=0)


def test_validate_signed_url_rejects_http() -> None:
    with pytest.raises(ValueError, match="https://"):
        _validate_signed_url("http://example.com/upload")


def test_validate_signed_url_rejects_localhost() -> None:
    with pytest.raises(ValueError, match="localhost"):
        _validate_signed_url("https://localhost/upload")


def test_validate_signed_url_rejects_ip_literals() -> None:
    with pytest.raises(ValueError, match="private"):
        _validate_signed_url("https://127.0.0.1/upload")
    with pytest.raises(ValueError, match="private"):
        _validate_signed_url("https://[::1]/upload")
    with pytest.raises(ValueError, match="private"):
        _validate_signed_url("https://192.168.1.1/upload")
    with pytest.raises(ValueError, match="private"):
        _validate_signed_url("https://10.0.0.1/upload")
    with pytest.raises(ValueError, match="private"):
        _validate_signed_url("https://172.16.0.1/upload")
    with pytest.raises(ValueError, match="private"):
        _validate_signed_url("https://169.254.1.1/upload")
    with pytest.raises(ValueError, match="object-storage"):
        _validate_signed_url("https://8.8.8.8/upload")


def test_validate_signed_url_accepts_known_object_storage_hostnames() -> None:
    _validate_signed_url("https://s3.amazonaws.com/bucket/object?sig=abc")
    _validate_signed_url("https://bucket.s3.amazonaws.com/object?sig=abc")
    _validate_signed_url("https://bucket.s3.us-west-2.amazonaws.com/object?sig=abc")
    _validate_signed_url("https://account.blob.core.windows.net/container/object?sig=abc")
    _validate_signed_url("https://storage.googleapis.com/bucket/object?sig=abc")
    _validate_signed_url("https://bucket.storage.googleapis.com/object?sig=abc")


def test_validate_signed_url_rejects_arbitrary_hostnames() -> None:
    with pytest.raises(ValueError, match="object-storage"):
        _validate_signed_url("https://attacker.example.com/path?sig=abc")


def test_validate_signed_url_rejects_embedded_credentials() -> None:
    with pytest.raises(ValueError, match="credentials"):
        _validate_signed_url("https://user:pass@example.com/upload")
