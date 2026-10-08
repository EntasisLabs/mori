use std::path::Path;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use locus_core_rs::domain::contracts::NodeValidator;
use locus_core_rs::domain::models::AvecState;
use locus_core_rs::parsing::{SttpContentSlice, SttpDocumentBuilder, SttpDocumentMetadata};
use locus_core_rs::{InMemoryNodeStore, SttpNodeParser, TreeSitterValidator};
use locus_sdk::prelude::{
    CompositeInputItem, CompositeNodeFromTextOptions, CompositeNodeFromTextRequest, CompositeRole,
    CompositeRoleAvecOverrides, MemoryCompositionService,
};
use serde_json::Value;
use std::sync::Arc;

use crate::stage::{ContextKind, StagedContext};

/// Compiled context. `raw_sttp` is the wire form the SDK produced or accepted.
/// Callers that only want to compile stop here and never open SurrealKV.
#[derive(Debug, Clone)]
pub struct CompiledContext {
    pub kind: ContextKind,
    pub session: String,
    pub source: String,
    pub summary: String,
    pub raw_sttp: String,
    pub timestamp: DateTime<Utc>,
}

pub fn compile_context(staged: &StagedContext) -> Result<CompiledContext> {
    let kind = match staged.kind {
        ContextKind::Auto => detect_kind(&staged.source, &staged.text),
        other => other,
    };
    let summary = summary_line(&staged.source, &staged.text);

    let raw_sttp = if kind == ContextKind::Sttp {
        validate_sttp(&staged.text)?;
        staged.text.trim().to_string()
    } else {
        render_with_sdk(staged, kind, &summary)?
    };

    Ok(CompiledContext {
        kind,
        session: staged.session.clone(),
        source: staged.source.clone(),
        summary,
        raw_sttp,
        timestamp: staged.added_at,
    })
}

/// Pick a kind from the text itself so callers never have to hand-write STTP.
pub fn detect_kind(source: &str, text: &str) -> ContextKind {
    if looks_like_sttp(text) {
        return ContextKind::Sttp;
    }
    let name = Path::new(source)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(source)
        .to_ascii_lowercase();
    let ext = Path::new(source)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let chat_name = name.contains("chat") || name.contains("transcript");
    let structured = ext == "json" || ext == "jsonl" || chat_name;
    if structured && parse_chat(text).is_some() {
        return ContextKind::Chat;
    }
    ContextKind::Document
}

fn looks_like_sttp(text: &str) -> bool {
    text.contains("⊕⟨") || text.contains("⦿⟨") || text.contains("◈⟨") || text.contains("⍉⟨")
}

fn validate_sttp(text: &str) -> Result<()> {
    let validator = TreeSitterValidator::new();
    let validation = validator.validate(text);
    if !validation.is_valid {
        bail!(
            "invalid STTP: {}",
            validation
                .error
                .unwrap_or_else(|| "validation failed".to_string())
        );
    }
    let parsed = SttpNodeParser::new().try_parse(text, "mori");
    if !parsed.success {
        bail!(
            "STTP did not parse: {}",
            parsed.error.unwrap_or_else(|| "parse failed".to_string())
        );
    }
    Ok(())
}

fn render_with_sdk(staged: &StagedContext, kind: ContextKind, summary: &str) -> Result<String> {
    let items = items_for(kind, &staged.text, summary)?;
    let store = Arc::new(InMemoryNodeStore::new());
    let composition = MemoryCompositionService::new(store);
    let request = CompositeNodeFromTextRequest {
        items,
        options: CompositeNodeFromTextOptions {
            role_avec: default_role_avec(),
            global_avec: Some(AvecState::analytical()),
            allow_llm_avec_fallback: false,
            max_recursion_depth: 5,
        },
    };
    let built = composition
        .build_content_from_text(&request)
        .context("locus composition failed")?;
    if built.requires_llm_avec {
        bail!("locus could not resolve an AVEC state for this context");
    }
    let content = match built.content {
        Value::Object(map) => map,
        other => bail!("locus composition returned {other}, expected an object"),
    };

    let avec = envelope_avec(kind);
    let metadata = SttpDocumentMetadata::new(staged.session.trim())
        .with_timestamp(staged.added_at)
        .with_context_summary(summary)
        .with_avec(avec, avec)
        .with_semantic_tags(semantic_tags(kind, &staged.source, &staged.tags));

    let document = SttpDocumentBuilder::new(metadata)
        .merge(SttpContentSlice::from_confidence_map(content)?)?
        .build()?;
    Ok(document.render_canonical())
}

