from __future__ import annotations

from datetime import UTC, datetime
from typing import Any

from pydantic import BaseModel, Field


def utc_now() -> datetime:
    return datetime.now(UTC)


class TokenRecord(BaseModel):
    account_id: str
    access_token: str
    refresh_token: str | None = None
    username: str | None = None
    label: str | None = None
    expires_at: float | None = None
    scope: str | None = None
    token_type: str = "bearer"
    created_at: datetime = Field(default_factory=utc_now)
    updated_at: datetime = Field(default_factory=utc_now)

    def safe_dict(self) -> dict[str, Any]:
        return {
            "account_id": self.account_id,
            "username": self.username,
            "label": self.label,
            "expires_at": self.expires_at,
            "scope": self.scope,
            "token_type": self.token_type,
            "created_at": self.created_at.isoformat(),
            "updated_at": self.updated_at.isoformat(),
            "has_access_token": bool(self.access_token),
            "has_refresh_token": bool(self.refresh_token),
        }


class TokenSet(BaseModel):
    access_token: str
    refresh_token: str | None = None
    expires_in: int | None = None
    expires_at: float | None = None
    scope: str | None = None
    token_type: str = "bearer"
    raw: dict[str, Any] = Field(default_factory=dict)


class SuuntoErrorPayload(BaseModel):
    status_code: int
    message: str
    method: str | None = None
    url: str | None = None
    response_preview: str | None = None


class ImportedFileRecord(BaseModel):
    import_id: str
    source_path: str
    stored_path: str | None = None
    account_id: str | None = None
    kind: str
    sha256: str
    size: int
    imported_at: datetime = Field(default_factory=utc_now)
    parsed_summary: dict[str, Any] = Field(default_factory=dict)

    def safe_dict(self) -> dict[str, Any]:
        return self.model_dump(mode="json")
