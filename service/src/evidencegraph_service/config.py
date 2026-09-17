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

import re
from typing import Literal

from pydantic import Field, SecretStr, field_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

#: Below this length, a configured token is treated as invalid/absent
#: configuration (fail closed) rather than a usable credential -- guards
#: against a trivially-guessable value being installed by mistake.
MIN_SESSION_TOKEN_LENGTH = 16

#: The only accepted standalone credential shape: unpadded Base64URL
#: (RFC 4648 SS5) -- `A-Z a-z 0-9 - _` only. This is a strict subset of the
#: `token68` alphabet from RFC 9110 SS11.3, so a value that passes this
#: check always round-trips cleanly through an `Authorization: Bearer <..>`
#: header. It's also exactly the alphabet `secrets.token_urlsafe()`
#: produces (see `service/README.md` for the generation command), so a
#: cryptographically random credential is valid by construction -- no
#: non-ASCII character, space, or control character ever matches.
_TOKEN_ALPHABET_PATTERN = re.compile(r"^[A-Za-z0-9_-]+$")

#: The literal, documented S1-03 bind address (`docs/SPRINT_1_BACKLOG.md`
#: S1-03, `docs/ARCHITECTURE.md` SS4, `docs/SECURITY.md` T-09). Exactly this
#: string -- not any other loopback form (`::1`, `127.x.x.x`, IPv4-mapped
#: IPv6, `localhost`, ...) -- is accepted; broadening this is an
#: architecture-level decision (`docs/AGENT_INSTRUCTIONS.md` rule 13), not a
#: routine implementation choice for this ticket.
_REQUIRED_HOST = "127.0.0.1"

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
    address. A missing/too-short/malformed session token is a different,
    expected case (the standalone process legitimately starts before a
    credential is installed) and fails closed a different way: every
    request is rejected by the auth middleware instead (see `auth.py`,
    `has_valid_credential_configured`).
    """

    model_config = SettingsConfigDict(env_prefix="EVIDENCEGRAPH_", extra="ignore")

    host: str = _REQUIRED_HOST
    port: int = Field(default=DEFAULT_PORT, ge=1, le=65535)
    environment: Literal["development", "production"] = "production"
    session_token: SecretStr | None = None

    @field_validator("host")
    @classmethod
    def _require_documented_bind_address(cls, value: str) -> str:
        if value != _REQUIRED_HOST:
            raise ValueError(
                f"host must be exactly {_REQUIRED_HOST!r} -- the documented S1-03 bind "
                "contract (docs/SPRINT_1_BACKLOG.md S1-03, docs/ARCHITECTURE.md SS4, "
                "docs/SECURITY.md T-09). No other loopback form (::1, 127.x.x.x, an "
                "IPv4-mapped IPv6 loopback, localhost), wildcard/unspecified address "
                "(0.0.0.0, ::), or LAN/public address is permitted here "
                "(docs/AGENT_INSTRUCTIONS.md rule 13)."
            )
        return value

    @property
    def has_valid_credential_configured(self) -> bool:
        """True only if a session token is set, long enough, and header-safe.

        "Header-safe" means it matches `_TOKEN_ALPHABET_PATTERN` (unpadded
        Base64URL ASCII) -- see that constant's docstring for why. Does not
        compare against any request -- see `auth.py` for the actual
        constant-time credential check. Never logs or raises on an invalid
        value; an unusable credential simply causes every request to be
        rejected, the same fail-closed behavior as no credential at all.
        """
        if self.session_token is None:
            return False
        raw = self.session_token.get_secret_value()
        return (
            len(raw) >= MIN_SESSION_TOKEN_LENGTH
            and _TOKEN_ALPHABET_PATTERN.fullmatch(raw) is not None
        )
