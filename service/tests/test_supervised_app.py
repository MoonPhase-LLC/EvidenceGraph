"""In-process (TestClient) coverage of the supervised-mode `create_app`
wiring: credential installed *after* the app is already serving, the
challenge route's method/path scoping, and standalone-mode's continued
zero-exemption behavior. The real end-to-end private-channel handshake
against an actual subprocess is `test_supervised_process.py`.
"""

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
TOKEN = "supervised-test-token-abcdefghi"


def _make() -> tuple[TestClient, CredentialStore, StartupChallengeState]:
    settings = Settings(host=HOST, port=PORT, session_token=None)
    store = CredentialStore()
    challenge_state = StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    app = create_app(
        settings,
        credential_store=store,
        startup_challenge=challenge_state,
        supervised=True,
    )
    return TestClient(app), store, challenge_state


def test_health_is_401_before_credential_installed() -> None:
    client, _, _ = _make()

    response = client.get("/health")

    assert response.status_code == 401


def test_health_succeeds_once_credential_is_installed_at_runtime() -> None:
    """The whole point of `CredentialStore`: install happens well after
    `create_app`/`TestClient` construction, simulating what the
    private-channel protocol thread does in the real process."""
    client, store, _ = _make()

    assert store.install(TOKEN) is True

    response = client.get("/health", headers={"Authorization": f"Bearer {TOKEN}"})
    assert response.status_code == 200


def test_wrong_token_still_401_after_install() -> None:
    client, store, _ = _make()
    store.install(TOKEN)

    response = client.get("/health", headers={"Authorization": "Bearer wrong-value-entirely"})

    assert response.status_code == 401


def test_challenge_exemption_does_not_apply_to_get() -> None:
    """Only `POST <challenge_path>` is exempt -- a GET to the same path is
    treated like any other unauthenticated request."""
    client, _, _ = _make()

    response = client.get(CHALLENGE_PATH)

    assert response.status_code == 401


def test_challenge_exemption_does_not_apply_to_other_paths() -> None:
    client, _, _ = _make()

    response = client.post("/health", json={"nonce": b64url.encode(bytes(range(16)))})

    assert response.status_code == 401


def test_get_to_challenge_path_with_valid_token_is_405_not_200() -> None:
    """With auth satisfied, routing takes over: only POST is registered for
    this path, so GET must be method-not-allowed, not silently accepted."""
    client, store, _ = _make()
    store.install(TOKEN)

    response = client.get(CHALLENGE_PATH, headers={"Authorization": f"Bearer {TOKEN}"})

    assert response.status_code == 405


def test_challenge_still_exempt_even_with_a_bogus_authorization_header() -> None:
    """The exemption is unconditional while open -- presence of a bad
    Authorization header must not accidentally route this request through
    the normal auth-rejection path instead of the challenge handler."""
    client, _, _ = _make()

    response = client.post(
        CHALLENGE_PATH,
        json={"nonce": b64url.encode(bytes(range(16)))},
        headers={"Authorization": "Bearer some-wrong-token-value-here"},
    )

    assert response.status_code == 200
