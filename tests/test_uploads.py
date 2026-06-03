from __future__ import annotations

from typing import cast

import pytest

from suunto_mcp.client import SuuntoClient
from suunto_mcp.tools.uploads import _is_terminal_upload_status, _upload_status_value


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
