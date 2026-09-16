"""Runtime configuration for the standalone service process.

Every field is resolved from `EVIDENCEGRAPH_*` environment variables (see
`docs/SPRINT_1_BACKLOG.md` S1-03: "an env-var ... configurable port with a
documented default is sufficient" for this ticket) or explicit constructor
kwargs, so tests can instantiate arbitrary configurations without touching
the real process environment.

In supervised (Tauri-launched) mode, S1-04 installs the session token after
this process has already started, over a private inherited pipe/handle
channel -- never via an environment variable read at startup
(`docs/DECISIONS.md` D-009/D-025). `EVIDENCEGRAPH_SESSION_TOKEN` here is
only the standalone/test entry point.
"""

from __future__ import annotations

import ipaddress
from typing import Literal

from pydantic import Field, SecretStr, field_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

#: Below this length, a configured token is treated as invalid/absent
#: configuration (fail closed) rather than a usable credential -- guards
#: against a trivially-guessable value being installed by mistake.
MIN_SESSION_TOKEN_LENGTH = 16

#: Arbitrary default chosen from the IANA dynamic/private port range
#: (49152-65535) to minimize collision with common development services
#: (3000, 5173, 8000, 8080, ...). Final allocation strategy for the
#: supervised (Tauri-launched) flow is D-018/D-025's OS-assigned port 0,
#: validated in S1-04/S1-09 -- see `docs/ARCHITECTURE.md` SS4.
DEFAULT_PORT = 51823


class Settings(BaseSettings):
    """Explicit, validated startup configuration.

    Invalid *structural* security configuration (a non-loopback host, a
    port outside the valid range) fails closed by refusing to construct
    `Settings` at all, so the process never starts listening on an unsafe
    address. A missing/too-short session token is a different, expected
    case (the standalone process legitimately starts before a credential
    is installed) and fails closed a different way: every request is
    rejected by the auth middleware instead (see `auth.py`).
    """

    model_config = SettingsConfigDict(env_prefix="EVIDENCEGRAPH_", extra="ignore")

    host: str = "127.0.0.1"
    port: int = Field(default=DEFAULT_PORT, ge=1, le=65535)
    environment: Literal["development", "production"] = "production"
    session_token: SecretStr | None = None

    @field_validator("host")
    @classmethod
    def _require_literal_loopback_address(cls, value: str) -> str:
        try:
            address = ipaddress.ip_address(value)
        except ValueError as exc:
            raise ValueError(
                "host must be a literal loopback IP address (e.g. 127.0.0.1), "
                "not a hostname -- refusing to resolve one at startup"
            ) from exc
        if not address.is_loopback:
            raise ValueError(
                "host must be a loopback address (127.0.0.1 or ::1); "
                "binding to a non-loopback address is never permitted here "
                "(docs/SECURITY.md T-09, docs/AGENT_INSTRUCTIONS.md rule 13)"
            )
        return value

    @property
    def has_valid_credential_configured(self) -> bool:
        """True only if a session token is set and meets the minimum length.

        Does not compare against any request -- see `auth.py` for the
        actual constant-time credential check.
        """
        if self.session_token is None:
            return False
        return len(self.session_token.get_secret_value()) >= MIN_SESSION_TOKEN_LENGTH
