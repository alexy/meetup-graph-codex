# By the Bay Graph Loader Report

## Summary

This repository contains a Rust scraper for By the Bay conference and Meetup
data plus a backend-neutral graph loader. The project lives at:

```text
/Users/alexy/src/bythebay/cdx/bythebay-scraper
```

The scraper writes a JSON export. The loader turns that export into a portable
`GraphData` value made of `GraphNode` and `GraphEdge` records, then calls the
top-level API:

```rust
load_graph(&GraphLoadConfig, &GraphData)
```

Backend names are now selected only through `--backend`. The loader binary is
only `load_graph`.

## Backend Options

The current backend choices are:

```bash
--backend falkor
--backend helix-http
--backend helix-rust-sdk
--backend surrealdb
--backend surrealdb-rust-sdk
```

`falkor` is the default. The recommended typed-client paths are
`helix-rust-sdk` for HelixDB and `surrealdb-rust-sdk` for SurrealDB. The HTTP
paths remain useful for inspecting wire payloads and isolating SDK behavior.

## Exported Graph Model

The export is intentionally simple:

- Nodes have a stable string `id`, one primary label, and properties.
- Edges have `from`, `to`, and `relationship`.
- Talk-like records include labels such as `Talk`, `Workshop`,
  `Announcement`, `Event`, `SocialEvent`, and `Unconference`.

The main relationship types in the current export are:

- `PRESENTS`: `Person -> Talk`
- `PART_OF_MEETUP`: `Talk -> Meetup`
- `SPEAKS_AT`: `Person -> Conference`

The meetup traversal pattern used below is:

```text
(speaker:Person)-[:PRESENTS]->(talk:Talk)-[:PART_OF_MEETUP]->(meetup:Meetup)
(other:Person)-[:PRESENTS]->(other_talk:Talk)-[:PART_OF_MEETUP]->(same meetup)
```

This handles both cases the data needs:

- A meetup has several talks, so `other_talk` may be different from `talk`.
- A talk can have several speakers, so `other_talk` may also be the same talk.

## Small Test Dataset

A compact illustration dataset is available at:

```text
data/sample-meetup-graph.json
```

It is intentionally small enough to inspect by hand:

- Meetups:
  - `meetup:graph-night` / Graph Night
  - `meetup:rust-data-night` / Rust Data Night
- Speakers:
  - Ada Lovelace
  - Grace Hopper
  - Margaret Hamilton
  - Katherine Johnson
- Talks:
  - Loading Portable Graphs, presented by Ada Lovelace and Grace Hopper at
    Graph Night
  - Query Patterns for Meetup Graphs, presented by Margaret Hamilton at Graph
    Night
  - Async Loaders in Rust, presented by Ada Lovelace and Katherine Johnson at
    Rust Data Night

This fixture exercises the two important traversal cases:

- Graph Night has multiple talks.
- Loading Portable Graphs and Async Loaders in Rust have multiple speakers.

For the example queries below, using Ada Lovelace as the input speaker should
return:

| Meetup | Other speakers |
| --- | --- |
| Graph Night | Grace Hopper, Margaret Hamilton |
| Rust Data Night | Katherine Johnson |

Load the sample into any backend by pointing the loader at the sample file:

```bash
cargo run --bin load_graph -- \
  --input data/sample-meetup-graph.json \
  --backend falkor \
  --graph bythebay_sample \
  --replace
```

For SurrealDB:

```bash
cargo run --bin load_graph -- \
  --input data/sample-meetup-graph.json \
  --backend surrealdb-rust-sdk \
  --surreal-url http://127.0.0.1:8000/sql \
  --surreal-ns bythebay_sample \
  --surreal-db graph \
  --replace
```

For HelixDB:

```bash
cargo run --bin load_graph -- \
  --input data/sample-meetup-graph.json \
  --backend helix-rust-sdk \
  --helix-url http://127.0.0.1:8080/v1/query \
  --replace
```

## Loader Architecture

The scraper and CLI do not know about FalkorDB, HelixDB, or SurrealDB write
details. They only build neutral graph data:

```rust
GraphData {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}
```

The backend module owns all database-specific behavior:

- FalkorDB converts neutral nodes and edges into Cypher over Redis commands.
- HelixDB converts them into dynamic query writes, through HTTP JSON or the
  Rust SDK.
