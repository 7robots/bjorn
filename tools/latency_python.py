"""The Python Bjorn's input-to-frame latency, the way `bjorn-gate --latency` measures the Rust one.

    cd ~/GitHub/bjorn && .venv/bin/python ~/GitHub/bjorn-rust/tools/latency_python.py
"""

from __future__ import annotations

import asyncio
import statistics
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path.home() / "GitHub" / "bjorn" / "src"))

from bjorn.app import BjornApp  # noqa: E402
from bjorn.bear import BearClient, resolve_bearcli  # noqa: E402
from bjorn.config import Config  # noqa: E402
from bjorn.model import View  # noqa: E402


async def wait_until(pred, timeout: float = 60.0) -> None:
    loop = asyncio.get_event_loop()
    deadline = loop.time() + timeout
    while loop.time() < deadline:
        if pred():
            return
        await asyncio.sleep(0.005)
    raise SystemExit("timeout")


def ms(seconds: float) -> float:
    return seconds * 1000.0


async def main() -> None:
    client = BearClient(resolve_bearcli(""))
    app = BjornApp(Config(poll_seconds=0, icon_style="none"), client=client, environ={})
    async with app.run_test(size=(140, 44)) as pilot:
        await wait_until(lambda: app.loaded and app.note_list.notes)
        await wait_until(lambda: app.note_view.note is not None)
        await pilot.pause()
        n = len(app.note_list.notes)
        print(f"implementation=python notes={n}")

        # 1. j: key processed and the screen refreshed; then the reader following.
        app.note_list.list_view.focus()
        await pilot.pause()
        key_frame, reader_follow = [], []
        for _ in range(20):
            before = app.note_view.note.id if app.note_view.note else None
            t = time.perf_counter()
            await pilot.press("j")
            key_frame.append(ms(time.perf_counter() - t))
            t = time.perf_counter()
            await wait_until(lambda: app.note_view.note is not None and app.note_view.note.id != before and app.note_view._full_text is not None)
            reader_follow.append(ms(time.perf_counter() - t))
        print(f"cursor_key_to_frame_ms={statistics.median(key_frame):.1f}")
        print(f"cursor_to_reader_ms={statistics.median(reader_follow):.0f}   (includes the 120 ms debounce and one bearcli cat)")

        # 2. View switches on the whole library.
        views = []
        for _ in range(10):
            t = time.perf_counter()
            await pilot.press("2")
            await wait_until(lambda: app.selection.view is View.UNTAGGED and app.note_list.mounted)
            await pilot.press("1")
            await wait_until(lambda: app.selection.view is View.ALL and len(app.note_list.notes) == n and app.note_list.mounted)
            await pilot.pause()
            views.append(ms(time.perf_counter() - t) / 2)
        print(f"view_switch_ms={statistics.median(views):.1f}")

        # 3. tab through the panes.
        tabs = []
        for _ in range(30):
            t = time.perf_counter()
            await pilot.press("tab")
            tabs.append(ms(time.perf_counter() - t))
        print(f"tab_focus_ms={statistics.median(tabs):.2f}")

        # 4. A refresh with nothing changed.
        frames = []
        for _ in range(30):
            t = time.perf_counter()
            app.screen.refresh()
            await pilot.pause()
            frames.append(ms(time.perf_counter() - t))
        print(f"idle_frame_ms={statistics.median(frames):.2f}")

        # 5. A broad search.
        app.note_list.list_view.focus()
        t = time.perf_counter()
        await pilot.press("slash")
        for ch in "the":
            await pilot.press(ch)
        await pilot.press("enter")
        await wait_until(lambda: app.search_query == "the" and app.selection.search_ids is not None)
        await pilot.pause()
        print(f"search_the_ms={ms(time.perf_counter() - t):.0f} results={len(app.note_list.notes)}")
        await pilot.press("escape")
        await wait_until(lambda: app.search_query == "" and len(app.note_list.notes) == n)

        # 6. Sidebar: F expands every tag, then j walks 20 rows.
        app.sidebar.tree.focus()
        await pilot.pause()
        t = time.perf_counter()
        await pilot.press("F")
        await pilot.pause()
        print(f"fold_all_ms={ms(time.perf_counter() - t):.1f} rows={len(app.sidebar.tree._tree_lines)}")
        app.sidebar.move_to_tag(str(app.sidebar.tag_roots()[0].data))
        await pilot.pause()
        side = []
        for _ in range(20):
            t = time.perf_counter()
            await pilot.press("j")
            side.append(ms(time.perf_counter() - t))
        print(f"sidebar_step_ms={statistics.median(side):.1f}")


if __name__ == "__main__":
    asyncio.run(main())
