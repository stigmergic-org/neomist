# apps.neomist.eth

`apps.neomist.eth` is a generated ENS/IPFS application index. It tracks mainnet `.eth` names whose ENS contenthash points at IPFS or IPNS, probes those roots through Kubo, analyzes reachable apps, exports a static JSON tree, and publishes that tree as the ENS contenthash for `apps.neomist.eth`.

The published root is data, not a web UI. Consumers should treat it as a static JSON API addressed by immutable IPFS CID or by the mutable ENS name.

## Published Root

`ipfs-root/` is the directory added to IPFS and published on ENS. Do not edit it by hand; `npm run export-ipfs` rebuilds it from SQLite state.

Access patterns:

- `https://apps.neomist.eth/<path>` for the current ENS-published root through NeoMist
- `https://<root-cid>.ipfs.localhost/<path>` for an immutable IPFS root through NeoMist
- `ipfs://<root-cid>/<path>` as the canonical content URL stored on ENS

Current local publish metadata, when present, is in `state/ens-publish.json`. It records the root CID, encoded ENS contenthash, resolver, transaction hash, and publish time.

## Root Layout

```text
ipfs-root/
  meta/
    generated.json
    stats.json
  names/
    by-node/<node[2:4]>/<node[4:6]>/<node>.json
    by-category/index.json
    by-category/<category-slug>/index.json
    by-category/<category-slug>/page-0000.json
  analysis/
    by-cid/<cid[-4:-2]>/<cid[-2:]>/<cid>.json
  search/
    index.json
    docs/page-0000.json
    grams/<gram[0:2]>.json
```

All JSON files are compact single-line JSON. Paths in records are relative to the published root.

## Metadata

`meta/generated.json` describes the export run:

- `generated_at`: ISO timestamp for this export.
- `exported_names`: current names with successful latest probe.
- `exported_categories`: category index count.
- `exported_search_docs`: search document count.

`meta/stats.json` mirrors database stats at export time:

- `overview`: total names, probes, analyses, sync cursors.
- `coverage`: probe and analysis coverage for current names.
- `app_scope`: analyzed current apps and unique root CIDs.
- `apps_by_category`, `quality_tiers`, `security_risks`, `threat_types`: breakdowns.
- `safe_to_list`, `probe_health`, `contenthash_protocols`, `domain_shape`, `unique_root_cids`: operational summaries.

Use `meta/generated.json` for lightweight freshness checks. Use `meta/stats.json` for dashboards.

## Name Records

Canonical record path is by ENS node:

```text
names/by-node/<node[2:4]>/<node[4:6]>/<node>.json
```

`node` is the ENS namehash hex string. The first shard is `node.slice(2, 4)`. The second shard is `node.slice(4, 6)`.

Example lookup:

```js
import { namehash } from 'ethers';

const base = 'https://apps.neomist.eth';
const node = namehash('ethdevnews.eth');
const path = `names/by-node/${node.slice(2, 4)}/${node.slice(4, 6)}/${node}.json`;
const record = await fetch(`${base}/${path}`).then((res) => res.json());
```

Name record fields:

- `node`: ENS node hash.
- `name`: ENS name.
- `parent_name`: parent ENS name when known.
- `is_subdomain`: boolean.
- `root_cid`: current decoded IPFS/IPNS root CID from ENS contenthash.
- `title`: probed HTML title, when detected.
- `icon_url`: probed icon URL, when detected.
- `manifest_url`: probed web manifest URL, when detected.
- `analysis`: summary for latest successful analysis of `root_cid`, or `null`.
- `contenthashes`: known contenthash versions for this node, newest first.

`contenthashes[]` entries include:

- `contenthash_protocol`: `ipfs` or `ipns`.
- `root_cid`: decoded root CID.
- `contenthash_set_block`: source ENS event block.
- `contenthash_set_tx_hash`: source ENS event transaction hash.

## Analysis Records

Full analysis records are stored once per root CID:

```text
analysis/by-cid/<cid[-4:-2]>/<cid[-2:]>/<cid>.json
```

