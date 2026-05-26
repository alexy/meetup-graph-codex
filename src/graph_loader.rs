use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose};
use clap::ValueEnum;
use helix_db::{Client as HelixClient, dsl::prelude::*};
use redis::{Client as RedisClient, Value as RedisValue};
use serde_json::json;
use surrealdb::{Surreal, engine::remote::ws::Ws, opt::auth::Root};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum GraphBackend {
    Falkor,
    HelixHttp,
    HelixRustSdk,
    Surrealdb,
    SurrealdbRustSdk,
}

#[derive(Debug, Clone)]
pub struct GraphLoadConfig {
    pub backend: GraphBackend,
    pub redis_url: String,
    pub helix_url: String,
    pub surreal_url: String,
    pub surreal_user: String,
    pub surreal_pass: String,
    pub surreal_ns: String,
    pub surreal_db: String,
    pub graph: String,
    pub replace: bool,
    pub batch_size: usize,
}

impl Default for GraphLoadConfig {
    fn default() -> Self {
        Self {
            backend: GraphBackend::Falkor,
            redis_url: "redis://127.0.0.1:6379".to_string(),
            helix_url: "http://127.0.0.1:8080/v1/query".to_string(),
            surreal_url: "http://127.0.0.1:8000/sql".to_string(),
            surreal_user: "root".to_string(),
            surreal_pass: "root".to_string(),
            surreal_ns: "bythebay".to_string(),
            surreal_db: "graph".to_string(),
            graph: "bythebay".to_string(),
            replace: false,
            batch_size: 100,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone)]
pub struct GraphNode {
    pub label: String,
    pub props: BTreeMap<String, GraphValue>,
}

impl GraphNode {
    pub fn new(label: impl Into<String>, props: BTreeMap<String, GraphValue>) -> Self {
        Self {
            label: label.into(),
            props,
        }
    }

