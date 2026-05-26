#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import html
import json
import re
import shutil
import unicodedata
from collections import defaultdict
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_GRAPH = ROOT / "data" / "bythebay-graph.json"
DEFAULT_SOURCES = ROOT / "data" / "raw" / "sources"
DEFAULT_OUT = ROOT / "data" / "enriched"

TITLE_WORDS = {
    "advocate",
    "architect",
    "author",
    "ceo",
    "chair",
    "chief",
    "co-founder",
    "cofounder",
    "contributor",
    "cto",
    "data",
    "developer",
    "director",
    "engineer",
    "engineering",
    "evangelist",
    "fellow",
    "founder",
    "gm",
    "head",
    "inventor",
    "lead",
    "manager",
    "marketing",
    "member",
    "mts",
    "officer",
    "president",
    "principal",
    "product",
    "professor",
    "research",
    "researcher",
    "scientist",
    "software",
    "staff",
    "student",
    "technologist",
    "vp",
}

COMPANY_SUFFIXES = {
    "ai",
    "cloud",
    "collective",
    "company",
    "corp",
    "corporation",
    "foundation",
    "inc",
    "labs",
    "lab",
    "llc",
    "school",
    "systems",
    "team",
    "technologies",
    "university",
}

TITLE_BLOCKLIST = {
    "abstract",
    "agenda",
    "audience",
    "bio",
    "description",
    "event",
    "food",
    "format",
    "level",
    "links",
    "location",
    "notes",
    "please rsvp",
    "registration",
    "schedule",
    "speaker",
    "speakers",
    "sponsors",
    "thank you",
    "thanks",
    "venue",
    "welcome",
}

PERSON_BLOCKLIST = {
    "ai alliance",
    "apache spark",
    "bay area",
    "big data",
    "data science",
    "developer advocate",
    "food drinks",
    "general assembly",
    "google cloud",
    "machine learning",
    "meetup event",
    "open source",
    "product management",
    "san francisco",
    "scala center",
    "software engineer",
    "technical meetup",
    "world champion",
}

PROJECT_BLOCKLIST = {
    "AI",
    "API",
    "Abstract",
    "Agenda",
    "Bay Area",
    "Data",
    "Developer",
    "Event",
    "Format",
    "GitHub",
    "LLM",
    "LLMs",
    "Meetup",
    "Project",
    "Speaker",
    "Talk",
    "Workshop",
    "and",
}


def read_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8") as file:
        return json.load(file)


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as file:
        json.dump(value, file, indent=2, ensure_ascii=False, sort_keys=True)
        file.write("\n")


