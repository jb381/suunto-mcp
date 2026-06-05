from __future__ import annotations

import time
from collections.abc import Mapping
from typing import Any, Literal

import httpx

from suunto_mcp.auth import record_from_token_set, refresh_access_token
from suunto_mcp.config import Settings, settings
from suunto_mcp.models import TokenRecord
from suunto_mcp.quota import QuotaExceededError, rate_limiter, weekly_quota_guard
from suunto_mcp.token_store import TokenStore, get_token_store

HTTPMethod = Literal["GET", "POST", "PUT", "DELETE", "PATCH"]


class SuuntoAPIError(RuntimeError):
    def __init__(
        self,
        message: str,
        *,
        status_code: int | None = None,
        method: str | None = None,
        url: str | None = None,
        response_preview: str | None = None,
    ) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.method = method
        self.url = url
        self.response_preview = response_preview

    def to_dict(self) -> dict[str, Any]:
        return {
            "message": str(self),
            "status_code": self.status_code,
            "method": self.method,
            "url": self.url,
        }


def _truncated_response_preview(text: str, limit: int = 1000) -> str:
    return text[:limit]


class SuuntoClient:
    def __init__(
        self,
        *,
        account_id: str | None = None,
        settings_obj: Settings = settings,
        token_store: TokenStore | None = None,
        http_client: httpx.AsyncClient | None = None,
    ) -> None:
        self.account_id = account_id
        self.settings = settings_obj
        self.token_store = token_store or get_token_store(settings_obj)
        self._http_client = http_client
        self._owns_client = http_client is None

    async def __aenter__(self) -> SuuntoClient:
        if self._http_client is None:
            self._http_client = httpx.AsyncClient(timeout=httpx.Timeout(30.0, connect=10.0))
        return self

    async def __aexit__(self, *_exc: object) -> None:
        if self._owns_client and self._http_client is not None:
            await self._http_client.aclose()
        self._http_client = None

    @property
    def http_client(self) -> httpx.AsyncClient:
        if self._http_client is None:
            raise RuntimeError("SuuntoClient must be used as an async context manager.")
        return self._http_client

    async def _record(self) -> TokenRecord:
        record = self.token_store.resolve(self.account_id, self.settings.DEFAULT_ACCOUNT_ID)
        if record.expires_at is not None and record.expires_at <= time.time() + 60:
            if not record.refresh_token:
                raise SuuntoAPIError(
                    "Stored access token is expired and no refresh token is available."
                )
            token_set = await refresh_access_token(
                record.refresh_token,
                settings_obj=self.settings,
                client=self.http_client,
            )
            record = record_from_token_set(token_set, account_id=record.account_id, existing=record)
            self.token_store.save(record)
        return record

    async def _headers(
        self,
        *,
        content_type: str | None = None,
        accept: str | None = None,
    ) -> dict[str, str]:
        if not self.settings.SUBSCRIPTION_KEY:
            raise SuuntoAPIError("SUUNTO_SUBSCRIPTION_KEY is required for Suunto Cloud API calls.")
        record = await self._record()
        headers = {
            "Authorization": f"Bearer {record.access_token}",
            "Ocp-Apim-Subscription-Key": self.settings.SUBSCRIPTION_KEY,
        }
        if content_type:
            headers["Content-Type"] = content_type
        if accept:
            headers["Accept"] = accept
        return headers

    async def _apply_api_call_limits(self, *, method: HTTPMethod, path: str) -> None:
        rate_key = self.settings.SUBSCRIPTION_KEY or self.settings.API_BASE
        await rate_limiter.wait(
            key=rate_key,
            calls_per_minute=self.settings.API_CALLS_PER_MINUTE,
        )
        try:
            await weekly_quota_guard.record_call(
                settings_obj=self.settings,
                account_id=self.account_id or self.settings.DEFAULT_ACCOUNT_ID or None,
                method=method,
                path=path,
            )
        except QuotaExceededError as exc:
            raise SuuntoAPIError(str(exc), method=method, url=path) from exc

    async def request(
        self,
        method: HTTPMethod,
        path: str,
        *,
        params: Mapping[str, Any] | None = None,
        json_body: Any | None = None,
        content: bytes | None = None,
        content_type: str | None = None,
        accept: str | None = None,
        retry_refresh: bool = True,
    ) -> httpx.Response:
        await self._apply_api_call_limits(method=method, path=path)
        url = self.settings.API_BASE.rstrip("/") + "/" + path.lstrip("/")
        headers = await self._headers(content_type=content_type, accept=accept)
        response = await self.http_client.request(
            method,
            url,
            params=dict(params or {}),
            json=json_body,
            content=content,
            headers=headers,
        )
        if response.status_code == 401 and retry_refresh:
            record = self.token_store.resolve(self.account_id, self.settings.DEFAULT_ACCOUNT_ID)
            if record.refresh_token:
                token_set = await refresh_access_token(
                    record.refresh_token,
                    settings_obj=self.settings,
                    client=self.http_client,
                )
                record = record_from_token_set(
                    token_set, account_id=record.account_id, existing=record
                )
                self.token_store.save(record)
                await self._apply_api_call_limits(method=method, path=path)
                headers = await self._headers(content_type=content_type, accept=accept)
                response = await self.http_client.request(
                    method,
                    url,
                    params=dict(params or {}),
                    json=json_body,
                    content=content,
                    headers=headers,
                )
        if response.status_code >= 400:
            raise SuuntoAPIError(
                f"Suunto API returned HTTP {response.status_code}.",
                status_code=response.status_code,
                method=method,
                url=url,
                response_preview=_truncated_response_preview(response.text),
            )
        return response

    async def get_json(self, path: str, *, params: Mapping[str, Any] | None = None) -> Any:
        response = await self.request("GET", path, params=params, accept="application/json")
        return response.json()

    async def post_json(
        self,
        path: str,
        *,
        params: Mapping[str, Any] | None = None,
        body: Any | None = None,
    ) -> Any:
        response = await self.request(
            "POST",
            path,
            params=params,
            json_body=body,
            content_type="application/json",
            accept="application/json",
        )
        return response.json()

    async def put_json(self, path: str, *, body: Any | None = None) -> Any:
        response = await self.request(
            "PUT",
            path,
            json_body=body,
            content_type="application/json",
            accept="application/json",
        )
        return response.json() if response.content else {"ok": True}

    async def delete_json(self, path: str) -> Any:
        response = await self.request("DELETE", path, accept="application/json")
        return response.json() if response.content else {"ok": True}

    async def get_bytes(
        self,
        path: str,
        *,
        params: Mapping[str, Any] | None = None,
        accept: str | None = None,
    ) -> tuple[bytes, str | None]:
        response = await self.request("GET", path, params=params, accept=accept)
        return response.content, response.headers.get("content-type")

    async def post_bytes(
        self,
        path: str,
        *,
        content: bytes,
        params: Mapping[str, Any] | None = None,
        content_type: str,
        accept: str | None = None,
    ) -> tuple[bytes, str | None]:
        response = await self.request(
            "POST",
            path,
            params=params,
            content=content,
            content_type=content_type,
            accept=accept,
        )
        return response.content, response.headers.get("content-type")

    async def put_bytes(
        self,
        path: str,
        *,
        content: bytes,
        content_type: str,
        accept: str | None = None,
    ) -> tuple[bytes, str | None]:
        response = await self.request(
            "PUT",
            path,
            content=content,
            content_type=content_type,
            accept=accept,
        )
        return response.content, response.headers.get("content-type")
