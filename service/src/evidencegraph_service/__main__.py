"""`python -m evidencegraph_service` entry point.

Delegates to the exact same `main()` the `evidencegraph-service` console
script uses (see `pyproject.toml` `[project.scripts]`), so the two
advertised entry points can never diverge in startup/configuration
behavior -- there is only one implementation of `main()`.
"""

from __future__ import annotations

from .main import main

if __name__ == "__main__":
    main()
