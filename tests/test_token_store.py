from __future__ import annotations

from suunto_mcp.models import TokenRecord
from suunto_mcp.token_store import JsonFileTokenStore


def test_json_file_token_store_round_trips_without_safe_secret_leak(tmp_path) -> None:
    store = JsonFileTokenStore(tmp_path / "tokens.json")
    record = TokenRecord(
        account_id="runner42",
        username="runner42",
        access_token="access-secret",
        refresh_token="refresh-secret",
        expires_at=123456.0,
    )

    store.save(record)
    loaded = store.resolve()

    assert loaded.account_id == "runner42"
    assert loaded.access_token == "access-secret"
    assert store.list_records()[0].account_id == "runner42"
    assert store.delete("runner42")
    assert store.list_records() == []

    safe = record.safe_dict()
    assert "access-secret" not in str(safe)
    assert "refresh-secret" not in str(safe)
    assert safe["has_access_token"] is True
    assert safe["has_refresh_token"] is True
