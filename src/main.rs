use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use chrono::Local;
use clap::{Parser, ValueEnum};
use regex::Regex;
use reqwest::Client;
use scraper::Html;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use slug::slugify;
use url::Url;

const SCALE_SPEAKERS_URL: &str = "https://scale.bythebay.io/speakers";
const AI_SPEAKERS_URL: &str = "https://ai.bythebay.io/speakers";
const AI_TALKS_URL: &str = "https://ai.bythebay.io/talks";
const DEFAULT_MEETUP_URLS: [&str; 9] = [
    "https://www.meetup.com/bythebay/events/?type=past",
    "https://www.meetup.com/bay-area-ai/events/?type=past",
    "https://www.meetup.com/sf-scala/events/?type=past",
    "https://www.meetup.com/unstructured-data-sf/events/?type=past",
    "https://www.meetup.com/hadoopsf/events/?type=past",
    "https://www.meetup.com/graphql-by-the-bay/events/?type=past",
    "https://www.meetup.com/scala-bay/events/?type=past",
    "https://www.meetup.com/sf-data-and-ai-engineering/events/?type=past",
    "https://www.meetup.com/big-data-developers-in-nyc/events/?type=past",
];
const MEETUP_GQL_URL: &str = "https://www.meetup.com/gql2";
const MEETUP_PAST_EVENTS_QUERY: &str = r#"
query getPastGroupEvents($urlname: String!, $after: String, $beforeDateTime: DateTime) {
  groupByUrlname(urlname: $urlname) {
    id
    name
    timezone
    events(
      filter: { status: [ACTIVE, PAST, CANCELLED], beforeDateTime: $beforeDateTime }
      sort: DESC
      first: 10
      after: $after
    ) {
      totalCount
      pageInfo {
        endCursor
        hasNextPage
      }
      edges {
        node {
          id
          title
          eventUrl
          description
          dateTime
          endTime
          status
          eventType
          isOnline
          group {
            id
            name
            timezone
          }
        }
      }
    }
  }
}
"#;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Scrape By the Bay speaker and talk data into graph-oriented JSON"
)]
struct Args {
    #[arg(short, long, default_value = "data/bythebay-graph.json")]
    output: PathBuf,

    #[arg(long, default_value = "data/raw")]
    raw_dir: PathBuf,

    #[arg(long)]
    no_raw: bool,

    #[arg(long = "meetup-url")]
    meetup_urls: Vec<String>,

    #[arg(long, value_enum, default_value_t = Extractor::Llm)]
    extractor: Extractor,

    #[arg(long, default_value = "gpt-5.5")]
    openai_model: String,

    #[arg(long, default_value = "https://api.openai.com/v1/responses")]
    openai_responses_url: String,
}

#[derive(Clone, Debug, ValueEnum)]
enum Extractor {
    Regex,
    Llm,
}

#[derive(Debug, Serialize)]
struct GraphExport {
    source_urls: Vec<String>,
    conferences: Vec<Conference>,
    meetups: Vec<MeetupGroup>,
    people: Vec<Person>,
    talks: Vec<Talk>,
    edges: Vec<Edge>,
}

#[derive(Debug, Serialize)]
struct Conference {
    id: String,
    name: String,
    site: String,
    url: String,
}

#[derive(Clone, Debug, Serialize)]
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

#[derive(Clone, Debug, Serialize)]
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

#[derive(Clone, Debug, Serialize)]
struct MeetupGroup {
    id: String,
    name: String,
    url: String,
    timezone: Option<String>,
}

#[derive(Debug, Serialize)]
struct Edge {
    from: String,
    to: String,
    relationship: String,
}

#[derive(Debug, Serialize)]
struct RawSpeaker {
    name: String,
    role: Option<String>,
    talk_title: Option<String>,
    talk_url: Option<String>,
    source_id: String,
    source_file: String,
    extracted_file: String,
}

#[derive(Clone, Debug, Serialize)]
struct RawMeetupSpeaker {
    name: String,
    company: Option<String>,
}

#[derive(Debug, Serialize)]
struct RawTalk {
    title: String,
    speaker_text: String,
    tags: Vec<String>,
    url: Option<String>,
    description: Option<String>,
    source_id: String,
    source_file: String,
    extracted_file: String,
}

#[derive(Debug)]
struct RawMeetup {
    id: String,
    name: String,
    url: String,
    timezone: Option<String>,
    events: Vec<RawMeetupEvent>,
}

#[derive(Debug)]
struct RawMeetupEvent {
    id: String,
    title: String,
    url: String,
    kind: String,
    tags: Vec<String>,
    description: Option<String>,
    date_time: Option<String>,
    end_time: Option<String>,
    speakers: Vec<RawMeetupSpeaker>,
    sessions: Vec<RawMeetupSession>,
    source_id: String,
    source_file: String,
    extracted_file: String,
    source_json: Value,
}

#[derive(Clone, Debug, Serialize)]
struct RawMeetupSession {
    title: String,
    description: Option<String>,
    speakers: Vec<RawMeetupSpeaker>,
    kind: String,
    source_id: String,
    source_file: String,
    extracted_file: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let client = Client::builder()
        .user_agent("bythebay-scraper/0.1 (+https://bythebay.io)")
        .build()
        .context("failed to build HTTP client")?;

    let scale_speakers_html = fetch(&client, SCALE_SPEAKERS_URL).await?;
    let ai_speakers_html = fetch(&client, AI_SPEAKERS_URL).await?;
    let ai_talks_html = fetch(&client, AI_TALKS_URL).await?;
    let mut meetup_results = Vec::new();
    let run_date = Local::now().format("%Y%m%d").to_string();
    let meetup_urls = if args.meetup_urls.is_empty() {
        DEFAULT_MEETUP_URLS
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    } else {
        args.meetup_urls.clone()
    };

    for meetup_url in &meetup_urls {
        meetup_results.push(fetch_meetup_archive(&client, meetup_url).await?);
    }

    let mut meetups = meetup_results
        .into_iter()
        .map(|(meetup, _)| meetup)
        .collect::<Vec<_>>();

    let (mut scale_speakers, mut ai_speakers, mut ai_talks) = match args.extractor {
        Extractor::Regex => (
            parse_scale_speakers(&scale_speakers_html)?,
            parse_ai_speakers(&ai_speakers_html)?,
            parse_ai_talks(&ai_talks_html)?,
        ),
        Extractor::Llm => {
            let llm = LlmExtractor::new(&client, &args.openai_responses_url, &args.openai_model)?;
            let mut scale_speakers = llm
                .extract_scale_speakers(&scale_speakers_html)
                .await
                .context("failed to extract Scale By the Bay speakers with LLM")?;
            let mut ai_speakers = llm
                .extract_ai_speakers(&ai_speakers_html)
                .await
                .context("failed to extract AI By the Bay speakers with LLM")?;
            let mut ai_talks = llm
                .extract_ai_talks(&ai_talks_html)
                .await
                .context("failed to extract AI By the Bay talks with LLM")?;
            llm.enrich_meetups(&mut meetups)
                .await
                .context("failed to extract Meetup event details with LLM")?;

            if scale_speakers.is_empty() {
                scale_speakers = parse_scale_speakers(&scale_speakers_html)?;
            }
            if ai_speakers.is_empty() {
                ai_speakers = parse_ai_speakers(&ai_speakers_html)?;
            }
            if ai_talks.is_empty() {
                ai_talks = parse_ai_talks(&ai_talks_html)?;
            }

            (scale_speakers, ai_speakers, ai_talks)
        }
    };
    assign_source_ids(
        &run_date,
        &mut scale_speakers,
        &mut ai_speakers,
        &mut ai_talks,
        &mut meetups,
    );

    if !args.no_raw {
        write_split_source_records(
            &args.raw_dir,
            &scale_speakers,
            &ai_speakers,
            &ai_talks,
            &meetups,
        )?;
    }

    let export = build_graph(scale_speakers, ai_speakers, ai_talks, meetups);

    if !args.no_raw {
        write_split_extracted_records(&args.raw_dir, &export)?;
    }

    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent).context("failed to create output directory")?;
    }
    fs::write(&args.output, serde_json::to_string_pretty(&export)?)
        .with_context(|| format!("failed to write {}", args.output.display()))?;

    println!(
        "wrote {} conferences, {} meetups, {} people, {} talks, {} edges to {}",
        export.conferences.len(),
        export.meetups.len(),
        export.people.len(),
        export.talks.len(),
        export.edges.len(),
        args.output.display()
    );

    Ok(())
}