    fn id(&self) -> Result<&str> {
        match self.props.get("id") {
            Some(GraphValue::String(id)) => Ok(id),
            _ => bail!("all graph nodes must include a string id property"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphValue {
    String(String),
    StringArray(Vec<String>),
}

impl From<String> for GraphValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for GraphValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub relationship: String,
}

pub fn load_graph(config: &GraphLoadConfig, graph: &GraphData) -> Result<()> {
    match config.backend {
        GraphBackend::Falkor => FalkorLoader.load(config, graph),
        GraphBackend::HelixHttp => HelixHttpLoader.load(config, graph),
        GraphBackend::HelixRustSdk => HelixRustSdkLoader.load(config, graph),
        GraphBackend::Surrealdb => SurrealHttpLoader.load(config, graph),
        GraphBackend::SurrealdbRustSdk => SurrealRustSdkLoader.load(config, graph),
    }
}

trait GraphLoader {
    fn load(&self, config: &GraphLoadConfig, graph: &GraphData) -> Result<()>;
}

struct FalkorLoader;
struct HelixHttpLoader;
struct HelixRustSdkLoader;
struct SurrealHttpLoader;
struct SurrealRustSdkLoader;

const GRAPH_LABELS: &[&str] = &[
    "Talk",
    "Announcement",
    "Workshop",
    "Unconference",
    "SocialEvent",
    "Event",
    "Record",
    "Person",
    "Speaker",
    "Company",
    "Project",
    "Meetup",
    "Group",
    "Conference",
    "Source",
];

impl GraphLoader for FalkorLoader {
    fn load(&self, config: &GraphLoadConfig, graph: &GraphData) -> Result<()> {
        let client = RedisClient::open(config.redis_url.as_str())
            .with_context(|| format!("invalid Redis/FalkorDB URL {}", config.redis_url))?;
        let mut connection = client
            .get_connection()
            .with_context(|| format!("failed to connect to {}", config.redis_url))?;

        if config.replace {
            delete_falkor_graph_if_exists(&mut connection, &config.graph)?;
        }

        for node in &graph.nodes {
            let query = format!(
                "MERGE (n:{} {{id:{}}}) SET n += {}",
                falkor_labels(node),
                cypher_string(node.id()?),
                cypher_map(&node.props)
            );
            falkor_query(&mut connection, &config.graph, &query)?;
        }
        for edge in &graph.edges {
            let query = format!(
                "MATCH (a {{id:{}}}), (b {{id:{}}}) MERGE (a)-[:{}]->(b)",
                cypher_string(&edge.from),
                cypher_string(&edge.to),
                relationship_type(&edge.relationship)
            );
            falkor_query(&mut connection, &config.graph, &query)?;
        }

        println!(
            "loaded {} nodes and {} edges into FalkorDB graph '{}'",
            graph.nodes.len(),
            graph.edges.len(),
            config.graph
        );
        Ok(())
    }
}

impl GraphLoader for HelixHttpLoader {
    fn load(&self, config: &GraphLoadConfig, graph: &GraphData) -> Result<()> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("failed to build Helix HTTP client")?;

        if config.replace {
            post_helix(&client, &config.helix_url, &helix_drop_labels_request())?;
        }
        for chunk in graph.nodes.chunks(config.batch_size.max(1)) {
            post_helix(&client, &config.helix_url, &helix_add_nodes_request(chunk)?)?;
        }
        for chunk in graph.edges.chunks(config.batch_size.max(1)) {
            post_helix(&client, &config.helix_url, &helix_add_edges_request(chunk)?)?;
        }

        println!(
            "loaded {} nodes and {} edges into Helix at {}",
            graph.nodes.len(),
            graph.edges.len(),
            config.helix_url
        );
        Ok(())
    }
}

impl GraphLoader for HelixRustSdkLoader {
    fn load(&self, config: &GraphLoadConfig, graph: &GraphData) -> Result<()> {
        let base_url = helix_base_url(&config.helix_url)?;
        let client = HelixClient::new(Some(&base_url))
            .map_err(|err| anyhow::anyhow!("failed to build Helix SDK client: {err}"))?;
        let runtime = tokio::runtime::Runtime::new().context("failed to create Tokio runtime")?;

        if config.replace {
            runtime.block_on(post_helix_sdk_drop_labels(&client))?;
        }
        for chunk in graph.nodes.chunks(config.batch_size.max(1)) {
            runtime.block_on(post_helix_sdk_nodes(&client, chunk))?;
        }
        for chunk in graph.edges.chunks(config.batch_size.max(1)) {
            runtime.block_on(post_helix_sdk_edges(&client, chunk))?;
        }

        println!(
            "loaded {} nodes and {} edges into Helix through the Rust SDK at {}",
            graph.nodes.len(),
            graph.edges.len(),
            base_url
        );
        Ok(())
    }
}

impl GraphLoader for SurrealHttpLoader {
    fn load(&self, config: &GraphLoadConfig, graph: &GraphData) -> Result<()> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("failed to build SurrealDB HTTP client")?;
        let id_tables = surreal_id_tables(&graph.nodes)?;

        post_surrealdb_bootstrap(&client, config)?;
        if config.replace {
            post_surrealdb(&client, config, &surreal_delete_tables_query())?;
        }
        for chunk in graph.nodes.chunks(config.batch_size.max(1)) {
            post_surrealdb(&client, config, &surreal_create_nodes_query(chunk)?)?;
        }
        for chunk in graph.edges.chunks(config.batch_size.max(1)) {
            post_surrealdb(
                &client,
                config,
                &surreal_create_edges_query(chunk, &id_tables)?,
            )?;
        }

        println!(
            "loaded {} nodes and {} edges into SurrealDB at {} namespace '{}' database '{}'",
            graph.nodes.len(),
            graph.edges.len(),
            config.surreal_url,
            config.surreal_ns,
            config.surreal_db
        );
        Ok(())
    }
}

impl GraphLoader for SurrealRustSdkLoader {
    fn load(&self, config: &GraphLoadConfig, graph: &GraphData) -> Result<()> {
        let runtime = tokio::runtime::Runtime::new().context("failed to create Tokio runtime")?;
        runtime.block_on(load_surrealdb_rust_sdk_async(config, graph))
    }
}

async fn load_surrealdb_rust_sdk_async(config: &GraphLoadConfig, graph: &GraphData) -> Result<()> {
    let address = surreal_ws_address(&config.surreal_url)?;
    let db = Surreal::new::<Ws>(&address)
        .await
        .with_context(|| format!("failed to connect to SurrealDB at {address}"))?;
    db.signin(Root {
        username: config.surreal_user.clone(),
        password: config.surreal_pass.clone(),
    })
    .await
    .context("failed to authenticate with SurrealDB")?;
    post_surrealdb_sdk_bootstrap(&db, config).await?;
    db.use_ns(&config.surreal_ns)
        .use_db(&config.surreal_db)
        .await
        .context("failed to select SurrealDB namespace/database")?;

    let id_tables = surreal_id_tables(&graph.nodes)?;
    if config.replace {
        post_surrealdb_sdk(&db, &surreal_delete_tables_query()).await?;
    }
    for chunk in graph.nodes.chunks(config.batch_size.max(1)) {
        post_surrealdb_sdk(&db, &surreal_create_nodes_query(chunk)?).await?;
    }
    for chunk in graph.edges.chunks(config.batch_size.max(1)) {
        post_surrealdb_sdk(&db, &surreal_create_edges_query(chunk, &id_tables)?).await?;
    }

    println!(
        "loaded {} nodes and {} edges into SurrealDB through the Rust SDK at {} namespace '{}' database '{}'",
        graph.nodes.len(),
        graph.edges.len(),
        address,
        config.surreal_ns,
        config.surreal_db
    );
    Ok(())
}

fn delete_falkor_graph_if_exists(connection: &mut redis::Connection, graph: &str) -> Result<()> {
    match redis::cmd("GRAPH.DELETE")
        .arg(graph)
        .query::<RedisValue>(connection)
    {
        Ok(_) => Ok(()),
        Err(err) if err.to_string().contains("Invalid graph operation") => Ok(()),
        Err(err) if err.to_string().contains("graph not found") => Ok(()),
        Err(err) if err.to_string().contains("does not exist") => Ok(()),
        Err(err) => Err(err).context("failed to delete existing FalkorDB graph"),
    }
}

fn falkor_query(
    connection: &mut redis::Connection,
    graph: &str,
    query: &str,
) -> Result<RedisValue> {
    redis::cmd("GRAPH.QUERY")
        .arg(graph)
        .arg(query)
        .query::<RedisValue>(connection)
        .with_context(|| format!("FalkorDB query failed: {query}"))
}

fn falkor_labels(node: &GraphNode) -> String {
    node.props
        .get("labels")
        .and_then(|value| match value {
            GraphValue::StringArray(labels) => Some(labels.as_slice()),
            _ => None,
        })
        .map(|labels| labels.join(":"))
        .unwrap_or_else(|| node.label.clone())
}

fn helix_add_nodes_request(nodes: &[GraphNode]) -> Result<serde_json::Value> {
    let queries = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            json!({
                "Query": {
                    "name": format!("created_{index}"),
                    "steps": [{
                        "AddN": {
                            "label": node.label,
                            "properties": helix_http_properties(node)
                        }
                    }],
                    "condition": null
                }
            })
        })
        .collect::<Vec<_>>();
    let returns = (0..nodes.len())
        .map(|index| format!("created_{index}"))
        .collect::<Vec<_>>();
    Ok(json!({
        "request_type": "write",
        "query": {"queries": queries, "returns": returns},
        "parameters": {},
        "parameter_types": {}
    }))
}

