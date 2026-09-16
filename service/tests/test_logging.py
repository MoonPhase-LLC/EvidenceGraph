from __future__ import annotations

import logging
from collections.abc import Callable

from fastapi.testclient import TestClient

from .conftest import VALID_TOKEN

WRONG_TOKEN = "this-is-a-wrong-but-plausible-token-value"


def _all_log_text(caplog: object) -> str:
    records = caplog.records  # type: ignore[attr-defined]
    chunks: list[str] = []
    for record in records:
        chunks.append(record.getMessage())
        for value in record.__dict__.values():
            chunks.append(str(value))
    return "\n".join(chunks)


def test_no_credential_configured_warning_never_logs_a_token_value(
    make_client: Callable[..., TestClient], caplog: object
) -> None:
    caplog.set_level(logging.INFO)  # type: ignore[attr-defined]

    make_client(session_token=None)

    log_text = _all_log_text(caplog)
    assert VALID_TOKEN not in log_text
    assert WRONG_TOKEN not in log_text


def test_auth_rejection_logging_never_includes_the_supplied_or_expected_token(
    client: TestClient, caplog: object
) -> None:
    caplog.set_level(logging.INFO)  # type: ignore[attr-defined]

    client.get("/health", headers={"Authorization": f"Bearer {WRONG_TOKEN}"})
    client.get("/health")
    client.get("/health", headers={"Authorization": "Malformed no-scheme-value"})

    log_text = _all_log_text(caplog)
    assert VALID_TOKEN not in log_text
    assert WRONG_TOKEN not in log_text


def test_successful_auth_does_not_log_the_token_either(
    client: TestClient, auth_headers: dict[str, str], caplog: object
) -> None:
    caplog.set_level(logging.INFO)  # type: ignore[attr-defined]

    response = client.get("/health", headers=auth_headers)

    assert response.status_code == 200
    log_text = _all_log_text(caplog)
    assert VALID_TOKEN not in log_text
