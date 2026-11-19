use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use nytheris_aether_core::{AetherError, Result};

use crate::embedder::{embed_amu_text, Embedder};
use crate::types::{
    Amu, AmuId, AmuKind, AmuStatus, ContentHash, ScopeTag, SourcePointer, VaultPath, VaultTier,
};

const BUILTIN_IGNORED_DIRS: &[&str] = &[".obsidian", ".git", ".trash", ".aether"];
const BUILTIN_IGNORED_PREFIXES: &[&str] = &[
    "aether/inbox.md",
    "aether/compiled",
    ".obsidian/plugins/obsidian-aether/data.json",
];
const BUILTIN_IGNORED_EXTENSIONS: &[&str] = &[".canvas", ".excalidraw", ".excalidraw.md"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawFrontmatterTier {
    Public(VaultTier),
    Private,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParsedNote {
    Private,
    Public(Vec<ParsedSection>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedSection {
    pub(crate) amu: Amu,
    pub(crate) verbatim_section: String,
    pub(crate) embedding: Vec<f32>,
}

/// Shared newline-separated path/glob rule evaluator (used by `.aetherignore` and `.aetherhot`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PathRules {
    patterns: Vec<String>,
}

impl PathRules {
    pub(crate) fn from_str(contents: &str) -> Self {
        let patterns = contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(normalize_rel_path)
            .filter(|pat| !pat.is_empty())
            .collect();
        Self { patterns }
    }

    pub(crate) fn load_file(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => Self::from_str(&contents),
            Err(_) => Self::default(),
        }
    }

    pub(crate) fn matches_normalized(&self, rel: &str) -> bool {
        !rel.is_empty()
            && self
                .patterns
                .iter()
                .any(|pat| matches_path_pattern(rel, pat))
    }
}

/// Canonical `.aetherignore` plus built-in exclusion evaluator.
#[derive(Debug, Clone, Default)]
pub(crate) struct IgnoreRules {
    rules: PathRules,
}

impl IgnoreRules {
    #[cfg(test)]
    pub(crate) fn from_str(contents: &str) -> Self {
        Self {
            rules: PathRules::from_str(contents),
        }
    }

    pub(crate) fn load(vault_root: &Path) -> Self {
        Self {
            rules: PathRules::load_file(&vault_root.join(".aetherignore")),
        }
    }

    pub(crate) fn is_ignored(&self, relative_path: &str) -> bool {
        let rel = normalize_rel_path(relative_path);
        if rel.is_empty() {
            return true;
        }
        let lower = rel.to_ascii_lowercase();

        if BUILTIN_IGNORED_PREFIXES
            .iter()
            .any(|prefix| path_matches_prefix(&lower, prefix))
        {
            return true;
        }

        self.matches_user_patterns_or_dirs_or_exts(&rel, &lower)
    }

    pub(crate) fn is_ignored_for_inspect(&self, relative_path: &str) -> bool {
        let rel = normalize_rel_path(relative_path);
        if rel.is_empty() {
            return true;
        }
        if rel.eq_ignore_ascii_case(crate::inbox::APPROVED_MIRROR_REL_PATH) {
            let lower = rel.to_ascii_lowercase();
            return self.matches_user_patterns_or_dirs_or_exts(&rel, &lower);
        }
        self.is_ignored(&rel)
    }

    fn matches_user_patterns_or_dirs_or_exts(&self, rel: &str, lower: &str) -> bool {
        if BUILTIN_IGNORED_EXTENSIONS
            .iter()
            .any(|ext| lower.ends_with(ext))
        {
            return true;
        }

        if rel.split('/').any(|seg| {
            let seg_lower = seg.to_ascii_lowercase();
            BUILTIN_IGNORED_DIRS.contains(&seg_lower.as_str())
        }) {
            return true;
        }

        self.rules.matches_normalized(rel)
    }
}

fn normalize_rel_path(raw: &str) -> String {
    let replaced = raw.trim().replace('\\', "/");
    let mut s = replaced.as_str();
    while let Some(rest) = s.strip_prefix("./") {
        s = rest;
    }
    s.trim_start_matches('/').to_string()
}

fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn matches_path_pattern(rel: &str, pattern: &str) -> bool {
    if let Some(dir) = pattern.strip_suffix('/') {
        return path_matches_prefix(rel, dir) || rel.split('/').any(|seg| seg == dir);
    }
    if pattern.contains('*') || pattern.contains('?') {
        if pattern.contains('/') {
            return glob_match(pattern, rel);
        }
        let base = rel.rsplit('/').next().unwrap_or(rel);
        return glob_match(pattern, base) || glob_match(pattern, rel);
    }
    if pattern.contains('/') {
        return path_matches_prefix(rel, pattern);
    }
    rel == pattern || rel.split('/').any(|seg| seg == pattern)
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star_p, mut star_t) = (None, 0);

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_p = Some(pi);
            star_t = ti;
            pi += 1;
        } else if let Some(sp) = star_p {
            pi = sp + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Converts an ATX heading into an Obsidian-compatible heading anchor (`Token Rotation` -> `Token-Rotation`).
pub(crate) fn slugify_heading(heading: &str) -> String {
    let stripped = heading.trim().trim_start_matches('#').trim();
    let mut out = String::with_capacity(stripped.len());
    let mut prev_dash = false;

    for ch in stripped.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            out.push(ch);
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }

    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "section".to_string()
    } else {
        trimmed
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NoteFrontmatterHint {
    pub(crate) is_private: bool,
    pub(crate) is_fleeting: bool,
    pub(crate) scope: ScopeTag,
}

/// Inspects frontmatter without indexing and returns visibility, fleeting flag, primary scope, and body.
pub(crate) fn inspect_note_frontmatter<'a>(
    rel_path: &str,
    raw_markdown: &'a str,
) -> (NoteFrontmatterHint, &'a str) {
    let (fm_text, body) = split_frontmatter(raw_markdown);
    let fm = match fm_text {
        Some(raw) => parse_frontmatter(raw, rel_path, Utc::now()),
        None => Frontmatter::default_for_path(rel_path, Utc::now()),
    };
    let scope = fm
        .scopes
        .into_iter()
        .next()
        .unwrap_or_else(ScopeTag::global);
    (
        NoteFrontmatterHint {
            is_private: fm.tier == RawFrontmatterTier::Private,
            is_fleeting: fm.kind == AmuKind::Fleeting,
            scope,
        },
        body,
    )
}

/// Inspects frontmatter without indexing and returns all `aether_scope` tags (or `[global]`).
pub(crate) fn inspect_note_scopes(rel_path: &str, raw_markdown: &str) -> Vec<ScopeTag> {
    let (fm_text, _) = split_frontmatter(raw_markdown);
    let fm = match fm_text {
        Some(raw) => parse_frontmatter(raw, rel_path, Utc::now()),
        None => Frontmatter::default_for_path(rel_path, Utc::now()),
    };
    fm.scopes
}

/// Inspects frontmatter alone and returns `true` when `aether_tier: private` is declared.
pub(crate) fn is_private_note(raw_markdown: &str) -> bool {
    let (fm_text, _) = split_frontmatter(raw_markdown);
    let Some(fm) = fm_text else {
        return false;
    };
    parse_frontmatter(fm, "note.md", Utc::now()).tier == RawFrontmatterTier::Private
}

/// Parses YAML frontmatter and splits Markdown outside fenced code blocks into AMUs.
pub(crate) fn parse_note(
    relative_path: &str,
    raw_markdown: &str,
    fallback_valid_from: DateTime<Utc>,
    embedder: &dyn Embedder,
) -> Result<ParsedNote> {
    let vault_path = VaultPath::parse(relative_path)?;
    let (fm_text, body) = split_frontmatter(raw_markdown);
    let fm = match fm_text {
        Some(raw) => parse_frontmatter(raw, vault_path.as_str(), fallback_valid_from),
        None => Frontmatter::default_for_path(vault_path.as_str(), fallback_valid_from),
    };

    let tier = match fm.tier {
        RawFrontmatterTier::Private => return Ok(ParsedNote::Private),
        RawFrontmatterTier::Public(t) => t,
    };

    let raw_sections = split_body_sections(body);
    if raw_sections.is_empty() {
        return Ok(ParsedNote::Public(Vec::new()));
    }

    let note_tags: BTreeSet<String> = fm.tags.into_iter().collect();
    let mut slug_counts: HashMap<String, usize> = HashMap::new();
    let mut heading_stack: Vec<(u8, String)> = Vec::new();
    let mut out = Vec::with_capacity(raw_sections.len());

    for raw_sec in raw_sections {
        let (pointer, heading_path) = match (&raw_sec.heading_title, raw_sec.level) {
            (Some(title), level) => {
                while heading_stack
                    .last()
                    .is_some_and(|(top_lvl, _)| *top_lvl >= level)
                {
                    heading_stack.pop();
                }
                heading_stack.push((level, title.clone()));
                let base_slug = slugify_heading(title);
                let count = slug_counts.entry(base_slug.clone()).or_insert(0);
                let unique_slug = if *count == 0 {
                    base_slug
                } else {
                    format!("{base_slug}-{}", *count)
                };
                *count += 1;
                let path_str = heading_stack
                    .iter()
                    .map(|(_, t)| t.as_str())
                    .collect::<Vec<_>>()
                    .join(" > ");
                let ptr = SourcePointer::new(vault_path.clone(), Some(unique_slug))?;
                (ptr, path_str)
            }
            (None, _) => {
                let anchor = raw_sec.explicit_anchor.clone();
                let ptr = SourcePointer::new(vault_path.clone(), anchor)?;
                let heading_label = vault_path.as_str().to_string();
                (ptr, heading_label)
            }
        };

        let verbatim = raw_sec.verbatim.trim().to_string();
        if verbatim.is_empty()
            || (raw_sec.heading_title.is_some() && section_body_lines(&verbatim).is_empty())
        {
            continue;
        }

        let amu_id = AmuId::from_pointer(&pointer);
        let graph_edges = extract_graph_edges(&verbatim, &note_tags);
        let authority_weight = compute_authority_weight(fm.kind, &verbatim);
        let distilled_fact = distill_section(&heading_path, &verbatim);
        let embed_input = embed_amu_text(&heading_path, &distilled_fact);
        let embedding = embedder.embed(&embed_input);
        let content_hash = ContentHash::compute(verbatim.as_bytes());

        out.push(ParsedSection {
            amu: Amu {
                amu_id,
                tier,
                kind: fm.kind,
                status: fm.status,
                heading_path,
                distilled_fact,
                source_pointer: pointer,
                graph_edges,
                valid_from: fm.valid_from,
                superseded_by: fm.superseded_by,
                scopes: fm.scopes.clone(),
                authority_weight,
                content_hash,
            },
            verbatim_section: verbatim,
            embedding,
        });
    }

    Ok(ParsedNote::Public(out))
}

/// Reads the raw Markdown note and returns the exact verbatim section matching `pointer`.
/// Rejects `aether_tier: private` notes at the boundary.
pub(crate) fn extract_verbatim_section(
    raw_markdown: &str,
    pointer: &SourcePointer,
) -> Result<String> {
    if is_private_note(raw_markdown) {
        return Err(AetherError::InvalidInput(
            "cannot inspect source of a private note".into(),
        ));
    }

    let (_, body) = split_frontmatter(raw_markdown);
    let Some(target_anchor) = pointer.heading_anchor() else {
        return Ok(body.trim().to_string());
    };

    let sections = split_body_sections(body);
    let mut slug_counts: HashMap<String, usize> = HashMap::new();

    for sec in sections {
        let candidate_anchor = match &sec.heading_title {
            Some(title) => {
                let base = slugify_heading(title);
                let count = slug_counts.entry(base.clone()).or_insert(0);
                let slug = if *count == 0 {
                    base
                } else {
                    format!("{base}-{}", *count)
                };
                *count += 1;
                Some(slug)
            }
            None => sec.explicit_anchor.clone(),
        };

        if let Some(anchor) = candidate_anchor {
            if anchor == target_anchor || anchor.eq_ignore_ascii_case(target_anchor) {
                return Ok(sec.verbatim.trim_end().to_string());
            }
        }
    }

    Err(AetherError::NotFound(format!(
        "heading `{target_anchor}` not found in `{}`",
        pointer.relative_path()
    )))
}

#[derive(Debug, Clone)]
struct Frontmatter {
    tier: RawFrontmatterTier,
    scopes: Vec<ScopeTag>,
    kind: AmuKind,
    status: AmuStatus,
    valid_from: DateTime<Utc>,
    superseded_by: Option<AmuId>,
    tags: Vec<String>,
}

impl Frontmatter {
    fn default_for_path(rel_path: &str, fallback_valid_from: DateTime<Utc>) -> Self {
        let lower = rel_path.to_ascii_lowercase();
        let default_kind = if lower.starts_with("daily/")
            || lower.starts_with("journal/")
            || lower.contains("/daily/")
            || lower.contains("/journal/")
        {
            AmuKind::Fleeting
        } else {
            AmuKind::Spec
        };
        Self {
            tier: RawFrontmatterTier::Public(VaultTier::Warm),
            scopes: vec![ScopeTag::global()],
            kind: default_kind,
            status: AmuStatus::Active,
            valid_from: fallback_valid_from,
            superseded_by: None,
            tags: Vec::new(),
        }
    }
}

fn split_frontmatter(raw: &str) -> (Option<&str>, &str) {
    let Some(after_open) = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))
    else {
        return (None, raw);
    };

    let open_len = raw.len() - after_open.len();
    let mut offset = 0usize;
    for chunk in after_open.split_inclusive('\n') {
        let line = chunk.trim_end_matches(['\r', '\n']);
        if line.trim() == "---" {
            let fm = &after_open[..offset];
            let rest = &raw[open_len + offset + chunk.len()..];
            return (Some(fm), rest);
        }
        offset += chunk.len();
    }

    (None, raw)
}

