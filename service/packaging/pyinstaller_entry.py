"""PyInstaller entry point for the bundled service executable (S1-09).

PyInstaller freezes a script, not a `python -m` module invocation, so this
file stands in for `python -m evidencegraph_service`. It calls the same
dispatcher, so the packaged `evidencegraph-service.exe --supervised` runs
exactly the code the development launch (`python -m evidencegraph_service
--supervised`) runs. See `docs/PACKAGING.md`.
"""

from evidencegraph_service.__main__ import _dispatch

if __name__ == "__main__":
    _dispatch()
