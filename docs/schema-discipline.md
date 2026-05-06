# Schema Discipline

The `adsb-core` workspace crate is consumed as a git dependency by sister projects (notably the `UtilitarianTechnology` repo's `rust/adsb-poc/` and `rust/adsb-adapter/` crates). Changes to public types in `adsb-core` propagate to those consumers without an explicit version bump. This doc spells out what counts as breaking and how to coordinate.

The HTTP API contracts are pinned by `rust/adsb-server/src/web/routes.rs::consumer_contract_tests` — a dedicated test module that asserts JSON shape (bare array vs envelope) and enrichment-populated invariants for every endpoint UtilTech consumers hit. **Run those tests before pushing API changes.** If you see the contract test fail, you're holding a coordination obligation.

*As of 2026-05-05.*

---

## What's exposed

The library surface that matters to external consumers lives in `adsb-core`. Treat anything publicly re-exported from `adsb-core/src/lib.rs` as a contract:

- `AircraftState` — the canonical struct representing one aircraft's current position + identity. Field-level changes ripple into every consumer.
- Decoder entry points (`decode_message`, etc.) — protocol-level functions that consumers may call directly.
- Filter / aggregation helpers used by `adsb-server` and re-exported.

The HTTP API (in `adsb-server`) is a separate contract — covered in [`docs/ais-ingester-runbook.md`](ais-ingester-runbook.md) and route docs in `web/routes.rs`. API-shape changes are also breaking for downstream consumers (notably the correlator). The Apr 28 incident — where the correlator assumed `{"positions": [...]}` envelopes but the API returned bare arrays — is exactly the kind of mismatch this doc is trying to prevent.

---

## What counts as breaking

**Breaking — coordinate before merging:**
- Removing a field from `AircraftState` (or any other publicly-exposed struct)
- Renaming a field
- Changing a field's type (`Option<i32>` → `i32`, `String` → `&str`, etc.)
- Changing the variants of a public enum
- Changing the signature of a public function
- Changing the JSON shape of any HTTP response (e.g., bare array → wrapped object, or vice versa)
- Changing query-param names or semantics on any `/api/*` endpoint

**Non-breaking — safe to ship without coordination:**
- Adding a new field to a struct (consumers that destructure should already use `..` rest patterns or named-field destructuring; if they don't, that's a consumer-side bug)
- Adding a new variant to a non-exhaustive enum (mark the enum `#[non_exhaustive]` for this to be safe)
- Adding a new public function or struct
- Adding a new HTTP endpoint
- Internal refactoring that doesn't change public signatures

---

## How consumers pin

External consumers pin to a git commit, not a semver tag. Example from the UtilTech `adsb-poc` Cargo.toml:

```toml
adsb-core = { git = "https://github.com/blueOctopusAI/adsb-decode", rev = "<commit-sha>" }
```

That means:
- Consumers don't auto-update when this repo's `main` advances. They explicitly bump the `rev`.
- A breaking change in `main` doesn't break consumers immediately — but the moment they bump, it does.
- There's no published crate, so there's no "yank" mechanism. Once a commit is pushed, consumers can pin to it forever.

This pinning model means the burden is on this repo to **avoid surprise breakage in commits that consumers might bump to**. It does not mean "anything goes on `main`."

---

## Coordination protocol

When you're about to make a breaking change in this repo:

1. **Open an issue or PR comment** noting the change is breaking and naming the consumers it affects (currently: `UtilitarianTechnology/rust/adsb-poc/`, `UtilitarianTechnology/rust/adsb-adapter/`, `UtilitarianTechnology/orin/scripts/adsb_correlator.py`).
2. **Ship the consumer-side fix first** if it's a struct or enum change — the consumer can land a patch that's compatible with both old and new shapes (e.g., destructure with `..`, or use `serde(default)` for new fields).
3. **Then ship here.**
4. **Bump consumer `rev`** in the consumer Cargo.toml or pin file.
5. **Verify the consumer builds + tests pass** against the new pin.

For HTTP API changes:
- Add the new shape as a new endpoint or new query param; deprecate the old shape; remove only after consumers bump.
- Or: change the shape, document it in `ROADMAP.md`'s Recent section, and update consumers in the same session.

---

## Schema-changes checklist

Before opening a PR that touches `adsb-core/src/lib.rs` or any public API endpoint:

- [ ] Does this rename, remove, or retype a public field, function, or struct?
- [ ] Does this change the JSON shape of any `/api/*` response?
- [ ] Does this change query-param names or semantics?
- [ ] Does this change the variants of a public enum that isn't `#[non_exhaustive]`?

If any answer is yes — coordinate with downstream consumers before merging.

---

## Recent breakages (so they don't repeat)

| Date | What broke | Caught by | Fix |
|---|---|---|---|
| 2026-04-28 | Correlator (`adsb_correlator.py`) assumed `/api/positions` returned `{"positions": [...]}`; actual shape is a bare array. Tests mocked the wrong shape. | Real round-trip during VPS cutover verification | Fixed correlator + tests; documented here. Now pinned by `consumer_contract_tests::contract_api_positions_is_bare_array`. |
| 2026-05-05 | `/api/positions/all` and `/api/query` did not include `is_military` / `registration` / `country` / `callsign` — the correlator's `ADSBCandidate` reads those with `.get(..., default)`, so historical-replay queries silently treated every aircraft as civilian. Discovered while writing the new contract regression tests. | Author-time review during contract test scaffolding | Enriched `PositionRow` with these fields; both SQLite and Postgres SQL queries updated to JOIN aircraft + latest sighting. Pinned by `consumer_contract_tests::contract_api_positions_all_enrichment_is_populated` and `_query_enrichment_is_populated`. |

When a new breakage is caught, log it in this table — the goal is a small, growing record of the kinds of mismatches we've already learned about.
