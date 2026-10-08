//! Opt-in content cuts for recall and find.
//!
//! The default command output stays a summary line. `--excerpt` and `--match`
//! ask this module to cut each hit down to the matching markdown section, or
//! to a paragraph when the note has no headings.

use anyhow::{Context, Result};
use locus_core_rs::parsing::state_machine::SttpLayerStateMachine;
use locus_sdk::application::memory_lexical::parse_lexical_query;
use regex::RegexBuilder;

use crate::memory::Recalled;

const MAX_SECTIONS: usize = 3;

/// A compiled `--match` pattern. Matching is case-insensitive.
pub struct MatchPattern {
    regex: regex::Regex,
}

impl MatchPattern {
    pub fn compile(pattern: Option<&str>) -> Result<Option<Self>> {
        let Some(pattern) = pattern.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .with_context(|| format!("invalid --match pattern: {pattern}"))?;
        Ok(Some(Self { regex }))
    }
}

/// How to cut a recall or find result. Summary lines are the caller's job
/// when neither `excerpt` nor `full` is set; this still filters and reorders
/// when `pattern` is set.
pub struct ViewOptions<'a> {
    pub query: Option<&'a str>,
    pub pattern: Option<&'a MatchPattern>,
    /// Literal substring used when find has `--contains` and no `--match`.
    pub literal: Option<&'a str>,
    pub excerpt: bool,
    pub full: bool,
    pub context_lines: Option<usize>,
    pub limit: usize,
}

pub struct ViewedHit {
    pub summary: String,
    pub raw_sttp: String,
    pub path: String,
    /// Set for `--excerpt` and `--full`. Empty when the caller prints `summary`.
    pub lines: Vec<String>,
}

struct Section {
    headings: Vec<String>,
    lines: Vec<String>,
}

struct ScoredHit {
    index: usize,
    score: u32,
    viewed: ViewedHit,
}

/// Filter, re-score, and cut `hits`.
///
/// `--match` drops hits whose stored text does not match. `--excerpt` prints
/// the best matching sections. `--full` prints the stored text. Hits are
/// ordered by their best section score so a long note that only mentions the
/// query in passing falls behind a section that is about it.
pub fn view_hits(hits: &[Recalled], options: &ViewOptions<'_>) -> Result<Vec<ViewedHit>> {
    let terms = if options.pattern.is_some() {
        Vec::new()
    } else {
        terms_for(options.query, options.literal)
    };
    let mut scored = Vec::new();
    for (index, hit) in hits.iter().enumerate() {
        let text = extract_document_text(&hit.raw_sttp).unwrap_or_default();
        let searchable = if text.is_empty() {
            hit.raw_sttp.as_str()
        } else {
            text.as_str()
        };
        if let Some(pattern) = options.pattern {
            if !pattern.regex.is_match(searchable) {
                continue;
            }
        }

        let sections = split_sections(if text.is_empty() { searchable } else { &text });
        let section_scores = sections
            .iter()
            .map(|section| score_section(section, &terms, options.pattern))
            .collect::<Vec<_>>();
        let score = section_scores.iter().copied().max().unwrap_or(0);
        let lines = if options.full {
            full_lines(hit, &text)
        } else if options.excerpt {
            excerpt_lines(hit, &sections, &section_scores, &terms, options)
        } else {
            Vec::new()
        };
        scored.push(ScoredHit {
            index,
            score,
            viewed: ViewedHit {
                summary: hit.summary.clone(),
                raw_sttp: hit.raw_sttp.clone(),
                path: hit.path.clone(),
                lines,
            },
        });
    }

    scored.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.index.cmp(&right.index))
    });
    let limit = options.limit.max(1);
    Ok(scored
        .into_iter()
        .take(limit)
        .map(|hit| hit.viewed)
        .collect())
}

fn terms_for(query: Option<&str>, literal: Option<&str>) -> Vec<Vec<String>> {
    let mut terms = query
        .map(parse_lexical_query)
        .map(|parsed| parsed.coverage)
        .unwrap_or_default();
    if let Some(literal) = literal.map(str::trim).filter(|value| !value.is_empty()) {
        let token = literal.to_ascii_lowercase();
        if !terms
            .iter()
            .any(|group| group.iter().any(|term| term == &token))
        {
            terms.push(vec![token]);
        }
    }
    terms
}

fn full_lines(hit: &Recalled, text: &str) -> Vec<String> {
    let mut lines = vec![display_source(hit)];
    if text.trim().is_empty() {
        lines.push(hit.summary.clone());
        return lines;
    }
    for line in text.lines() {
        lines.push(indent(line));
    }
    lines
}