fn items_for(kind: ContextKind, text: &str, summary: &str) -> Result<Vec<CompositeInputItem>> {
    let text = text.trim();
    if text.is_empty() {
        bail!("context is empty");
    }

    match kind {
        ContextKind::Chat => Ok(chat_items(text, summary)),
        ContextKind::Sttp | ContextKind::Auto => {
            bail!("internal: STTP is stored through the parser, not the composer")
        }
        ContextKind::Document | ContextKind::Note | ContextKind::Context => {
            Ok(vec![CompositeInputItem {
                role: CompositeRole::Document,
                text: text.to_string(),
                avec_override: None,
                context: Vec::new(),
            }])
        }
    }
}

fn chat_items(text: &str, summary: &str) -> Vec<CompositeInputItem> {
    let turns = parse_chat(text).unwrap_or_else(|| {
        vec![Turn {
            role: CompositeRole::Conversation,
            text: text.to_string(),
        }]
    });

    if turns.len() == 1 && turns[0].role == CompositeRole::Conversation {
        return vec![CompositeInputItem {
            role: CompositeRole::Conversation,
            text: turns[0].text.clone(),
            avec_override: None,
            context: Vec::new(),
        }];
    }

    let context = turns
        .into_iter()
        .map(|turn| CompositeInputItem {
            role: turn.role,
            text: turn.text,
            avec_override: None,
            context: Vec::new(),
        })
        .collect();

    vec![CompositeInputItem {
        role: CompositeRole::Conversation,
        text: summary.to_string(),
        avec_override: None,
        context,
    }]
}

struct Turn {
    role: CompositeRole,
    text: String,
}

/// Accept a JSON chat log or a `user:` / `assistant:` transcript.
/// Returns `None` when the text has no recognizable turns.
fn parse_chat(text: &str) -> Option<Vec<Turn>> {
    let trimmed = text.trim();
    if let Some(turns) = parse_chat_json(trimmed) {
        if !turns.is_empty() {
            return Some(turns);
        }
    }
    parse_chat_transcript(trimmed).filter(|turns| !turns.is_empty())
}

fn parse_chat_json(text: &str) -> Option<Vec<Turn>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let messages = match &value {
        Value::Array(items) => items,
        Value::Object(map) => map
            .get("messages")
            .or_else(|| map.get("turns"))
            .and_then(Value::as_array)?,
        _ => return None,
    };

    let mut turns = Vec::new();
    for message in messages {
        let object = message.as_object()?;
        let body = ["content", "text", "message", "body"]
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str))
            .unwrap_or("")
            .trim();
        if body.is_empty() {
            continue;
        }
        let role_name = object
            .get("role")
            .or_else(|| object.get("speaker"))
            .and_then(Value::as_str)
            .unwrap_or("user");
        turns.push(Turn {
            role: role_from_name(role_name),
            text: body.to_string(),
        });
    }
    Some(turns)
}

fn parse_chat_transcript(text: &str) -> Option<Vec<Turn>> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut saw_speaker = false;
    for line in text.lines() {
        if let Some((role, rest)) = split_speaker(line) {
            saw_speaker = true;
            turns.push(Turn {
                role,
                text: rest.to_string(),
            });
        } else if let Some(current) = turns.last_mut() {
            if !line.trim().is_empty() {
                if !current.text.is_empty() {
                    current.text.push('\n');
                }
                current.text.push_str(line.trim());
            }
        }
    }
    if !saw_speaker {
        return None;
    }
    turns.retain(|turn| !turn.text.trim().is_empty());
    Some(turns)
}