async fn fetch(client: &Client, url: &str) -> Result<String> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to fetch {url}"))?
        .error_for_status()
        .with_context(|| format!("server returned an error for {url}"))?;

    response
        .text()
        .await
        .with_context(|| format!("failed to read response body for {url}"))
}

struct LlmExtractor<'a> {
    client: &'a Client,
    responses_url: String,
    model: String,
    api_key: String,
}

#[derive(Debug, Deserialize)]
struct LlmConferenceExtraction {
    #[serde(default)]
    speakers: Vec<LlmSpeaker>,
    #[serde(default)]
    talks: Vec<LlmTalk>,
}

#[derive(Clone, Debug, Deserialize)]
struct LlmSpeaker {
    name: String,
    #[serde(default)]
    company: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    talk_title: Option<String>,
    #[serde(default)]
    talk_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlmTalk {
    title: String,
    #[serde(default)]
    abstract_text: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    speakers: Vec<LlmSpeaker>,
    #[serde(default)]
    speaker_text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlmMeetupExtraction {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    speakers: Vec<LlmSpeaker>,
    #[serde(default)]
    sessions: Vec<LlmMeetupSession>,
}

#[derive(Debug, Deserialize)]
struct LlmMeetupSession {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    speakers: Vec<LlmSpeaker>,
}

impl<'a> LlmExtractor<'a> {
    fn new(client: &'a Client, responses_url: &str, model: &str) -> Result<Self> {
        let api_key = env::var("OPENAI_API_KEY")
            .context("OPENAI_API_KEY must be set when --extractor llm is used")?;
        Ok(Self {
            client,
            responses_url: responses_url.to_string(),
            model: model.to_string(),
            api_key,
        })
    }

    async fn extract_scale_speakers(&self, html: &str) -> Result<Vec<RawSpeaker>> {
        let extraction: LlmConferenceExtraction = self
            .extract_json(
                "Extract speaker cards and any talk title/url from the Scale By the Bay speakers page HTML.",
                html,
                conference_schema_prompt(),
            )
            .await?;
        Ok(extraction
            .speakers
            .into_iter()
            .map(llm_speaker_to_raw)
            .collect())
    }

    async fn extract_ai_speakers(&self, html: &str) -> Result<Vec<RawSpeaker>> {
        let extraction: LlmConferenceExtraction = self
            .extract_json(
                "Extract speaker cards from the AI By the Bay speakers page HTML.",
                html,
                conference_schema_prompt(),
            )
            .await?;
        Ok(extraction
            .speakers
            .into_iter()
            .map(llm_speaker_to_raw)
            .collect())
    }

    async fn extract_ai_talks(&self, html: &str) -> Result<Vec<RawTalk>> {
        let extraction: LlmConferenceExtraction = self
            .extract_json(
                "Extract talks from the AI By the Bay talks page HTML.",
                html,
                conference_schema_prompt(),
            )
            .await?;
        Ok(extraction
            .talks
            .into_iter()
            .filter_map(llm_talk_to_raw)
            .collect())
    }

    async fn enrich_meetups(&self, meetups: &mut [RawMeetup]) -> Result<()> {
        for meetup in meetups {
            for event in &mut meetup.events {
                let source = json!({
                    "meetup_name": meetup.name,
                    "event_id": event.id,
                    "event_title": event.title,
                    "event_url": event.url,
                    "date_time": event.date_time,
                    "end_time": event.end_time,
                    "description_html": event.source_json.get("description").and_then(Value::as_str),
                    "graphql_event": event.source_json,
                });
                let extraction: LlmMeetupExtraction = self
                    .extract_json(
                        "Extract the real agenda items from this Meetup event. Separate announcements/social/unconference/workshop records from actual talks. Extract speakers, companies, titles, abstracts, and session kinds.",
                        &serde_json::to_string(&source)?,
                        meetup_schema_prompt(),
                    )
                    .await
                    .with_context(|| format!("LLM extraction failed for Meetup event {}", event.id))?;

                if let Some(kind) = extraction.kind.as_deref().map(normalize_kind) {
                    event.kind = kind;
                }
                if !extraction.tags.is_empty() {
                    event.tags = normalize_tags(extraction.tags);
                }
                event.speakers = extraction
                    .speakers
                    .into_iter()
                    .filter_map(llm_speaker_to_meetup_speaker)
                    .collect();
                event.sessions = extraction
                    .sessions
                    .into_iter()
                    .filter_map(llm_session_to_raw)
                    .collect();
            }
        }
        Ok(())
    }

    async fn extract_json<T: for<'de> Deserialize<'de>>(
        &self,
        task: &str,
        source: &str,
        schema_prompt: &str,
    ) -> Result<T> {
        let body = json!({
            "model": self.model,
            "input": [
                {
                    "role": "system",
                    "content": [
                        {"type": "input_text", "text": "You extract structured conference and meetup data. Return only valid JSON matching the requested shape. Do not invent missing data; use null or empty arrays when absent."}
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": format!("{task}\n\n{schema_prompt}\n\nSOURCE:\n{source}")}
                    ]
                }
            ],
            "text": {
                "format": {"type": "json_object"}
            }
        });

        let response: Value = self
            .client
            .post(&self.responses_url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("failed to call OpenAI Responses API")?
            .error_for_status()
            .context("OpenAI Responses API returned an error")?
            .json()
            .await
            .context("failed to parse OpenAI Responses API response")?;

        let text =
            extract_response_text(&response).context("OpenAI response had no output text")?;
        serde_json::from_str(&text).with_context(|| format!("failed to parse LLM JSON: {text}"))
    }
}

fn conference_schema_prompt() -> &'static str {
    r#"Return JSON:
{
  "speakers": [{"name": string, "company": string|null, "title": string|null, "talk_title": string|null, "talk_url": string|null}],
  "talks": [{
    "title": string,
    "abstract_text": string|null,
    "description": string|null,
    "url": string|null,
    "tags": [string],
    "speakers": [{"name": string, "company": string|null, "title": string|null, "talk_title": string|null, "talk_url": string|null}],
    "speaker_text": string|null
  }]
}
For speaker pages, include speakers even when no talk is present. For talk pages, include talk title, abstract/description, tags, speakers, companies, and speaker titles if visible."#
}

fn meetup_schema_prompt() -> &'static str {
    r#"Return JSON:
{
  "kind": "talk"|"announcement"|"event"|"social"|"unconference"|"workshop",
  "tags": [string],
  "speakers": [{"name": string, "company": string|null, "title": string|null, "talk_title": string|null, "talk_url": string|null}],
  "sessions": [{
    "title": string,
    "description": string|null,
    "kind": "talk"|"announcement"|"event"|"social"|"unconference"|"workshop"|null,
    "speakers": [{"name": string, "company": string|null, "title": string|null, "talk_title": string|null, "talk_url": string|null}]
  }]
}
If the Meetup page is only registration/CFP/social/unconference, do not invent talk sessions. If it contains multiple agenda talks, return each as a session with its own speakers and abstract/description. Put company in company, not in name."#
}

fn extract_response_text(response: &Value) -> Option<String> {
    if let Some(text) = response.get("output_text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let text = response
        .get("output")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .filter_map(|content| {
            content
                .get("text")
                .or_else(|| content.get("output_text"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("");
    nonempty(text)
}

fn llm_speaker_to_raw(speaker: LlmSpeaker) -> RawSpeaker {
    RawSpeaker {
        name: clean_text(&speaker.name),
        role: role_from_llm(speaker.title.as_deref(), speaker.company.as_deref()),
        talk_title: speaker
            .talk_title
            .map(|value| clean_text(&value))
            .and_then(nonempty),
        talk_url: speaker
            .talk_url
            .map(|value| clean_text(&value))
            .and_then(nonempty),
        source_id: String::new(),
        source_file: String::new(),
        extracted_file: String::new(),
    }
}

fn llm_talk_to_raw(talk: LlmTalk) -> Option<RawTalk> {
    let title = clean_text(&talk.title);
    if title.is_empty() {
        return None;
    }
    let speaker_text = talk.speaker_text.unwrap_or_else(|| {
        talk.speakers
            .iter()
            .map(|speaker| {
                let name = clean_text(&speaker.name);
                if let Some(company) = speaker
                    .company
                    .as_deref()
                    .map(clean_text)
                    .and_then(nonempty)
                {
                    format!("{name}, {company}")
                } else {
                    name
                }
            })
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("; ")
    });

    Some(RawTalk {
        title,
        speaker_text,
        tags: normalize_tags(talk.tags),
        url: talk.url.and_then(|url| nonempty(clean_text(&url))),
        description: talk
            .abstract_text
            .or(talk.description)
            .map(|value| clean_text(&value))
            .and_then(nonempty),
        source_id: String::new(),
        source_file: String::new(),
        extracted_file: String::new(),
    })
}

fn llm_speaker_to_meetup_speaker(speaker: LlmSpeaker) -> Option<RawMeetupSpeaker> {
    let name = clean_text(&speaker.name);
    if name.is_empty() || !is_likely_person_name(&name) {
        return None;
    }
    Some(RawMeetupSpeaker {
        name,
        company: speaker
            .company
            .map(|value| clean_company(&value))
            .and_then(nonempty),
    })
}

fn llm_session_to_raw(session: LlmMeetupSession) -> Option<RawMeetupSession> {
    let title = clean_text(&session.title);
    if title.is_empty() {
        return None;
    }
    Some(RawMeetupSession {
        title,
        description: session
            .description
            .map(|value| clean_text(&value))
            .and_then(nonempty),
        speakers: session
            .speakers
            .into_iter()
            .filter_map(llm_speaker_to_meetup_speaker)
            .collect(),
        kind: session
            .kind
            .as_deref()
            .map(normalize_kind)
            .unwrap_or_else(|| "talk".to_string()),
        source_id: String::new(),
        source_file: String::new(),
        extracted_file: String::new(),
    })
}

fn role_from_llm(title: Option<&str>, company: Option<&str>) -> Option<String> {
    let title = title.map(clean_text).and_then(nonempty);
    let company = company.map(clean_company).and_then(nonempty);
    match (title, company) {
        (Some(title), Some(company)) => Some(format!("{title} @ {company}")),
        (Some(title), None) => Some(title),
        (None, Some(company)) => Some(company),
        (None, None) => None,
    }
}

fn normalize_kind(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "announcement" => "announcement",
        "social" | "socialevent" | "social_event" => "social",
        "unconference" | "unmeetup" => "unconference",
        "workshop" | "training" | "hackathon" => "workshop",
        "talk" | "session" | "presentation" => "talk",
        _ => "event",
    }
    .to_string()
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    tags.into_iter()
        .map(|tag| clean_text(&tag))
        .filter(|tag| !tag.is_empty() && seen.insert(tag.to_ascii_lowercase()))
        .collect()
}

async fn fetch_meetup_archive(client: &Client, url: &str) -> Result<(RawMeetup, String)> {
    let urlname = meetup_urlname(url)?;
    let archive_url = meetup_archive_url(url)?;
    let mut after = None::<String>;
    let mut pages = Vec::<Value>::new();

    loop {
        let payload = json!({
            "operationName": "getPastGroupEvents",
            "variables": {
                "urlname": urlname,
                "after": after,
                "beforeDateTime": "2999-01-01T00:00:00.000Z"
            },
            "query": MEETUP_PAST_EVENTS_QUERY
        });

        let body = client
            .post(MEETUP_GQL_URL)
            .header("content-type", "application/json")
            .body(payload.to_string())
            .send()
            .await
            .with_context(|| format!("failed to fetch Meetup archive for {url}"))?
            .error_for_status()
            .with_context(|| format!("Meetup GraphQL returned an error for {url}"))?
            .text()
            .await
            .context("failed to read Meetup GraphQL response")?;

        let page: Value =
            serde_json::from_str(&body).context("failed to parse Meetup GraphQL JSON")?;
        let events = page
            .pointer("/data/groupByUrlname/events")
            .context("Meetup GraphQL response did not contain group events")?;

        let page_info = events.get("pageInfo").unwrap_or(&Value::Null);
        after = page_info
            .get("endCursor")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let has_next_page = page_info
            .get("hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        pages.push(page);
        if !has_next_page || after.is_none() {
            break;
        }
    }

    let raw_json = serde_json::to_string_pretty(&pages)?;
    let meetup = parse_meetup_graphql_pages(&archive_url, &pages)?;
    Ok((meetup, raw_json))
}

fn write_split_source_records(
    raw_dir: &Path,
    scale_speakers: &[RawSpeaker],
    ai_speakers: &[RawSpeaker],
    ai_talks: &[RawTalk],
    meetups: &[RawMeetup],
) -> Result<()> {
    let source_dir = raw_dir.join("sources");
    if source_dir.exists() {
        fs::remove_dir_all(&source_dir)
            .with_context(|| format!("failed to clean {}", source_dir.display()))?;
    }
    fs::create_dir_all(&source_dir).context("failed to create split source directory")?;

    for speaker in scale_speakers.iter().chain(ai_speakers.iter()) {
        write_source_json(raw_dir, &speaker.source_file, speaker)?;
    }

    for talk in ai_talks {
        write_source_json(raw_dir, &talk.source_file, talk)?;
    }

    for meetup in meetups {
        for event in &meetup.events {
            let source_record = json!({
                "source_id": event.source_id,
                "source_url": event.url,
                "meetup_id": meetup.id,
                "meetup_name": meetup.name,
                "meetup_url": meetup.url,
                "event": event.source_json,
            });
            write_source_json(raw_dir, &event.source_file, &source_record)?;

            for session in &event.sessions {
                let source_record = json!({
                    "source_id": session.source_id,
                    "source_url": event.url,
                    "meetup_id": meetup.id,
                    "meetup_name": meetup.name,
                    "meetup_url": meetup.url,
                    "event_id": event.id,
                    "event_title": event.title,
                    "session": session,
                    "event": event.source_json,
                });
                write_source_json(raw_dir, &session.source_file, &source_record)?;
            }
        }
    }

    Ok(())
}

fn write_split_extracted_records(raw_dir: &Path, export: &GraphExport) -> Result<()> {
    let extracted_dir = raw_dir.join("extracted");
    if extracted_dir.exists() {
        fs::remove_dir_all(&extracted_dir)
            .with_context(|| format!("failed to clean {}", extracted_dir.display()))?;
    }
    fs::create_dir_all(&extracted_dir).context("failed to create split extracted directory")?;

    for talk in &export.talks {
        if let Some(extracted_file) = &talk.extracted_file {
            write_source_json(raw_dir, extracted_file, talk)?;
        }
    }

    Ok(())
}

fn write_source_json<T: Serialize>(raw_dir: &Path, file_name: &str, value: &T) -> Result<()> {
    let file_path = Path::new(file_name);
    let path = raw_dir.join(file_path.strip_prefix("data/raw").unwrap_or(file_path));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&path, serde_json::to_string_pretty(value)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn assign_source_ids(
    run_date: &str,
    scale_speakers: &mut [RawSpeaker],
    ai_speakers: &mut [RawSpeaker],
    ai_talks: &mut [RawTalk],
    meetups: &mut [RawMeetup],
) {
    let mut counts = BTreeMap::<String, usize>::new();

    for speaker in scale_speakers {
        let company = speaker.role.as_deref().and_then(|role| split_role(role).0);
        let title = speaker.talk_title.as_deref().unwrap_or(&speaker.name);
        let source_id = unique_source_id(
            &mut counts,
            run_date,
            &[title, &speaker.name, company.as_deref().unwrap_or("")],
        );
        set_speaker_source_files(speaker, source_id);
    }

    for speaker in ai_speakers {
        let company = speaker.role.as_deref().and_then(|role| split_role(role).0);
        let source_id = unique_source_id(
            &mut counts,
            run_date,
            &[&speaker.name, company.as_deref().unwrap_or("ai-by-the-bay")],
        );
        set_speaker_source_files(speaker, source_id);
    }

    for talk in ai_talks {
        let (speaker, company) = speaker_label_parts(&talk.speaker_text);
        let source_id = unique_source_id(
            &mut counts,
            run_date,
            &[
                &talk.title,
                speaker.as_deref().unwrap_or("speaker"),
                company.as_deref().unwrap_or(""),
            ],
        );
        talk.source_file = source_file_for(&source_id);
        talk.extracted_file = extracted_file_for(&source_id);
        talk.source_id = source_id;
    }

    for meetup in meetups {
        for event in &mut meetup.events {
            let speaker = event.speakers.first();
            let source_id = unique_source_id(
                &mut counts,
                run_date,
                &[
                    &meetup.name,
                    &event.title,
                    speaker
                        .map(|speaker| speaker.name.as_str())
                        .unwrap_or("event"),
                    speaker
                        .and_then(|speaker| speaker.company.as_deref())
                        .unwrap_or(""),
                ],
            );
            event.source_file = source_file_for(&source_id);
            event.extracted_file = extracted_file_for(&source_id);
            event.source_id = source_id;

            for session in &mut event.sessions {
                let speaker = session.speakers.first().or_else(|| event.speakers.first());
                let source_id = unique_source_id(
                    &mut counts,
                    run_date,
                    &[
                        &meetup.name,
                        &session.title,
                        speaker
                            .map(|speaker| speaker.name.as_str())
                            .unwrap_or("event"),
                        speaker
                            .and_then(|speaker| speaker.company.as_deref())
                            .unwrap_or(""),
                    ],
                );
                session.source_file = source_file_for(&source_id);
                session.extracted_file = extracted_file_for(&source_id);
                session.source_id = source_id;
            }
        }
    }
}

fn set_speaker_source_files(speaker: &mut RawSpeaker, source_id: String) {
    speaker.source_file = source_file_for(&source_id);
    speaker.extracted_file = extracted_file_for(&source_id);
    speaker.source_id = source_id;
}

fn unique_source_id(
    counts: &mut BTreeMap<String, usize>,
    run_date: &str,
    parts: &[&str],
) -> String {
    let label = parts
        .iter()
        .map(|part| slugify(clean_text(part)))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let label = truncate_slug(&label, 180);
    let base = format!("bythebay-{run_date}-{label}");
    let count = counts.entry(base.clone()).or_insert(0);
    *count += 1;
    if *count == 1 {
        base
    } else {
        format!("{base}-{count}")
    }
}

fn truncate_slug(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }

    value
        .char_indices()
        .take_while(|(idx, _)| *idx < max_len)
        .map(|(_, ch)| ch)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

fn source_file_for(source_id: &str) -> String {
    format!("data/raw/sources/{source_id}.json")
}

fn extracted_file_for(source_id: &str) -> String {
    format!("data/raw/extracted/{source_id}.json")
}

fn speaker_label_parts(speaker_text: &str) -> (Option<String>, Option<String>) {
    let first = speaker_text
        .split([',', ';', '\n'])
        .next()
        .map(clean_text)
        .and_then(nonempty);
    let company = speaker_text
        .split(" @ ")
        .nth(1)
        .or_else(|| speaker_text.split(" at ").nth(1))
        .map(clean_company)
        .and_then(nonempty);
    (first, company)
}

fn parse_scale_speakers(html: &str) -> Result<Vec<RawSpeaker>> {
    let item_re = Regex::new(
        r#"(?s)"metaData":\{"description":"(?P<description>(?:\\.|[^"\\])*)","title":"(?P<title>(?:\\.|[^"\\])*)".*?\},"mediaUrl""#,
    )?;
    let url_re = Regex::new(r#""url":"(?P<url>(?:\\.|[^"\\])*)""#)?;
    let mut speakers = Vec::new();
    let mut seen = BTreeSet::new();

    for caps in item_re.captures_iter(html) {
        let name = clean_text(&json_unescape(&caps["title"])?);
        let description = json_unescape(&caps["description"])?;
        if name.is_empty() || !seen.insert(name.clone()) {
            continue;
        }

        let lines = split_lines(&description);
        let talk_url = url_re
            .captures(caps.get(0).map_or("", |m| m.as_str()))
            .and_then(|c| json_unescape(&c["url"]).ok());

        let (role, talk_title) = split_scale_description(&lines, talk_url.as_deref());
        speakers.push(RawSpeaker {
            name,
            role,
            talk_title,
            talk_url,
            source_id: String::new(),
            source_file: String::new(),
            extracted_file: String::new(),
        });
    }

    Ok(speakers)
}

fn parse_ai_speakers(html: &str) -> Result<Vec<RawSpeaker>> {
    let names = extract_alt_components(html, "comp-mgka6yse")?;
    let name_text = extract_html_components(html, "comp-mgka6ysh")?;
    let roles = extract_html_components(html, "comp-mgka6ysj")?;
    let mut speakers = Vec::new();
    let mut seen = BTreeSet::new();

    for (id, alt_name) in names {
        let name = name_text.get(&id).cloned().unwrap_or(alt_name);
        let name = clean_text(&name);
        if name.is_empty() || !seen.insert(name.clone()) {
            continue;
        }

        speakers.push(RawSpeaker {
            name,
            role: roles
                .get(&id)
                .map(|s| clean_text(s))
                .filter(|s| !s.is_empty()),
            talk_title: None,
            talk_url: None,
            source_id: String::new(),
            source_file: String::new(),
            extracted_file: String::new(),
        });
    }

    Ok(speakers)
}

fn parse_ai_talks(html: &str) -> Result<Vec<RawTalk>> {
    let titles = extract_html_components(html, "comp-mf4zu2l7")?;
    let speakers = extract_html_components(html, "comp-mf4wfav01")?;
    let tags = extract_html_components(html, "comp-mf4zy9o2")?;
    let urls = extract_link_components(html, "comp-mf4wfaur5")?;
    let mut talks = Vec::new();

    for (id, title) in titles {
        let title = clean_text(&title);
        let Some(speaker_text) = speakers.get(&id).map(|s| clean_text(s)) else {
            continue;
        };

        talks.push(RawTalk {
            title,
            speaker_text,
            tags: tags
                .get(&id)
                .map(|raw| {
                    raw.split(',')
                        .map(clean_text)
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            url: urls.get(&id).cloned(),
            description: None,
            source_id: String::new(),
            source_file: String::new(),
            extracted_file: String::new(),
        });
    }

    Ok(talks)
}

fn parse_meetup_graphql_pages(url: &str, pages: &[Value]) -> Result<RawMeetup> {
    let first_group = pages
        .first()
        .and_then(|page| page.pointer("/data/groupByUrlname"))
        .context("Meetup GraphQL pages did not contain group data")?;

    let fallback_slug = meetup_urlname(url)?;
    let name = first_group
        .get("name")
        .and_then(Value::as_str)
        .map(clean_text)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback_slug.clone());
    let id = first_group
        .get("id")
        .and_then(Value::as_str)
        .map(|id| format!("meetup:{id}"))
        .unwrap_or_else(|| format!("meetup:{fallback_slug}"));
    let timezone = first_group
        .get("timezone")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let mut raw_events = Vec::new();
    let mut seen = BTreeSet::new();
    for event in pages.iter().flat_map(meetup_events_from_page) {
        let Some(event_id) = event.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !seen.insert(event_id.to_string()) {
            continue;
        }

        let title = event
            .get("title")
            .and_then(Value::as_str)
            .map(clean_text)
            .unwrap_or_else(|| format!("Meetup event {event_id}"));
        let event_url = event
            .get("eventUrl")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("{}/events/{event_id}/", url.trim_end_matches('/')));
        let raw_description = event.get("description").and_then(Value::as_str);
        let speakers = raw_description
            .map(|description| extract_meetup_speakers(&title, description))
            .unwrap_or_default();
        let description = raw_description.map(clean_text).and_then(nonempty);
        let (kind, tags) = classify_meetup_event(&title, description.as_deref(), &speakers);
        let sessions = raw_description
            .map(|description| extract_meetup_sessions(&title, description, &speakers))
            .unwrap_or_default();

        raw_events.push(RawMeetupEvent {
            id: event_id.to_string(),
            title,
            url: event_url,
            kind,
            tags,
            description,
            date_time: event
                .get("dateTime")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            end_time: event
                .get("endTime")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            speakers,
            sessions,
            source_id: String::new(),
            source_file: String::new(),
            extracted_file: String::new(),
            source_json: event.clone(),
        });
    }

    raw_events.sort_by(|a, b| a.date_time.cmp(&b.date_time).then(a.id.cmp(&b.id)));

    Ok(RawMeetup {
        id,
        name,
        url: url.to_string(),
        timezone,
        events: raw_events,
    })
}

fn meetup_events_from_page(page: &Value) -> impl Iterator<Item = &Value> {
    page.pointer("/data/groupByUrlname/events/edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| edge.get("node"))
}

fn extract_meetup_speakers(title: &str, description: &str) -> Vec<RawMeetupSpeaker> {
    let mut names = BTreeSet::new();
    let lines = split_lines(description);

    extract_speaker_patterns(&mut names, title);
    extract_speaker_patterns(&mut names, description);
    extract_speaker_patterns(&mut names, &clean_text(description));

    for (idx, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("speaker:")
            || lower.starts_with("speakers:")
            || lower == "speaker"
            || lower == "speakers"
        {
            if let Some(after_marker) = line.split_once(':').map(|(_, value)| clean_text(value)) {
                insert_speaker_candidates(&mut names, &after_marker);
            }
            for candidate in lines.iter().skip(idx + 1).take(3) {
                if is_description_section_break(candidate) {
                    break;
                }
                insert_speaker_candidates(&mut names, candidate);
            }
        } else if lower.starts_with("speaker bio") {
            if let Some(after_marker) = line.split_once(':').map(|(_, value)| clean_text(value)) {
                insert_speaker_candidates(&mut names, &after_marker);
            }
            if let Some(candidate) = lines.get(idx + 1) {
                insert_speaker_candidates(&mut names, candidate);
            }
        } else if is_agenda_speaker_line(line) {
            insert_speaker_candidates(&mut names, line);
        }
    }

    names
        .into_iter()
        .map(|name| RawMeetupSpeaker {
            company: extract_company_for_speaker(&name, description),
            name,
        })
        .collect()
}

fn extract_meetup_sessions(
    event_title: &str,
    description: &str,
    event_speakers: &[RawMeetupSpeaker],
) -> Vec<RawMeetupSession> {
    let lines = split_lines(description);
    let mut sessions = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        let title = if lower.starts_with("presentation:") {
            clean_text(line.split_once(':').map(|(_, value)| value).unwrap_or(""))
        } else if lower.starts_with("talk:") {
            clean_text(line.split_once(':').map(|(_, value)| value).unwrap_or(""))
        } else {
            String::new()
        };

        if title.is_empty() || !is_likely_session_title(&title) {
            continue;
        }

        let window = lines
            .iter()
            .skip(idx + 1)
            .take(8)
            .take_while(|next| !is_next_session_boundary(next))
            .cloned()
            .collect::<Vec<_>>();
        let window_text = window.join("\n");
        let mut speakers = extract_meetup_speakers(&title, &window_text);
        if speakers.is_empty() && event_speakers.len() == 1 {
            speakers = event_speakers.to_vec();
        }

        sessions.push(RawMeetupSession {
            title,
            description: nonempty(window.join(" ")),
            speakers,
            kind: "talk".to_string(),
            source_id: String::new(),
            source_file: String::new(),
            extracted_file: String::new(),
        });
    }

    let quoted_talk_re = Regex::new(
        r#"(?i)(?:talk|presentation)\s+(?:will\s+be\s+)?(?:given|presented)\s+by\s+([A-Z][[:alpha:]'’.-]+(?:\s+[A-Z][[:alpha:]'’.-]+){1,3})\s*:\s*["“]([^"”]+)"#,
    )
    .expect("static regex is valid");
    for caps in quoted_talk_re.captures_iter(&clean_text(description)) {
        let speaker_name = clean_text(&caps[1]);
        let title = clean_text(&caps[2]);
        if !is_likely_session_title(&title) {
            continue;
        }
        if sessions.iter().any(|session| session.title == title) {
            continue;
        }
        let speakers = vec![RawMeetupSpeaker {
            company: extract_company_for_speaker(&speaker_name, description),
            name: speaker_name,
        }];
        sessions.push(RawMeetupSession {
            title,
            description: None,
            speakers,
            kind: "talk".to_string(),
            source_id: String::new(),
            source_file: String::new(),
            extracted_file: String::new(),
        });
    }

    if sessions.is_empty()
        && !event_speakers.is_empty()
        && is_likely_session_title(event_title)
        && !is_announcement_title(event_title)
    {
        sessions.push(RawMeetupSession {
            title: clean_event_title(event_title),
            description: clean_text(description)
                .split("Agenda:")
                .next()
                .map(clean_text)
                .and_then(nonempty),
            speakers: event_speakers.to_vec(),
            kind: "talk".to_string(),
            source_id: String::new(),
            source_file: String::new(),
            extracted_file: String::new(),
        });
    }

    sessions
}

fn is_next_session_boundary(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("presentation:")
        || lower.starts_with("talk:")
        || lower.starts_with("agenda:")
        || lower.starts_with("bio:")
        || lower.starts_with("speaker:")
        || lower.starts_with("additional information")
        || lower.starts_with("location:")
        || lower.starts_with("please ")
}

fn is_likely_session_title(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.split_whitespace().count() >= 2
        && !matches!(
            lower.as_str(),
            "food and drinks"
                | "food drinks and networking"
                | "networking"
                | "welcome"
                | "introductions"
                | "breakout sessions"
                | "breakouts"
                | "q&a"
        )
        && !lower.contains("register at")
        && !lower.contains("registration")
        && !lower.contains("rsvp at")
        && !lower.contains("cfp")
}

fn classify_meetup_event(
    title: &str,
    description: Option<&str>,
    speakers: &[RawMeetupSpeaker],
) -> (String, Vec<String>) {
    let title_lower = title.to_ascii_lowercase();
    let text = format!("{} {}", title, description.unwrap_or("")).to_ascii_lowercase();
    let mut tags = vec!["meetup".to_string()];
    if title_lower.contains("cross-post")
        || title_lower.contains("cross-listed")
        || title_lower.contains("register at")
        || title_lower.contains("rsvp at")
        || title_lower.contains("external registration")
    {
        tags.push("crosspost".to_string());
    }

    let kind = if is_hard_announcement_title(title) {
        "announcement"
    } else if is_registration_prelude_title(title) && speakers.is_empty() {
        "announcement"
    } else if text.contains("workshop") || text.contains("hackathon") || text.contains("training") {
        "workshop"
    } else if !speakers.is_empty() {
        "talk"
    } else if text.contains("this talk")
        || text.contains("in this talk")
        || text.contains("the following talk")
        || text.contains("presentation:")
        || text.contains("presented by")
    {
        "talk"
    } else if text.contains("cfp")
        || text.contains("registration is now open")
        || text.contains("early bird")
    {
        "announcement"
    } else if title_lower.contains("party")
        || title_lower.contains("mixer")
        || title_lower.contains("networking") && speakers.is_empty()
    {
        "social"
    } else if text.contains("unmeetup")
        || text.contains("unconference")
        || text.contains("breakout sessions begin")
    {
        "unconference"
    } else if speakers.is_empty() {
        "event"
    } else {
        "talk"
    };

    tags.push(kind.to_string());
    (kind.to_string(), tags)
}

fn is_announcement_title(title: &str) -> bool {
    is_hard_announcement_title(title) || is_registration_prelude_title(title)
}

fn is_hard_announcement_title(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    lower.contains("cfp")
        || lower.contains("program is live")
        || lower.contains("early bird")
        || lower.contains("conference]")
}

fn is_registration_prelude_title(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    lower.contains("registration")
        || lower.contains("register at")
        || lower.contains("rsvp at")
        || lower.contains("external registration")
        || lower.contains("cross-post")
        || lower.contains("cross-listed")
}

fn clean_event_title(title: &str) -> String {
    let bracket_re = Regex::new(r#"(?i)^\[[^\]]+\]\s*"#).expect("static regex is valid");
    clean_text(&bracket_re.replace_all(title, ""))
}

fn extract_company_for_speaker(name: &str, text: &str) -> Option<String> {
    let escaped_name = regex::escape(name);
    let patterns = [
        format!(
            r#"(?i){}\s*,\s*[^.\n,]+?\s+(?:at|@)\s+([^.\n,;:()]+)"#,
            escaped_name
        ),
        format!(
            r#"(?i){}\s+is\s+(?:a|an|the)\b[^.\n]{{0,160}}?\b(?:at|with|for)\s+([^.\n,;:()]+)"#,
            escaped_name
        ),
        format!(
            r#"(?i){}\s+(?:at|@|from|of)\s+([^.\n,;:()]+)"#,
            escaped_name
        ),
        format!(
            r#"(?i){}\s*[-–]\s*[^.\n,]+?,\s*([^.\n,;:()]+)"#,
            escaped_name
        ),
    ];

    for pattern in patterns {
        let re = Regex::new(&pattern).expect("generated regex is valid");
        if let Some(company) = re
            .captures(text)
            .and_then(|caps| caps.get(1))
            .map(|m| clean_company(m.as_str()))
            .and_then(nonempty)
            .filter(|company| is_likely_company(company))
        {
            return Some(company);
        }
    }

    None
}

fn clean_company(value: &str) -> String {
    clean_text(value)
        .split(" talking ")
        .next()
        .unwrap_or("")
        .split(" who ")
        .next()
        .unwrap_or("")
        .split(" where ")
        .next()
        .unwrap_or("")
        .split(" about ")
        .next()
        .unwrap_or("")
        .split(" -- ")
        .next()
        .unwrap_or("")
        .split(" - ")
        .next()
        .unwrap_or("")
        .trim_matches(|ch: char| matches!(ch, '-' | '–' | '|' | ':' | ';' | '[' | ']'))
        .trim()
        .to_string()
}

fn is_likely_company(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let blocked = [
        "aicamp",
        "founder",
        "founder and ceo",
        "our partners",
        "product management",
        "sbtb",
        "the book",
        "welcome remarks",
    ];
    !value.is_empty()
        && value.split_whitespace().count() <= 5
        && !blocked.contains(&lower.as_str())
        && !lower.starts_with("the ")
        && !lower.starts_with("our ")
        && !lower.contains("founder")
        && !lower.contains("talking about")
        && !lower.contains("speaker")
}

fn extract_speaker_patterns(names: &mut BTreeSet<String>, text: &str) {
    let patterns = [
        r#"(?i)\b(?:speaker|speakers|presenter|presenters|panelists?)\s*:\s*([^.\n]+)"#,
        r#"(?i)\b(?:given|presented)\s+by\s+([^.\n]+)"#,
        r#"(?i)\bpresentation\s+by\s+([^.\n]+)"#,
        r#"(?i)\b(?:talk|session|keynote)\s+(?:will\s+be\s+)?(?:given|presented)\s+by\s+([^.\n]+)"#,
        r#"(?i)\b([A-Z][[:alpha:]'’.-]+(?:\s+[A-Z][[:alpha:]'’.-]+){1,3})\s+is\s+(?:a|an|the)\b[^.\n]{0,160}?\b(?:at|with|for)\s+[^.\n]+"#,
        r#"(?i)\b([A-Z][[:alpha:]'’.-]+(?:\s+[A-Z][[:alpha:]'’.-]+){1,3})\s+is\s+(?:a|an|the)\b[^.\n]+"#,
        r#""[^"]+"\s+by\s+([^.\n]+)"#,
        r#""[^"]+"\s+([A-Z][[:alpha:]'’.-]+(?:\s+[A-Z][[:alpha:]'’.-]+){1,3})\s*,"#,
        r#"[“][^”]+[”]\s+by\s+([^.\n]+)"#,
        r#"(?i)\bby\s+([A-Z][[:alpha:]'’.-]+(?:\s+[A-Z][[:alpha:]'’.-]+){1,3})\b"#,
    ];

    for pattern in patterns {
        let re = Regex::new(pattern).expect("static regex is valid");
        for caps in re.captures_iter(text) {
            if let Some(candidate) = caps.get(1) {
                insert_speaker_candidates(names, candidate.as_str());
            }
        }
    }
}

fn insert_speaker_candidates(names: &mut BTreeSet<String>, candidate: &str) {
    for part in split_speaker_candidates(candidate) {
        insert_speaker_candidate(names, &part);
    }
}

fn split_speaker_candidates(candidate: &str) -> Vec<String> {
    let cleaned = clean_text(candidate);
    if cleaned.is_empty() {
        return Vec::new();
    }

    let cleaned = cleaned.replace(['*', '`'], "");
    let without_intro = cleaned
        .trim_start_matches("Speaker:")
        .trim_start_matches("Speakers:")
        .trim_start_matches("Panelists:")
        .trim();
    let first_clause = without_intro
        .split(" Abstract")
        .next()
        .unwrap_or(without_intro)
        .split(" About ")
        .last()
        .unwrap_or(without_intro)
        .split(" Bio")
        .next()
        .unwrap_or(without_intro)
        .split(" Description")
        .next()
        .unwrap_or(without_intro)
        .split(" Agenda")
        .next()
        .unwrap_or(without_intro)
        .split(" Summary")
        .next()
        .unwrap_or(without_intro);

    first_clause
        .replace(" and ", ", ")
        .replace(" & ", ", ")
        .replace(" / ", ", ")
        .split(',')
        .map(clean_text)
        .filter(|part| !part.is_empty())
        .collect()
}

fn insert_speaker_candidate(names: &mut BTreeSet<String>, candidate: &str) {
    let candidate = clean_text(candidate);
    if candidate.is_empty() || candidate.starts_with("http") {
        return;
    }

    let first_clause = candidate
        .split(['.', ';'])
        .next()
        .unwrap_or(&candidate)
        .split(" - ")
        .next()
        .unwrap_or(&candidate);
    let name_part = first_clause
        .split(',')
        .next()
        .unwrap_or(first_clause)
        .split(" of ")
        .next()
        .unwrap_or(first_clause);
    let name = clean_text(name_part);

    if is_likely_person_name(&name) {
        names.insert(name);
        return;
    }

    if let Some(name) = leading_person_name(&candidate) {
        names.insert(name);
    }
}

fn is_description_section_break(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.starts_with("http")
        || lower.starts_with("abstract")
        || lower.starts_with("description")
        || lower.starts_with("agenda")
        || lower.starts_with("bio")
        || lower.starts_with("speaker")
        || lower.starts_with("join us")
}

fn is_agenda_speaker_line(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    (lower.contains(" - ") || lower.contains(" -- "))
        && (lower.contains("pm") || lower.contains("am"))
        && value.split_whitespace().any(|word| {
            word.chars()
                .next()
                .is_some_and(|first| first.is_uppercase())
        })
}

fn is_likely_person_name(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let blocked = [
        "additional information",
        "abstract",
        "agenda",
        "ai-powered personalization",
        "airbnb nerds",
        "akka alex",
        "akka streams",
        "amazon kindle",
        "american express",
        "analytical platform",
        "apache hbase",
        "apache mxnet",
        "apache nlpcraft",
        "apache samza",
        "apache sqoop",
        "apache trafodian",
        "artificial intelligence",
        "arize ceo",
        "associate professor",
        "asst. professor",
        "bea systems",
        "bigdata techcon",
        "bio",
        "business intelligence",
        "caber systems",
        "capital one",
        "chief technologist",
        "computer vision",
        "computer science",
        "description",
        "developer advocate",
        "food",
        "global program manager",
        "google deepmind",
        "google apps",
        "google checkout",
        "google flights",
        "google hotel finder",
        "gridgain systems",
        "introduction",
        "just systems",
        "linkedin's publishing platform",
        "lightning talks",
        "networking",
        "open source science",
        "openaisrisatish ambati",
        "presentation",
        "product management",
        "product marketing",
        "scalar conf",
        "speaker",
        "speakers",
        "talk",
        "technical product management",
        "topics",
        "twitter scalding team",
        "welcome",
    ];
    if blocked.contains(&lower.as_str())
        || lower.ends_with(" meetup")
        || lower.starts_with("about ")
        || lower.starts_with("talk ")
        || value.contains(')')
        || value.contains('&')
        || value.ends_with(':')
        || value.ends_with(". He")
        || value.ends_with(". Please")
        || value
            .split_whitespace()
            .any(|word| word.ends_with('.') && word.trim_end_matches('.').chars().count() > 1)
    {
        return false;
    }

    let words: Vec<_> = value.split_whitespace().collect();
    let word_count = words.len();
    (2..=4).contains(&word_count)
        && words.iter().all(|word| {
            let stripped = word.trim_matches(|ch: char| !ch.is_alphabetic());
            !stripped.is_empty()
                && !(stripped.contains('.') && stripped.len() > 2)
                && !(stripped.chars().count() > 1 && stripped.chars().all(|ch| ch.is_uppercase()))
                && !matches!(
                    stripped.to_ascii_lowercase().as_str(),
                    "ai" | "api"
                        | "architect"
                        | "aws"
                        | "big"
                        | "bank"
                        | "cloud"
                        | "ceo"
                        | "conf"
                        | "content"
                        | "corporation"
                        | "cto"
                        | "data"
                        | "datastax"
                        | "deep"
                        | "developer"
                        | "engineer"
                        | "engineering"
                        | "finder"
                        | "graph"
                        | "graphql"
                        | "group"
                        | "hadoop"
                        | "inc"
                        | "labs"
                        | "machine"
                        | "manager"
                        | "meetup"
                        | "openai"
                        | "pmc"
                        | "product"
                        | "professor"
                        | "scala"
                        | "science"
                        | "spark"
                        | "staff"
                        | "summary"
                        | "systems"
                        | "talk"
                        | "team"
                        | "technologies"
                        | "technology"
                        | "technologist"
                        | "the"
                        | "university"
                )
                && word
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_uppercase())
                && word.chars().any(|ch| ch.is_alphabetic())
        })
}

fn leading_person_name(value: &str) -> Option<String> {
    let re = Regex::new(r#"^([A-Z][[:alpha:]'’.-]+(?:\s+[A-Z][[:alpha:]'’.-]+){1,3})\b"#)
        .expect("static regex is valid");
    let name = re
        .captures(value)
        .and_then(|caps| caps.get(1))
        .map(|m| clean_text(m.as_str()))?;
    is_likely_person_name(&name).then_some(name)
}

fn extract_html_components(html: &str, prefix: &str) -> Result<BTreeMap<String, String>> {
    let re = Regex::new(&format!(
        r#"(?s)"{}__(?P<id>[a-f0-9-]+)":\{{"html":"(?P<html>(?:\\.|[^"\\])*)"\}}"#,
        regex::escape(prefix)
    ))?;
    let mut values = BTreeMap::new();

    for caps in re.captures_iter(html) {
        let fragment_html = json_unescape(&caps["html"])?;
        values.insert(caps["id"].to_string(), html_to_text(&fragment_html));
    }

    Ok(values)
}

fn extract_alt_components(html: &str, prefix: &str) -> Result<BTreeMap<String, String>> {
    let re = Regex::new(&format!(
        r#"(?s)"{}__(?P<id>[a-f0-9-]+)":\{{.*?"alt":"(?P<alt>(?:\\.|[^"\\])*)""#,
        regex::escape(prefix)
    ))?;
    let mut values = BTreeMap::new();

    for caps in re.captures_iter(html) {
        values.insert(
            caps["id"].to_string(),
            clean_text(&json_unescape(&caps["alt"])?),
        );
    }

    Ok(values)
}

fn extract_link_components(html: &str, prefix: &str) -> Result<BTreeMap<String, String>> {
    let re = Regex::new(&format!(
        r#"(?s)"{}__(?P<id>[a-f0-9-]+)":\{{.*?"href":"(?P<href>(?:\\.|[^"\\])*)""#,
        regex::escape(prefix)
    ))?;
    let mut values = BTreeMap::new();

    for caps in re.captures_iter(html) {
        values.insert(caps["id"].to_string(), json_unescape(&caps["href"])?);
    }

    Ok(values)
}

fn split_scale_description(
    lines: &[String],
    talk_url: Option<&str>,
) -> (Option<String>, Option<String>) {
    if lines.is_empty() {
        return (None, None);
    }

    let has_post_url = talk_url.is_some_and(|url| url.contains("/post/"));
    let last = lines.last().cloned().unwrap_or_default();
    let has_talk = lines.len() >= 3 || has_post_url && is_likely_talk_title(&last);

    if has_talk {
        let role = lines[..lines.len() - 1].join(", ");
        (nonempty(role), nonempty(last))
    } else {
        (nonempty(lines.join(", ")), None)
    }
}

fn is_likely_talk_title(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value == "Keynote"
        || value.contains(':')
        || value.contains('?')
        || lower.starts_with("scaling ")
        || lower.starts_with("building ")
        || lower.starts_with("open source ")
        || lower.starts_with("latency ")
        || lower.starts_with("apache ")
        || lower.starts_with("full stack ")
        || lower.starts_with("test-driven ")
        || lower.starts_with("oblivious ")
        || lower.starts_with("aging:")
        || lower.starts_with("pinot,")
}

fn build_graph(
    scale_speakers: Vec<RawSpeaker>,
    ai_speakers: Vec<RawSpeaker>,
    ai_talks: Vec<RawTalk>,
    raw_meetups: Vec<RawMeetup>,
) -> GraphExport {
    let conferences = vec![
        Conference {
            id: "scale-by-the-bay".to_string(),
            name: "Scale By the Bay".to_string(),
            site: "scale.bythebay.io".to_string(),
            url: "https://scale.bythebay.io/".to_string(),
        },
        Conference {
            id: "ai-by-the-bay".to_string(),
            name: "AI By the Bay".to_string(),
            site: "ai.bythebay.io".to_string(),
            url: "https://ai.bythebay.io/".to_string(),
        },
    ];

    let mut people = BTreeMap::<String, Person>::new();
    let mut talks = BTreeMap::<String, Talk>::new();
    let mut meetups = Vec::new();
    let mut edges = Vec::<Edge>::new();

    for speaker in scale_speakers {
        let person = make_person(
            &speaker.name,
            "scale-by-the-bay",
            None,
            speaker.role.as_deref(),
            SCALE_SPEAKERS_URL,
            Some(&speaker.source_id),
            Some(&speaker.source_file),
        );
        let person_id = person.id.clone();
        people.insert(person_id.clone(), person);
        edges.push(Edge {
            from: person_id.clone(),
            to: "scale-by-the-bay".to_string(),
            relationship: "SPEAKS_AT".to_string(),
        });

        if let Some(title) = speaker.talk_title {
            let talk = make_talk(
                &title,
                "scale-by-the-bay",
                None,
                None,
                Some(&speaker.name),
                Vec::new(),
                speaker.talk_url,
                None,
                None,
                None,
                "talk",
                None,
                None,
                Some(speaker.source_id.clone()),
                Some(speaker.source_file.clone()),
                Some(speaker.extracted_file.clone()),
            );
            let talk_id = talk.id.clone();
            talks.insert(talk_id.clone(), talk);
            edges.push(Edge {
                from: person_id,
                to: talk_id,
                relationship: "PRESENTS".to_string(),
            });
        }
    }

    for speaker in ai_speakers {
        let person = make_person(
            &speaker.name,
            "ai-by-the-bay",
            None,
            speaker.role.as_deref(),
            AI_SPEAKERS_URL,
            Some(&speaker.source_id),
            Some(&speaker.source_file),
        );
        let person_id = person.id.clone();
        people.insert(person_id.clone(), person);
        edges.push(Edge {
            from: person_id,
            to: "ai-by-the-bay".to_string(),
            relationship: "SPEAKS_AT".to_string(),
        });
    }

    let ai_person_names: Vec<(String, String)> = people
        .values()
        .filter(|p| p.conference_id == "ai-by-the-bay")
        .map(|p| (p.name.clone(), p.id.clone()))
        .collect();

    for raw in ai_talks {
        let speaker_text = raw.speaker_text.clone();
        let talk = make_talk(
            &raw.title,
            "ai-by-the-bay",
            None,
            None,
            Some(&speaker_text),
            raw.tags,
            raw.url,
            raw.description,
            None,
            None,
            "talk",
            None,
            None,
            Some(raw.source_id),
            Some(raw.source_file),
            Some(raw.extracted_file),
        );
        let talk_id = talk.id.clone();
        talks.insert(talk_id.clone(), talk);

        for (_, person_id) in ai_person_names
            .iter()
            .filter(|(name, _)| speaker_text.contains(name))
        {
            edges.push(Edge {
                from: person_id.clone(),
                to: talk_id.clone(),
                relationship: "PRESENTS".to_string(),
            });
        }
    }

    for raw_meetup in raw_meetups {
        let meetup_id = raw_meetup.id.clone();
        meetups.push(MeetupGroup {
            id: meetup_id.clone(),
            name: raw_meetup.name.clone(),
            url: raw_meetup.url.clone(),
            timezone: raw_meetup.timezone.clone(),
        });

        for event in raw_meetup.events {
            let session_records = if event.sessions.is_empty() {
                Vec::new()
            } else {
                event.sessions.clone()
            };

            let event_is_replaced_by_sessions = !session_records.is_empty()
                && matches!(event.kind.as_str(), "talk" | "event" | "workshop");

            if !event_is_replaced_by_sessions {
                let speaker_text = speaker_text_from(&event.speakers);
                let talk = make_talk(
                    &clean_event_title(&event.title),
                    &meetup_id,
                    Some(&format!("event-{}", event.id)),
                    Some(&meetup_id),
                    speaker_text.as_deref(),
                    event.tags.clone(),
                    Some(event.url.clone()),
                    event.description.clone(),
                    event.date_time.clone(),
                    event.end_time.clone(),
                    &event.kind,
                    Some(event.id.clone()),
                    Some(event.title.clone()),
                    Some(event.source_id.clone()),
                    Some(event.source_file.clone()),
                    Some(event.extracted_file.clone()),
                );
                let talk_id = talk.id.clone();
                talks.insert(talk_id.clone(), talk);

                edges.push(Edge {
                    from: talk_id.clone(),
                    to: meetup_id.clone(),
                    relationship: "PART_OF_MEETUP".to_string(),
                });

                for speaker in event.speakers.clone() {
                    insert_meetup_person_edges(
                        &mut people,
                        &mut edges,
                        &meetup_id,
                        &event.url,
                        &event.source_id,
                        &event.source_file,
                        &talk_id,
                        &speaker,
                    );
                }
            }

            for (idx, session) in session_records.into_iter().enumerate() {
                let speaker_text = speaker_text_from(&session.speakers);
                let tags = vec!["meetup".to_string(), session.kind.clone()];
                let talk = make_talk(
                    &session.title,
                    &meetup_id,
                    Some(&format!("event-{}-session-{}", event.id, idx + 1)),
                    Some(&meetup_id),
                    speaker_text.as_deref(),
                    tags,
                    Some(event.url.clone()),
                    session
                        .description
                        .clone()
                        .or_else(|| event.description.clone()),
                    event.date_time.clone(),
                    event.end_time.clone(),
                    &session.kind,
                    Some(event.id.clone()),
                    Some(event.title.clone()),
                    Some(session.source_id.clone()),
                    Some(session.source_file.clone()),
                    Some(session.extracted_file.clone()),
                );
                let talk_id = talk.id.clone();
                talks.insert(talk_id.clone(), talk);

                edges.push(Edge {
                    from: talk_id.clone(),
                    to: meetup_id.clone(),
                    relationship: "PART_OF_MEETUP".to_string(),
                });

                for speaker in session.speakers {
                    insert_meetup_person_edges(
                        &mut people,
                        &mut edges,
                        &meetup_id,
                        &event.url,
                        &session.source_id,
                        &session.source_file,
                        &talk_id,
                        &speaker,
                    );
                }
            }
        }
    }

    GraphExport {
        source_urls: [
            vec![
                SCALE_SPEAKERS_URL.to_string(),
                AI_SPEAKERS_URL.to_string(),
                AI_TALKS_URL.to_string(),
            ],
            meetups.iter().map(|meetup| meetup.url.clone()).collect(),
        ]
        .concat(),
        conferences,
        meetups,
        people: people.into_values().collect(),
        talks: talks.into_values().collect(),
        edges,
    }
}

fn make_person(
    name: &str,
    conference_id: &str,
    meetup_id: Option<&str>,
    role: Option<&str>,
    source_url: &str,
    source_id: Option<&str>,
    source_file: Option<&str>,
) -> Person {
    let (organization, title) = if meetup_id.is_some() {
        (role.map(clean_text).and_then(nonempty), None)
    } else {
        role.map(split_role).unwrap_or((None, None))
    };
    Person {
        id: format!("person:{conference_id}:{}", slugify(name)),
        name: name.to_string(),
        conference_id: conference_id.to_string(),
        meetup_id: meetup_id.map(ToOwned::to_owned),
        organization,
        title,
        source_url: source_url.to_string(),
        source_id: source_id.map(ToOwned::to_owned),
        source_file: source_file.map(ToOwned::to_owned),
    }
}

fn speaker_text_from(speakers: &[RawMeetupSpeaker]) -> Option<String> {
    (!speakers.is_empty()).then(|| {
        speakers
            .iter()
            .map(|speaker| {
                if let Some(company) = &speaker.company {
                    format!("{} @ {}", speaker.name, company)
                } else {
                    speaker.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    })
}

fn insert_meetup_person_edges(
    people: &mut BTreeMap<String, Person>,
    edges: &mut Vec<Edge>,
    meetup_id: &str,
    source_url: &str,
    source_id: &str,
    source_file: &str,
    talk_id: &str,
    speaker: &RawMeetupSpeaker,
) {
    let person = make_person(
        &speaker.name,
        meetup_id,
        Some(meetup_id),
        speaker.company.as_deref(),
        source_url,
        Some(source_id),
        Some(source_file),
    );
    let person_id = person.id.clone();
    people.insert(person_id.clone(), person);
    edges.push(Edge {
        from: person_id.clone(),
        to: meetup_id.to_string(),
        relationship: "SPEAKS_AT".to_string(),
    });
    edges.push(Edge {
        from: person_id,
        to: talk_id.to_string(),
        relationship: "PRESENTS".to_string(),
    });
}

fn make_talk(
    title: &str,
    conference_id: &str,
    id_suffix: Option<&str>,
    meetup_id: Option<&str>,
    speaker_text: Option<&str>,
    tags: Vec<String>,
    url: Option<String>,
    description: Option<String>,
    date_time: Option<String>,
    end_time: Option<String>,
    kind: &str,
    event_id: Option<String>,
    event_title: Option<String>,
    source_id: Option<String>,
    source_file: Option<String>,
    extracted_file: Option<String>,
) -> Talk {
    Talk {
        id: format!(
            "talk:{conference_id}:{}",
            source_id
                .as_deref()
                .map(slugify)
                .or_else(|| id_suffix.map(slugify))
                .unwrap_or_else(|| slugify(title))
        ),
        source_id,
        source_file,
        extracted_file,
        kind: kind.to_string(),
        event_id,
        event_title,
        title: title.to_string(),
        conference_id: conference_id.to_string(),
        meetup_id: meetup_id.map(ToOwned::to_owned),
        speaker_text: speaker_text.map(ToOwned::to_owned),
        tags,
        url,
        description,
        date_time,
        end_time,
    }
}

fn split_role(role: &str) -> (Option<String>, Option<String>) {
    let role = clean_text(role);
    if let Some((title, organization)) = role.split_once(" @ ") {
        return (nonempty(organization), nonempty(title));
    }
    if let Some((organization, title)) = role.split_once(", ") {
        return (nonempty(organization), nonempty(title));
    }
    (None, nonempty(role))
}

fn meetup_urlname(url: &str) -> Result<String> {
    let parsed = Url::parse(url).with_context(|| format!("invalid Meetup URL: {url}"))?;
    parsed
        .path_segments()
        .and_then(|mut segments| {
            segments.find(|segment| !segment.is_empty() && *segment != "events")
        })
        .map(ToOwned::to_owned)
        .context("Meetup URL did not contain a group urlname")
}

fn meetup_archive_url(url: &str) -> Result<String> {
    let parsed = Url::parse(url).with_context(|| format!("invalid Meetup URL: {url}"))?;
    let urlname = meetup_urlname(url)?;
    Ok(format!(
        "{}://{}/{}/events/?type=past",
        parsed.scheme(),
        parsed
            .host_str()
            .context("Meetup URL did not contain a host")?,
        urlname
    ))
}

fn json_unescape(value: &str) -> Result<String> {
    serde_json::from_str(&format!("\"{value}\"")).context("failed to unescape JSON string")
}

fn html_to_text(fragment_html: &str) -> String {
    let fragment = Html::parse_fragment(fragment_html);
    let text = fragment.root_element().text().collect::<Vec<_>>().join(" ");
    clean_text(&text)
}

fn split_lines(value: &str) -> Vec<String> {
    value
        .replace("\\n", "\n")
        .lines()
        .map(clean_text)
        .filter(|line| !line.is_empty())
        .collect()
}

fn clean_text(value: &str) -> String {
    html_escape::decode_html_entities(value)
        .replace('\u{200b}', "")
        .replace('\u{a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn nonempty(value: impl Into<String>) -> Option<String> {
    let value = clean_text(&value.into());
    (!value.is_empty()).then_some(value)
}