fn excerpt_lines(
    hit: &Recalled,
    sections: &[Section],
    scores: &[u32],
    terms: &[Vec<String>],
    options: &ViewOptions<'_>,
) -> Vec<String> {
    let chosen = choose_sections(sections, scores);
    if chosen.is_empty() {
        return vec![hit.summary.clone()];
    }
    let mut lines = Vec::new();
    for (offset, index) in chosen.into_iter().enumerate() {
        if offset > 0 {
            lines.push(String::new());
        }
        let section = &sections[index];
        lines.push(section_label(&display_source(hit), &section.headings));
        lines.extend(render_body(section, terms, options));
    }
    lines
}

fn choose_sections(sections: &[Section], scores: &[u32]) -> Vec<usize> {
    let mut ranked = scores
        .iter()
        .enumerate()
        .filter(|(_, score)| **score > 0)
        .map(|(index, score)| (index, *score))
        .collect::<Vec<_>>();
    if ranked.is_empty() {
        return sections
            .iter()
            .position(|section| section.lines.iter().any(|line| !line.trim().is_empty()))
            .into_iter()
            .collect();
    }
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ranked.truncate(MAX_SECTIONS);
    ranked.sort_by_key(|(index, _)| *index);
    ranked.into_iter().map(|(index, _)| index).collect()
}

fn render_body(section: &Section, terms: &[Vec<String>], options: &ViewOptions<'_>) -> Vec<String> {
    let lines = trimmed_edges(&section.lines);
    if lines.is_empty() {
        return Vec::new();
    }
    let Some(window) = options.context_lines else {
        return lines.iter().map(|line| indent(line)).collect();
    };
    let hits = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line_matches(line, terms, options.pattern))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if hits.is_empty() {
        return lines.iter().map(|line| indent(line)).collect();
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for index in hits {
        let start = index.saturating_sub(window);
        let end = (index + window).min(lines.len() - 1);
        if let Some((_, range_end)) = ranges.last_mut() {
            if start <= *range_end + 1 {
                *range_end = (*range_end).max(end);
                continue;
            }
        }
        ranges.push((start, end));
    }
    let mut rendered = Vec::new();
    for (offset, (start, end)) in ranges.into_iter().enumerate() {
        if offset > 0 {
            rendered.push("  --".to_string());
        }
        for line in &lines[start..=end] {
            rendered.push(indent(line));
        }
    }
    rendered
}

fn line_matches(line: &str, terms: &[Vec<String>], pattern: Option<&MatchPattern>) -> bool {
    if let Some(pattern) = pattern {
        return pattern.regex.is_match(line);
    }
    terms
        .iter()
        .any(|variants| variants.iter().any(|term| contains_term(line, term)))
}

fn score_section(section: &Section, terms: &[Vec<String>], pattern: Option<&MatchPattern>) -> u32 {
    let heading = section.headings.join(" ");
    let body = section.lines.join("\n");
    if let Some(pattern) = pattern {
        let heading_hits = pattern.regex.find_iter(&heading).count() as u32;
        let body_hits = pattern.regex.find_iter(&body).count() as u32;
        return heading_hits.saturating_mul(5).saturating_add(body_hits);
    }
    if terms.is_empty() {
        return 0;
    }
    let mut score = 0u32;
    for variants in terms {
        let mut heading_hit = false;
        let mut body_hit = false;
        for term in variants {
            if !heading_hit && contains_term(&heading, term) {
                heading_hit = true;
            }
            if !body_hit && contains_term(&body, term) {
                body_hit = true;
            }
        }
        if heading_hit {
            score += 5;
        }
        if body_hit {
            score += 2;
        }
        if let Some(primary) = variants.first() {
            let primary = primary.to_ascii_lowercase();
            score += body.to_ascii_lowercase().matches(&primary).count().min(4) as u32;
        }
    }
    score
}

fn section_label(source: &str, headings: &[String]) -> String {
    if headings.is_empty() {
        source.to_string()
    } else {
        format!("{source} § {}", headings.join(" > "))
    }
}

fn display_source(hit: &Recalled) -> String {
    if !hit.source.is_empty() && hit.source != "-" {
        return hit.source.clone();
    }
    hit.summary
        .split_once(':')
        .map(|(name, _)| name.trim())
        .filter(|name| !name.is_empty())
        .unwrap_or(hit.summary.as_str())
        .to_string()
}

fn indent(line: &str) -> String {
    if line.is_empty() {
        String::new()
    } else {
        format!("  {line}")
    }
}

fn trimmed_edges(lines: &[String]) -> Vec<String> {
    let Some(start) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return Vec::new();
    };
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .unwrap_or(start);
    lines[start..=end].to_vec()
}

fn split_sections(text: &str) -> Vec<Section> {
    let raw_lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    if raw_lines.iter().any(|line| heading_of(line).is_some()) {
        split_markdown(&raw_lines)
    } else {
        split_paragraphs(&raw_lines)
    }
}

