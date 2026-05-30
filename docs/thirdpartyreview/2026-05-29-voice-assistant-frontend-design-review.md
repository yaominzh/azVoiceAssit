# Third-Party Review — Voice Assistant Front-End Design Spec

**Reviewed doc:** `docs/superpowers/specs/2026-05-29-voice-assistant-frontend-design.md`
**Date:** 2026-05-29
**Grounded against:** `docs/superpowers/specs/2026-05-29-voice-assistant-p0-design.md`, `assistant.py`, `tests/test_assistant.py`

## Verdict

Strong, well-scoped design that matches the P0 spec's discipline: zero-dependency, opt-in,
CLI-path-preserving, with a clean `UiBus`/`NullBus` seam and a sensible test plan. It is close
to implementation-ready. Below are the gaps worth closing before writing the implementation
plan — most are small, a couple are real correctness/robustness issues.

## Should-fix

### 1. Port-bind failure is silently swallowed
The error-handling section says an exception in the daemon thread does not stop the loop. But
if the chosen port is already in use, `ThreadingHTTPServer` raises on **bind**, the daemon
thread dies, and a user who explicitly passed `--ui` gets a pure-CLI session with no error and
no page. Recommend: **bind the socket on the main thread** (so `--ui` fails loudly with a clear
message, mirroring the oMLX reachability check in `assistant.py`), then hand the bound server to
the daemon thread for `serve_forever()`. Also note explicitly that the default port must **not**
collide with oMLX's `:8002`.

### 2. New SSE clients get no initial state snapshot
`subscribe` only enqueues *future* events, so a browser that connects while the assistant is
`muted` or `speaking` shows a blank/default state until the next transition. Add a rule: on
subscribe, immediately emit the **current state** event. (Transcript replay staying out of scope
is fine — but current-state-on-connect is a correctness need, not history.)

### 3. Drop-oldest policy is wrong for `state` events
The bounded per-client queue "drops oldest if a client stalls." For `turn` events that is fine.
For `state`, dropping means a client can get stuck showing a stale state (e.g., stays "speaking"
after the loop returned to "listening"). State is last-write-wins; turns are a stream.
Recommend: coalesce/replace the pending `state` event rather than dropping the newest, or always
keep the latest state separately and re-send on overflow.

## Consider

### 4. Pin down exactly what `NullBus` prints, to guarantee unchanged CLI output
The loop is refactored to *always* go through a bus, even in CLI mode. Today the loop prints
`heard:`/`refined:`/timing. The spec says `NullBus` "just prints (today's behavior)" but does not
say which method prints what. Specify: `NullBus.push_turn` prints the three lines; `set_state`/
`clear` are no-ops; and `NullBus.listening_enabled` is always `True` (the audio callback reads it
— see #5). Otherwise CLI output / existing tests could drift.

### 5. Make the echo-guard vs. `listening_enabled` relationship explicit
The callback today gates on the local `state["speaking"]`. The spec says to also gate on
`bus.listening_enabled`. Clarify that the fast `speaking` echo-guard **stays** as a local bool and
`listening_enabled` is *additive* (`if not speaking and bus.listening_enabled`). Note the audio
thread must only read the bool — never touch the lock-protected queues — to stay realtime-safe.

### 6. Define state precedence when muting mid-turn
If mic is toggled off while `thinking`/`speaking`, which wins — `muted` or the in-flight state?
The decisions table and SSE enum do not say. One line resolving precedence (e.g., "`muted`
reflects mic state and overrides the symbol color only while the loop is idle/listening") avoids a
flickery UI.

### 7. `clear` during an in-flight turn is a benign race — say so
`POST /control/clear` calls `history.clear()` from the server thread while the main thread may be
inside `refine()` (`history.append`). deque ops are individually GIL-atomic so it will not crash,
but you can end up with a lone assistant message in a freshly-cleared deque. Worth a sentence
noting it is benign and self-corrects next turn.

### 8. Static serving — guard path traversal
Serving `static/` via a hand-rolled handler risks `..` traversal. Note the intent to reuse
`SimpleHTTPRequestHandler`'s `translate_path` (or whitelist the three files) so this is not
reinvented unsafely.

## Nits

### 9. SSE `turn` timing omits `endpoint`
The schema has `stt`/`refine`/`reply_start` but drops `endpoint`, which the CLI shows. Manual e2e
says "timing appears" — include `endpoint` for parity or note the omission is intentional.

### 10. `--ui` interaction with `--once` unstated
`main()` dispatches `--once` and returns early. Note `--ui` is irrelevant/ignored for `--once`.

### 11. Thread-per-SSE parking
`ThreadingHTTPServer` parks one worker thread per open `/events` connection forever (dead ones
detected only on next write via `BrokenPipeError`). Fine for single-user v0 — a one-line
acknowledgment sets expectations.

### 12. UX: print the URL on startup
Spec serves at `localhost:<port>` but does not surface it. Printing the URL (and optionally
`webbrowser.open`) on `--ui` startup is a cheap win.

### 13. Test plan could add an SSE wire-format assertion
Tests cover bus enqueuing and control handlers but nothing asserts the `data: {json}\n\n` framing.
A tiny formatter test would lock the wire contract.

---

Items **1–3** are the ones to resolve before implementation; the rest are clarifications that will
save round-trips.
