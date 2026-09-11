"""The Python Bjorn measured the way `bjorn-gate --bench` measures the Rust one.

    cd ~/GitHub/bjorn && .venv/bin/python ~/GitHub/bjorn-rust/tools/bench_python.py
"""

from __future__ import annotations

import asyncio
import sys
import time
from pathlib import Path

STARTED = time.perf_counter()
sys.path.insert(0, str(Path.home() / "GitHub" / "bjorn" / "src"))

from bjorn.app import BjornApp  # noqa: E402
from bjorn.bear import BearClient, resolve_bearcli  # noqa: E402
from bjorn.config import Config  # noqa: E402
from bjorn.render import preprocess  # noqa: E402


async def wait_until(pred, timeout: float = 60.0) -> None:
    loop = asyncio.get_event_loop()
    deadline = loop.time() + timeout
    while loop.time() < deadline:
        if pred():
            return
        await asyncio.sleep(0.02)
    raise SystemExit("timeout")


async def main() -> None:
    client = BearClient(resolve_bearcli(""))
    app = BjornApp(Config(poll_seconds=0, icon_style="none"), client=client, environ={})
    async with app.run_test(size=(140, 44)) as pilot:
        await wait_until(lambda: app.loaded and app.note_list.notes)
        await wait_until(lambda: app.note_view.note is not None)
        first_frame = time.perf_counter() - STARTED
        notes = len(app.snapshot.notes)

        fresh = BearClient(resolve_bearcli(""))
        t = time.perf_counter()
        snap = await fresh.snapshot()
        cold = time.perf_counter() - t
        t = time.perf_counter()
        await fresh.snapshot()
        warm = time.perf_counter() - t
        t = time.perf_counter()
        await fresh.probe()
        probe = time.perf_counter() - t

        longest = max((n for n in snap.notes if not n.locked), key=lambda n: n.length)
        content = await fresh.cat(longest.id)
        t = time.perf_counter()
        text = preprocess(content.content)
        render = time.perf_counter() - t
        t = time.perf_counter()
        await app.note_list.select_id(longest.id)
        await app.note_view.show(longest, content.content, max_lines=None)
        await pilot.pause()
        show = time.perf_counter() - t

        print("implementation=python")
        print(f"notes={notes}")
        print(f"first_frame_ms={first_frame * 1000:.0f}")
        print(f"cold_snapshot_ms={cold * 1000:.0f}")
        print(f"warm_snapshot_ms={warm * 1000:.0f}")
        print(f"probe_ms={probe * 1000:.0f}")
        print(f"longest_note_bytes={len(content.content.encode())} lines={len(content.content.splitlines())} rows={len(text.splitlines())}")
        print(f"render_longest_ms={render * 1000:.1f}")
        print(f"show_longest_ms={show * 1000:.0f}")


if __name__ == "__main__":
    asyncio.run(main())
