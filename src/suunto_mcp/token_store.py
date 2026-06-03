from __future__ import annotations

import json
import os
import stat
from abc import ABC, abstractmethod
from pathlib import Path
from typing import Any, cast

import keyring

from suunto_mcp.config import Settings, settings
from suunto_mcp.models import TokenRecord

KEYRING_SERVICE = "suunto-mcp"
KEYRING_ACCOUNT = "tokens"


class TokenStoreError(RuntimeError):
    pass


class TokenStore(ABC):
    @abstractmethod
    def list_records(self) -> list[TokenRecord]:
        raise NotImplementedError

    @abstractmethod
    def get(self, account_id: str) -> TokenRecord | None:
        raise NotImplementedError

    @abstractmethod
    def save(self, record: TokenRecord) -> None:
        raise NotImplementedError

    @abstractmethod
    def delete(self, account_id: str) -> bool:
        raise NotImplementedError

    def resolve(
        self, account_id: str | None = None, default_account_id: str | None = None
    ) -> TokenRecord:
        if account_id:
            record = self.get(account_id)
            if record is None:
                raise TokenStoreError(f"No Suunto account stored for account_id={account_id!r}.")
            return record

        default_id = default_account_id or settings.DEFAULT_ACCOUNT_ID
        if default_id:
            record = self.get(default_id)
            if record is None:
                raise TokenStoreError(f"Default account_id={default_id!r} is not stored.")
            return record

        records = self.list_records()
        if not records:
            raise TokenStoreError(
                "No Suunto accounts are stored. Authorize or import tokens first."
            )
        if len(records) > 1:
            ids = ", ".join(record.account_id for record in records)
            raise TokenStoreError(
                f"Multiple accounts are stored ({ids}); pass account_id explicitly."
            )
        return records[0]


class InMemoryTokenStore(TokenStore):
    def __init__(self) -> None:
        self._records: dict[str, TokenRecord] = {}

    def list_records(self) -> list[TokenRecord]:
        return sorted(self._records.values(), key=lambda record: record.account_id)

    def get(self, account_id: str) -> TokenRecord | None:
        return self._records.get(account_id)

    def save(self, record: TokenRecord) -> None:
        self._records[record.account_id] = record

    def delete(self, account_id: str) -> bool:
        return self._records.pop(account_id, None) is not None


class JsonFileTokenStore(TokenStore):
    def __init__(self, path: str | Path) -> None:
        self.path = Path(path).expanduser()

    def _read_raw(self) -> dict[str, Any]:
        if not self.path.exists():
            return {"accounts": {}}
        data_obj: object = json.loads(self.path.read_text(encoding="utf-8"))
        if not isinstance(data_obj, dict):
            raise TokenStoreError(f"Token file {self.path} is not a JSON object.")
        data = cast(dict[str, Any], data_obj)
        accounts = data.get("accounts")
        if not isinstance(accounts, dict):
            raise TokenStoreError(f"Token file {self.path} does not contain an accounts object.")
        return data

    def _write_raw(self, data: dict[str, Any]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix(self.path.suffix + ".tmp")
        tmp.write_text(json.dumps(data, indent=2, sort_keys=True), encoding="utf-8")
        os.chmod(tmp, stat.S_IRUSR | stat.S_IWUSR)
        tmp.replace(self.path)
        os.chmod(self.path, stat.S_IRUSR | stat.S_IWUSR)

    def list_records(self) -> list[TokenRecord]:
        raw = self._read_raw()["accounts"]
        return sorted(
            (TokenRecord.model_validate(value) for value in raw.values()),
            key=lambda record: record.account_id,
        )

    def get(self, account_id: str) -> TokenRecord | None:
        raw = self._read_raw()["accounts"]
        value = raw.get(account_id)
        return TokenRecord.model_validate(value) if value else None

    def save(self, record: TokenRecord) -> None:
        data = self._read_raw()
        accounts = data["accounts"]
        accounts[record.account_id] = record.model_dump(mode="json")
        self._write_raw(data)

    def delete(self, account_id: str) -> bool:
        data = self._read_raw()
        removed = data["accounts"].pop(account_id, None) is not None
        if removed:
            self._write_raw(data)
        return removed


class KeyringTokenStore(TokenStore):
    def _read_raw(self) -> dict[str, Any]:
        try:
            value = keyring.get_password(KEYRING_SERVICE, KEYRING_ACCOUNT)
        except Exception as exc:  # pragma: no cover - backend specific
            raise TokenStoreError(f"Could not read from keyring: {exc}") from exc
        if not value:
            return {"accounts": {}}
        data_obj: object = json.loads(value)
        if not isinstance(data_obj, dict):
            raise TokenStoreError("Keyring token payload has an invalid shape.")
        data = cast(dict[str, Any], data_obj)
        if not isinstance(data.get("accounts"), dict):
            raise TokenStoreError("Keyring token payload has an invalid shape.")
        return data

    def _write_raw(self, data: dict[str, Any]) -> None:
        try:
            keyring.set_password(KEYRING_SERVICE, KEYRING_ACCOUNT, json.dumps(data, sort_keys=True))
        except Exception as exc:  # pragma: no cover - backend specific
            raise TokenStoreError(f"Could not write to keyring: {exc}") from exc

    def list_records(self) -> list[TokenRecord]:
        raw = self._read_raw()["accounts"]
        return sorted(
            (TokenRecord.model_validate(value) for value in raw.values()),
            key=lambda record: record.account_id,
        )

    def get(self, account_id: str) -> TokenRecord | None:
        value = self._read_raw()["accounts"].get(account_id)
        return TokenRecord.model_validate(value) if value else None

    def save(self, record: TokenRecord) -> None:
        data = self._read_raw()
        data["accounts"][record.account_id] = record.model_dump(mode="json")
        self._write_raw(data)

    def delete(self, account_id: str) -> bool:
        data = self._read_raw()
        removed = data["accounts"].pop(account_id, None) is not None
        if removed:
            self._write_raw(data)
        return removed


_memory_store = InMemoryTokenStore()


def default_token_file(settings_obj: Settings = settings) -> Path:
    if settings_obj.TOKEN_FILE:
        return Path(settings_obj.TOKEN_FILE).expanduser()
    return Path(settings_obj.LOCAL_DATA_DIR).expanduser().parent / "tokens.json"


def get_token_store(settings_obj: Settings = settings) -> TokenStore:
    if settings_obj.TOKEN_STORE == "memory":
        return _memory_store
    if settings_obj.TOKEN_STORE == "file":
        return JsonFileTokenStore(default_token_file(settings_obj))
    if settings_obj.TOKEN_STORE == "keyring":
        return KeyringTokenStore()
    raise TokenStoreError(f"Unsupported token store: {settings_obj.TOKEN_STORE}")