fn split_speaker(line: &str) -> Option<(CompositeRole, &str)> {
    let (name, rest) = line.trim().split_once(':')?;
    let name = name.trim();
    if name.is_empty() || name.len() > 24 || name.contains(' ') {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let known = matches!(
        lower.as_str(),
        "user" | "human" | "assistant" | "model" | "bot" | "system" | "document" | "note"
    );
    if !known {
        return None;
    }
    Some((role_from_name(&lower), rest.trim()))
}

fn role_from_name(name: &str) -> CompositeRole {
    match name.trim().to_ascii_lowercase().as_str() {
        "assistant" | "model" | "bot" | "system" => CompositeRole::Model,
        "document" | "note" => CompositeRole::Document,
        "conversation" => CompositeRole::Conversation,
        _ => CompositeRole::User,
    }
}

fn default_role_avec() -> CompositeRoleAvecOverrides {
    CompositeRoleAvecOverrides {
        user: Some(AvecState::collaborative()),
        model: Some(AvecState::analytical()),
        document: Some(AvecState::focused()),
        conversation: Some(AvecState::exploratory()),
    }
}

fn envelope_avec(kind: ContextKind) -> AvecState {
    match kind {
        ContextKind::Chat => AvecState::exploratory(),
        ContextKind::Note => AvecState::collaborative(),
        ContextKind::Document | ContextKind::Context => AvecState::focused(),
        ContextKind::Sttp | ContextKind::Auto => AvecState::analytical(),
    }
}

fn summary_line(source: &str, text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let body = if collapsed.is_empty() {
        source_tag(source)
    } else {
        collapsed.chars().take(180).collect()
    };
    let label = source_tag(source);
    if label == "-" || body.starts_with(&label) {
        body
    } else {
        format!("{label}: {body}")
    }
}

/// Kind and source tags Mori already writes, plus the facets from `mori add --tag`.
fn semantic_tags(kind: ContextKind, source: &str, user_tags: &[String]) -> Vec<String> {
    let mut tags = vec![
        kind.as_str().to_string(),
        format!("source:{}", source_tag(source)),
    ];
    for tag in user_tags {
        if tags
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(tag))
        {
            continue;
        }
        tags.push(tag.clone());
    }
    tags
}

fn source_tag(source: &str) -> String {
    if source == "-" {
        return "-".to_string();
    }
    Path::new(source)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(source)
        .chars()
        .take(80)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{compile_context, detect_kind, parse_chat};
    use crate::stage::{ContextKind, StagedContext};
    use chrono::Utc;
    use locus_sdk::prelude::CompositeRole;

    fn staged(kind: ContextKind, source: &str, text: &str) -> StagedContext {
        StagedContext {
            id: "1".to_string(),
            kind,
            session: "main".to_string(),
            source: source.to_string(),
            text: text.to_string(),
            added_at: Utc::now(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn detects_documents_chats_and_raw_sttp() {
        assert_eq!(
            detect_kind("notes.md", "the orchard plan waits"),
            ContextKind::Document
        );
        assert_eq!(
            detect_kind(
                "thread.json",
                r#"[{"role":"user","content":"hello there"}]"#
            ),
            ContextKind::Chat
        );
        assert_eq!(
            detect_kind("node.sttp", "prefix ⊕⟨ raw ⟩ tail"),
            ContextKind::Sttp
        );
    }

    #[test]
    fn parses_a_speaker_transcript() {
        let turns = parse_chat("user: where is the fence\nassistant: north side\n").unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, CompositeRole::User);
        assert_eq!(turns[1].role, CompositeRole::Model);
    }

    #[test]
    fn compiles_a_document_without_opening_a_database() {
        let compiled = compile_context(&staged(
            ContextKind::Document,
            "notes.md",
            "the orchard plan waits on the north fence",
        ))
        .unwrap();
        assert!(compiled.raw_sttp.contains("⊕⟨"));
        assert!(compiled.raw_sttp.contains("orchard"));
        assert!(compiled.summary.contains("orchard"));
        assert!(compiled.raw_sttp.contains("source:notes.md"));
    }

    #[test]
    fn user_tags_merge_with_the_document_tags() {
        let mut entry = staged(
            ContextKind::Document,
            "notes.md",
            "the orchard plan waits on the north fence",
        );
        entry.tags = vec!["homelab".to_string(), "pxe".to_string()];
        let compiled = compile_context(&entry).unwrap();
        assert!(
            compiled.raw_sttp.contains("homelab"),
            "{}",
            compiled.raw_sttp
        );
        assert!(compiled.raw_sttp.contains("pxe"), "{}", compiled.raw_sttp);
        assert!(
            compiled.raw_sttp.contains("source:notes.md"),
            "{}",
            compiled.raw_sttp
        );
    }

    #[test]
    fn compiles_a_chat_log() {
        let compiled = compile_context(&staged(
            ContextKind::Chat,
            "thread.json",
            r#"[{"role":"user","content":"where did we leave the orchard plan"},{"role":"assistant","content":"it waits on the north fence"}]"#,
        ))
        .unwrap();
        assert!(compiled.raw_sttp.contains("orchard"));
        assert!(compiled.raw_sttp.contains("fence"));
    }
}