- SurrealDB converts them into SurrealQL, through HTTP `/sql` or the Rust SDK.

This keeps future database backends additive: implement a new branch behind
`GraphBackend`, without changing the scraper.

The public API stays intentionally small:

```rust
load_graph(&GraphLoadConfig, &GraphData)
```

Internally, each backend now implements a small `GraphLoader` trait. That keeps
the external API stable for the scraper while making backend implementations
easier to separate and extend.

## Import Formats

The primary importer reads the consolidated By the Bay export:

```bash
cargo run --bin load_graph -- \
  --input data/bythebay-graph.json \
  --input-format export \
  --backend falkor
```

An alternate importer reads Claude-style per-talk record files:

```bash
cargo run --bin load_graph -- \
  --input data/talks \
  --input-format talk-records \
  --backend falkor
```

That importer accepts either one JSON file or a directory of JSON files. Each
file should contain `nodes` and `edges`; node kind can be named `type` or
`kind`, and edge relationship can be named `type`, `kind`, or `relationship`.
The importer deduplicates nodes by stable ID and edges by `(from, to,
relationship)`, then sends the result through the same top-level `load_graph`
API as the consolidated export.

## FalkorDB Representation

FalkorDB is the most direct graph representation.

- Nodes are stored as Cypher nodes.
- Relationships are stored as first-class Cypher relationships.
- Record nodes can carry multiple labels, for example `Record:Talk`.
- Node and relationship writes use `MERGE`, so repeat loads are naturally more
  idempotent.
- `--replace` deletes the whole named graph with `GRAPH.DELETE`.

Load example:

```bash
cargo run --bin load_graph -- \
  --backend falkor \
  --input data/bythebay-graph.json \
  --redis-url redis://127.0.0.1:6379 \
  --graph bythebay \
  --replace
```

Query example: for a given speaker, list meetups where they presented and all
other speakers who presented at those same meetups:

```cypher
MATCH (speaker:Person {name: 'Ada Lovelace'})
      -[:PRESENTS]->(:Talk)
      -[:PART_OF_MEETUP]->(meetup:Meetup)
MATCH (other:Person)
      -[:PRESENTS]->(other_talk:Talk)
      -[:PART_OF_MEETUP]->(meetup)
WHERE other.id <> speaker.id
RETURN
  meetup.name AS meetup,
  collect(DISTINCT other.name) AS other_speakers,
  collect(DISTINCT other_talk.title) AS talks
ORDER BY meetup;
```

This query includes co-speakers on the same talk because `other_talk` is not
required to differ from the original talk.

## HelixDB Representation

HelixDB uses a dynamic query API rather than Cypher.

- Nodes are written with `AddN`.
- Edges are written with `AddE`.
- The edge target must be a Helix node reference, so the loader first finds the
  target by exported string `id`, then creates the edge from the source stream.
- Helix `AddN` takes one label, so record nodes use the most specific label,
  such as `Talk`, instead of multi-label `Record:Talk`.
- `--replace` drops known By the Bay labels before loading, because the current
  dynamic write path does not use Cypher-style `MERGE`.

HTTP load example:

```bash
cargo run --bin load_graph -- \
  --backend helix-http \
  --input data/bythebay-graph.json \
  --helix-url http://127.0.0.1:8080/v1/query \
  --replace
```

Rust SDK load example:

```bash
cargo run --bin load_graph -- \
  --backend helix-rust-sdk \
  --input data/bythebay-graph.json \
  --helix-url http://127.0.0.1:8080/v1/query \
  --replace
```

The equivalent Helix traversal is best expressed as a dynamic read pipeline:

```text
1. NWhere Person.name == $speaker_name
2. Traverse outgoing PRESENTS to Talk
3. Traverse outgoing PART_OF_MEETUP to Meetup
4. For each Meetup:
   a. Traverse incoming PART_OF_MEETUP to all Talk nodes at that meetup
   b. Traverse incoming PRESENTS to Person nodes
   c. Filter Person.id != original speaker.id
5. Return meetup plus distinct other speakers and talks
```

In SDK-oriented pseudocode, the query shape is:

```rust
let speaker = g().n_where(SourcePredicate::eq("name", speaker_name));
let meetups = speaker
    .out("PRESENTS")
    .out("PART_OF_MEETUP");

let others = meetups
    .in_("PART_OF_MEETUP")
    .in_("PRESENTS")
    .filter(SourcePredicate::neq("id", speaker_id));
```