fn helix_http_properties(node: &GraphNode) -> Vec<serde_json::Value> {
    node.props
        .iter()
        .filter_map(|(key, value)| match (key.as_str(), value) {
            ("labels", _) => None,
            (_, GraphValue::String(value)) => Some(json!([key, {"Value": {"String": value}}])),
            (_, GraphValue::StringArray(_)) => None,
        })
        .collect()
}

fn helix_add_edges_request(edges: &[GraphEdge]) -> Result<serde_json::Value> {
    let mut queries = Vec::with_capacity(edges.len() * 2);
    let mut returns = Vec::with_capacity(edges.len());
    for (index, edge) in edges.iter().enumerate() {
        let target_name = format!("target_{index}");
        let linked_name = format!("linked_{index}");
        queries.push(json!({
            "Query": {
                "name": target_name,
                "steps": [{"NWhere": {"Eq": ["id", {"String": edge.to}]}}],
                "condition": null
            }
        }));
        queries.push(json!({
            "Query": {
                "name": linked_name,
                "steps": [
                    {"NWhere": {"Eq": ["id", {"String": edge.from}]}},
                    {
                        "AddE": {
                            "label": relationship_type(&edge.relationship),
                            "to": {"Var": target_name},
                            "properties": []
                        }
                    }
                ],
                "condition": null
            }
        }));
        returns.push(linked_name);
    }
    Ok(json!({
        "request_type": "write",
        "query": {"queries": queries, "returns": returns},
        "parameters": {},
        "parameter_types": {}
    }))
}

