# By the Bay Scraper

Rust 2024 scraper and graph loader for By the Bay conference pages and related
Meetup archives.

The scraper produces a graph-oriented JSON export with:

- `conferences`: conference nodes such as Scale By the Bay and AI By the Bay.
- `meetups`: Meetup group nodes with URL and timezone metadata.
- `people`: speaker/person nodes scoped to conference or Meetup source.
- `talks`: talks, events, announcements, workshops, and related records.
- `edges`: graph relationships such as `SPEAKS_AT`, `PRESENTS`, and
  `PART_OF_MEETUP`.

## Project Layout

```text
Cargo.toml
README.md
REPORT.md
data/
  bythebay-graph.json
  enriched/
    bythebay-entities.json
    bythebay-enriched-graph.json
  raw/
    sources/
    extracted/
scripts/
  enrich_meetup_entities.py
src/
  main.rs
  bin/
    load_graph.rs
```

`src/main.rs` scrapes and extracts the source data. `src/bin/load_graph.rs` loads the exported graph through the backend selected with `--backend`.

## Requirements

- Rust toolchain with the 2024 edition supported.
- Python 3 when running cached-download enrichment.
- Network access when scraping live source pages.
- `OPENAI_API_KEY` if using the default LLM extractor.
- FalkorDB, HelixDB, or SurrealDB when loading into a graph database.

## Scrape Data

Run the scraper and write the graph export:

```bash
cargo run -- --output data/bythebay-graph.json
```

The default extractor is `llm`. It sends source HTML and Meetup event JSON to
the OpenAI Responses API using `gpt-5.5`:

```bash
export OPENAI_API_KEY=...
cargo run -- --extractor llm --output data/bythebay-graph.json
```

The regex/DOM extractor is available for offline or no-LLM runs:

```bash
cargo run -- --extractor regex --output data/bythebay-graph.json
```

By default, the scraper includes these source families:

- https://scale.bythebay.io/speakers
- https://ai.bythebay.io/speakers
- https://ai.bythebay.io/talks
- https://www.meetup.com/bythebay/
- https://www.meetup.com/bay-area-ai/
- https://www.meetup.com/sf-scala/
- https://www.meetup.com/unstructured-data-sf/
- https://www.meetup.com/hadoopsf/
- https://www.meetup.com/graphql-by-the-bay/
- https://www.meetup.com/scala-bay/
- https://www.meetup.com/sf-data-and-ai-engineering/
- https://www.meetup.com/big-data-developers-in-nyc/

Override or extend Meetup sources by repeating `--meetup-url`:

```bash
cargo run -- \
  --output data/bythebay-graph.json \
  --meetup-url https://www.meetup.com/bythebay/ \
  --meetup-url https://www.meetup.com/bay-area-ai/ \
  --meetup-url https://www.meetup.com/example-group/
```

Raw source records are cached in `data/raw/sources/`. Extracted records are
cached in `data/raw/extracted/`. Disable raw preservation with:

```bash
cargo run -- --output data/bythebay-graph.json --no-raw
```

## Enriched Entity Records

To re-parse the cached source downloads into richer entity records with
first-class speakers, talks, projects, companies, meetups, and graph edges:

```bash
python3 scripts/enrich_meetup_entities.py --batch-size 100
```

The enrichment output is written under `data/enriched/`:

- `summary.json`: counts and output locations for the latest run.
- `bythebay-entities.json`: aggregate entity arrays.
- `bythebay-enriched-graph.json`: `nodes`/`edges` graph record export.
- `{speakers,talks,projects,companies,meetups,conferences}.json`: one array per
  entity type.
- `{speakers,talks,projects,companies,meetups,conferences}.jsonl`: newline
  delimited records for streaming tools.
- `entities/{speakers,talks,projects,companies,meetups,conferences}/`: one JSON
  record per entity.
- `batches/`: batch summaries for each group of parsed Meetup downloads.

The current cached run produced 1,218 talks, 677 speakers, 185 companies,
219 projects, 9 meetups, 2 conferences, and a graph-record export with 2,310
nodes and 3,399 edges.

## Load Graph Data

The loader supports backend values for each database connection path:

