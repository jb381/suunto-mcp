from __future__ import annotations

from pathlib import Path
from typing import Literal

from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(
        env_prefix="SUUNTO_",
        env_file=".env",
        env_file_encoding="utf-8",
        case_sensitive=True,
        extra="ignore",
    )

    # Suunto API credentials
    CLIENT_ID: str = ""
    CLIENT_SECRET: str = ""
    REDIRECT_URI: str = ""
    SUBSCRIPTION_KEY: str = ""

    # API base URLs
    API_BASE: str = "https://cloudapi.suunto.com"
    OAUTH_BASE: str = "https://cloudapi-oauth.suunto.com"

    # API limit safety. Set either value to 0 to disable that guard.
    API_CALLS_PER_MINUTE: float = 10.0
    API_WEEKLY_CALL_LIMIT: int = 200
    API_QUOTA_FILE: str = ""

    # Token storage
    TOKEN_STORE: Literal["keyring", "file", "memory"] = "keyring"
    TOKEN_FILE: str = ""

    # Multi-user
    DEFAULT_ACCOUNT_ID: str = ""

    # Local data / no-key mode
    LOCAL_DATA_DIR: str = str(Path.home() / ".suunto-mcp" / "data")
    EXPORT_DIR: str = ""
    MAX_BASE64_BYTES: int = 5_000_000

    # HTTP auth for MCP server
    MCP_API_TOKEN: str = ""

    # Webhook config
    WEBHOOK_SECRET: str = ""
    WEBHOOK_STORE: Literal["sqlite", "jsonl", "memory"] = "jsonl"
    WEBHOOK_STORE_PATH: str = ""

    # Safety gates
    ENABLE_WRITE_TOOLS: bool = False

    # Dev
    LIVE_TESTS: bool = False

    def redacted_display(self) -> dict[str, str]:
        d: dict[str, str] = self.model_dump()
        redact_keys = (
            "CLIENT_ID",
            "CLIENT_SECRET",
            "REDIRECT_URI",
            "SUBSCRIPTION_KEY",
            "MCP_API_TOKEN",
            "WEBHOOK_SECRET",
        )
        for key in redact_keys:
            if d.get(key):
                d[key] = d[key][:4] + "..." if len(d[key]) > 8 else "***"
        return d


settings = Settings()
