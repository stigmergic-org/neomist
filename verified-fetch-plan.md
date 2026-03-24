# Verified Fetch Plan

## Goal

Implement a Rust retrieval path for NeoMist that behaves like `@helia/verified-fetch` when P2P networking is disabled:

- discover remote HTTPS-capable IPFS providers/gateways
- fetch only the raw blocks needed for the requested content
- import those blocks into the local offline Kubo node
- keep serving through the existing local gateway and MFS flow

## Core Approach

This should mirror verified-fetch's high-level 3-step flow:

1. Query delegated routing for providers for the root CID
2. Convert provider HTTP(S) multiaddrs into gateway candidates and append a static fallback gateway
3. Fetch raw blocks over HTTPS from those candidates

NeoMist differs only in the final sink:

- verified-fetch reads from a Helia blockstore in-process
- NeoMist will import fetched blocks into local Kubo, then let the existing gateway serve them

## Discovery Flow

Use the same delegated routing source verified-fetch uses:

- `GET https://delegated-ipfs.dev/routing/v1/providers/<root-cid>`

Read the response fields:

- `Providers[*].Protocols`
- `Providers[*].Addrs`

Keep providers that advertise HTTP gateway transport over HTTPS:

- protocol: `transport-ipfs-gateway-http`
- address: HTTPS multiaddr (for example `/dns/example.com/tcp/443/https`)

Gateway candidate construction:

- convert matching HTTPS multiaddrs into base URLs
- dedupe candidates
- append static fallback `https://trustless-gateway.link`

Operational notes:

- provider-discovered gateways may fail or reset connections, so failover is required
- we should cache gateway discovery results briefly to avoid repeated routing lookups for the same root CID

## Retrieval Strategy

Do not fetch full CAR files for sites.

Instead:

- always fetch individual blocks as raw bytes
- use trustless gateway requests of the form:
  - `GET <gateway>/ipfs/<cid>?format=raw`

The retriever should fetch only the minimum block set needed for the request.

### Bare CID Requests

For `/ipfs/<cid>` or equivalent direct CID host routing:

- fetch the root block
- if the root is a file, fetch the file closure needed to serve that file
- if the root is a directory, fetch only the blocks needed to resolve `index.html`, then fetch the `index.html` file closure

### CID/Path Requests

For `/ipfs/<cid>/<path>`:

- fetch the root block
- fetch only the blocks needed to walk each path segment
- fetch the terminal node closure
- if the terminal node is a directory, resolve/fetch its `index.html`

### DAG/UnixFS Requirements

Rust needs enough IPLD/UnixFS parsing to walk requested paths correctly:

- UnixFS `dag-pb`
- file DAGs/chunked UnixFS files
- directory nodes
- HAMT-sharded directories

Important scope rule:

- fetch the blocks required to resolve the requested path and materialize the requested file or directory index target
- do not recursively fetch all descendants of a directory

This gives us lazy loading:

- first page load brings in HTML and the minimum supporting blocks
- later CSS/JS/image requests fetch their own block closures on demand

## Import into Local Kubo

Do not build CAR files in Rust for v1.

Instead, after fetching each raw block:

- call Kubo `/api/v0/block/put`
- import blocks one-by-one, not batched
- use `cid-codec=raw`
- pass through the exact bytes fetched from the gateway

Important details:

- `block/put` stores the raw bytes; it does not validate or reinterpret the block contents
- using `cid-codec=raw` is acceptable even when the original block CID uses another codec such as `dag-pb`
- because of that, the `Key` returned by `block/put` may differ from the expected CID even when the bytes are correct
- do not compare the returned `Key` directly to the expected CID

Verification strategy:

- after `block/put`, verify the block is locally addressable under the expected CID via `block/stat <expected-cid>` (or an equivalent local lookup)

Hash compatibility note:

- `cid-codec` can stay `raw`
- for non-default multihashes, we may still need to derive and pass `mhtype` and `mhlen` from the expected CID so Kubo computes the right multihash during `block/put`

Persistence strategy:

- do not add a separate pinning mechanism for this path
- rely on the existing MFS/cache strategy already used by NeoMist to keep content available

## Serving Model

The local Kubo node remains:

- the local block store
- the thing that serves the local gateway

Rust is responsible for:

- provider discovery
- gateway selection
- path walking
- raw block retrieval
- importing blocks into local Kubo

After the needed blocks are present locally:

- continue serving through the existing gateway implementation unchanged

## Integration Points in This Repo

Suggested code changes:

- `src/http_server.rs`
  - intercept direct CID and CID/path gateway requests before proxying to local Kubo
  - ensure required blocks are available locally before the existing proxy path runs
- `src/ens.rs`
  - after ENS / `.wei` resolution produces a CID, ensure the requested path is locally available before calling the existing MFS cache flow
- `src/ipfs.rs`
  - add helpers for `block/put`, `block/stat`, and local block existence checks
- `src/state.rs` and `src/main.rs`
  - wire a shared verified-fetch-style retriever service into app state
- new module, e.g. `src/verified_fetch.rs`
  - delegated routing query
  - gateway candidate building
  - UnixFS/IPLD path walking
  - raw block retrieval and Kubo import
- `src/config.rs`
  - add config for delegated routing URL and fallback gateways

## Runtime Behavior

The retriever should:

- run only when P2P networking is disabled / managed Kubo is offline for network retrieval
- dedupe in-flight work so concurrent requests for the same CID/path do not refetch the same blocks repeatedly
- retry or fail over across discovered gateways per block
- keep local gateway serving as the single response path to the browser

## Why This Plan

This approach keeps the good parts of verified-fetch while fitting NeoMist's architecture:

- same discovery source and general gateway flow as verified-fetch
- no dependency on local Kubo doing network retrieval
- no wasteful full-site CAR download for large website directories
- minimal per-request fetch scope
- existing local gateway and MFS behavior stay intact

## Test Cases

Minimum test coverage should include:

- bare CID that points to a file
- bare CID that points to a directory with `index.html`
- CID/path to a nested file
- CID/path where the terminal node is a directory
- ENS / `.wei` resolution path
- HAMT-sharded directory traversal
- provider/gateway failure with fallback to another candidate
- second request for the same asset being served locally without refetching