fn split_markdown(lines: &[String]) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut headings: Vec<String> = Vec::new();
    let mut body: Vec<String> = Vec::new();
    let flush = |headings: &[String], body: &mut Vec<String>, sections: &mut Vec<Section>| {
        let has_body = body.iter().any(|line| !line.trim().is_empty());
        if has_body || !headings.is_empty() {
            sections.push(Section {
                headings: headings.to_vec(),
                lines: std::mem::take(body),
            });
        } else {
            body.clear();
        }
    };
    for line in lines {
        if let Some((level, title)) = heading_of(line) {
            if body.iter().any(|item| !item.trim().is_empty()) {
                sections.push(Section {
                    headings: headings.clone(),
                    lines: std::mem::take(&mut body),
                });
            } else {
                body.clear();
            }
            let level = level.min(headings.len() + 1);
            headings.truncate(level - 1);
            headings.push(title.to_string());
        } else {
            body.push(line.clone());
        }
    }
    flush(&headings, &mut body, &mut sections);
    if sections.is_empty() {
        sections.push(Section {
            headings,
            lines: Vec::new(),
        });
    }
    sections
}

fn split_paragraphs(lines: &[String]) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut body: Vec<String> = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            if body.iter().any(|item| !item.trim().is_empty()) {
                sections.push(Section {
                    headings: Vec::new(),
                    lines: std::mem::take(&mut body),
                });
            } else {
                body.clear();
            }
        } else {
            body.push(line.clone());
        }
    }
    if body.iter().any(|item| !item.trim().is_empty()) {
        sections.push(Section {
            headings: Vec::new(),
            lines: body,
        });
    }
    if sections.is_empty() {
        sections.push(Section {
            headings: Vec::new(),
            lines: Vec::new(),
        });
    }
    sections
}

fn heading_of(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim();
    let marks = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&marks) {
        return None;
    }
    let rest = &trimmed[marks..];
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let title = rest.trim();
    if title.is_empty() {
        None
    } else {
        Some((marks, title))
    }
}

/// Same light inflection rule Locus uses when a recall term hits stored text.
fn contains_term(haystack: &str, term: &str) -> bool {
    if term.is_empty() {
        return false;
    }
    let haystack = haystack.to_ascii_lowercase();
    let term = term.to_ascii_lowercase();
    if haystack.contains(&term) {
        return true;
    }
    if term.len() < 5 {
        return false;
    }
    haystack
        .split(|ch: char| !ch.is_alphanumeric())
        .any(|word| word.len() >= 5 && (word.starts_with(&term) || term.starts_with(word)))
}

/// Pull the original note out of a stored STTP document.
///
/// Locus keeps that text on the content layer as `text(.70)`. Chat turns live
/// on nested `text` fields; a single document field is returned as itself.
pub fn extract_document_text(raw: &str) -> Option<String> {
    let content = SttpLayerStateMachine::parse(raw).content?;
    let texts = collect_text_fields(content);
    let texts = texts
        .into_iter()
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>();
    match texts.len() {
        0 => None,
        1 => texts.into_iter().next(),
        _ => {
            if let Some(longest) = texts.iter().max_by_key(|text| text.len()) {
                if texts.iter().all(|text| longest.contains(text.as_str())) {
                    return Some(longest.clone());
                }
            }
            Some(texts.join("\n\n"))
        }
    }
}

fn collect_text_fields(content: &str) -> Vec<String> {
    let mut texts = Vec::new();
    let mut in_string = false;
    let mut escape = false;
    let mut reading_key = false;
    let mut key = String::new();
    let mut string_buf = String::new();
    let mut string_is_text = false;

    for ch in content.chars() {
        if in_string {
            if escape {
                string_buf.push(ch);
                escape = false;
                continue;
            }
            match ch {
                '\\' => escape = true,
                '"' => {
                    in_string = false;
                    if string_is_text {
                        texts.push(std::mem::take(&mut string_buf));
                    } else {
                        string_buf.clear();
                    }
                    string_is_text = false;
                }
                _ => {
                    if string_is_text {
                        string_buf.push(ch);
                    }
                }
            }
            continue;
        }

        match ch {
            '{' => {
                key.clear();
                reading_key = true;
            }
            '}' => {
                key.clear();
                reading_key = false;
            }
            ',' => {
                key.clear();
                reading_key = true;
            }
            ':' => reading_key = false,
            '"' => {
                in_string = true;
                string_is_text = is_text_key(key.trim());
                string_buf.clear();
            }
            _ if reading_key && !ch.is_whitespace() => key.push(ch),
            _ => {}
        }
    }
    texts
}

