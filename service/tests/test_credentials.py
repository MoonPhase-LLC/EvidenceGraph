from __future__ import annotations

import threading

from evidencegraph_service.credentials import CredentialStore

VALID_TOKEN = "test-only-session-token-abcdef12"
OTHER_VALID_TOKEN = "another-valid-token-zyxwvu9876"


def test_empty_store_has_nothing_installed() -> None:
    store = CredentialStore()
    assert store.is_installed is False
    assert store.matches(VALID_TOKEN) is False


def test_install_valid_token_succeeds() -> None:
    store = CredentialStore()
    assert store.install(VALID_TOKEN) is True
    assert store.is_installed is True
    assert store.matches(VALID_TOKEN) is True
    assert store.matches("wrong-token-entirely-different") is False


def test_install_rejects_malformed_token_and_stays_empty() -> None:
    store = CredentialStore()
    assert store.install("too short") is False
    assert store.is_installed is False
    assert store.matches("too short") is False


def test_install_is_one_time_only() -> None:
    """A second `install()` call -- even with a different, otherwise-valid
    token -- never replaces the first. This is what makes a restarted
    child's fresh (new-process) store the only way to get a new token in;
    nothing can rotate the credential of a running instance."""
    store = CredentialStore()
    assert store.install(VALID_TOKEN) is True
    assert store.install(OTHER_VALID_TOKEN) is False

    assert store.matches(VALID_TOKEN) is True
    assert store.matches(OTHER_VALID_TOKEN) is False


def test_preinstalled_from_valid_token() -> None:
    store = CredentialStore.preinstalled(VALID_TOKEN)
    assert store.is_installed is True
    assert store.matches(VALID_TOKEN) is True


def test_preinstalled_from_none_is_empty() -> None:
    store = CredentialStore.preinstalled(None)
    assert store.is_installed is False


def test_preinstalled_from_invalid_token_is_empty() -> None:
    store = CredentialStore.preinstalled("too short")
    assert store.is_installed is False


def test_concurrent_install_attempts_only_one_wins() -> None:
    """Whitebox concurrency check: many threads racing `install()` with
    distinct valid tokens must result in exactly one being installed --
    the lock must make the check-then-set atomic, not just individually
    thread-safe reads/writes."""
    store = CredentialStore()
    tokens = [f"race-token-{i:03d}-abcdefghijklmnop" for i in range(50)]
    results: list[bool] = [False] * len(tokens)

    def _attempt(i: int) -> None:
        results[i] = store.install(tokens[i])

    threads = [threading.Thread(target=_attempt, args=(i,)) for i in range(len(tokens))]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    assert sum(results) == 1
    winning_token = tokens[results.index(True)]
    assert store.matches(winning_token) is True
    for token in tokens:
        if token != winning_token:
            assert store.matches(token) is False