fn helix_drop_labels_request() -> serde_json::Value {
    let queries = GRAPH_LABELS
        .iter()
        .enumerate()
        .map(|(index, label)| {
            json!({
                "Query": {
                    "name": format!("drop_{index}"),
                    "steps": [{"NWhere": {"Eq": ["$label", {"String": label}]}}, "Drop"],
                    "condition": null
                }
            })
        })
        .collect::<Vec<_>>();
    json!({
        "request_type": "write",
        "query": {"queries": queries, "returns": []},
        "parameters": {},
        "parameter_types": {}
    })
}

fn post_helix(
    client: &reqwest::blocking::Client,
    helix_url: &str,
    request: &serde_json::Value,
) -> Result<()> {
    let response = client
        .post(helix_url)
        .json(request)
        .send()
        .with_context(|| format!("failed to POST Helix query to {helix_url}"))?;
    let status = response.status();
    let body = response.text().context("failed to read Helix response")?;
    if !status.is_success() {
        bail!("Helix query failed with status {status}: {body}");
    }
    Ok(())
}

async fn post_helix_sdk_nodes(client: &HelixClient, nodes: &[GraphNode]) -> Result<()> {
    let mut batch = write_batch();
    let mut returns = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        let name = format!("created_{index}");
        batch = batch.var_as(
            &name,
            g().add_n(node.label.clone(), helix_sdk_properties(node)),
        );
        returns.push(name);
    }
    let request = DynamicQueryRequest::write(batch.returning(returns));
    let _: serde_json::Value = client
        .query()
        .dynamic_query(request)
        .send()
        .await
        .map_err(|err| anyhow::anyhow!("Helix SDK node write failed: {err}"))?;
    Ok(())
}

fn helix_sdk_properties(node: &GraphNode) -> Vec<(String, PropertyInput)> {
    node.props
        .iter()
        .filter_map(|(key, value)| {
            if key == "labels" {
                return None;
            }
            match value {
                GraphValue::String(value) => {
                    Some((key.clone(), PropertyInput::from(value.clone())))
                }
                GraphValue::StringArray(_) => None,
            }
        })
        .collect()
}

async fn post_helix_sdk_edges(client: &HelixClient, edges: &[GraphEdge]) -> Result<()> {
    let mut batch = write_batch();
    let mut returns = Vec::with_capacity(edges.len());
    for (index, edge) in edges.iter().enumerate() {
        let target_name = format!("target_{index}");
        let linked_name = format!("linked_{index}");
        batch = batch
            .var_as(
                &target_name,
                g().n_where(SourcePredicate::eq("id", edge.to.clone())),
            )
            .var_as(
                &linked_name,
                g().n_where(SourcePredicate::eq("id", edge.from.clone()))
                    .add_e(
                        relationship_type(&edge.relationship),
                        NodeRef::var(&target_name),
                        Vec::<(String, String)>::new(),
                    ),
            );
        returns.push(linked_name);
    }
    let request = DynamicQueryRequest::write(batch.returning(returns));
    let _: serde_json::Value = client
        .query()
        .dynamic_query(request)
        .send()
        .await
        .map_err(|err| anyhow::anyhow!("Helix SDK edge write failed: {err}"))?;
    Ok(())
}