fn is_text_key(key: &str) -> bool {
    let Some((name, rest)) = key.split_once('(') else {
        return false;
    };
    name == "text" && rest.ends_with(')')
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{extract_document_text, view_hits, MatchPattern, ViewOptions};
    use crate::compile::compile_context;
    use crate::memory::Recalled;
    use crate::stage::{ContextKind, StagedContext};

    fn compiled(source: &str, text: &str) -> Recalled {
        let staged = StagedContext {
            id: "1".to_string(),
            kind: ContextKind::Document,
            session: "main".to_string(),
            source: source.to_string(),
            text: text.to_string(),
            added_at: Utc::now(),
            tags: Vec::new(),
        };
        let compiled = compile_context(&staged).unwrap();
        Recalled {
            session: "main".to_string(),
            summary: compiled.summary,
            raw_sttp: compiled.raw_sttp,
            path: "lexicalfallback".to_string(),
            source: source.to_string(),
        }
    }

    fn excerpt<'a>(query: Option<&'a str>, pattern: Option<&'a MatchPattern>) -> ViewOptions<'a> {
        ViewOptions {
            query,
            pattern,
            literal: None,
            excerpt: true,
            full: false,
            context_lines: None,
            limit: 8,
        }
    }

    const RUNBOOK: &str = "\
# API runbook

The service overview stays quiet.

## Escalation

Primary on-call has 15 minutes to ack. After that PagerDuty escalates to the secondary.

## Rollback

Use deployctl to roll back the last release.
";

    #[test]
    fn extracts_document_text_without_the_envelope() {
        let hit = compiled("runbook.md", RUNBOOK);
        let text = extract_document_text(&hit.raw_sttp).unwrap();
        assert!(text.contains("## Escalation"), "{text}");
        assert!(text.contains("deployctl"), "{text}");
        assert!(!text.contains('⊕'), "{text}");
    }

    #[test]
    fn excerpt_keeps_the_matching_section_and_heading_path() {
        let hit = compiled("runbook.md", RUNBOOK);
        let viewed = view_hits(&[hit], &excerpt(Some("pagerduty escalation"), None)).unwrap();
        let text = viewed[0].lines.join("\n");
        assert!(
            text.contains("runbook.md § API runbook > Escalation"),
            "{text}"
        );
        assert!(text.contains("PagerDuty"), "{text}");
        assert!(!text.contains("deployctl"), "{text}");
        assert!(!text.contains("service overview"), "{text}");
    }

    #[test]
    fn match_pattern_selects_a_different_section_than_the_query() {
        let hit = compiled("runbook.md", RUNBOOK);
        let pattern = MatchPattern::compile(Some("deployctl")).unwrap();
        let viewed = view_hits(
            std::slice::from_ref(&hit),
            &excerpt(Some("api runbook"), pattern.as_ref()),
        )
        .unwrap();
        let text = viewed[0].lines.join("\n");
        assert!(text.contains("Rollback"), "{text}");
        assert!(text.contains("deployctl"), "{text}");
        assert!(!text.contains("15 minutes"), "{text}");
    }

    #[test]
    fn context_lines_narrow_a_section_to_the_matching_line() {
        let note = "\
# Handoff

## Pager

The rotation calendar is stale.
Fix the xylophone entry in pagerduty before the next rotation.
";
        let hit = compiled("handoff.md", note);
        let mut options = excerpt(Some("xylophone"), None);
        options.context_lines = Some(0);
        let viewed = view_hits(&[hit], &options).unwrap();
        let text = viewed[0].lines.join("\n");
        assert!(text.contains("handoff.md § Handoff > Pager"), "{text}");
        assert!(text.contains("xylophone"), "{text}");
        assert!(!text.contains("rotation calendar"), "{text}");
    }

    #[test]
    fn plain_text_falls_back_to_paragraphs() {
        let note = "alpha beta\n\ngamma zebra delta\n\nepsilon\n";
        let hit = compiled("note.txt", note);
        let viewed = view_hits(&[hit], &excerpt(Some("zebra"), None)).unwrap();
        let text = viewed[0].lines.join("\n");
        assert!(text.starts_with("note.txt\n"), "{text}");
        assert!(!text.contains('§'), "{text}");
        assert!(text.contains("gamma zebra delta"), "{text}");
        assert!(!text.contains("epsilon"), "{text}");
        assert!(!text.contains("alpha beta"), "{text}");
    }

    #[test]
    fn best_section_score_reorders_hits() {
        let weak = compiled(
            "handoff.md",
            "# Handoff\n\nA passing mention of pagerduty escalation in a list.\n",
        );
        let strong = compiled("runbook.md", RUNBOOK);
        let viewed = view_hits(
            &[weak, strong],
            &excerpt(Some("pagerduty escalation"), None),
        )
        .unwrap();
        assert!(
            viewed[0].lines[0].starts_with("runbook.md"),
            "{:?}",
            viewed[0].lines
        );
        assert!(
            viewed[0].lines[0].contains("Escalation"),
            "{:?}",
            viewed[0].lines
        );
    }
}
