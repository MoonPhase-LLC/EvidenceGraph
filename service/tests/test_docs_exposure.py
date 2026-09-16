from __future__ import annotations

from collections.abc import Callable

import pytest
from fastapi.testclient import TestClient

DOC_PATHS = ["/docs", "/redoc", "/openapi.json"]


@pytest.mark.parametrize("path", DOC_PATHS)
def test_docs_not_registered_in_production_even_with_valid_auth(
    make_client: Callable[..., TestClient],
    auth_headers: dict[str, str],
    path: str,
) -> None:
    """Proves the route genuinely doesn't exist in production mode, not
    merely that it's blocked by auth: a valid token is supplied, and it
    must still 404."""
    client = make_client(environment="production")

    response = client.get(path, headers=auth_headers)

    assert response.status_code == 404


@pytest.mark.parametrize("path", DOC_PATHS)
def test_docs_unreachable_in_production_without_auth_either(
    make_client: Callable[..., TestClient], path: str
) -> None:
    client = make_client(environment="production")

    response = client.get(path)

    assert response.status_code == 401


@pytest.mark.parametrize("path", DOC_PATHS)
def test_docs_are_registered_in_development_mode_with_valid_auth(
    make_client: Callable[..., TestClient],
    auth_headers: dict[str, str],
    path: str,
) -> None:
    client = make_client(environment="development")

    response = client.get(path, headers=auth_headers)

    assert response.status_code == 200


@pytest.mark.parametrize("path", DOC_PATHS)
def test_docs_still_require_auth_in_development_mode(
    make_client: Callable[..., TestClient], path: str
) -> None:
    client = make_client(environment="development")

    response = client.get(path)

    assert response.status_code == 401
