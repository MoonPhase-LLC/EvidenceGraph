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
        f"Basic {VALID_TOKEN}",  # wrong (but well-formed) scheme
        f"Bearerx {VALID_TOKEN}",  # scheme is not exactly "bearer"
        "Bearer",  # scheme with no token at all
        "Bearer ",  # scheme with only whitespace as the token
        "Bearer   ",  # scheme with only whitespace (several spaces) as the token
        f"Bearer\t{VALID_TOKEN}",  # tab is not a permitted separator (RFC 9110 SS11.4: 1*SP only)
        f"Bearer\n{VALID_TOKEN}",  # newline is not a permitted separator either
    ],
)
def test_malformed_authorization_header_is_rejected(client: TestClient, header_value: str) -> None:
    response = client.get("/health", headers={"Authorization": header_value})

    assert response.status_code == 401
    assert response.json() == {"detail": "Unauthorized"}


def test_valid_token_is_accepted(client: TestClient, auth_headers: dict[str, str]) -> None:
    response = client.get("/health", headers=auth_headers)

    assert response.status_code == 200


# RFC 9110 SS11.1: auth-scheme is a `token`, and tokens are matched
# case-insensitively -- "Bearer", "bearer", "BEARER", and any mixed case
# must all authenticate successfully when the credentials are correct.
@pytest.mark.parametrize("scheme", ["Bearer", "bearer", "BEARER", "BeArEr", "bEARER"])
def test_bearer_scheme_is_matched_case_insensitively(client: TestClient, scheme: str) -> None:
    response = client.get("/health", headers={"Authorization": f"{scheme} {VALID_TOKEN}"})

    assert response.status_code == 200


# RFC 9110 SS11.4 / SS11.6.2: `credentials = auth-scheme [1*SP (token68 /
# #auth-param)]` -- one-or-more SP characters separate the scheme from the
# credentials, not exactly one.
@pytest.mark.parametrize("space_count", [1, 2, 3, 8])
def test_multiple_separating_spaces_are_accepted(client: TestClient, space_count: int) -> None:
    header_value = "Bearer" + (" " * space_count) + VALID_TOKEN

    response = client.get("/health", headers={"Authorization": header_value})

    assert response.status_code == 200


def test_case_and_spacing_variants_combine_successfully(client: TestClient) -> None:
    response = client.get("/health", headers={"Authorization": f"bEaReR   {VALID_TOKEN}"})

    assert response.status_code == 200


@pytest.mark.parametrize(
    "header_value",
    [
        None,  # no Authorization header at all
        f"Bearer {VALID_TOKEN}wrong",  # correct scheme, incorrect token
        f"Basic {VALID_TOKEN}",  # unsupported scheme
        "Bearer",  # malformed: no credentials
    ],
)
def test_every_rejection_reason_includes_www_authenticate_bearer(
    client: TestClient, header_value: str | None
) -> None:
    headers = {"Authorization": header_value} if header_value is not None else {}

    response = client.get("/health", headers=headers)

    assert response.status_code == 401
    assert response.headers["www-authenticate"] == "Bearer"


def test_successful_request_status_and_body_unaffected_by_www_authenticate_change(
    client: TestClient, auth_headers: dict[str, str]
) -> None:
    response = client.get("/health", headers=auth_headers)

    assert response.status_code == 200
    assert "www-authenticate" not in {k.lower() for k in response.headers}


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
