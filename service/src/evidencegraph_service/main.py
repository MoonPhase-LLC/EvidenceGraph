"""Standalone process entry point.

`python -m evidencegraph_service` or the `evidencegraph-service` console
script. Reads `Settings` from the environment (see `config.py`), fails
closed at startup if the host/port configuration is structurally invalid,
and otherwise starts serving -- including with no session token configured
yet, which is the normal pre-S1-04 state (every request gets 401 until one
is set).
"""

from __future__ import annotations

import logging
import sys

import uvicorn
from pydantic import ValidationError

from .app import create_app
from .config import Settings
from .logging_config import configure_logging

logger = logging.getLogger("evidencegraph_service")


def main() -> None:
    configure_logging()

    try:
        settings = Settings()
    except ValidationError as exc:
        # Startup-only, operator-facing diagnostic on stderr -- never an
        # HTTP response. Configuration values (host/port/environment) are
        # not secrets; the session token is a `SecretStr` and pydantic
        # never renders its value in this output.
        print(f"evidencegraph-service: invalid configuration:\n{exc}", file=sys.stderr)
        raise SystemExit(1) from exc

    app = create_app(settings)

    logger.info(
        "starting",
        extra={
            "host": settings.host,
            "port": settings.port,
            "environment": settings.environment,
            "credential_configured": settings.has_valid_credential_configured,
        },
    )

    uvicorn.run(app, host=settings.host, port=settings.port, log_level="warning")


if __name__ == "__main__":
    main()
