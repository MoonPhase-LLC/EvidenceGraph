from __future__ import annotations

from fastapi.testclient import TestClient

from .conftest import VALID_TOKEN


def test_health_with_valid_credential_returns_200_and_expected_shape(
    client: TestClient, auth_headers: dict[str, str]
) -> None:
    response = client.get("/health", headers=auth_headers)

    assert response.status_code == 200
    body = response.json()
    assert body == {
        "status": "ok",
        "service": "evidencegraph-service",
        "version": "0.1.0",
    }


def test_health_has_no_authentication_exemption(client: TestClient) -> None:
    response = client.get("/health")

    assert response.status_code == 401
    assert VALID_TOKEN not in response.text
