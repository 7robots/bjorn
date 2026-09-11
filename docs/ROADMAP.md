# Roadmap

Single source of truth for planned and deferred work in bjorn-rust. The active
plan lives in `docs/plans/bjorn-rust.md`.

## Next

- Decide, after the Phase 26 gate, whether `bjorn-rs` replaces the Python
  `bjorn` launcher in `~/bin` or the two keep coexisting.

## Deferred (carried from bjorn's ROADMAP, still open here)

- PDF export (headless-browser concern recorded in bjorn's ROADMAP).
- EPUB export.
- Search-box completion inside a multi-word tag; completion candidate cycling.
- Persisted body previews for a metadata-only cold start.
- Notes list snippet line (first body line, as Bear shows).
- Permanent delete from Trash (no bearcli command yet).
- Attachments listing and save; tag rename/delete; archive from the TUI;
  section navigation from `bearcli outline`; locked-note explanation;
  change detection smarter than polling; triage follow-ons (edit todo text,
  snooze, un-tick `@done`, "Todo in workspace" view).

## Closed by design in the Rust port

- Long-note render time and the head-then-rest truncation: the renderer draws
  only the viewport.
- Notes list windowing in 120-row batches: same reason.
- Search highlights inside fenced code and tables: spans over rendered lines
  cover them.
- Reader match count wrong before the first `]` on a truncated note: nothing
  is truncated.
