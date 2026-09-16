"""GET /health -- the sole endpoint in the S1-03 skeleton.

Authenticated like every other route (see `auth.py`): there is no
exemption for health/readiness in this architecture
(`docs/DECISIONS.md` D-009). The response body is intentionally minimal
and contains no configuration/environment detail.
"""

from __future__ import annotations

from fastapi import APIRouter

router = APIRouter()

SERVICE_NAME = "evidencegraph-service"
SERVICE_VERSION = "0.1.0"


@router.get("/health")
async def health() -> dict[str, str]:
    return {"status": "ok", "service": SERVICE_NAME, "version": SERVICE_VERSION}
