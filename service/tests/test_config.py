from __future__ import annotations

import secrets

import pytest
from pydantic import ValidationError

from evidencegraph_service.config import DEFAULT_PORT, MIN_SESSION_TOKEN_LENGTH, Settings


def test_default_host_is_loopback() -> None:
    settings = Settings(session_token="x" * MIN_SESSION_TOKEN_LENGTH)

    assert settings.host == "127.0.0.1"


def test_default_port_is_documented_constant() -> None:
    settings = Settings(session_token="x" * MIN_SESSION_TOKEN_LENGTH)

    assert settings.port == DEFAULT_PORT


def test_the_documented_bind_address_is_accepted() -> None:
    settings = Settings(host="127.0.0.1", session_token="x" * MIN_SESSION_TOKEN_LENGTH)

    assert settings.host == "127.0.0.1"


@pytest.mark.parametrize(
    "host",
    [
        "0.0.0.0",  # all interfaces -- exactly what must never be permitted
        "::",  # IPv6 all-interfaces equivalent
        "::1",  # a real loopback address, but not the one S1-03 documents
        "127.0.0.2",  # another 127/8 address -- still not the documented one
        "127.1.1.1",
        "127.1",  # alternate short textual form of a 127/8 address
        "::ffff:127.0.0.1",  # IPv4-mapped IPv6 loopback form
        "192.168.1.5",  # a real, non-loopback LAN address
        "10.0.0.1",
        "8.8.8.8",  # a public address
        "localhost",  # a hostname, not a literal loopback address
        "example.com",
        "not-an-ip",
        "127.0.0.1 ",  # trailing whitespace -- not an exact match
        " 127.0.0.1",  # leading whitespace -- not an exact match
        "127.0.0.1.",  # trailing dot -- not an exact match
        "",
    ],
)
def test_every_address_other_than_the_documented_one_is_rejected(host: str) -> None:
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


@pytest.mark.parametrize(
    "token",
    [
        "café-secret-value-1234",  # non-ASCII letter
        "secret-—-value-1234",  # non-ASCII punctuation (em dash)
        "ñot-ascii-1234567890",  # leading non-ASCII character
    ],
)
def test_non_ascii_credential_is_not_valid(token: str) -> None:
    settings = Settings(session_token=token)

    assert settings.has_valid_credential_configured is False


@pytest.mark.parametrize(
    "token",
    [
        "has a space in it!!",  # embedded space
        " leading-space-1234567",
        "trailing-space-1234567 ",
        "tab\tcharacter-1234567890",
        "newline\ncharacter-1234567",
    ],
)
def test_credential_with_whitespace_is_not_valid(token: str) -> None:
    settings = Settings(session_token=token)

    assert settings.has_valid_credential_configured is False


@pytest.mark.parametrize(
    "token",
    [
        "control-char-\x00-1234567",  # NUL
        "control-char-\x01-1234567",  # SOH
        "control-char-\x7f-1234567",  # DEL
        "control-char-\x1b-1234567",  # ESC
    ],
)
def test_credential_with_control_characters_is_not_valid(token: str) -> None:
    settings = Settings(session_token=token)

    assert settings.has_valid_credential_configured is False


@pytest.mark.parametrize(
    "token",
    [
        # Exactly the alphabet `secrets.token_urlsafe()` produces: unpadded
        # Base64URL (A-Z a-z 0-9 - _).
        "kQ9f2vX7ZpL3mN8wR1sT4uY6bC0aD5e-",
        secrets.token_urlsafe(32),
        secrets.token_urlsafe(24),
        "A" * MIN_SESSION_TOKEN_LENGTH,  # boundary: minimum length, valid alphabet
    ],
)
def test_valid_generated_style_credential_is_accepted(token: str) -> None:
    settings = Settings(session_token=token)

    assert settings.has_valid_credential_configured is True


def test_credential_with_equals_padding_is_not_valid() -> None:
    """Base64 padding (`=`) is intentionally not part of the accepted
    alphabet -- `secrets.token_urlsafe()` never produces it, so requiring
    its absence doesn't reject any legitimately generated credential."""
    settings = Settings(session_token="valid-looking-value-but==")

    assert settings.has_valid_credential_configured is False


def test_malformed_credential_repr_never_exposes_the_raw_value() -> None:
    token = "not ascii é and has spaces"
    settings = Settings(session_token=token)

    assert settings.has_valid_credential_configured is False
    assert token not in repr(settings)
    assert token not in str(settings)


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
