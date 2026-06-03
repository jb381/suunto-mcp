from __future__ import annotations

import base64
import json
from collections.abc import Mapping

import pytest

from suunto_mcp.auth import decode_jwt_payload_unverified, infer_account_id, token_set_from_response


def _jwt(payload: dict[str, object]) -> str:
    header = {"alg": "none", "typ": "JWT"}

    def encode(data: Mapping[str, object]) -> str:
        raw = json.dumps(data, separators=(",", ":")).encode("utf-8")
        return base64.urlsafe_b64encode(raw).decode("ascii").rstrip("=")

    return f"{encode(header)}.{encode(payload)}."


def test_decode_jwt_payload_unverified_extracts_suunto_user() -> None:
    token = _jwt({"user": "runner42", "exp": 123456})

    assert decode_jwt_payload_unverified(token)["user"] == "runner42"
    assert infer_account_id(token) == "runner42"


def test_token_set_from_response_uses_jwt_expiry() -> None:
    token = _jwt({"user": "runner42", "exp": 123456})

    token_set = token_set_from_response({"access_token": token, "refresh_token": "refresh"})

    assert token_set.expires_at == 123456
    assert token_set.refresh_token == "refresh"


def test_infer_account_id_requires_override_for_non_jwt_token() -> None:
    with pytest.raises(ValueError):
        infer_account_id("not-a-jwt")

    assert infer_account_id("not-a-jwt", explicit_account_id="manual") == "manual"
