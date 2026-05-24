use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use anyhow::{Context, Result};
use bythebay_scraper::graph_loader::{
    GraphBackend, GraphData, GraphEdge, GraphLoadConfig, GraphNode, GraphValue, load_graph,
};
use clap::{Parser, ValueEnum};
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(version, about = "Load By the Bay graph JSON into a graph database")]
struct Args {
    #[arg(short, long, default_value = "data/bythebay-graph.json")]
    input: PathBuf,

    #[arg(long, value_enum, default_value_t = InputFormat::Export)]
    input_format: InputFormat,

    #[arg(long, value_enum, default_value_t = GraphBackend::Falkor)]
    backend: GraphBackend,

    #[arg(long, default_value = "redis://127.0.0.1:6379")]
    redis_url: String,

    #[arg(long, default_value = "http://127.0.0.1:8080/v1/query")]
    helix_url: String,

    #[arg(long, default_value = "http://127.0.0.1:8000/sql")]
    surreal_url: String,

    #[arg(long, default_value = "root")]
    surreal_user: String,

    #[arg(long, default_value = "root")]
    surreal_pass: String,

    #[arg(long, default_value = "bythebay")]
    surreal_ns: String,

    #[arg(long, default_value = "graph")]
    surreal_db: String,

    #[arg(short, long, default_value = "bythebay")]
    graph: String,

    #[arg(long)]
    replace: bool,

