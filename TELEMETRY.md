# Telemetry in padctl

`padctl` collects **anonymous, aggregate usage telemetry** so I can see which
features are used and where they fail. It is **on by default** and trivial to
turn off.

It emits the shared DataZoo `telemetry_schema: 2` envelope — the same one used
by `erpl`, `erpl-adt` and `flapi` — so this product is comparable with the rest
of the stack. Data is ingested by PostHog in the EU
(`https://eu.i.posthog.com`). The schema is vendored at
[`docs/TELEMETRY-SCHEMA.md`](docs/TELEMETRY-SCHEMA.md).

## How to opt out

Any **one** of these disables it completely — nothing leaves your machine, and
no local state is kept either:

| Method | Scope |
|---|---|
| `--no-telemetry` | the single invocation |
| `DATAZOO_DISABLE_TELEMETRY=1` (or `true`/`yes`) | cross-product kill switch |
| `DO_NOT_TRACK=1` | honours <https://consoledonottrack.com/> |
| `PADCTL_NO_TELEMETRY=1` | product-local |
| `telemetry.enabled = false` in `config.toml` | permanent, per install |

On the first run with telemetry enabled, padctl prints one line to stderr
pointing here.

## The privacy contract

padctl sees your treadmill, its Bluetooth address, and — if you use the
integration — a Google Fit token. **None of that is ever transmitted.**
Specifically, we never send:

- Device **names** or **BLE addresses**
- Google **OAuth tokens**, refresh tokens, or account identifiers
- File paths, hostnames or usernames
- Your **walking data** — no distances, speeds, step counts or timestamps
  beyond a coarse session duration
- Error **messages** — only a class from a fixed enum

This is enforced by construction: every property passes an **allow-list** in
`src/telemetry.rs` naming the permitted keys and, for string properties, the
exact permitted values. Anything else is dropped before serialisation, so a
mistake at a call site cannot become a leak. Strings are additionally clamped
to 512 bytes as a backstop.

A test (`tests/telemetry.rs`) feeds a real device model, a MAC address, an
OAuth token, an email address and a file path through the capture path and
asserts none appear in any payload.

## Identifiers

`distinct_id` is the **salted SHA-256 of your OS machine id**
(`/etc/machine-id`, MAC address fallback) — a stable pseudonymous machine
hash, not your username and not your IP. It matches the id used by the other
DataZoo products. Where no stable hardware id exists, a per-process
**ephemeral** id is used and stamped `$process_person_profile: false`, so those
events never create a Person.

## What is collected

Every event carries the shared envelope: `product` (`padctl`),
`product_version`, `product_edition` (`oss`), `os`, `arch`, `platform`,
`is_ci`, `is_container`, `$session_id`, `identity_source`.

| Event | When | Properties |
|---|---|---|
| `cli_started` | launch | envelope only |
| `feature_used` | an enumerated capability runs | `feature` (`session_started`, `session_ended`, `device_connected`, `google_fit_synced`), `duration_ms` |
| `$exception` | a caught error | `error_class` (enum), `phase` |

The `feature` vocabulary is a fixed list in `src/main.rs`; a value outside it
is dropped rather than sent.

## Why it is on by default

Opt-in telemetry is answered by almost nobody, which makes it worthless for
deciding what to build. On-by-default with a genuine, verifiable privacy
contract and five ways to turn it off is the trade I have made. If you would
rather not, the table at the top takes one command.