def write_jsonl(path: Path, records: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as file:
        for record in records:
            file.write(json.dumps(record, ensure_ascii=False, sort_keys=True))
            file.write("\n")


def slugify(value: str, fallback: str = "unknown", max_len: int = 140) -> str:
    value = unicodedata.normalize("NFKD", value)
    value = value.encode("ascii", "ignore").decode("ascii")
    value = re.sub(r"[^A-Za-z0-9]+", "-", value.lower()).strip("-")
    value = re.sub(r"-+", "-", value)
    if not value:
        value = fallback
    return value[:max_len].strip("-") or fallback


def entity_filename(entity_id: str) -> str:
    digest = hashlib.sha1(entity_id.encode("utf-8")).hexdigest()[:10]
    return f"{slugify(entity_id, max_len=180)}-{digest}.json"


def clean_text(value: Any) -> str:
    if value is None:
        return ""
    text = str(value)
    text = html.unescape(text)
    text = text.replace("\u200b", " ").replace("\u200e", " ").replace("\ufeff", " ")
    text = text.replace("&#39;", "'")
    text = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", r"\1", text)
    text = re.sub(r"[*_`#]+", " ", text)
    text = re.sub(r"<[^>]+>", " ", text)
    text = text.replace("–", "-").replace("—", "-")
    text = re.sub(r"\s+", " ", text)
    return text.strip(" \t\r\n-:;.")


def clean_heading(value: str) -> str:
    text = value.strip()
    text = re.sub(r"^#+\s*", "", text)
    text = re.sub(r"^\d+[.)]\s*", "", text)
    text = re.sub(r"^talk\s*#?\d+\s*[:.-]\s*", "", text, flags=re.I)
    return clean_text(text)


def clean_company(value: Any) -> str:
    text = clean_text(value)
    if re.search(r"https?://|github\.com|linkedin\.com|mastodon\.social", text, flags=re.I):
        return ""
    text = re.split(
        r"\b(?:who|where|which|talking|speaking|presenting|will|and will|about|on)\b",
        text,
        maxsplit=1,
        flags=re.I,
    )[0]
    text = re.split(r"\s+[-|]\s+", text, maxsplit=1)[0]
    text = text.strip(" ,.;:()[]")
    return text


def clean_title(value: Any) -> str:
    text = clean_text(value)
    if re.search(r"https?://|github\.com|linkedin\.com|mastodon\.social", text, flags=re.I):
        return ""
    text = text.strip(" ,.;:()[]")
    text = re.sub(r"^(?:a|an|the)\s+", "", text, flags=re.I)
    return text


def is_title_like(value: str) -> bool:
    words = re.findall(r"[A-Za-z][A-Za-z.+-]*", value.lower())
    return any(word in TITLE_WORDS for word in words)


def is_company_like(value: str) -> bool:
    text = clean_company(value)
    if not text or len(text) > 80:
        return False
    lower = text.lower().strip()
    blocked = {
        "&",
        "bio",
        "bio to be added",
        "links",
        "q&a",
        "speaker",
        "speakers",
        "to be added",
    }
    if lower in blocked or "to be added" in lower:
        return False
    if not any(ch.isalpha() for ch in text):
        return False
    if is_title_like(text) and not any(suffix in lower.split() for suffix in COMPANY_SUFFIXES):
        return False
    words = re.findall(r"[A-Za-z0-9.+-]+", lower)
    return len(words) <= 7 and (
        any(word in COMPANY_SUFFIXES for word in words)
        or any(ch.isupper() for ch in text)
        or "." in text
    )


def parse_role(role: str | None, organization: str | None = None) -> tuple[str | None, str | None]:
    candidates = [clean_text(role), clean_text(organization)]
    candidates = [value for value in candidates if value]
    if not candidates:
        return None, None
    value = " @ ".join(candidates) if len(candidates) == 2 else candidates[0]
    if re.search(r"https?://|github\.com|linkedin\.com|mastodon\.social", value, flags=re.I):
        return None, None
    value = re.sub(r"\s*@\s*", " @ ", value)

    at_match = re.search(r"\s+(?:@|at|with|for)\s+(.+)$", value, flags=re.I)
    if at_match:
        left = clean_title(value[: at_match.start()])
        right = clean_company(at_match.group(1))
        if right:
            return (left or None), right

    parts = [clean_text(part) for part in re.split(r"\s*,\s*", value) if clean_text(part)]
    if len(parts) >= 2:
        first, rest = parts[0], parts[1:]
        if is_title_like(first):
            company = next((clean_company(part) for part in rest if is_company_like(part)), "")
            title_parts = [first] + [part for part in rest if is_title_like(part) and not is_company_like(part)]
            return (", ".join(dict.fromkeys(title_parts)) or None), (company or None)
        if any(is_title_like(part) for part in rest):
            title = ", ".join(part for part in rest if is_title_like(part))
            company_parts = [first] + [part for part in rest if is_company_like(part) and not is_title_like(part)]
            company = clean_company(company_parts[0]) if company_parts else ""
            return (title or None), (company or None)

    if is_title_like(value):
        return clean_title(value), None
    return None, clean_company(value) or None


def parse_person_affiliation(name: str, text: str) -> tuple[str | None, str | None]:
    escaped = re.escape(name)
    patterns = [
        rf"{escaped}\s+is\s+(?:currently\s+)?(?:a|an|the)?\s*(?P<title>[^.\n;:()]{{2,100}}?)\s+(?:at|with|for)\s+(?P<company>[A-Z0-9][^.\n;:()]{{1,100}})",
        rf"{escaped}\s+is\s+(?:currently\s+)?(?:a|an|the)?\s*(?P<title>[^.\n;:()]{{2,80}}?)\s+of\s+(?P<company>[A-Z0-9][A-Za-z0-9& .,'/-]{{1,80}}?)(?:\.|\s+where|\s+who|\s+and|$)",
        rf"{escaped}\s*,\s*(?P<title>[^.\n,;:()]{{2,80}}?)\s*(?:@|at)\s*(?P<company>[^.\n,;:()]{{1,100}})",
        rf"{escaped}\s*,\s*(?P<title>[^.\n,;:()]{{2,80}}?)\s*,\s*(?P<company>[A-Z0-9][^.\n;:()]{{1,100}})",
    ]
    candidate_lines = [line for line in text.splitlines() if name in line and "http" not in line.lower()]
    if not candidate_lines:
        flattened = clean_text(text)
        name_pos = flattened.find(name)
        if name_pos >= 0:
            candidate_lines = [flattened[name_pos : name_pos + 260]]
    for candidate in candidate_lines:
        candidate = clean_text(candidate)
        for pattern in patterns:
            match = re.search(pattern, candidate, flags=re.I)
            if not match:
                continue
            title = clean_title(match.groupdict().get("title"))
            company = clean_company(match.groupdict().get("company"))
            if title and len(title.split()) > 10:
                title = ""
            if company and len(company.split()) > 8:
                company = ""
            if company or title:
                return title or None, company or None
    return None, None


def is_likely_person_name(value: str) -> bool:
    name = clean_text(value)
    lower = name.lower()
    if lower in PERSON_BLOCKLIST:
        return False
    if not name or any(ch.isdigit() for ch in name) or len(name) > 70:
        return False
    if any(token in lower for token in ("http", "www.", "meetup", "registration", "rsvp")):
        return False
    words = name.split()
    if not (2 <= len(words) <= 5):
        return False
    blocked_tokens = {
        "ai",
        "api",
        "aws",
        "cloud",
        "data",
        "deep",
        "developer",
        "engineer",
        "engineering",
        "foundation",
        "graph",
        "hadoop",
        "inc",
        "machine",
        "manager",
        "open",
        "project",
        "scala",
        "science",
        "spark",
        "systems",
        "team",
        "technology",
    }
    for word in words:
        stripped = word.strip(".,:;()[]{}'\"")
        if not stripped:
            return False
        if stripped.lower() in blocked_tokens:
            return False
        if stripped.isupper() and len(stripped) > 2:
            return False
        if not stripped[0].isupper() and stripped.lower() not in {"de", "del", "der", "van", "von"}:
            return False
    return True


def source_ref(path: Path | None = None, record: dict[str, Any] | None = None) -> dict[str, str]:
    ref: dict[str, str] = {}
    if path is not None:
        ref["source_file"] = str(path.relative_to(ROOT))
    if record:
        for key in ("source_id", "source_url"):
            value = record.get(key)
            if isinstance(value, str) and value:
                ref[key] = value
    return ref


def append_unique(values: list[Any], value: Any) -> None:
    if value is None or value == "":
        return
    if value not in values:
        values.append(value)


class EntityStore:
    def __init__(self) -> None:
        self.conferences: dict[str, dict[str, Any]] = {}
        self.meetups: dict[str, dict[str, Any]] = {}
        self.speakers: dict[str, dict[str, Any]] = {}
        self.companies: dict[str, dict[str, Any]] = {}
        self.talks: dict[str, dict[str, Any]] = {}
        self.projects: dict[str, dict[str, Any]] = {}
        self.edges: dict[tuple[str, str, str], dict[str, str]] = {}

    def add_edge(self, from_id: str, to_id: str, relationship: str) -> None:
        if not from_id or not to_id:
            return
        key = (from_id, to_id, relationship)
        self.edges[key] = {"from": from_id, "to": to_id, "relationship": relationship}

    def add_conference(self, conference: dict[str, Any]) -> str:
        conference_id = conference["id"]
        record = self.conferences.setdefault(
            conference_id,
            {
                "id": conference_id,
                "type": "conference",
                "name": conference.get("name") or conference_id,
                "site": conference.get("site"),
                "url": conference.get("url"),
                "talk_ids": [],
            },
        )
        for key in ("name", "site", "url"):
            if conference.get(key) and not record.get(key):
                record[key] = conference[key]
        return conference_id

    def add_meetup(self, meetup: dict[str, Any]) -> str:
        meetup_id = meetup["id"]
        record = self.meetups.setdefault(
            meetup_id,
            {
                "id": meetup_id,
                "type": "meetup",
                "name": meetup.get("name") or meetup_id,
                "url": meetup.get("url"),
                "timezone": meetup.get("timezone"),
                "source_urls": [],
                "talk_ids": [],
            },
        )
        for key in ("name", "url", "timezone"):
            if meetup.get(key) and not record.get(key):
                record[key] = meetup[key]
        append_unique(record["source_urls"], meetup.get("url"))
        return meetup_id

    def add_company(self, name: str | None, source: dict[str, str] | None = None) -> str | None:
        company = clean_company(name)
        if not company or not is_company_like(company):
            return None
        company_id = f"company:{slugify(company)}"
        record = self.companies.setdefault(
            company_id,
            {
                "id": company_id,
                "type": "company",
                "name": company,
                "speaker_ids": [],
                "talk_ids": [],
                "source_files": [],
                "source_urls": [],
                "source_ids": [],
            },
        )
        for key, target in (
            ("source_file", "source_files"),
            ("source_url", "source_urls"),
            ("source_id", "source_ids"),
        ):
            if source:
                append_unique(record[target], source.get(key))
        return company_id

    def add_speaker(
        self,
        name: str,
        title: str | None = None,
        company: str | None = None,
        source: dict[str, str] | None = None,
        meetup_id: str | None = None,
        conference_id: str | None = None,
    ) -> str | None:
        name = clean_text(name)
        if not is_likely_person_name(name):
            return None
        speaker_id = f"speaker:{slugify(name)}"
        title = clean_title(title) or None
        company_id = self.add_company(company, source)
        company_name = self.companies[company_id]["name"] if company_id else None
        record = self.speakers.setdefault(
            speaker_id,
            {
                "id": speaker_id,
                "type": "speaker",
                "name": name,
                "title": title,
                "company_id": company_id,
                "company_name": company_name,
                "titles": [],
                "company_ids": [],
                "company_names": [],
                "affiliations": [],
                "talk_ids": [],
                "meetup_ids": [],
                "conference_ids": [],
                "source_files": [],
                "source_urls": [],
                "source_ids": [],
            },
        )
        if title and not record.get("title"):
            record["title"] = title
        if company_id and not record.get("company_id"):
            record["company_id"] = company_id
            record["company_name"] = company_name
        append_unique(record["titles"], title)
        append_unique(record["company_ids"], company_id)
        append_unique(record["company_names"], company_name)
        if title or company_id:
            affiliation = {"title": title, "company_id": company_id, "company_name": company_name}
            if affiliation not in record["affiliations"]:
                record["affiliations"].append(affiliation)
        append_unique(record["meetup_ids"], meetup_id)
        append_unique(record["conference_ids"], conference_id)
        for key, target in (
            ("source_file", "source_files"),
            ("source_url", "source_urls"),
            ("source_id", "source_ids"),
        ):
            if source:
                append_unique(record[target], source.get(key))
        if company_id:
            append_unique(self.companies[company_id]["speaker_ids"], speaker_id)
            self.add_edge(speaker_id, company_id, "WORKS_FOR")
        return speaker_id

    def add_project(
        self,
        name: str,
        github_repository: str | None,
        source: dict[str, str] | None,
        talk_id: str | None = None,
    ) -> str | None:
        name = clean_text(name)
        if not is_project_name(name, has_repo=bool(github_repository)):
            return None
        repo = canonical_github_url(github_repository)
        project_id = f"project:{slugify(repo or name)}"
        record = self.projects.setdefault(
            project_id,
            {
                "id": project_id,
                "type": "project",
                "name": name,
                "github_repository": repo,
                "github_repositories": [],
                "talk_ids": [],
                "source_files": [],
                "source_urls": [],
                "source_ids": [],
            },
        )
        if repo and not record.get("github_repository"):
            record["github_repository"] = repo
        append_unique(record["github_repositories"], repo)
        append_unique(record["talk_ids"], talk_id)
        for key, target in (
            ("source_file", "source_files"),
            ("source_url", "source_urls"),
            ("source_id", "source_ids"),
        ):
            if source:
                append_unique(record[target], source.get(key))
        if talk_id:
            self.add_edge(talk_id, project_id, "MENTIONS_PROJECT")
        return project_id

    def add_talk(
        self,
        *,
        title: str,
        abstract: str | None,
        kind: str,
        source: dict[str, str],
        meetup_id: str | None,
        conference_id: str | None,
        event_id: str | None = None,
        event_title: str | None = None,
        url: str | None = None,
        date_time: str | None = None,
        end_time: str | None = None,
        tags: list[str] | None = None,
        speaker_mentions: list[dict[str, str | None]] | None = None,
        project_mentions: list[dict[str, str | None]] | None = None,
    ) -> str | None:
        title = clean_text(title)
        if not is_candidate_title(title):
            return None
        scope = meetup_id or conference_id or "bythebay"
        suffix_seed = source.get("source_id") or event_id or title
        talk_id = f"talk:{slugify(scope)}:{slugify(suffix_seed)}:{slugify(title, max_len=90)}"
        record = self.talks.setdefault(
            talk_id,
            {
                "id": talk_id,
                "type": "talk",
                "kind": clean_text(kind).lower() or "talk",
                "title": title,
                "abstract": clean_text(abstract) or None,
                "meetup_id": meetup_id,
                "conference_id": conference_id,
                "event_id": event_id,
                "event_title": clean_text(event_title) or None,
                "url": url,
                "date_time": date_time,
                "end_time": end_time,
                "tags": [],
                "speaker_ids": [],
                "speakers": [],
                "project_ids": [],
                "github_repositories": [],
                "source_files": [],
                "source_urls": [],
                "source_ids": [],
            },
        )
        if abstract and not record.get("abstract"):
            record["abstract"] = clean_text(abstract)
        for key in ("meetup_id", "conference_id", "event_id", "url", "date_time", "end_time"):
            value = locals().get(key)
            if value and not record.get(key):
                record[key] = value
        for tag in tags or []:
            append_unique(record["tags"], clean_text(tag))
        for key, target in (
            ("source_file", "source_files"),
            ("source_url", "source_urls"),
            ("source_id", "source_ids"),
        ):
            append_unique(record[target], source.get(key))
        if meetup_id and meetup_id in self.meetups:
            append_unique(self.meetups[meetup_id]["talk_ids"], talk_id)
            self.add_edge(talk_id, meetup_id, "PART_OF_MEETUP")
        if conference_id:
            if conference_id in self.conferences:
                append_unique(self.conferences[conference_id]["talk_ids"], talk_id)
            self.add_edge(talk_id, conference_id, "PART_OF_CONFERENCE")

        for mention in speaker_mentions or []:
            speaker_id = self.add_speaker(
                mention.get("name") or "",
                mention.get("title"),
                mention.get("company"),
                source,
                meetup_id,
                conference_id,
            )
            if not speaker_id:
                continue
            append_unique(record["speaker_ids"], speaker_id)
            mention_company = clean_company(mention.get("company")) or None
            if mention_company and not is_company_like(mention_company):
                mention_company = None
            speaker_summary = {
                "id": speaker_id,
                "name": self.speakers[speaker_id]["name"],
                "title": clean_title(mention.get("title")) or None,
                "company_name": mention_company,
            }
            if speaker_summary not in record["speakers"]:
                record["speakers"].append(speaker_summary)
            append_unique(self.speakers[speaker_id]["talk_ids"], talk_id)
            self.add_edge(speaker_id, talk_id, "PRESENTS")
            if self.speakers[speaker_id].get("company_id"):
                company_id = self.speakers[speaker_id]["company_id"]
                append_unique(self.companies[company_id]["talk_ids"], talk_id)
                self.add_edge(talk_id, company_id, "PRESENTED_BY_COMPANY")

        text_for_projects = "\n".join(value for value in [title, abstract or ""] if value)
        mentions = list(project_mentions or [])
        mentions.extend(extract_projects(text_for_projects))
        for mention in dedupe_project_mentions(mentions):
            project_id = self.add_project(
                mention.get("name") or "",
                mention.get("github_repository"),
                source,
                talk_id,
            )
            if not project_id:
                continue
            append_unique(record["project_ids"], project_id)
            append_unique(record["github_repositories"], self.projects[project_id].get("github_repository"))
        return talk_id

    def graph_records(self) -> dict[str, Any]:
        nodes: list[dict[str, Any]] = []
        for label, records in (
            ("Conference", self.conferences.values()),
            ("Meetup", self.meetups.values()),
            ("Speaker", self.speakers.values()),
            ("Company", self.companies.values()),
            ("Talk", self.talks.values()),
            ("Project", self.projects.values()),
        ):
            for record in sorted(records, key=lambda item: item["id"]):
                props = dict(record)
                props.pop("type", None)
                nodes.append({"id": record["id"], "type": label, "properties": props})
        return {"nodes": nodes, "edges": sorted(self.edges.values(), key=lambda edge: (edge["from"], edge["to"], edge["relationship"]))}


def canonical_github_url(value: str | None) -> str | None:
    if not value:
        return None
    match = re.search(r"github\.com[:/]+([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)", value)
    if not match:
        return None
    owner = match.group(1).strip(".")
    repo = match.group(2).strip(".")
    if repo.endswith(".git"):
        repo = repo[:-4]
    if not owner or not repo:
        return None
    return f"https://github.com/{owner}/{repo}"


def github_repo_name(url: str) -> str:
    repo = canonical_github_url(url)
    if not repo:
        return clean_text(url)
    return repo.rstrip("/").split("/")[-1]


def clean_project_name(value: Any) -> str:
    name = clean_text(value)
    name = name.strip(".,;:()[]{}")
    name = re.sub(r"-?based$", "", name, flags=re.I)
    name = re.split(r"\+Apache|\s+-\s+Apache|-Apache", name, maxsplit=1)[0]
    name = re.sub(r"\s+", " ", name)
    return name


def is_project_name(value: str, has_repo: bool = False) -> bool:
    name = clean_project_name(value)
    if not name or name in PROJECT_BLOCKLIST or name.lower() in PROJECT_BLOCKLIST:
        return False
    if re.search(r"https?://|github\.com", name, flags=re.I):
        return False
    if len(name.split()) > 5 or len(name) > 80:
        return False
    if has_repo:
        return True
    if name.startswith("Apache ") and name.split(maxsplit=1)[1] in {
        "Big",
        "Foundation",
        "Incubator",
        "Software",
    }:
        return False
    if name.lower() in {"actor", "agent", "agentic", "automated", "biases", "drive", "http"}:
        return False
    return any(ch.isupper() for ch in name) or "-" in name or "." in name


def extract_projects(text: str) -> list[dict[str, str | None]]:
    mentions: list[dict[str, str | None]] = []
    for link_text, url in re.findall(r"\[([^\]]+)\]\((https?://github\.com/[^)\s]+)\)", text, flags=re.I):
        repo = canonical_github_url(url)
        if repo:
            link_name = clean_project_name(link_text)
            if (
                re.search(r"github\.com", link_name, flags=re.I)
                or len(link_name.split()) > 3
                or link_name.lower() in {"here", "github", "github repo", "github show notes", "first notebook"}
            ):
                link_name = github_repo_name(repo)
            mentions.append({"name": link_name or github_repo_name(repo), "github_repository": repo})
    for url in re.findall(r"https?://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+[^\s)\]]*", text, flags=re.I):
        repo = canonical_github_url(url)
        if repo:
            mentions.append({"name": github_repo_name(repo), "github_repository": repo})
    for match in re.finditer(r"\b(?:Apache|Eclipse)\s+[A-Z][A-Za-z0-9.+-]+", text):
        mentions.append({"name": clean_project_name(match.group(0)), "github_repository": None})
    intro = re.search(r"\b(?:Introducing|Meet)\s+([A-Za-z][A-Za-z0-9_.-]{2,})\b", text)
    if intro:
        mentions.append({"name": clean_project_name(intro.group(1)), "github_repository": None})
    dash = re.match(r"\s*([A-Z][A-Za-z0-9_.-]{2,})\s+-\s+", text)
    if dash:
        mentions.append({"name": clean_project_name(dash.group(1)), "github_repository": None})
    for match in re.finditer(
        r"\b([A-Z][A-Za-z0-9_.+-]{2,}(?:\s+[A-Z][A-Za-z0-9_.+-]{2,}){0,2})\s+(?:project|library|framework|tool|stack|platform|model|database)\b",
        text,
    ):
        name = clean_project_name(match.group(1))
        if is_project_name(name):
            mentions.append({"name": name, "github_repository": None})
    return mentions


def dedupe_project_mentions(mentions: list[dict[str, str | None]]) -> list[dict[str, str | None]]:
    seen: set[str] = set()
    output: list[dict[str, str | None]] = []
    for mention in mentions:
        repo = canonical_github_url(mention.get("github_repository"))
        name = clean_project_name(mention.get("name") or github_repo_name(repo or ""))
        key = repo or name.lower()
        if not is_project_name(name, has_repo=bool(repo)) or key in seen:
            continue
        seen.add(key)
        output.append({"name": name, "github_repository": repo})
    return output


def is_candidate_title(value: str) -> bool:
    title = clean_heading(value)
    lower = title.lower()
    if not title or len(title) < 4:
        return False
    if any(lower == blocked or lower.startswith(f"{blocked}:") for blocked in TITLE_BLOCKLIST):
        return False
    if any(
        marker in lower
        for marker in (
            "arrival",
            "event end",
            "food and drink",
            "grab a drink",
            "make our way",
            "networking",
            "registration is required",
            "please rsvp",
            "join us for",
            "thank you to",
            "welcome to",
        )
    ):
        return False
    words = title.split()
    if len(words) == 1 and not re.search(r"[A-Z]{2,}|[.-]", title):
        return False
    return len(words) <= 24


def normalize_kind(value: str | None) -> str:
    lower = clean_text(value).lower()
    if lower in {"workshop", "training", "hackathon"}:
        return "workshop"
    if lower in {"social", "party", "mixer"}:
        return "social"
    if lower in {"unconference", "unmeetup"}:
        return "unconference"
    if lower in {"announcement", "cfp"}:
        return "announcement"
    return "talk"


def known_name_positions(text: str, known_names: list[str]) -> list[tuple[int, int, str]]:
    positions: list[tuple[int, int, str]] = []
    for name in known_names:
        if len(name) < 5:
            continue
        pattern = rf"(?<![A-Za-z]){re.escape(name)}(?![A-Za-z])"
        for match in re.finditer(pattern, text):
            positions.append((match.start(), match.end(), name))
    positions.sort(key=lambda item: (item[0], -(item[1] - item[0])))
    deduped: list[tuple[int, int, str]] = []
    last_end = -1
    for start, end, name in positions:
        if start < last_end:
            continue
        deduped.append((start, end, name))
        last_end = end
    return deduped


def parse_speaker_segment(name: str, segment: str) -> dict[str, str | None]:
    after_name = segment[len(name) :]
    after_name = after_name.strip(" ,;:-")
    role_lines = [
        clean_text(line)
        for line in re.split(r"[\n\r]+", after_name)
        if clean_text(line) and not re.search(r"https?://|github\.com|linkedin\.com|mastodon\.social", line, flags=re.I)
    ]
    role_text = role_lines[0] if role_lines else after_name
    if role_text.lower().startswith(("is ", "was ", "has ")):
        title, company = None, None
    else:
        title, company = parse_role(role_text)
    bio_title, bio_company = parse_person_affiliation(name, segment)
    return {
        "name": name,
        "title": title or bio_title,
        "company": company or bio_company,
    }


def parse_speaker_text(text: str, known_names: list[str]) -> list[dict[str, str | None]]:
    text = clean_text(text)
    if not text:
        return []
    positions = known_name_positions(text, known_names)
    mentions: list[dict[str, str | None]] = []
    if positions:
        for idx, (start, _end, name) in enumerate(positions):
            next_start = positions[idx + 1][0] if idx + 1 < len(positions) else len(text)
            segment = text[start:next_start]
            mentions.append(parse_speaker_segment(name, segment))
        propagate_common_affiliation(mentions)
        return dedupe_speakers(mentions)

    rough_parts = re.split(r"\s*(?:;|\band\b|&|/)\s*", text)
    for part in rough_parts:
        match = re.match(r"([A-Z][A-Za-z'’.-]+(?:\s+[A-Z][A-Za-z'’.-]+){1,4})(.*)$", part.strip())
        if not match:
            continue
        name = clean_text(match.group(1))
        if is_likely_person_name(name):
            mentions.append(parse_speaker_segment(name, part.strip()))
    return dedupe_speakers(mentions)


def propagate_common_affiliation(mentions: list[dict[str, str | None]]) -> None:
    companies = [mention.get("company") for mention in mentions if mention.get("company")]
    if not companies:
        return
    common = max(set(companies), key=companies.count)
    if companies.count(common) < 2:
        return
    for mention in mentions:
        if not mention.get("company"):
            mention["company"] = common


def dedupe_speakers(mentions: list[dict[str, str | None]]) -> list[dict[str, str | None]]:
    by_name: dict[str, dict[str, str | None]] = {}
    for mention in mentions:
        name = clean_text(mention.get("name"))
        if not is_likely_person_name(name):
            continue
        key = slugify(name)
        existing = by_name.setdefault(key, {"name": name, "title": None, "company": None})
        if mention.get("title") and not existing.get("title"):
            existing["title"] = clean_title(mention.get("title"))
        if mention.get("company") and not existing.get("company"):
            existing["company"] = clean_company(mention.get("company"))
    return list(by_name.values())


def extract_speakers_from_section(lines: list[str], title: str, known_names: list[str]) -> list[dict[str, str | None]]:
    text = "\n".join(lines)
    mentions: list[dict[str, str | None]] = []

    in_speaker_block = False
    speaker_block_lines: list[str] = []
    for line in lines:
        stripped = line.strip()
        plain = clean_text(stripped)
        lower = plain.lower()
        if re.match(r"^#{1,4}\s*(speaker|speakers|panelists?)\b", stripped, flags=re.I) or lower in {"speaker", "speakers", "panelists"}:
            in_speaker_block = True
            continue
        if in_speaker_block and re.match(r"^#{1,3}\s+", stripped) and lower not in {"speaker", "speakers", "panelists"}:
            break
        if in_speaker_block:
            speaker_block_lines.append(stripped)
        bold = re.match(r"^\s*(?:[-*]\s*)?\*\*([^*]{3,80})\*\*\s*$", stripped)
        if bold:
            name = clean_text(bold.group(1))
            if is_likely_person_name(name):
                mentions.append({"name": name, "title": None, "company": None})

    for line in speaker_block_lines:
        mentions.extend(parse_speaker_text(line, known_names))

    for line in lines:
        plain = clean_text(line)
        if re.search(r"\bby\b", plain, flags=re.I) and title.lower()[:18] in plain.lower():
            mentions.extend(parse_speaker_text(re.split(r"\bby\b", plain, maxsplit=1, flags=re.I)[-1], known_names))
        if plain.lower().startswith(("speaker:", "speakers:", "presenter:", "presenters:")):
            mentions.extend(parse_speaker_text(plain.split(":", 1)[1], known_names))

    mentions.extend(parse_speaker_text(text, known_names))
    deduped = dedupe_speakers(mentions)
    for mention in deduped:
        aff_title, aff_company = parse_person_affiliation(mention["name"] or "", text)
        if aff_title and not mention.get("title"):
            mention["title"] = aff_title
        if aff_company and not mention.get("company"):
            mention["company"] = aff_company
    return dedupe_speakers(deduped)


def abstract_from_lines(lines: list[str]) -> str | None:
    kept: list[str] = []
    for line in lines:
        plain = clean_text(line)
        lower = plain.lower()
        if not plain:
            continue
        if lower in TITLE_BLOCKLIST or lower.startswith(("speaker", "bio", "agenda", "thank", "notes", "links")):
            break
        if lower.startswith(("please rsvp", "register", "arrival", "welcome to", "event end")):
            continue
        kept.append(plain)
    abstract = clean_text(" ".join(kept))
    if abstract and len(abstract.split()) >= 5:
        return abstract
    return None


def heading_candidates(description: str, event_title: str, known_names: list[str]) -> list[dict[str, Any]]:
    lines = description.splitlines()
    headings: list[tuple[int, int, str]] = []
    for idx, line in enumerate(lines):
        match = re.match(r"^\s*(#{1,4})\s+(.+?)\s*$", line)
        if match:
            headings.append((idx, len(match.group(1)), clean_heading(match.group(2))))
    candidates: list[dict[str, Any]] = []
    for pos, (idx, level, title) in enumerate(headings):
        if not is_candidate_title(title):
            continue
        end = len(lines)
        for next_idx, next_level, _ in headings[pos + 1 :]:
            if next_level <= level:
                end = next_idx
                break
        section = lines[idx + 1 : end]
        if not section and title.lower() == clean_heading(event_title).lower():
            continue
        candidates.append(
            {
                "title": title,
                "abstract": abstract_from_lines(section),
                "kind": "talk",
                "speakers": extract_speakers_from_section(section, title, known_names),
                "project_mentions": extract_projects("\n".join([title, *section])),
            }
        )
    return candidates


def marker_candidates(description: str, known_names: list[str]) -> list[dict[str, Any]]:
    lines = description.splitlines()
    starts: list[tuple[int, str]] = []
    for idx, line in enumerate(lines):
        plain = clean_text(line)
        patterns = [
            r"^(?:Talk|Presentation)\s*#?\d*\s*[:.-]\s*(.+)$",
            r"^Title\s*[:.-]\s*(.+)$",
            r"^Talk Title\s*[:.-]?\s*(.+)$",
        ]
        for pattern in patterns:
            match = re.match(pattern, plain, flags=re.I)
            if match and is_candidate_title(match.group(1)):
                starts.append((idx, clean_heading(match.group(1))))
                break
    candidates: list[dict[str, Any]] = []
    for pos, (idx, title) in enumerate(starts):
        end = starts[pos + 1][0] if pos + 1 < len(starts) else min(len(lines), idx + 28)
        section = lines[idx + 1 : end]
        candidates.append(
            {
                "title": title,
                "abstract": abstract_from_lines(section),
                "kind": "talk",
                "speakers": extract_speakers_from_section(section, title, known_names),
                "project_mentions": extract_projects("\n".join([title, *section])),
            }
        )
    return candidates


def agenda_candidates(description: str, known_names: list[str]) -> list[dict[str, Any]]:
    candidates: list[dict[str, Any]] = []
    for raw_line in description.splitlines():
        line = clean_text(raw_line)
        if not re.search(r"\b(?:am|pm)\b|\d[:.]\d{2}", line, flags=re.I):
            continue
        line = re.sub(r"^\s*\d{1,2}(?:[:.]\d{2})?\s*(?:am|pm)?\s*[-–]\s*", "", line, flags=re.I)
        by_match = re.search(r"\s+by\s+(.+)$", line, flags=re.I)
        if by_match:
            title = clean_heading(line[: by_match.start()])
            if is_candidate_title(title):
                candidates.append(
                    {
                        "title": title,
                        "abstract": None,
                        "kind": "talk",
                        "speakers": parse_speaker_text(by_match.group(1), known_names),
                        "project_mentions": extract_projects(title),
                    }
                )
    return candidates


def quoted_candidates(description: str, known_names: list[str]) -> list[dict[str, Any]]:
    text = clean_text(description)
    candidates: list[dict[str, Any]] = []
    patterns = [
        r"(?P<speaker>[A-Z][A-Za-z'’.-]+(?:\s+[A-Z][A-Za-z'’.-]+){1,4})[^.]{0,120}?\b(?:present|speak|talk)[^.]{0,80}?[\"“](?P<title>[^\"”]{5,140})[\"”]",
        r"[\"“](?P<title>[^\"”]{5,140})[\"”]\s+(?:by\s+)?(?P<speaker>[A-Z][A-Za-z'’.-]+(?:\s+[A-Z][A-Za-z'’.-]+){1,4})",
    ]
    for pattern in patterns:
        for match in re.finditer(pattern, text, flags=re.I):
            title = clean_heading(match.group("title"))
            speaker = clean_text(match.group("speaker"))
            if is_candidate_title(title) and is_likely_person_name(speaker):
                candidates.append(
                    {
                        "title": title,
                        "abstract": None,
                        "kind": "talk",
                        "speakers": parse_speaker_text(speaker, known_names),
                        "project_mentions": extract_projects(title),
                    }
                )
    return candidates


def merge_candidates(candidates: list[dict[str, Any]]) -> list[dict[str, Any]]:
    by_title: dict[str, dict[str, Any]] = {}
    for candidate in candidates:
        title = clean_heading(candidate.get("title", ""))
        if not is_candidate_title(title):
            continue
        key = slugify(title)
        record = by_title.setdefault(
            key,
            {
                "title": title,
                "abstract": None,
                "kind": normalize_kind(candidate.get("kind")),
                "speakers": [],
                "project_mentions": [],
            },
        )
        if candidate.get("abstract") and not record.get("abstract"):
            record["abstract"] = clean_text(candidate["abstract"])
        record["speakers"] = dedupe_speakers(record["speakers"] + candidate.get("speakers", []))
        record["project_mentions"] = dedupe_project_mentions(record["project_mentions"] + candidate.get("project_mentions", []))
    return list(by_title.values())


def base_talk_to_candidate(talk: dict[str, Any], known_names: list[str]) -> dict[str, Any]:
    speaker_text = talk.get("speaker_text") or ""
    return {
        "title": talk.get("title"),
        "abstract": talk.get("description"),
        "kind": normalize_kind(talk.get("kind")),
        "speakers": parse_speaker_text(speaker_text, known_names),
        "project_mentions": extract_projects("\n".join([talk.get("title") or "", talk.get("description") or ""])),
        "base_talk": talk,
    }


def session_to_candidate(session_record: dict[str, Any], known_names: list[str]) -> dict[str, Any] | None:
    session = session_record.get("session") or {}
    title = clean_heading(session.get("title") or "")
    if not is_candidate_title(title):
        return None
    speakers = []
    for speaker in session.get("speakers") or []:
        title_guess, company_guess = parse_role(speaker.get("company"))
        speakers.append(
            {
                "name": clean_text(speaker.get("name")),
                "title": title_guess,
                "company": company_guess or clean_company(speaker.get("company")) or None,
            }
        )
    if not speakers:
        speakers = parse_speaker_text(session.get("description") or "", known_names)
    return {
        "title": title,
        "abstract": session.get("description"),
        "kind": normalize_kind(session.get("kind")),
        "speakers": dedupe_speakers(speakers),
        "project_mentions": extract_projects("\n".join([title, session.get("description") or ""])),
    }


def event_candidates(
    event_record: dict[str, Any],
    session_records: list[dict[str, Any]],
    base_talks: list[dict[str, Any]],
    known_names: list[str],
) -> list[dict[str, Any]]:
    event = event_record.get("event") or {}
    description = event.get("description") or ""
    event_title = event.get("title") or ""
    candidates: list[dict[str, Any]] = []
    candidates.extend(heading_candidates(description, event_title, known_names))
    candidates.extend(marker_candidates(description, known_names))
    candidates.extend(agenda_candidates(description, known_names))
    candidates.extend(quoted_candidates(description, known_names))
    direct_titles = {slugify(candidate.get("title", "")) for candidate in candidates}
    event_title_slug = slugify(event_title)
    for session_record in session_records:
        candidate = session_to_candidate(session_record, known_names)
        if not candidate:
            continue
        candidate_slug = slugify(candidate.get("title", ""))
        if direct_titles and (
            candidate_slug == event_title_slug
            or candidate_slug in direct_titles
        ):
            continue
        if candidate:
            candidates.append(candidate)
    merged = merge_candidates(candidates)

    if merged:
        return merged

    fallback = [base_talk_to_candidate(talk, known_names) for talk in base_talks]
    fallback = [candidate for candidate in fallback if is_candidate_title(candidate.get("title", ""))]
    if fallback:
        return merge_candidates(fallback)

    text = clean_text(description)
    if re.search(r"\b(talk|presentation|speaker|workshop)\b", text, flags=re.I) and is_candidate_title(event_title):
        return [
            {
                "title": clean_heading(event_title),
                "abstract": text or None,
                "kind": "workshop" if re.search(r"\bworkshop\b", text, flags=re.I) else "talk",
                "speakers": parse_speaker_text(text, known_names),
                "project_mentions": extract_projects("\n".join([event_title, description])),
            }
        ]
    return []


def build_known_names(graph: dict[str, Any], source_records: list[tuple[Path, dict[str, Any]]]) -> list[str]:
    names = {clean_text(person.get("name")) for person in graph.get("people", [])}
    for _path, record in source_records:
        if record.get("name"):
            names.add(clean_text(record["name"]))
        for speaker in (record.get("session") or {}).get("speakers") or []:
            names.add(clean_text(speaker.get("name")))
    names = {name for name in names if is_likely_person_name(name)}
    return sorted(names, key=lambda name: (-len(name), name.lower()))


def classify_source_records(source_dir: Path) -> tuple[
    list[tuple[Path, dict[str, Any]]],
    list[tuple[Path, dict[str, Any]]],
    dict[str, tuple[Path, dict[str, Any]]],
    dict[str, list[tuple[Path, dict[str, Any]]]],
    list[tuple[Path, dict[str, Any]]],
]:
    speaker_sources: list[tuple[Path, dict[str, Any]]] = []
    talk_sources: list[tuple[Path, dict[str, Any]]] = []
    event_sources: dict[str, tuple[Path, dict[str, Any]]] = {}
    session_sources: dict[str, list[tuple[Path, dict[str, Any]]]] = defaultdict(list)
    all_records: list[tuple[Path, dict[str, Any]]] = []
    for path in sorted(source_dir.glob("*.json")):
        record = read_json(path)
        all_records.append((path, record))
        if "event" in record:
            event_id = str((record.get("event") or {}).get("id") or "")
            if not event_id:
                continue
            if "session" in record:
                session_sources[event_id].append((path, record))
            if event_id not in event_sources or "session" not in record:
                event_sources[event_id] = (path, record)
        elif "speaker_text" in record:
            talk_sources.append((path, record))
        elif "role" in record or "name" in record:
            speaker_sources.append((path, record))
    return speaker_sources, talk_sources, event_sources, session_sources, all_records


def add_base_context(store: EntityStore, graph: dict[str, Any]) -> tuple[dict[str, list[dict[str, Any]]], dict[str, dict[str, Any]]]:
    source_by_person_id: dict[str, dict[str, Any]] = {}
    for conference in graph.get("conferences", []):
        store.add_conference(conference)
    for meetup in graph.get("meetups", []):
        store.add_meetup(meetup)
    for person in graph.get("people", []):
        org = clean_text(person.get("organization"))
        title = clean_text(person.get("title"))
        if org and title and is_title_like(org) and not is_title_like(title):
            org, title = title, org
        elif org and not title:
            parsed_title, parsed_company = parse_role(org)
            if parsed_title or parsed_company:
                title = parsed_title or ""
                org = parsed_company or org
        source = {
            "source_file": person.get("source_file") or "",
            "source_id": person.get("source_id") or "",
            "source_url": person.get("source_url") or "",
        }
        speaker_id = store.add_speaker(
            person.get("name") or "",
            title or None,
            org or None,
            source,
            person.get("meetup_id"),
            None if person.get("meetup_id") else person.get("conference_id"),
        )
        if speaker_id:
            source_by_person_id[person["id"]] = {"speaker_id": speaker_id, "source": source}

    talks_by_event: dict[str, list[dict[str, Any]]] = defaultdict(list)
    talks_by_id = {talk["id"]: talk for talk in graph.get("talks", [])}
    for talk in graph.get("talks", []):
        if talk.get("event_id"):
            talks_by_event[str(talk["event_id"])].append(talk)

    for edge in graph.get("edges", []):
        if edge.get("relationship") != "PRESENTS":
            continue
        source = source_by_person_id.get(edge.get("from"))
        talk = talks_by_id.get(edge.get("to"))
        if not source or not talk:
            continue
        talk.setdefault("_base_speaker_ids", []).append(source["speaker_id"])
    return talks_by_event, talks_by_id


def process_conference_speakers(
    store: EntityStore,
    speaker_sources: list[tuple[Path, dict[str, Any]]],
) -> None:
    for path, record in speaker_sources:
        source = source_ref(path, record)
        title, company = parse_role(record.get("role"))
        source_id = record.get("source_id") or path.stem
        conference_id = "ai-by-the-bay" if record.get("talk_title") is None else "scale-by-the-bay"
        speaker_id = store.add_speaker(record.get("name") or "", title, company, source, None, conference_id)
        if record.get("talk_title") and speaker_id:
            store.add_talk(
                title=record["talk_title"],
                abstract=None,
                kind="talk",
                source=source,
                meetup_id=None,
                conference_id="scale-by-the-bay",
                url=record.get("talk_url"),
                tags=[],
                speaker_mentions=[{"name": record.get("name"), "title": title, "company": company}],
                project_mentions=extract_projects(record["talk_title"]),
            )


def process_conference_talks(
    store: EntityStore,
    talk_sources: list[tuple[Path, dict[str, Any]]],
    known_names: list[str],
) -> None:
    for path, record in talk_sources:
        source = source_ref(path, record)
        text = "\n".join([record.get("title") or "", record.get("description") or "", record.get("speaker_text") or ""])
        store.add_talk(
            title=record.get("title") or path.stem,
            abstract=record.get("description"),
            kind="talk",
            source=source,
            meetup_id=None,
            conference_id="ai-by-the-bay",
            url=record.get("url"),
            tags=record.get("tags") or [],
            speaker_mentions=parse_speaker_text(record.get("speaker_text") or "", known_names),
            project_mentions=extract_projects(text),
        )


def event_source_metadata(record: dict[str, Any]) -> dict[str, Any]:
    event = record.get("event") or {}
    group = event.get("group") or {}
    return {
        "id": record.get("meetup_id") or f"meetup:{group.get('id') or slugify(record.get('meetup_name') or group.get('name') or 'meetup')}",
        "name": record.get("meetup_name") or group.get("name"),
        "url": record.get("meetup_url"),
        "timezone": group.get("timezone"),
    }


def process_events(
    store: EntityStore,
    event_sources: dict[str, tuple[Path, dict[str, Any]]],
    session_sources: dict[str, list[tuple[Path, dict[str, Any]]]],
    talks_by_event: dict[str, list[dict[str, Any]]],
    known_names: list[str],
    out_dir: Path,
    batch_size: int,
) -> None:
    items = sorted(event_sources.items(), key=lambda item: item[0])
    batch_dir = out_dir / "batches"
    batch_dir.mkdir(parents=True, exist_ok=True)
    for batch_index, start in enumerate(range(0, len(items), batch_size), start=1):
        batch = items[start : start + batch_size]
        batch_summary = {
            "batch_index": batch_index,
            "event_ids": [],
            "source_files": [],
            "talk_ids": [],
            "speaker_ids": [],
            "project_ids": [],
            "company_ids": [],
        }
        before_counts = {
            "talks": len(store.talks),
            "speakers": len(store.speakers),
            "projects": len(store.projects),
            "companies": len(store.companies),
        }
        for event_id, (path, record) in batch:
            event = record.get("event") or {}
            meetup_id = store.add_meetup(event_source_metadata(record))
            source = source_ref(path, record)
            append_unique(batch_summary["event_ids"], event_id)
            append_unique(batch_summary["source_files"], str(path.relative_to(ROOT)))
            candidates = event_candidates(
                record,
                [session_record for _session_path, session_record in session_sources.get(event_id, [])],
                talks_by_event.get(event_id, []),
                known_names,
            )
            for idx, candidate in enumerate(candidates, start=1):
                candidate_source = dict(source)
                if len(candidates) > 1:
                    candidate_source["source_id"] = f"{source.get('source_id') or event_id}-talk-{idx}"
                talk_id = store.add_talk(
                    title=candidate["title"],
                    abstract=candidate.get("abstract"),
                    kind=candidate.get("kind") or "talk",
                    source=candidate_source,
                    meetup_id=meetup_id,
                    conference_id=None,
                    event_id=event_id,
                    event_title=event.get("title"),
                    url=event.get("eventUrl") or record.get("source_url"),
                    date_time=event.get("dateTime"),
                    end_time=event.get("endTime"),
                    tags=["meetup", candidate.get("kind") or "talk"],
                    speaker_mentions=candidate.get("speakers") or [],
                    project_mentions=candidate.get("project_mentions") or [],
                )
                append_unique(batch_summary["talk_ids"], talk_id)
        batch_summary["new_counts"] = {
            "talks": len(store.talks) - before_counts["talks"],
            "speakers": len(store.speakers) - before_counts["speakers"],
            "projects": len(store.projects) - before_counts["projects"],
            "companies": len(store.companies) - before_counts["companies"],
        }
        batch_summary["speaker_ids"] = sorted(store.speakers.keys())
        batch_summary["project_ids"] = sorted(store.projects.keys())
        batch_summary["company_ids"] = sorted(store.companies.keys())
        write_json(batch_dir / f"batch-{batch_index:03d}.json", batch_summary)
        print(
            f"batch {batch_index:03d}: {len(batch)} events, "
            f"{batch_summary['new_counts']['talks']} new talks, "
            f"{batch_summary['new_counts']['speakers']} new speakers, "
            f"{batch_summary['new_counts']['projects']} new projects"
        )


def save_entities(store: EntityStore, out_dir: Path) -> None:
    entities = {
        "conferences": sorted(store.conferences.values(), key=lambda item: item["id"]),
        "meetups": sorted(store.meetups.values(), key=lambda item: item["id"]),
        "speakers": sorted(store.speakers.values(), key=lambda item: item["id"]),
        "companies": sorted(store.companies.values(), key=lambda item: item["id"]),
        "talks": sorted(store.talks.values(), key=lambda item: item["id"]),
        "projects": sorted(store.projects.values(), key=lambda item: item["id"]),
    }
    write_json(out_dir / "bythebay-entities.json", entities)
    for name, records in entities.items():
        write_json(out_dir / f"{name}.json", records)
        write_jsonl(out_dir / f"{name}.jsonl", records)
        entity_dir = out_dir / "entities" / name
        entity_dir.mkdir(parents=True, exist_ok=True)
        for record in records:
            write_json(entity_dir / entity_filename(record["id"]), record)
    graph = store.graph_records()
    write_json(out_dir / "bythebay-enriched-graph.json", graph)
    summary = {
        "counts": {
            "meetups": len(store.meetups),
            "conferences": len(store.conferences),
            "speakers": len(store.speakers),
            "companies": len(store.companies),
            "talks": len(store.talks),
            "projects": len(store.projects),
            "nodes": len(graph["nodes"]),
            "edges": len(graph["edges"]),
        },
        "outputs": {
            "entities": "data/enriched/bythebay-entities.json",
            "graph": "data/enriched/bythebay-enriched-graph.json",
            "per_entity_records": "data/enriched/entities",
            "batch_records": "data/enriched/batches",
        },
    }
    write_json(out_dir / "summary.json", summary)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Extract enriched By the Bay entity records from cached source downloads.")
    parser.add_argument("--graph", type=Path, default=DEFAULT_GRAPH)
    parser.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--batch-size", type=int, default=100)
    parser.add_argument("--keep-existing", action="store_true")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if args.out.exists() and not args.keep_existing:
        shutil.rmtree(args.out)
    args.out.mkdir(parents=True, exist_ok=True)

    graph = read_json(args.graph)
    speaker_sources, talk_sources, event_sources, session_sources, all_records = classify_source_records(args.sources)
    known_names = build_known_names(graph, all_records)
    store = EntityStore()
    talks_by_event, _talks_by_id = add_base_context(store, graph)

    print(f"source records: {len(all_records)} total, {len(event_sources)} unique meetup events")
    print(f"context: {len(known_names)} known speaker names")
    process_conference_speakers(store, speaker_sources)
    process_conference_talks(store, talk_sources, known_names)
    process_events(
        store,
        event_sources,
        session_sources,
        talks_by_event,
        known_names,
        args.out,
        max(args.batch_size, 1),
    )
    save_entities(store, args.out)
    summary = read_json(args.out / "summary.json")
    print("wrote enriched records:")
    for key, count in summary["counts"].items():
        print(f"  {key}: {count}")


if __name__ == "__main__":
    main()
