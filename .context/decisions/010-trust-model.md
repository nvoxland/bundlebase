# ADR-010: Trust Model — Trusted Client, Untrusted Bundle Data

**Status:** Accepted

**Date:** October 2026

## Context

Bundles are meant to be shared: someone builds a bundle, someone else opens
it. A bundle's manifest can reference external sources, ship function and
connector code (`.so`/`.dylib` libraries, executables, jars, `.py` files),
carry reports that render to Typst, and store config. A security review
(beads epic `bb-e0b`) had to decide who bundlebase protects from whom before
it could decide what to harden.

Two positions were tried and rejected along the way:

- **Restricting the client.** A read-only allowlist limited which SQL a
  read-only session could run (a2124f0). It was reverted (df873ab): a client
  that can reach the data is trusted by definition, and that boundary lives
  outside bundlebase (OS permissions, network access, Flight credentials).
  Blocking `IMPORT TEMP FUNCTION` or `SET CONFIG` for a trusted client only
  got in the way of legitimate read-only use.
- **Guarding source URLs.** A `url_safety` layer classified source URLs,
  refused loopback/metadata/internal addresses (SSRF) and forced `file://`
  sources to be copied into the bundle, behind
  `system.allow_internal_sources`. It was removed (c130ac5). See the
  decision below for the reasoning.

## Decision

**The client is trusted. Bundle data is not: opening or querying someone
else's bundle must not run code it ships unless the user opts in.**

### What `system.allow_external_code` gates

`system.allow_external_code` (runtime-only, default `false`, never persisted
in a manifest) is the single opt-in for running external code:

- **Connectors** with the `ffi` and `ipc` runtimes. Checked when the
  connector is used (discover/read), so `SET CONFIG` takes effect at once.
- **Functions** with the `ffi`, `ipc`, `java` and `docker` runtimes, and
  **file-backed Python** (`python::./x.py:f`), which is bundleable and runs
  the shipped `.py` in a subprocess. Gated functions still register (queries
  that don't call them work); calling one checks the live setting on every
  invocation and errors while it is off. Module-backed Python
  (`python::module:f`) runs in-process from the user's own environment, can't
  be bundled, and is not gated.
- **FFI connector verification at import.** `dlopen` runs a library's
  initializers, so full load-and-verify happens only when external code is
  allowed (and the library targets the host platform); otherwise only the
  file header is checked.

### Credentials are runtime-only

Connector credentials (`http` `auth`/`secret_headers`, postgres `password`)
are runtime-only, secure config: never written to the manifest, masked in
`SHOW CONFIG`, so they don't travel with a shared bundle. The postgres
connector rejects a password embedded in its persisted `url` argument. When credentials are
injected into an HTTP request, cross-origin redirects are refused so a source
can't bounce them to another host.

### Flight binds to loopback by default

`bundlebase serve` binds `127.0.0.1` unless `--host` says otherwise, so a
server started on someone's machine isn't reachable from the network by
accident.

### No source-URL guard

Bundlebase does **not** block which URLs or local paths a bundle may
reference — no SSRF filtering of loopback/internal/metadata addresses, no
forced copying of `file://` sources. Opening a bundle is an explicit user
action, like opening a document. `file://` references are common and must
not be rewritten or blocked by default. The one genuinely dangerous case — a
Flight server bound to all interfaces on someone's machine, letting remote
callers make it fetch internal URLs — is handled by the loopback default bind.
A "make this bundle self-contained" check can come back later as an explicit,
opt-in feature, not a silent default. **Do not re-add a URL guard** without a
new ADR superseding this one.

## Consequences

### Positive

- One switch to reason about for code execution, checked at the point of use
  for both connectors and functions.
- Shared bundles carry no secrets, and opening one does not run its code.
- Read-only sessions and local workflows aren't impeded by client-side
  restrictions that protect nothing.

### Negative

- Bundle data can make bundlebase read any URL or local path the user's
  process can reach. Users opening an untrusted bundle on a machine with
  sensitive internal endpoints get no protection from bundlebase itself.
- Enabling `allow_external_code` is all-or-nothing for a session: it trusts
  every gated function and connector in the bundle, not a chosen subset.

### Known gaps

- A credential set on the global `http` scope (rather than `http/<host>`) is
  sent to every host a bundle references.
- `bundlebase serve --host <non-loopback>` still starts with the default
  `admin`/`password` credentials, with only a startup warning.

### Neutral

- Typst report rendering escapes bundle-supplied text (titles, labels, cells,
  option strings) because report definitions are bundle data; that is input
  sanitization, not a code-execution gate.