fn parse_frontmatter(
    fm_text: &str,
    rel_path: &str,
    fallback_valid_from: DateTime<Utc>,
) -> Frontmatter {
    let mut fm = Frontmatter::default_for_path(rel_path, fallback_valid_from);
    let mut current_list_key: Option<String> = None;
    let mut parsed_scopes: Vec<ScopeTag> = Vec::new();
    let mut parsed_tags: Vec<String> = Vec::new();

    for raw_line in fm_text.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            if let Some(key) = &current_list_key {
                let clean = unquote_yaml(item);
                push_list_value(key, &clean, &mut parsed_scopes, &mut parsed_tags);
            }
            continue;
        }

        current_list_key = None;
        let Some((k, v)) = trimmed.split_once(':') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();

        if val.is_empty() {
            current_list_key = Some(key);
            continue;
        }

        match key.as_str() {
            "aether_tier" => {
                let norm = unquote_yaml(val).to_ascii_lowercase();
                if norm == "private" {
                    fm.tier = RawFrontmatterTier::Private;
                } else if let Some(t) = VaultTier::parse_str(&norm) {
                    fm.tier = RawFrontmatterTier::Public(t);
                }
            }
            "aether_scope" => {
                for item in parse_inline_or_scalar(val) {
                    if let Ok(scope) = ScopeTag::new(item) {
                        parsed_scopes.push(scope);
                    }
                }
            }
            "aether_kind" => {
                if let Some(kind) = AmuKind::parse_str(&unquote_yaml(val)) {
                    fm.kind = kind;
                }
            }
            "aether_status" => {
                if let Some(status) = AmuStatus::parse_str(&unquote_yaml(val)) {
                    fm.status = status;
                }
            }
            "valid_from" => {
                if let Some(dt) = parse_valid_from(&unquote_yaml(val)) {
                    fm.valid_from = dt;
                }
            }
            "superseded_by" => {
                if let Ok(id) = AmuId::parse(&unquote_yaml(val)) {
                    fm.superseded_by = Some(id);
                }
            }
            "tags" => {
                for item in parse_inline_or_scalar(val) {
                    let tag = item.trim_start_matches('#').to_ascii_lowercase();
                    if !tag.is_empty() {
                        parsed_tags.push(tag);
                    }
                }
            }
            _ => {}
        }
    }

    if !parsed_scopes.is_empty() {
        fm.scopes = parsed_scopes;
    }
    fm.tags = parsed_tags;
    fm
}

