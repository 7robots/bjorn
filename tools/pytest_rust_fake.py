"""pytest plugin: run the Python Bjorn's suite against the Rust fakes.

The Python conftest builds `BearClient([python, fake_bearcli.py])` and
`RemctlClient([python, fake_remctl.py])` directly. This plugin rewrites those
commands to the Rust `fake-bearcli` / `fake-remctl` binaries, so every test
that drives the fake as a subprocess proves the Rust fake honours the same
contract. The state files and their env vars are unchanged.

    cd ../bjorn
    PYTHONPATH=../bjorn-rust/tools uv run pytest -p pytest_rust_fake -q
"""

from __future__ import annotations

import os
from pathlib import Path

RUST_BIN = Path(os.environ.get("BJORN_RUST_BIN") or Path(__file__).resolve().parents[1] / "target" / "release")


def _swap(command, python_name: str, rust_name: str):
    parts = (command,) if isinstance(command, str) else tuple(command)
    if len(parts) >= 2 and str(parts[-1]).endswith(python_name):
        return (str(RUST_BIN / rust_name),)
    return parts


def pytest_configure(config):
    import bjorn.bear
    import bjorn.reminders

    bear_init = bjorn.bear.BearClient.__init__

    def bear_wrapped(self, command=bjorn.bear.DEFAULT_COMMAND):
        bear_init(self, _swap(command, "fake_bearcli.py", "fake-bearcli"))

    bjorn.bear.BearClient.__init__ = bear_wrapped

    remctl_init = bjorn.reminders.RemctlClient.__init__

    def remctl_wrapped(self, command=bjorn.reminders.DEFAULT_COMMAND):
        remctl_init(self, _swap(command, "fake_remctl.py", "fake-remctl"))

    bjorn.reminders.RemctlClient.__init__ = remctl_wrapped
    config.addinivalue_line("markers", "rust_fake: suite is running against the Rust fakes")
    print(f"\n[pytest_rust_fake] bearcli/remctl fakes -> {RUST_BIN}")