Name records and category pages point at these files with `analysis.path` or `analysis_path`.

Analysis record fields:

- `root_cid`: analyzed root CID.
- `model`: OpenCode model used for analysis.
- `analyzed_at`: ISO timestamp.
- `result.schema_version`: currently `1`.
- `result.category`: one category label such as `Finance`, `Wallet`, `Blog`, `Redirect`, or `Static data`.
- `result.category_confidence`: number from `0` to `1`.
- `result.summary`: one-sentence purpose summary.
- `result.signals`: evidence strings from mounted files and probe context.
- `result.quality`: tier, score, substance flags, and rationale.
- `result.security`: risk, score, threat type, safe-to-list flag, and concrete findings.
- `result.files_reviewed`: mounted files inspected during analysis.

Analysis is per CID, not per name. Multiple ENS names can point at the same `root_cid` and share one analysis file.

## Categories

Category root:

```text
names/by-category/index.json
```

It contains:

- `generated_at`
- `count`
- `categories[]` with `category`, `slug`, `count`, `path`, `page_size`, and `page_count`

Category index:

```text
names/by-category/<slug>/index.json
```

It contains page metadata only. Load pages from `pages[].path`.

Category page:

```text
names/by-category/<slug>/page-0000.json
```

It contains `apps[]`, sorted by `quality_score` descending, then name ascending. Each app entry is a compact joined view of name probe data and analysis summary:

- ENS fields: `node`, `name`, `parent_name`, `is_subdomain`
- Probe fields: `title`, `icon_url`, `manifest_url`, `root_cid`
- Cross-links: `name_path`, `analysis_path`
- Chain metadata: `contenthash_set_block`, `contenthash_set_tx_hash`, `last_seen_at`
- Analysis fields: `analyzed_at`, `summary`, `category_confidence`, `quality_tier`, `quality_score`, `security_risk`, `threat_type`, `safe_to_list`

Category pages include analyzed apps only. The broader name records include every current name with a successful latest probe, even when analysis is missing.

## Search Index

Search lives under `search/` and is intentionally simple for static clients.

`search/index.json` contains:

- `doc_count`, `doc_page_size`, `doc_page_count`
- `doc_path_template`: `docs/page-{page}.json`
- `doc_fields`: compact field map for docs.
- `name_path.template`: node-based name record template.
- `analysis_path.template`: CID-based analysis record template.
- `gram_size`: currently `3`.
- `min_query_length`: currently `3`.
- `gram_shard_size`: currently `2`.
- `gram_path_template`: `grams/{shard}.json`
- `searchable_fields`: `name`, `title`, `category`, `summary`.
- `grams`: counts by gram for planning and UI hints.

Search docs live in pages:

```text
search/docs/page-0000.json
```

Doc fields use compact keys:

- `n`: name.
- `c`: root CID.
- `q`: quality score.
- `k`: category.
- `r`: security risk.
- `t`: threat type.

Gram shards map 3-character lowercase ASCII grams to doc IDs:

```text
search/grams/<first-two-characters>.json
```

Client search flow:

1. Lowercase query.
2. Extract ASCII alphanumeric tokens with length at least `3`.
3. Generate 3-character grams for each token.
4. Fetch `search/grams/<gram.slice(0, 2)>.json` for each gram.
5. Collect candidate doc IDs from `grams[gram]`.
6. Intersect or score candidates by gram overlap.
7. Load each doc page with `Math.floor(id / doc_page_size)`.
8. Read doc at `id - id_start` inside that page.
9. Compute namehash from `doc.n` and load full name record from `names/by-node/...` when needed.
10. Load full analysis from `analysis/by-cid/...` when needed.

Search index does not store ENS node hashes. Compute `namehash(doc.n)` to reach the full name record.

## Commands

Install dependencies:

```bash
npm install
```

Useful commands:

```bash
npm run sync-name -- ethdevnews.eth
npm run sync-names -- --limit 200
npm run analyze-name -- ethdevnews.eth
npm run analyze-names -- --limit 50
npm run export-ipfs
npm run publish-ens -- --dry-run
npm run daily -- --dry-run
npm run db-stats
npm run list-names -- --sort score --limit 10 --details
npm run show-name -- ethdevnews.eth
npm run list-probe-failures -- --limit 25
```