fn push_list_value(key: &str, val: &str, scopes: &mut Vec<ScopeTag>, tags: &mut Vec<String>) {
    match key {
        "aether_scope" => {
            if let Ok(scope) = ScopeTag::new(val) {
                scopes.push(scope);
            }
        }
        "tags" => {
            let tag = val.trim_start_matches('#').to_ascii_lowercase();
            if !tag.is_empty() {
                tags.push(tag);
            }
        }
        _ => {}
    }
}

fn parse_inline_or_scalar(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if let Some(inner) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        inner
            .split(',')
            .map(unquote_yaml)
            .filter(|s| !s.is_empty())
            .collect()
    } else if trimmed.contains(',') {
        trimmed
            .split(',')
            .map(unquote_yaml)
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        let single = unquote_yaml(trimmed);
        if single.is_empty() {
            Vec::new()
        } else {
            vec![single]
        }
    }
}

fn unquote_yaml(raw: &str) -> String {
    let t = raw.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
    {
        t[1..t.len() - 1].trim().to_string()
    } else {
        t.to_string()
    }
}

fn parse_valid_from(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let naive_dt = date.and_hms_opt(0, 0, 0)?;
        return Some(Utc.from_utc_datetime(&naive_dt));
    }
    None
}

#[derive(Debug, Clone)]
struct RawSection {
    heading_title: Option<String>,
    explicit_anchor: Option<String>,
    level: u8,
    verbatim: String,
}

