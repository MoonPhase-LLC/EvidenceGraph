from __future__ import annotations

from collections.abc import Callable

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from evidencegraph_service.app import create_app
from evidencegraph_service.config import Settings

#: 32 chars -- comfortably above MIN_SESSION_TOKEN_LENGTH. Not a secret:
#: this value is fixed only so tests are deterministic and never resembles
#: a value that could be mistaken for a real, in-use credential.
VALID_TOKEN = "test-only-session-token-abcdef12"

SettingsFactory = Callable[..., Settings]


@pytest.fixture(autouse=True)
def _isolated_environment(monkeypatch: pytest.MonkeyPatch) -> None:
    """Strips any ambient `EVIDENCEGRAPH_*` variables from the real shell so
    the suite's behavior never depends on what happens to be set on the
    machine running it -- every test builds `Settings` from explicit kwargs
    or its own `monkeypatch.setenv` calls instead."""
    import os

    for key in list(os.environ):
        if key.startswith("EVIDENCEGRAPH_"):
            monkeypatch.delenv(key, raising=False)


@pytest.fixture
def make_settings() -> SettingsFactory:
    """Builds a `Settings` instance from explicit kwargs only.

    Deliberately does not fall back to the real process environment for
    any field under test, so tests are isolated from whatever
    `EVIDENCEGRAPH_*` variables happen to be set on the machine running
    them.
    """

    def _make(
        *,
        host: str = "127.0.0.1",
        port: int = 51823,
        environment: str = "production",
        session_token: str | None = VALID_TOKEN,
    ) -> Settings:
        return Settings(
            host=host,
            port=port,
            environment=environment,
            session_token=session_token,
        )

    return _make


@pytest.fixture
def make_app(make_settings: SettingsFactory) -> Callable[..., FastAPI]:
    def _make(**settings_kwargs: object) -> FastAPI:
        settings = make_settings(**settings_kwargs)
        return create_app(settings)

    return _make


@pytest.fixture
def make_client(make_app: Callable[..., FastAPI]) -> Callable[..., TestClient]:
    def _make(**settings_kwargs: object) -> TestClient:
        return TestClient(make_app(**settings_kwargs))

    return _make


@pytest.fixture
def client(make_client: Callable[..., TestClient]) -> TestClient:
    """The common case: production-mode app with a valid token configured."""
    return make_client()


@pytest.fixture
def auth_headers() -> dict[str, str]:
    return {"Authorization": f"Bearer {VALID_TOKEN}"}
