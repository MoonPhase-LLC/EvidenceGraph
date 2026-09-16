from __future__ import annotations

from collections.abc import Callable

import pytest
from fastapi.testclient import TestClient

from .conftest import VALID_TOKEN

# Every path a request could plausibly target, to demonstrate the auth
# middleware has no allowlist/exemption -- including a path this service
# never defines, and the root path.
PROTECTED_PATHS = ["/health", "/", "/nonexistent-path-should-still-require-auth"]


@pytest.mark.parametrize("path", PROTECTED_PATHS)
def test_missing_authorization_header_is_rejected(client: TestClient, path: str) -> None:
    response = client.get(path)

    assert response.status_code == 401
    assert response.json() == {"detail": "Unauthorized"}


def test_incorrect_token_is_rejected(client: TestClient) -> None:
    response = client.get(
        "/health", headers={"Authorization": "Bearer wrong-token-entirely-different"}
    )

    assert response.status_code == 401
    assert response.json() == {"detail": "Unauthorized"}


@pytest.mark.parametrize(
    "header_value",
    [
        "",  # empty header value
        VALID_TOKEN,  # missing "Bearer " scheme entirely
        f"bearer {VALID_TOKEN}",  # wrong case scheme
        f"Basic {VALID_TOKEN}",  # wrong scheme
        "Bearer",  # scheme with no token at all
        "Bearer ",  # scheme with only whitespace as the token
        f"Bearer  {VALID_TOKEN}",  # double space (token becomes " <token>")
    ],
)
def test_malformed_authorization_header_is_rejected(client: TestClient, header_value: str) -> None:
    response = client.get("/health", headers={"Authorization": header_value})

    assert response.status_code == 401
    assert response.json() == {"detail": "Unauthorized"}


def test_valid_token_is_accepted(client: TestClient, auth_headers: dict[str, str]) -> None:
    response = client.get("/health", headers=auth_headers)

    assert response.status_code == 200


def test_missing_credential_configuration_fails_closed_without_token(
    make_client: Callable[..., TestClient],
) -> None:
    client = make_client(session_token=None)

    response = client.get("/health")

    assert response.status_code == 401
    assert response.json() == {"detail": "Unauthorized"}


def test_missing_credential_configuration_fails_closed_even_with_a_supplied_token(
    make_client: Callable[..., TestClient],
) -> None:
    client = make_client(session_token=None)

    # A client guessing/reusing *some* token still cannot authenticate,
    # because no valid credential is installed at all -- this is the
    # explicit S1-03 acceptance criterion, not merely "no header supplied".
    response = client.get("/health", headers={"Authorization": f"Bearer {VALID_TOKEN}"})

    assert response.status_code == 401
    assert response.json() == {"detail": "Unauthorized"}


def test_too_short_configured_token_is_treated_as_no_credential_configured(
    make_client: Callable[..., TestClient],
) -> None:
    short_token = "short"  # below MIN_SESSION_TOKEN_LENGTH
    client = make_client(session_token=short_token)

    response = client.get("/health", headers={"Authorization": f"Bearer {short_token}"})

    assert response.status_code == 401


def test_rejection_responses_never_echo_the_configured_token(
    client: TestClient,
) -> None:
    for headers in (
        {},
        {"Authorization": "Bearer wrong"},
        {"Authorization": "not-bearer-scheme"},
    ):
        response = client.get("/health", headers=headers)
        assert VALID_TOKEN not in response.text


def test_all_rejection_reasons_produce_identical_response_bodies(
    client: TestClient,
) -> None:
    """No oracle: a caller cannot distinguish missing vs malformed vs wrong
    token from the response alone."""
    responses = [
        client.get("/health"),
        client.get("/health", headers={"Authorization": "Bearer wrong-token"}),
        client.get("/health", headers={"Authorization": "NotBearer x"}),
        client.get("/health", headers={"Authorization": "Bearer"}),
    ]

    bodies = {response.text for response in responses}
    statuses = {response.status_code for response in responses}

    assert statuses == {401}
    assert bodies == {'{"detail":"Unauthorized"}'}