fn split_body_sections(body: &str) -> Vec<RawSection> {
    let mut headings: Vec<(usize, u8, String)> = Vec::new();
    let mut fence: Option<(u8, usize)> = None;
    let mut offset = 0usize;

    for chunk in body.split_inclusive('\n') {
        let line = chunk.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim_start();
        if let Some((fchar, flen)) = fence {
            if is_closing_fence(trimmed, fchar, flen) {
                fence = None;
            }
        } else if let Some(opening) = detect_opening_fence(trimmed) {
            fence = Some(opening);
        } else if let Some((level, title)) = parse_atx_heading(trimmed) {
            headings.push((offset, level, title.to_string()));
        }
        offset += chunk.len();
    }

    if headings.is_empty() {
        if body.trim().is_empty() {
            return Vec::new();
        }
        return vec![RawSection {
            heading_title: None,
            explicit_anchor: None,
            level: 0,
            verbatim: body.to_string(),
        }];
    }

    let mut sections = Vec::new();
    let preamble = &body[..headings[0].0];
    if !preamble.trim().is_empty() {
        sections.push(RawSection {
            heading_title: None,
            explicit_anchor: Some("_preamble".to_string()),
            level: 0,
            verbatim: preamble.to_string(),
        });
    }

    for (idx, (start, level, title)) in headings.iter().enumerate() {
        let end = headings
            .get(idx + 1)
            .map_or(body.len(), |(next_start, _, _)| *next_start);
        sections.push(RawSection {
            heading_title: Some(title.clone()),
            explicit_anchor: None,
            level: *level,
            verbatim: body[*start..end].to_string(),
        });
    }

    sections
}

