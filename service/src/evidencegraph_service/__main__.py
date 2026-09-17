"""`python -m evidencegraph_service` entry point.

Two modes, dispatched purely on the presence of a `--supervised` CLI flag
(never a secret -- "environment variables and CLI arguments must not
contain the startup secret or session token", S1-04):

- No flag: delegates to the exact same `main()` the `evidencegraph-service`
  console script uses (see `pyproject.toml` `[project.scripts]`), so the
  standalone entry points can never diverge in startup/configuration
  behavior -- there is only one implementation of standalone `main()`.
- `--supervised`: delegates to `supervised.main()` (S1-04) -- the mode
  Tauri actually launches, via an explicit absolute path to this same
  module, never through PATH resolution or a shell.
"""

from __future__ import annotations

import sys


def _dispatch() -> None:
    if "--supervised" in sys.argv[1:]:
        from .supervised import main as supervised_main

        supervised_main()
    else:
        from .main import main as standalone_main

        standalone_main()


if __name__ == "__main__":
    _dispatch()