Run command-specific help:

```bash
node src/cli.mjs <command> --help
```

## System Flow

1. `sync-names` reads ENSNode `contenthashChangeds` events, replays a recent head window, and backfills older events with cursors stored in SQLite.
2. For candidate domains, it hydrates current ENS domain data, decodes `contenthash`, keeps mainnet `.eth` names, excludes configured namespaces such as `base.eth` and `linea.eth`, and keeps only IPFS/IPNS roots.
3. It probes current roots through Kubo RPC with `files.stat` and `cat`, looking for root files, `index.html`, `index.htm`, icons, and web manifests. Probe results store reachability, content type, title, icon, manifest, and fetch errors.
4. `analyze-names` mounts reachable IPFS/IPNS roots through `wac --mount-ipfs`, runs OpenCode with `prompts/ipfs-app-analysis-system.md`, validates strict `analysis.json`, and stores category, quality, and light security review by root CID.
5. `export-ipfs` rebuilds `ipfs-root/` from `state/index.sqlite`, writes sharded name records, CID-sharded analysis records, category pages, search docs, gram index, and metadata, then atomically replaces the old root.
6. `publish-ens` hashes or adds `ipfs-root/` with `ipfs add --recursive --cid-version=1 --raw-leaves`, encodes the root CID as an IPFS ENS contenthash, checks the current resolver, and sends `setContenthash` when publishing is allowed.
7. `daily` runs sync, analysis, export, and conditional publish in one command.

Publishing is skipped when the ENS contenthash is already current, gas price is at or above `--max-gas-price-mwei`, or the local publish marker is inside `--publish-cooldown-days`.

## State

Generated and runtime files:

- `state/index.sqlite`: SQLite store for names, contenthash versions, probes, analyses, and sync cursors.
- `state/analysis-work/`: temporary WAC/OpenCode work directories.
- `state/ens-publish.json`: last successful publish marker.
- `ipfs-root/`: generated root that gets added to IPFS and published on ENS.

These paths are ignored by git.

## Configuration

Environment variables:

- `APPS_NEOMIST_DB_PATH`: override SQLite path.
- `APPS_NEOMIST_ENSNODE_URL` or `ENSNODE_URL`: ENSNode GraphQL endpoint.
- `APPS_NEOMIST_KUBO_RPC_URL`, `KUBO_RPC_URL`, or `IPFS_API`: Kubo RPC API URL or multiaddr.
- `APPS_NEOMIST_ANALYSIS_MODEL`: OpenCode model for app analysis.
- `APPS_NEOMIST_ANALYSIS_MEMORY_LIMIT`: WAC memory limit.
- `APPS_NEOMIST_ENS_NAME`: ENS name to publish, default `apps.neomist.eth`.
- `APPS_NEOMIST_ETH_RPC_URL`, `ETH_RPC_URL`, `MAINNET_RPC_URL`, or `RPC_URL`: Ethereum mainnet RPC for publishing.
- `APPS_NEOMIST_ENS_PRIVATE_KEY`, `ENS_PRIVATE_KEY`, or `PRIVATE_KEY`: signer key for non-dry-run publishing.

Publish options:

- `--dry-run`: compute CID and contenthash without sending a transaction.
- `--cid CID`: publish an existing root CID instead of adding local `ipfs-root/`.
- `--no-pin`: add root without pinning blocks.
- `--max-gas-price-mwei N`: skip publish when gas is too high.
- `--publish-cooldown-days N`: skip publish when local marker is too recent.

## Safety Notes

- IPFS roots are untrusted input. Analysis prompt forbids executing code, installing dependencies, submitting forms, connecting wallets, or fetching external HTTP/HTTPS links.
- Publishing needs an Ethereum mainnet RPC and a signer private key unless using `--dry-run`.
- ENS points to a mutable latest root, but every IPFS root CID is immutable. Pin or cache a CID when reproducibility matters.