pub(crate) fn detect_opening_fence(trimmed: &str) -> Option<(u8, usize)> {
    let first = *trimmed.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let count = trimmed.bytes().take_while(|&b| b == first).count();
    if count >= 3 {
        Some((first, count))
    } else {
        None
    }
}

pub(crate) fn is_closing_fence(trimmed: &str, fchar: u8, min_len: usize) -> bool {
    let count = trimmed.bytes().take_while(|&b| b == fchar).count();
    count >= min_len && trimmed[count..].trim().is_empty()
}

pub(crate) fn parse_atx_heading(trimmed: &str) -> Option<(u8, &str)> {
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim();
    if title.is_empty() {
        None
    } else {
        Some((hashes as u8, title))
    }
}

fn extract_graph_edges(section_md: &str, note_tags: &BTreeSet<String>) -> Vec<String> {
    let mut edges: BTreeSet<String> = note_tags.iter().map(|t| format!("tag:{t}")).collect();
    let stripped = strip_fenced_and_inline_code(section_md);

    let mut rest = stripped.as_str();
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            break;
        };
        let inner = &after[..end];
        let target_part = inner.split('|').next().unwrap_or("");
        let note_part = target_part.split('#').next().unwrap_or("").trim();
        let normalized = normalize_rel_path(note_part).to_ascii_lowercase();
        let trimmed_md = normalized.strip_suffix(".md").unwrap_or(&normalized);
        if !trimmed_md.is_empty() {
            edges.insert(format!("note:{trimmed_md}"));
        }
        rest = &after[end + 2..];
    }

    let bytes = stripped.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let prev_ok = i == 0
                || bytes[i - 1].is_ascii_whitespace()
                || matches!(bytes[i - 1], b'(' | b',' | b'[');
            if prev_ok {
                let mut end = i + 1;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric()
                        || matches!(bytes[end], b'_' | b'-' | b'/'))
                {
                    end += 1;
                }
                if end > i + 1 {
                    let tag = &stripped[i + 1..end];
                    if tag.as_bytes()[0].is_ascii_alphabetic() {
                        edges.insert(format!("tag:{}", tag.to_ascii_lowercase()));
                    }
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }

    edges.into_iter().collect()
}

