from __future__ import annotations

from fastapi.testclient import TestClient

from evidencegraph_service import b64url
from evidencegraph_service.app import create_app
from evidencegraph_service.challenge import StartupChallengeState
from evidencegraph_service.config import Settings
from evidencegraph_service.credentials import CredentialStore
from evidencegraph_service.routes.challenge import CHALLENGE_PATH

SECRET = bytes(range(32))
HOST = "127.0.0.1"
PORT = 51900


def _supervised_client() -> tuple[TestClient, StartupChallengeState, CredentialStore]:
    settings = Settings(host=HOST, port=PORT, session_token=None)
    store = CredentialStore()
    challenge_state = StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    app = create_app(
        settings,
        credential_store=store,
        startup_challenge=challenge_state,
        supervised=True,
    )
    return TestClient(app), challenge_state, store


def _valid_nonce() -> str:
    return b64url.encode(bytes(range(16)))


def test_challenge_route_not_registered_in_standalone_mode() -> None:
    """Proves the route genuinely doesn't exist in standalone mode, not
    merely that it's blocked by auth: a request carrying a *valid* token
    still 404s -- mirrors `test_docs_exposure.py`'s pattern."""
    token = "x" * 32
    settings = Settings(host=HOST, port=PORT, session_token=token)
    app = create_app(settings)
    client = TestClient(app)

    with_valid_token = client.post(
        CHALLENGE_PATH,
        json={"nonce": _valid_nonce()},
        headers={"Authorization": f"Bearer {token}"},
    )
    without_token = client.post(CHALLENGE_PATH, json={"nonce": _valid_nonce()})

    assert with_valid_token.status_code == 404
    assert without_token.status_code == 401


def test_successful_challenge_returns_response_and_closes() -> None:
    client, state, _ = _supervised_client()

    response = client.post(CHALLENGE_PATH, json={"nonce": _valid_nonce()})

    assert response.status_code == 200
    body = response.json()
    assert isinstance(body.get("response"), str)
    # Decodes cleanly as Base64URL without raising -- shape check only,
    # since verifying the *value* is `challenge.py`'s own test's job.
    b64url.decode(body["response"], field="response")
    assert state.is_open is False


def test_challenge_route_closed_after_one_success() -> None:
    """Without any credential installed, a second POST is rejected by the
    auth middleware itself (401) before it can even reach the route --
    correct: closed-and-unauthenticated must look identical to every other
    unauthenticated path, not leak "this path exists" via a 404."""
    client, state, _ = _supervised_client()

    first = client.post(CHALLENGE_PATH, json={"nonce": _valid_nonce()})
    second = client.post(CHALLENGE_PATH, json={"nonce": _valid_nonce()})

    assert first.status_code == 200
    assert second.status_code == 401
    assert state.succeeded is True


TOKEN = "route-closure-test-token-abcdefg"


def test_challenge_route_still_closed_even_with_a_valid_bearer_token() -> None:
    """The route handler independently re-checks `is_open`, not just
    relying on the middleware exemption: once a credential is installed
    (as `supervised.py` does right after a successful challenge) and
    presented correctly, routing/auth would otherwise let this request
    through -- but the handler itself still refuses to compute a second
    response."""
    client, state, store = _supervised_client()
    client.post(CHALLENGE_PATH, json={"nonce": _valid_nonce()})
    store.install(TOKEN)
    assert state.is_open is False

    response = client.post(
        CHALLENGE_PATH,
        json={"nonce": _valid_nonce()},
        headers={"Authorization": f"Bearer {TOKEN}"},
    )

    assert response.status_code == 404


def test_closing_the_state_directly_also_closes_the_route() -> None:
    """An explicit `close()` (as `supervised.py` calls after credential
    install) takes effect immediately -- even for a request presenting a
    valid, installed credential."""
    client, state, store = _supervised_client()
    store.install(TOKEN)
    state.close()

    response = client.post(
        CHALLENGE_PATH,
        json={"nonce": _valid_nonce()},
        headers={"Authorization": f"Bearer {TOKEN}"},
    )

    assert response.status_code == 404


def test_malformed_json_body_rejected_without_consuming_success() -> None:
    client, state, _ = _supervised_client()

    response = client.post(
        CHALLENGE_PATH, content=b"not json", headers={"Content-Type": "application/json"}
    )

    assert response.status_code == 400
    assert state.is_open is True  # malformed body never reaches compute_response
    assert state.succeeded is False


def test_missing_nonce_field_rejected() -> None:
    client, _, _ = _supervised_client()

    response = client.post(CHALLENGE_PATH, json={})

    assert response.status_code == 400


def test_non_string_nonce_rejected() -> None:
    client, _, _ = _supervised_client()

    response = client.post(CHALLENGE_PATH, json={"nonce": 12345})

    assert response.status_code == 400


def test_oversized_body_rejected() -> None:
    client, _, _ = _supervised_client()

    response = client.post(CHALLENGE_PATH, json={"nonce": "A" * 10_000})

    assert response.status_code == 400


def test_challenge_response_never_leaks_the_startup_secret() -> None:
    client, _, _ = _supervised_client()

    response = client.post(CHALLENGE_PATH, json={"nonce": _valid_nonce()})

    assert b64url.encode(SECRET) not in response.text


def test_challenge_route_requires_no_authorization_header() -> None:
    """The one deliberate pre-authentication exemption: a request with no
    credential at all still reaches the handler while the challenge is
    open (that's the whole point -- there's no session token to present
    yet at this stage of the handshake)."""
    client, _, _ = _supervised_client()

    response = client.post(CHALLENGE_PATH, json={"nonce": _valid_nonce()})

    assert response.status_code == 200


def test_every_other_route_still_requires_auth_in_supervised_mode_before_install() -> None:
    client, _, _ = _supervised_client()

    response = client.get("/health")

    assert response.status_code == 401
