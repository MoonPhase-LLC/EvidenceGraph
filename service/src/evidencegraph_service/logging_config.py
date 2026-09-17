"""Minimal structured (JSON-lines) logging setup.

Stdlib-only (no new dependency for this). Callers pass extra context via
the standard `logging` `extra={...}` kwarg; only non-reserved keys are
merged into the emitted record. Per `docs/SECURITY.md` T-10 and
`docs/AGENT_INSTRUCTIONS.md` rule 14, nothing in this codebase ever passes
a credential/token value as a log message or `extra` field -- callers pass
short category strings (e.g. "token_mismatch"), never secret content.
"""

from __future__ import annotations

import json
import logging

_RESERVED_RECORD_KEYS = frozenset(logging.LogRecord("", 0, "", 0, "", (), None).__dict__) | {
    "message",
    "asctime",
}


class JsonLogFormatter(logging.Formatter):
    def format(self, record: logging.LogRecord) -> str:
        payload: dict[str, object] = {
            "timestamp": self.formatTime(record, "%Y-%m-%dT%H:%M:%S%z"),
            "level": record.levelname,
            "logger": record.name,
            "message": record.getMessage(),
        }
        for key, value in record.__dict__.items():
            if key not in _RESERVED_RECORD_KEYS:
                payload[key] = value
        if record.exc_info:
            payload["exc_type"] = record.exc_info[0].__name__ if record.exc_info[0] else None
        return json.dumps(payload, default=str)


def configure_logging(level: int = logging.INFO) -> None:
    handler = logging.StreamHandler()
    handler.setFormatter(JsonLogFormatter())
    root = logging.getLogger()
    root.handlers = [handler]
    root.setLevel(level)