fn strip_fenced_and_inline_code(section_md: &str) -> String {
    let mut non_fenced = String::with_capacity(section_md.len());
    let mut fence: Option<(u8, usize)> = None;

    for line in section_md.lines() {
        let trimmed = line.trim_start();
        if let Some((fchar, flen)) = fence {
            if is_closing_fence(trimmed, fchar, flen) {
                fence = None;
            }
            non_fenced.push('\n');
            continue;
        }
        if let Some(opening) = detect_opening_fence(trimmed) {
            fence = Some(opening);
            non_fenced.push('\n');
            continue;
        }
        non_fenced.push_str(line);
        non_fenced.push('\n');
    }

    let mut out = String::with_capacity(non_fenced.len());
    let mut in_inline = false;
    for ch in non_fenced.chars() {
        if ch == '`' {
            in_inline = !in_inline;
            out.push(' ');
        } else if in_inline {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

fn compute_authority_weight(kind: AmuKind, verbatim: &str) -> f32 {
    let base = match kind {
        AmuKind::Profile | AmuKind::Rule | AmuKind::Decision | AmuKind::Spec => 1.0,
        AmuKind::Fleeting => 0.6,
    };

    let mut unchecked = 0usize;
    let mut checked = 0usize;
    let mut prose = 0usize;
    let mut fence: Option<(u8, usize)> = None;

    for (idx, line) in verbatim.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if idx == 0 && parse_atx_heading(trimmed).is_some() {
            continue;
        }
        if let Some((fchar, flen)) = fence {
            if is_closing_fence(trimmed, fchar, flen) {
                fence = None;
            }
            prose += 1;
            continue;
        }
        if let Some(opening) = detect_opening_fence(trimmed) {
            fence = Some(opening);
            prose += 1;
            continue;
        }

        if trimmed.starts_with("- [ ]") || trimmed.starts_with("* [ ]") {
            unchecked += 1;
        } else if trimmed.starts_with("- [x]")
            || trimmed.starts_with("- [X]")
            || trimmed.starts_with("* [x]")
            || trimmed.starts_with("* [X]")
        {
            checked += 1;
        } else {
            prose += 1;
        }
    }

    if unchecked > 0 && unchecked > checked + prose {
        0.3
    } else {
        base
    }
}

fn section_body_lines(verbatim: &str) -> Vec<&str> {
    verbatim
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() || (idx == 0 && parse_atx_heading(trimmed).is_some()) {
                return None;
            }
            Some(trimmed)
        })
        .collect()
}