    #[arg(long, default_value_t = 100)]
    batch_size: usize,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum InputFormat {
    Export,
    TalkRecords,
}

impl From<&Args> for GraphLoadConfig {
    fn from(args: &Args) -> Self {
        Self {
            backend: args.backend,
            redis_url: args.redis_url.clone(),
            helix_url: args.helix_url.clone(),
            surreal_url: args.surreal_url.clone(),
            surreal_user: args.surreal_user.clone(),
            surreal_pass: args.surreal_pass.clone(),
            surreal_ns: args.surreal_ns.clone(),
            surreal_db: args.surreal_db.clone(),
            graph: args.graph.clone(),
            replace: args.replace,
            batch_size: args.batch_size,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GraphExport {
    source_urls: Vec<String>,
    conferences: Vec<Conference>,
    meetups: Vec<MeetupGroup>,
    people: Vec<Person>,
    talks: Vec<Talk>,
    edges: Vec<Edge>,
}

#[derive(Debug, Deserialize)]
struct Conference {
    id: String,
    name: String,
    site: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct MeetupGroup {
    id: String,
    name: String,
    url: String,
    timezone: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Person {
    id: String,
    name: String,
    conference_id: String,
    meetup_id: Option<String>,
    organization: Option<String>,
    title: Option<String>,
    source_url: String,
    source_id: Option<String>,
    source_file: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Talk {
    id: String,
    source_id: Option<String>,
    source_file: Option<String>,
    extracted_file: Option<String>,
    kind: String,
    event_id: Option<String>,
    event_title: Option<String>,
    title: String,
    conference_id: String,
    meetup_id: Option<String>,
    speaker_text: Option<String>,
    tags: Vec<String>,
    url: Option<String>,
    description: Option<String>,
    date_time: Option<String>,
    end_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Edge {
    from: String,
    to: String,
    relationship: String,
}

#[derive(Debug, Deserialize)]
struct TalkRecord {
    nodes: Vec<TalkRecordNode>,
    edges: Vec<TalkRecordEdge>,
}

#[derive(Debug, Deserialize)]
struct TalkRecordNode {
    id: String,
    #[serde(alias = "type", alias = "kind")]
    label: String,
    #[serde(default)]
    properties: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct TalkRecordEdge {
    from: String,
    to: String,
    #[serde(alias = "type", alias = "kind", alias = "relationship")]
    relationship: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let graph = read_graph_data(&args.input, args.input_format)?;
    load_graph(&GraphLoadConfig::from(&args), &graph)
}

fn read_graph_data(input: &PathBuf, format: InputFormat) -> Result<GraphData> {
    match format {
        InputFormat::Export => read_export_graph(input),
        InputFormat::TalkRecords => read_talk_records_graph(input),
    }
}

fn read_export_graph(input: &PathBuf) -> Result<GraphData> {
    let graph_json =
        fs::read_to_string(input).with_context(|| format!("failed to read {}", input.display()))?;
    let export: GraphExport = serde_json::from_str(&graph_json)
        .with_context(|| format!("failed to parse {}", input.display()))?;
    export.to_graph_data()
}

fn read_talk_records_graph(input: &PathBuf) -> Result<GraphData> {
    let mut nodes_by_id: BTreeMap<String, GraphNode> = BTreeMap::new();
    let mut edges_by_key: BTreeMap<(String, String, String), GraphEdge> = BTreeMap::new();

    for path in talk_record_paths(input)? {
        let json = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let record: TalkRecord = serde_json::from_str(&json)
            .with_context(|| format!("failed to parse {}", path.display()))?;

        for node in record.nodes {
            nodes_by_id
                .entry(node.id.clone())
                .or_insert_with(|| talk_record_node(node));
        }
        for edge in record.edges {
            edges_by_key
                .entry((
                    edge.from.clone(),
                    edge.to.clone(),
                    edge.relationship.clone(),
                ))
                .or_insert_with(|| GraphEdge {
                    from: edge.from,
                    to: edge.to,
                    relationship: edge.relationship,
                });
        }
    }

    Ok(GraphData {
        nodes: nodes_by_id.into_values().collect(),
        edges: edges_by_key.into_values().collect(),
    })
}

fn talk_record_paths(input: &PathBuf) -> Result<Vec<PathBuf>> {
    if input.is_file() {
        return Ok(vec![input.clone()]);
    }
    let mut paths = fs::read_dir(input)
        .with_context(|| format!("failed to read input directory {}", input.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("failed to list input directory {}", input.display()))?;
    paths.retain(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"));
    paths.sort();
    Ok(paths)
}

fn talk_record_node(node: TalkRecordNode) -> GraphNode {
    let mut props = node
        .properties
        .into_iter()
        .map(|(key, value)| (key, json_graph_value(value)))
        .collect::<BTreeMap<_, _>>();
    props.insert("id".to_string(), GraphValue::from(node.id.clone()));
    props
        .entry("nid".to_string())
        .or_insert_with(|| GraphValue::from(node.id));
    GraphNode::new(node.label, props)
}

fn json_graph_value(value: serde_json::Value) -> GraphValue {
    match value {
        serde_json::Value::String(value) => GraphValue::String(value),
        serde_json::Value::Array(values) => GraphValue::StringArray(
            values
                .into_iter()
                .filter_map(|value| match value {
                    serde_json::Value::String(value) if !value.is_empty() => Some(value),
                    _ => None,
                })
                .collect(),
        ),
        serde_json::Value::Null => GraphValue::String(String::new()),
        other => GraphValue::String(other.to_string()),
    }
}

impl GraphExport {
    fn to_graph_data(&self) -> Result<GraphData> {
        let mut nodes = Vec::new();
        nodes.extend(source_nodes(&self.source_urls)?);
        nodes.extend(self.conferences.iter().map(conference_node));
        nodes.extend(self.meetups.iter().map(meetup_node));
        nodes.extend(self.people.iter().map(person_node));
        nodes.extend(
            self.talks
                .iter()
                .map(record_node)
                .collect::<Result<Vec<_>>>()?,
        );

        Ok(GraphData {
            nodes,
            edges: self
                .edges
                .iter()
                .map(|edge| GraphEdge {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    relationship: edge.relationship.clone(),
                })
                .collect(),
        })
    }
}

fn source_nodes(source_urls: &[String]) -> Result<Vec<GraphNode>> {
    let mut seen = BTreeSet::new();
    Ok(source_urls
        .iter()
        .filter(|url| seen.insert((*url).clone()))
        .map(|url| {
            GraphNode::new(
                "Source",
                props([
                    ("id", format!("source:{}", slug::slugify(url)).into()),
                    ("url", url.clone().into()),
                ]),
            )
        })
        .collect())
}

fn conference_node(conference: &Conference) -> GraphNode {
    GraphNode::new(
        "Conference",
        props([
            ("id", conference.id.clone().into()),
            ("name", conference.name.clone().into()),
            ("site", conference.site.clone().into()),
            ("url", conference.url.clone().into()),
        ]),
    )
}

fn meetup_node(meetup: &MeetupGroup) -> GraphNode {
    let mut props = props([
        ("id", meetup.id.clone().into()),
        ("name", meetup.name.clone().into()),
        ("url", meetup.url.clone().into()),
    ]);
    insert_optional(&mut props, "timezone", meetup.timezone.as_deref());
    GraphNode::new("Meetup", props)
}

fn person_node(person: &Person) -> GraphNode {
    let mut props = props([
        ("id", person.id.clone().into()),
        ("name", person.name.clone().into()),
        ("conference_id", person.conference_id.clone().into()),
        ("source_url", person.source_url.clone().into()),
    ]);
    insert_optional(&mut props, "meetup_id", person.meetup_id.as_deref());
    insert_optional(&mut props, "organization", person.organization.as_deref());
    insert_optional(&mut props, "title", person.title.as_deref());
    insert_optional(&mut props, "source_id", person.source_id.as_deref());
    insert_optional(&mut props, "source_file", person.source_file.as_deref());
    GraphNode::new("Person", props)
}

fn record_node(talk: &Talk) -> Result<GraphNode> {
    let labels = record_labels(&talk.kind);
    let label = labels.last().copied().unwrap_or("Record").to_string();
    let mut props = props([
        ("id", talk.id.clone().into()),
        ("kind", talk.kind.clone().into()),
        ("title", talk.title.clone().into()),
        ("conference_id", talk.conference_id.clone().into()),
        ("tags_json", serde_json::to_string(&talk.tags)?.into()),
        ("tags_csv", talk.tags.join(", ").into()),
        (
            "tags",
            GraphValue::StringArray(
                talk.tags
                    .iter()
                    .filter(|tag| !tag.is_empty())
                    .cloned()
                    .collect(),
            ),
        ),
        (
            "labels",
            GraphValue::StringArray(labels.iter().map(|label| (*label).to_string()).collect()),
        ),
    ]);
    insert_optional(&mut props, "source_id", talk.source_id.as_deref());
    insert_optional(&mut props, "source_file", talk.source_file.as_deref());
    insert_optional(&mut props, "extracted_file", talk.extracted_file.as_deref());
    insert_optional(&mut props, "event_id", talk.event_id.as_deref());
    insert_optional(&mut props, "event_title", talk.event_title.as_deref());
    insert_optional(&mut props, "meetup_id", talk.meetup_id.as_deref());
    insert_optional(&mut props, "speaker_text", talk.speaker_text.as_deref());
    insert_optional(&mut props, "url", talk.url.as_deref());
    insert_optional(&mut props, "description", talk.description.as_deref());
    insert_optional(&mut props, "date_time", talk.date_time.as_deref());
    insert_optional(&mut props, "end_time", talk.end_time.as_deref());
    Ok(GraphNode::new(label, props))
}

fn record_labels(kind: &str) -> Vec<&'static str> {
    match kind {
        "announcement" => vec!["Record", "Announcement"],
        "event" => vec!["Record", "Event"],
        "social" => vec!["Record", "SocialEvent"],
        "talk" => vec!["Record", "Talk"],
        "unconference" => vec!["Record", "Unconference"],
        "workshop" => vec!["Record", "Workshop"],
        _ => vec!["Record"],
    }
}

fn props<const N: usize>(items: [(&str, GraphValue); N]) -> BTreeMap<String, GraphValue> {
    items
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn insert_optional(props: &mut BTreeMap<String, GraphValue>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        props.insert(key.to_string(), value.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_export_to_backend_neutral_graph() {
        let export = GraphExport {
            source_urls: vec!["https://example.test/source".to_string()],
            conferences: vec![Conference {
                id: "conference-1".to_string(),
                name: "By the Bay".to_string(),
                site: "bythebay".to_string(),
                url: "https://bythebay.io".to_string(),
            }],
            meetups: vec![],
            people: vec![Person {
                id: "person-1".to_string(),
                name: "Ada".to_string(),
                conference_id: "conference-1".to_string(),
                meetup_id: None,
                organization: None,
                title: None,
                source_url: "https://example.test/source".to_string(),
                source_id: None,
                source_file: None,
            }],
            talks: vec![Talk {
                id: "talk-1".to_string(),
                source_id: None,
                source_file: None,
                extracted_file: None,
                kind: "talk".to_string(),
                event_id: None,
                event_title: None,
                title: "Graph Loading".to_string(),
                conference_id: "conference-1".to_string(),
                meetup_id: None,
                speaker_text: Some("Ada".to_string()),
                tags: vec!["rust".to_string()],
                url: None,
                description: None,
                date_time: None,
                end_time: None,
            }],
            edges: vec![Edge {
                from: "person-1".to_string(),
                to: "talk-1".to_string(),
                relationship: "presents".to_string(),
            }],
        };

        let graph = export.to_graph_data().unwrap();
        assert_eq!(graph.nodes.len(), 4);
        assert_eq!(graph.edges.len(), 1);
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.label == "Talk" && node.props.contains_key("tags"))
        );
    }

    #[test]
    fn converts_talk_records_to_backend_neutral_graph() {
        let record = TalkRecord {
            nodes: vec![
                TalkRecordNode {
                    id: "talk:1".to_string(),
                    label: "Talk".to_string(),
                    properties: serde_json::Map::from_iter([
                        (
                            "title".to_string(),
                            serde_json::Value::String("Graph Loading".to_string()),
                        ),
                        ("tags".to_string(), serde_json::json!(["rust", "graphs"])),
                    ]),
                },
                TalkRecordNode {
                    id: "speaker:ada".to_string(),
                    label: "Speaker".to_string(),
                    properties: serde_json::Map::from_iter([(
                        "name".to_string(),
                        serde_json::Value::String("Ada".to_string()),
                    )]),
                },
            ],
            edges: vec![TalkRecordEdge {
                from: "talk:1".to_string(),
                to: "speaker:ada".to_string(),
                relationship: "PRESENTED_BY".to_string(),
            }],
        };

        let mut nodes_by_id = BTreeMap::new();
        for node in record.nodes {
            nodes_by_id.insert(node.id.clone(), talk_record_node(node));
        }

        let graph = GraphData {
            nodes: nodes_by_id.into_values().collect(),
            edges: record
                .edges
                .into_iter()
                .map(|edge| GraphEdge {
                    from: edge.from,
                    to: edge.to,
                    relationship: edge.relationship,
                })
                .collect(),
        };

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert!(graph.nodes.iter().any(|node| {
            node.label == "Talk"
                && node.props.get("id") == Some(&GraphValue::from("talk:1"))
                && node.props.get("nid") == Some(&GraphValue::from("talk:1"))
        }));
        assert!(graph.nodes.iter().any(|node| {
            node.label == "Talk"
                && node.props.get("tags")
                    == Some(&GraphValue::StringArray(vec![
                        "rust".to_string(),
                        "graphs".to_string(),
                    ]))
        }));
    }
}