async fn post_helix_sdk_drop_labels(client: &HelixClient) -> Result<()> {
    let mut batch = write_batch();
    for (index, label) in GRAPH_LABELS.iter().enumerate() {
        batch = batch.var_as(
            &format!("drop_{index}"),
            g().n_where(SourcePredicate::eq("$label", *label)).drop(),
        );
    }
    let request = DynamicQueryRequest::write(batch.returning(Vec::<String>::new()));
    let _: serde_json::Value = client
        .query()
        .dynamic_query(request)
        .send()
        .await
        .map_err(|err| anyhow::anyhow!("Helix SDK replace/drop failed: {err}"))?;
    Ok(())
}

fn helix_base_url(helix_url: &str) -> Result<String> {
    Ok(helix_url
        .strip_suffix("/v1/query")
        .unwrap_or(helix_url)
        .trim_end_matches('/')
        .to_string())
}

fn post_surrealdb(
    client: &reqwest::blocking::Client,
    config: &GraphLoadConfig,
    query: &str,
) -> Result<()> {
    let auth = general_purpose::STANDARD
        .encode(format!("{}:{}", config.surreal_user, config.surreal_pass));
    let response = client
        .post(&config.surreal_url)
        .header("Authorization", format!("Basic {auth}"))
        .header("Surreal-NS", &config.surreal_ns)
        .header("Surreal-DB", &config.surreal_db)
        .header("Accept", "application/json")
        .header("Content-Type", "application/surrealql")
        .body(query.to_string())
        .send()
        .with_context(|| format!("failed to POST SurrealQL to {}", config.surreal_url))?;
    let status = response.status();
    let body = response
        .text()
        .context("failed to read SurrealDB response")?;
    if !status.is_success() {
        bail!("SurrealDB query failed with status {status}: {body}");
    }
    if let Ok(results) = serde_json::from_str::<serde_json::Value>(&body) {
        if surreal_response_has_error(&results) {
            bail!("SurrealDB query returned an error: {body}");
        }
    }
    Ok(())
}

fn post_surrealdb_bootstrap(
    client: &reqwest::blocking::Client,
    config: &GraphLoadConfig,
) -> Result<()> {
    let query = surreal_bootstrap_query(config);
    let auth = general_purpose::STANDARD
        .encode(format!("{}:{}", config.surreal_user, config.surreal_pass));
    let response = client
        .post(&config.surreal_url)
        .header("Authorization", format!("Basic {auth}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/surrealql")
        .body(query)
        .send()
        .with_context(|| format!("failed to bootstrap SurrealDB at {}", config.surreal_url))?;
    let status = response.status();
    let body = response
        .text()
        .context("failed to read SurrealDB response")?;
    if !status.is_success() {
        bail!("SurrealDB bootstrap failed with status {status}: {body}");
    }
    if let Ok(results) = serde_json::from_str::<serde_json::Value>(&body) {
        if surreal_response_has_non_idempotent_error(&results) {
            bail!("SurrealDB bootstrap returned an error: {body}");
        }
    }
    Ok(())
}

async fn post_surrealdb_sdk_bootstrap<C>(db: &Surreal<C>, config: &GraphLoadConfig) -> Result<()>
where
    C: surrealdb::Connection,
{
    match db.query(surreal_bootstrap_query(config)).await {
        Ok(_) => Ok(()),
        Err(err) if err.to_string().contains("already exists") => Ok(()),
        Err(err) => Err(anyhow::anyhow!("SurrealDB SDK bootstrap failed: {err}")),
    }
}

async fn post_surrealdb_sdk<C>(db: &Surreal<C>, query: &str) -> Result<()>
where
    C: surrealdb::Connection,
{
    db.query(query)
        .await
        .map(|_| ())
        .map_err(|err| anyhow::anyhow!("SurrealDB SDK query failed: {err}"))
}

fn surreal_bootstrap_query(config: &GraphLoadConfig) -> String {
    format!(
        "DEFINE NAMESPACE {}; USE NS {}; DEFINE DATABASE {};",
        surreal_identifier(&config.surreal_ns),
        surreal_identifier(&config.surreal_ns),
        surreal_identifier(&config.surreal_db)
    )
}

fn surreal_response_has_error(value: &serde_json::Value) -> bool {
    value.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.get("status").and_then(|status| status.as_str()) == Some("ERR"))
    })
}

