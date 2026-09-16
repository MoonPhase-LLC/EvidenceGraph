from __future__ import annotations

import pytest
from pydantic import ValidationError

from evidencegraph_service.config import DEFAULT_PORT, MIN_SESSION_TOKEN_LENGTH, Settings


def test_default_host_is_loopback() -> None:
    settings = Settings(session_token="x" * MIN_SESSION_TOKEN_LENGTH)

    assert settings.host == "127.0.0.1"


def test_default_port_is_documented_constant() -> None:
    settings = Settings(session_token="x" * MIN_SESSION_TOKEN_LENGTH)

    assert settings.port == DEFAULT_PORT


@pytest.mark.parametrize("host", ["127.0.0.1", "::1"])
def test_loopback_hosts_are_accepted(host: str) -> None:
    settings = Settings(host=host, session_token="x" * MIN_SESSION_TOKEN_LENGTH)

    assert settings.host == host


@pytest.mark.parametrize(
    "host",
    [
        "0.0.0.0",  # all interfaces -- exactly what must never be permitted
        "::",  # IPv6 all-interfaces equivalent
        "192.168.1.5",  # a real, non-loopback LAN address
        "10.0.0.1",
        "localhost",  # a hostname, not a literal loopback address
        "example.com",
        "not-an-ip",
        "",
    ],
)
def test_non_loopback_or_non_literal_hosts_are_rejected(host: str) -> None:
    with pytest.raises(ValidationError):
        Settings(host=host, session_token="x" * MIN_SESSION_TOKEN_LENGTH)


@pytest.mark.parametrize("port", [0, -1, 65536, 100000])
def test_out_of_range_ports_are_rejected(port: int) -> None:
    with pytest.raises(ValidationError):
        Settings(port=port, session_token="x" * MIN_SESSION_TOKEN_LENGTH)


@pytest.mark.parametrize("port", [1, 1024, 65535])
def test_boundary_valid_ports_are_accepted(port: int) -> None:
    settings = Settings(port=port, session_token="x" * MIN_SESSION_TOKEN_LENGTH)

    assert settings.port == port


def test_environment_rejects_unknown_values() -> None:
    with pytest.raises(ValidationError):
        Settings(environment="staging", session_token="x" * MIN_SESSION_TOKEN_LENGTH)


def test_default_environment_is_production() -> None:
    settings = Settings(session_token="x" * MIN_SESSION_TOKEN_LENGTH)

    assert settings.environment == "production"


def test_no_credential_configured_by_default_when_unset() -> None:
    settings = Settings(session_token=None)

    assert settings.has_valid_credential_configured is False


def test_credential_below_minimum_length_is_not_valid() -> None:
    settings = Settings(session_token="x" * (MIN_SESSION_TOKEN_LENGTH - 1))

    assert settings.has_valid_credential_configured is False


def test_credential_at_minimum_length_is_valid() -> None:
    settings = Settings(session_token="x" * MIN_SESSION_TOKEN_LENGTH)

    assert settings.has_valid_credential_configured is True


def test_settings_reads_from_environment_variables(monkeypatch: pytest.MonkeyPatch) -> None:
    """The documented standalone-mode configuration path (S1-03): env vars,
    not CLI/Tauri-delivered secrets."""
    monkeypatch.setenv("EVIDENCEGRAPH_HOST", "127.0.0.1")
    monkeypatch.setenv("EVIDENCEGRAPH_PORT", "9000")
    monkeypatch.setenv("EVIDENCEGRAPH_ENVIRONMENT", "development")
    monkeypatch.setenv("EVIDENCEGRAPH_SESSION_TOKEN", "env-supplied-token-1234567890")

    settings = Settings()

    assert settings.host == "127.0.0.1"
    assert settings.port == 9000
    assert settings.environment == "development"
    assert settings.has_valid_credential_configured is True


def test_settings_repr_never_exposes_the_raw_token_value() -> None:
    token = "super-secret-value-should-not-appear"
    settings = Settings(session_token=token)

    assert token not in repr(settings)
    assert token not in str(settings)