The loader currently implements writes only. Adding reusable read helpers should
wrap this traversal shape in a Helix dynamic read request once the exact SDK
read traversal helpers are pinned in code.

## SurrealDB Representation

SurrealDB stores graph edges as records in edge tables.

- Nodes are records in tables derived from labels, such as `person`, `talk`,
  and `meetup`.
- Stable exported IDs are used as record IDs with `type::record(table, id)`.
- Edges are created with `RELATE`.
- Relationship names become edge table names, such as `presents` and
  `part_of_meetup`.
- SurrealDB can be loaded through direct HTTP `/sql` or through the official
  Rust SDK over WebSocket.
- The loader bootstraps the namespace/database before writing.

HTTP load example:

```bash
cargo run --bin load_graph -- \
  --backend surrealdb \
  --input data/bythebay-graph.json \
  --surreal-url http://127.0.0.1:8000/sql \
  --surreal-user root \
  --surreal-pass root \
  --surreal-ns bythebay \
  --surreal-db graph \
  --replace
```

Rust SDK load example:

```bash
cargo run --bin load_graph -- \
  --backend surrealdb-rust-sdk \
  --input data/bythebay-graph.json \
  --surreal-url http://127.0.0.1:8000/sql \
  --surreal-user root \
  --surreal-pass root \
  --surreal-ns bythebay \
  --surreal-db graph \
  --replace
```

SurrealQL query example using graph traversal from a speaker record:

```sql
LET $speaker = person:person_ada_lovelace;

SELECT
  name AS meetup,
  <-part_of_meetup<-talk<-presents<-person[
    WHERE id != $speaker.id
  ].name AS other_speakers,
  <-part_of_meetup<-talk[
    WHERE <-presents<-person[WHERE id != $speaker.id]
  ].title AS talks
FROM $speaker->presents->talk->part_of_meetup->meetup;
```

If querying by speaker name rather than record ID, first bind the speaker:

```sql
LET $speaker = (SELECT * FROM person WHERE name = 'Ada Lovelace' LIMIT 1)[0];

SELECT
  name AS meetup,
  <-part_of_meetup<-talk<-presents<-person[
    WHERE id != $speaker.id
  ].name AS other_speakers,
  <-part_of_meetup<-talk[
    WHERE <-presents<-person[WHERE id != $speaker.id]
  ].title AS talks
FROM $speaker->presents->talk->part_of_meetup->meetup;
```

This is the same logical traversal as the Falkor query, but SurrealDB exposes
edges as named record tables and lets the query walk forward and backward with
`->edge->table` and `<-edge<-table`.

## Backend Differences

| Backend | Node identity | Edge representation | Label behavior | Reload behavior |
| --- | --- | --- | --- | --- |
| FalkorDB | `id` property on Cypher node | Native Cypher relationship | Supports multi-label records such as `Record:Talk` | `MERGE` makes normal loads mostly idempotent; `--replace` deletes graph |
| HelixDB | `id` property plus internal node IDs | Dynamic `AddE` to a selected target node reference | One label per `AddN`; uses most specific label | Use `--replace`; otherwise `AddN` can duplicate nodes |
| SurrealDB | Record ID via `type::record(table, id)` plus `id` property | Edge records in relationship tables via `RELATE` | One table per primary label | Use `--replace`; otherwise `CREATE`/`RELATE` can duplicate edge records |

The stable exported string ID is the cross-backend contract. Every backend
either stores it as a property, uses it as the record ID, or both.

## Verification Performed

Current verification commands:

```bash
cargo fmt
cargo test --bins --lib
cargo run --bin load_graph -- --help
```

The help output includes:

```text
--backend <BACKEND> [possible values: falkor, helix-http, helix-rust-sdk, surrealdb, surrealdb-rust-sdk]
```

Live SurrealDB checks were also run against a local server at
`http://127.0.0.1:8000/sql` for both HTTP and Rust SDK paths.

## Remaining Next Steps

1. Add full integration tests that start FalkorDB, HelixDB, and SurrealDB
   automatically.
2. Add read-query helpers around the co-speaker meetup traversal for each
   backend.
3. Expand typed properties beyond `tags`, especially dates.
4. Add backend-specific upsert strategies where supported so repeat loads need
   `--replace` less often.