fn distill_section(heading_path: &str, verbatim: &str) -> String {
    let body_parts = section_body_lines(verbatim);
    let joined = if body_parts.is_empty() {
        heading_path.to_string()
    } else {
        format!("{heading_path}: {}", body_parts.join(" "))
    };
    truncate_chars(&joined, 400)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let prefix: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{prefix}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::{HashedNgramEmbedder, DEFAULT_EMBED_DIM};

    #[test]
    fn ignore_rules_filter_builtin_and_aetherignore_entries() {
        let rules = IgnoreRules::from_str("# private notes\nFinance/\n*.secret.md\nDrafts/*.tmp\n");

        assert!(rules.is_ignored(".obsidian/plugins/foo/main.js"));
        assert!(rules.is_ignored(".obsidian/plugins/obsidian-aether/data.json"));
        assert!(rules.is_ignored(".git/HEAD"));
        assert!(rules.is_ignored(".trash/Old.md"));
        assert!(rules.is_ignored("Aether/Inbox.md"));
        assert!(rules.is_ignored("Aether/Compiled/Hot.md"));
        assert!(rules.is_ignored("Diagrams/arch.canvas"));
        assert!(rules.is_ignored("Diagrams/flow.excalidraw.md"));
        assert!(rules.is_ignored("Finance/2026.md"));
        assert!(rules.is_ignored("Personal/keys.secret.md"));
        assert!(!rules.is_ignored("Architecture/Storage.md"));
    }

    #[test]
    fn private_notes_are_excluded_at_parse_boundary() {
        let md = "---\naether_tier: private\naether_scope:\n  - personal\n---\n# Secret\nNever index this.\n";
        let embedder = HashedNgramEmbedder::default();
        let parsed = parse_note("Personal/Diary.md", md, Utc::now(), &embedder).unwrap();
        assert_eq!(parsed, ParsedNote::Private);
        assert!(is_private_note(md));

        let ptr = SourcePointer::parse("Personal/Diary.md#Secret").unwrap();
        assert!(matches!(
            extract_verbatim_section(md, &ptr),
            Err(AetherError::InvalidInput(_))
        ));
    }

    #[test]
    fn parses_frontmatter_headings_wikilinks_tags_and_authority_weight() {
        let evergreen = "\
---
aether_tier: hot
aether_scope:
  - engineering
  - global
aether_kind: decision
aether_status: active
valid_from: 2026-04-01
---
Intro context before headings.

# Token Rotation
Rotate signing keys with `--grace-secs 300` and see [[Auth/Keys|Keys]] #security.

```rust
// # Not-A-Heading and [[NotALink]] #not-a-tag
let x = 1;
```

## Sub Check
Use port `8443`.
";
        let embedder = HashedNgramEmbedder::default();
        let fallback = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let ParsedNote::Public(sections) =
            parse_note("Specs/Auth.md", evergreen, fallback, &embedder).unwrap()
        else {
            panic!("expected public parsed note");
        };

        assert_eq!(sections.len(), 3);
        assert_eq!(
            sections[0].amu.source_pointer.as_str(),
            "Specs/Auth.md#_preamble"
        );
        assert_eq!(
            sections[1].amu.source_pointer.as_str(),
            "Specs/Auth.md#Token-Rotation"
        );
        assert_eq!(
            sections[2].amu.source_pointer.as_str(),
            "Specs/Auth.md#Sub-Check"
        );

        let token_sec = &sections[1];
        assert_eq!(token_sec.amu.tier, VaultTier::Hot);
        assert_eq!(token_sec.amu.kind, AmuKind::Decision);
        assert_eq!(token_sec.amu.status, AmuStatus::Active);
        assert!(token_sec
            .amu
            .graph_edges
            .iter()
            .any(|e| e == "note:auth/keys"));
        assert!(token_sec
            .amu
            .graph_edges
            .iter()
            .any(|e| e == "tag:security"));
        assert!(!token_sec
            .amu
            .graph_edges
            .iter()
            .any(|e| e.contains("not-a-tag") || e.contains("notalink")));

        let daily_tasks = "\
# Scratch
- [ ] maybe try caching
- [ ] another brainstorm item
- [ ] third unchecked item
";
        let ParsedNote::Public(daily_sections) =
            parse_note("Daily/2026-04-02.md", daily_tasks, fallback, &embedder).unwrap()
        else {
            panic!("expected daily note");
        };
        assert_eq!(daily_sections[0].amu.kind, AmuKind::Fleeting);
        assert!(sections[1].amu.authority_weight > daily_sections[0].amu.authority_weight);
        assert!((daily_sections[0].amu.authority_weight - 0.3).abs() < 1e-5);
    }

    #[test]
    fn extract_verbatim_section_preserves_code_blocks_and_flags() {
        let md = "\
---
aether_tier: warm
---
# Setup
Run `aether-mcp --vault /tmp/v --port 9001`.

```bash
# comment inside bash is not a heading
./bin --flag=42
```

# Next Section
Other text.
";
        let ptr = SourcePointer::parse("Guides/Run.md#Setup").unwrap();
        let section = extract_verbatim_section(md, &ptr).unwrap();
        assert!(section.contains("--vault /tmp/v --port 9001"));
        assert!(section.contains("# comment inside bash is not a heading"));
        assert!(!section.contains("Next Section"));
    }

    #[test]
    fn local_embedder_produces_unit_vectors_with_lexical_overlap() {
        let embedder = HashedNgramEmbedder::default();
        let a = embedder.embed("SQLite WAL transaction locking");
        let b = embedder.embed("SQLite WAL write locks");
        let c = embedder.embed("banana smoothie recipe");

        assert_eq!(a.len(), DEFAULT_EMBED_DIM);
        let sim_ab: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        let sim_ac: f32 = a.iter().zip(&c).map(|(x, y)| x * y).sum();
        assert!(sim_ab > sim_ac, "expected {sim_ab} > {sim_ac}");
    }

    #[test]
    fn skips_empty_heading_only_sections_while_preserving_hierarchy_and_slugs() {
        let md = "\
# Brand: Chogerlatte

## Usage
Chogerlatte appears on-screen only, not spoken.

# Brand: Chogerlatte
Show logo in upper-right corner.

# Trailing Empty Heading
";
        let embedder = HashedNgramEmbedder::default();
        let ParsedNote::Public(sections) =
            parse_note("Brand/Guide.md", md, Utc::now(), &embedder).unwrap()
        else {
            panic!("expected public parsed note");
        };

        assert_eq!(
            sections.len(),
            2,
            "expected only sections with body text, got: {:?}",
            sections
                .iter()
                .map(|s| (&s.amu.source_pointer, &s.amu.distilled_fact))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            sections[0].amu.source_pointer.as_str(),
            "Brand/Guide.md#Usage"
        );
        assert_eq!(sections[0].amu.heading_path, "Brand: Chogerlatte > Usage");
        assert_eq!(
            sections[1].amu.source_pointer.as_str(),
            "Brand/Guide.md#Brand-Chogerlatte-1"
        );
        assert_eq!(
            extract_verbatim_section(md, &sections[1].amu.source_pointer).unwrap(),
            "# Brand: Chogerlatte\nShow logo in upper-right corner."
        );

        let only_headings = "# Empty One\n## Empty Two\n";
        let ParsedNote::Public(empty_sections) =
            parse_note("Brand/Empty.md", only_headings, Utc::now(), &embedder).unwrap()
        else {
            panic!("expected public parsed note");
        };
        assert!(empty_sections.is_empty());
    }
}