fn surreal_response_has_non_idempotent_error(value: &serde_json::Value) -> bool {
    value.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("status").and_then(|status| status.as_str()) == Some("ERR")
                && item.get("kind").and_then(|kind| kind.as_str()) != Some("AlreadyExists")
        })
    })
}

fn surreal_id_tables(nodes: &[GraphNode]) -> Result<BTreeMap<String, String>> {
    nodes
        .iter()
        .map(|node| Ok((node.id()?.to_string(), surreal_table_name(&node.label))))
        .collect()
}

fn surreal_delete_tables_query() -> String {
    let mut tables = GRAPH_LABELS
        .iter()
        .map(|label| surreal_table_name(label))
        .collect::<BTreeSet<_>>();
    tables.insert("record".to_string());
    tables
        .into_iter()
        .map(|table| format!("DELETE {table};"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn surreal_create_nodes_query(nodes: &[GraphNode]) -> Result<String> {
    nodes
        .iter()
        .map(|node| {
            Ok(format!(
                "CREATE type::record({}, {}) SET {};",
                surreal_string(&surreal_table_name(&node.label)),
                surreal_string(node.id()?),
                surreal_node_props(node)?
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|statements| statements.join("\n"))
}

fn surreal_node_props(node: &GraphNode) -> Result<String> {
    Ok(node
        .props
        .iter()
        .filter(|(key, _)| key.as_str() != "labels")
        .map(|(key, value)| match value {
            GraphValue::String(value) => Ok(format!("{key} = {}", surreal_string(value))),
            GraphValue::StringArray(values) => {
                Ok(format!("{key} = {}", serde_json::to_string(values)?))
            }
        })
        .collect::<Result<Vec<_>>>()?
        .join(", "))
}

fn surreal_create_edges_query(
    edges: &[GraphEdge],
    id_tables: &BTreeMap<String, String>,
) -> Result<String> {
    edges
        .iter()
        .map(|edge| {
            let from_table = id_tables
                .get(&edge.from)
                .with_context(|| format!("missing source node for edge {}", edge.from))?;
            let to_table = id_tables
                .get(&edge.to)
                .with_context(|| format!("missing target node for edge {}", edge.to))?;
            Ok(format!(
                "RELATE (type::record({}, {}))->{}->(type::record({}, {})) SET relationship = {};",
                surreal_string(from_table),
                surreal_string(&edge.from),
                surreal_table_name(&relationship_type(&edge.relationship)),
                surreal_string(to_table),
                surreal_string(&edge.to),
                surreal_string(&edge.relationship)
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|statements| statements.join("\n"))
}

fn relationship_type(value: &str) -> String {
    let rel = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if rel.is_empty() {
        "RELATED_TO".to_string()
    } else {
        rel
    }
}

fn surreal_table_name(value: &str) -> String {
    let table = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if table.is_empty() {
        "related_to".to_string()
    } else {
        table
    }
}

fn surreal_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization cannot fail")
}

fn surreal_identifier(value: &str) -> String {
    let identifier = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if identifier.is_empty() {
        "default".to_string()
    } else {
        identifier
    }
}

fn surreal_ws_address(surreal_url: &str) -> Result<String> {
    let parsed = url::Url::parse(surreal_url)
        .with_context(|| format!("invalid SurrealDB URL {surreal_url}"))?;
    let host = parsed
        .host_str()
        .with_context(|| format!("SurrealDB URL has no host: {surreal_url}"))?;
    Ok(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn cypher_map(props: &BTreeMap<String, GraphValue>) -> String {
    let body = props
        .iter()
        .filter(|(key, _)| key.as_str() != "labels")
        .map(|(key, value)| match value {
            GraphValue::String(value) => format!("{key}:{}", cypher_string(value)),
            GraphValue::StringArray(values) => format!(
                "{key}:[{}]",
                values
                    .iter()
                    .map(|value| cypher_string(value))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{body}}}")
}

fn cypher_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('\'');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\'' => escaped.push_str("\\'"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped.push('\'');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_graph() -> GraphData {
        let mut talk_props = BTreeMap::new();
        talk_props.insert("id".to_string(), GraphValue::from("talk-1"));
        talk_props.insert("title".to_string(), GraphValue::from("A Talk"));
        talk_props.insert(
            "tags".to_string(),
            GraphValue::StringArray(vec!["rust".to_string(), "graphs".to_string()]),
        );

        let mut person_props = BTreeMap::new();
        person_props.insert("id".to_string(), GraphValue::from("person-1"));
        person_props.insert("name".to_string(), GraphValue::from("Ada Lovelace"));

        GraphData {
            nodes: vec![
                GraphNode::new("Talk", talk_props),
                GraphNode::new("Person", person_props),
            ],
            edges: vec![GraphEdge {
                from: "person-1".to_string(),
                to: "talk-1".to_string(),
                relationship: "presents".to_string(),
            }],
        }
    }

    #[test]
    fn helix_http_node_batch_omits_arrays() {
        let graph = sample_graph();
        let request = helix_add_nodes_request(&graph.nodes).unwrap();
        let properties = &request["query"]["queries"][0]["Query"]["steps"][0]["AddN"]["properties"];
        assert!(
            !properties
                .as_array()
                .unwrap()
                .iter()
                .any(|prop| prop[0] == "tags")
        );
    }

    #[test]
    fn helix_http_edge_batch_uses_target_variable() {
        let graph = sample_graph();
        let request = helix_add_edges_request(&graph.edges).unwrap();
        assert_eq!(
            request["query"]["queries"][1]["Query"]["steps"][1]["AddE"]["to"]["Var"],
            "target_0"
        );
    }

    #[test]
    fn surrealdb_batches_complete_sample_graph() {
        let graph = sample_graph();
        let id_tables = surreal_id_tables(&graph.nodes).unwrap();
        let node_query = surreal_create_nodes_query(&graph.nodes).unwrap();
        let edge_query = surreal_create_edges_query(&graph.edges, &id_tables).unwrap();

        assert!(node_query.contains("CREATE type::record(\"talk\", \"talk-1\")"));
        assert!(node_query.contains("tags = [\"rust\",\"graphs\"]"));
        assert!(edge_query.contains("->presents->"));
        assert!(edge_query.contains("type::record(\"person\", \"person-1\")"));
        assert!(edge_query.contains("type::record(\"talk\", \"talk-1\")"));
    }

    #[test]
    fn replace_queries_cover_known_labels() {
        assert_eq!(
            helix_drop_labels_request()["query"]["queries"]
                .as_array()
                .unwrap()
                .len(),
            GRAPH_LABELS.len()
        );
        let surreal_query = surreal_delete_tables_query();
        assert!(surreal_query.contains("DELETE talk;"));
        assert!(surreal_query.contains("DELETE person;"));
        assert!(surreal_query.contains("DELETE source;"));
    }

    #[test]
    fn strips_v1_query_for_sdk_base_url() {
        assert_eq!(
            helix_base_url("http://127.0.0.1:8080/v1/query").unwrap(),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn surrealdb_sdk_uses_ws_address_from_http_sql_url() {
        assert_eq!(
            surreal_ws_address("http://127.0.0.1:8000/sql").unwrap(),
            "127.0.0.1:8000"
        );
    }
}