- `falkor`: FalkorDB through RedisGraph/FalkorDB commands.
- `helix-rust-sdk`: HelixDB through the official Rust SDK.
- `helix-http`: HelixDB through direct dynamic-query HTTP JSON.
- `surrealdb`: SurrealDB through direct SurrealQL HTTP.
- `surrealdb-rust-sdk`: SurrealDB through the official Rust SDK.

Check the current CLI surface:

```bash
cargo run --bin load_graph -- --help
```

The default input format is the consolidated export written by this scraper:

```bash
cargo run --bin load_graph -- \
  --input data/bythebay-graph.json \
  --input-format export \
  --backend falkor
```

The loader can also read Claude-style per-talk record files from a single JSON
file or a directory of JSON files:

```bash
cargo run --bin load_graph -- \
  --input data/talks \
  --input-format talk-records \
  --backend falkor
```

The `talk-records` importer expects records with `nodes` and `edges`, node
fields named `id`, `type` or `kind`, and edge fields named `from`, `to`, and
`type`, `kind`, or `relationship`. It deduplicates nodes by stable ID and edges
by `(from, to, relationship)` before calling the same top-level graph loader.

The enriched graph export uses this same `nodes`/`edges` shape:

```bash
cargo run --bin load_graph -- \
  --input data/enriched/bythebay-enriched-graph.json \
  --input-format talk-records \
  --backend falkor
```

### FalkorDB

Start FalkorDB, then load the graph:

```bash
cargo run --bin load_graph -- \
  --input data/bythebay-graph.json \
  --backend falkor \
  --redis-url redis://127.0.0.1:6379 \
  --graph bythebay \
  --replace
```

`--replace` deletes the named Falkor graph before loading.

### HelixDB With Rust SDK

Install and run HelixDB v2 locally:

```bash
curl -sSL "https://install.helix-db.com" | bash
helix init local
helix run dev
```

Then load through the official Rust SDK backend:

```bash
cargo run --bin load_graph -- \
  --input data/bythebay-graph.json \
  --backend helix-rust-sdk \
  --helix-url http://127.0.0.1:8080/v1/query \
  --replace
```

The SDK backend builds `DynamicQueryRequest::write` payloads with
`write_batch()`, `g().add_n(...)`, `g().n_where(...)`, and `add_e(...)`, then
sends them with `helix_db::Client`.

### HelixDB With Direct HTTP

The direct HTTP backend sends the same dynamic query shape without the SDK:

```bash
cargo run --bin load_graph -- \
  --input data/bythebay-graph.json \
  --backend helix-http \
  --helix-url http://127.0.0.1:8080/v1/query \
  --replace
```

Use this path when debugging the raw dynamic query JSON or when isolating SDK
behavior from Helix gateway behavior.

Both Helix backends batch node and edge writes. Tune batch size with:

```bash
--batch-size 250
```

## Loaded Graph Model

The loader creates these node labels:

- `Source`
- `Conference`
- `Meetup`
- `Person`
- `Speaker`
- `Company`
- `Project`
- `Record`
- `Talk`
- `Announcement`
- `Workshop`
- `Unconference`
- `SocialEvent`
- `Event`

FalkorDB records can carry both `Record` and a specific label, such as
`Record:Talk`. HelixDB and SurrealDB receive the most specific single label in
the portable graph representation.

Each node receives a stable string `id` property from the export. Backends that
assign internal node IDs still use the stable `id` property to look up nodes
before creating edges.

Relationship names are normalized to uppercase ASCII with non-alphanumeric
characters converted to `_`. Empty names become `RELATED_TO`.

## Important Limitations

- Helix `--replace` drops known By the Bay labels before loading. Use it for
  repeat local loads.
- Helix loading uses `AddN`; without `--replace`, repeated loads create
  duplicate nodes. FalkorDB uses Cypher `MERGE` and can be reloaded into the
  same graph.
- Helix edge creation looks up source and target nodes by the exported `id`
  property, then creates the relationship to the target variable.
- HelixDB currently rejects array-valued graph mutation properties, so Helix
  backends omit properties such as `tags` and `labels`. Keep portable string
  alternatives such as `tags_json` and `tags_csv` when those values matter.

## Development Checks

Format:

```bash
cargo fmt
```

Compile-check the loader:

```bash
cargo check --bins
```

Compile and inspect CLI options:

```bash
cargo run --bin load_graph -- --help
```

Regenerate enriched entity records:

```bash
python3 scripts/enrich_meetup_entities.py --batch-size 100
```
