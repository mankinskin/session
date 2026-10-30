#[allow(unused_imports)]
use super::*;

pub(super) struct HandoffTicketDisplay {
    pub(super) reference: String,
    pub(super) what: String,
}

pub(super) struct ResolvedHandoffTicket {
    pub(super) id: Uuid,
    pub(super) title: Option<String>,
    pub(super) what: String,
}

pub(super) fn resolve_handoff_ticket_display(
    record: &SessionHandoffRecord,
    ticket: &crate::SessionHandoffTargetTicket,
    ticket_store: Option<&TicketStore>,
) -> HandoffTicketDisplay {
    let cached_title = record
        .workflow
        .workflow
        .nodes
        .iter()
        .find(|node| {
            node.ticket_urn
                .as_deref()
                .is_some_and(|urn| urn.ends_with(&ticket.id))
        })
        .and_then(|node| node.cached_ticket_title.as_deref())
        .unwrap_or(&ticket.id);
    let resolved = ticket_store.and_then(|store| {
        resolve_handoff_ticket(store, &ticket.id).map(|ticket| {
            (
                ticket.id,
                ticket.title.unwrap_or_else(|| cached_title.to_string()),
                ticket.what,
            )
        })
    });
    match resolved {
        Some((id, title, what)) => HandoffTicketDisplay {
            reference: handoff_ticket_reference(&id.to_string(), &title),
            what,
        },
        None => HandoffTicketDisplay {
            reference: format!("{} {}", ticket.id, cached_title),
            what: String::new(),
        },
    }
}

pub(super) fn resolve_handoff_ticket(
    ticket_store: &TicketStore,
    ticket_id: &str,
) -> Option<ResolvedHandoffTicket> {
    let id = resolve_uuid_with_prefix(ticket_store, ticket_id).ok()?;
    let projection = ticket_store
        .project(&id, &ReadProjection::Profile(ViewProfile::Summary))
        .ok()?;
    let title = projection
        .fields
        .get("title")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let what = projection
        .parts
        .iter()
        .find(|part| part.kind == "objective")
        .map(|part| part.content.clone())
        .unwrap_or_default();
    Some(ResolvedHandoffTicket { id, title, what })
}

pub(super) fn handoff_ticket_reference(ticket_id: &str, title: &str) -> String {
    format!(
        "[{} {}](.ticket/tickets/{ticket_id}/ticket.toml)",
        ticket_id.chars().take(8).collect::<String>(),
        title,
    )
}

pub(super) const MAX_HANDOFF_PROSE_TICKET_REFERENCES: usize = 128;

pub(super) fn linkify_handoff_prose(value: &str, ticket_store: Option<&TicketStore>) -> String {
    let Some(ticket_store) = ticket_store else {
        return value.to_string();
    };

    let mut output = String::with_capacity(value.len());
    let mut in_fence = false;
    let mut replacements = 0;
    for line in value.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            output.push_str(line);
        } else if in_fence {
            output.push_str(line);
        } else {
            output.push_str(&linkify_handoff_prose_line(
                line,
                ticket_store,
                &mut replacements,
            ));
        }
    }
    output
}

pub(super) fn linkify_handoff_prose_line(
    line: &str,
    ticket_store: &TicketStore,
    replacements: &mut usize,
) -> String {
    let bytes = line.as_bytes();
    let mut output = String::with_capacity(line.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'`' {
            let end = line[index + 1..]
                .find('`')
                .map(|offset| index + offset + 2)
                .unwrap_or(bytes.len());
            output.push_str(&line[index..end]);
            index = end;
            continue;
        }
        if bytes[index] == b'['
            && let Some(label_end) = line[index..].find("](")
            && let Some(target_end) = line[index + label_end + 2..].find(')')
        {
            let end = index + label_end + target_end + 3;
            output.push_str(&line[index..end]);
            index = end;
            continue;
        }
        if *replacements < MAX_HANDOFF_PROSE_TICKET_REFERENCES
            && let Some(ticket_id) = bare_ticket_id_at(line, index)
            && let Some(resolved) = resolve_handoff_ticket(ticket_store, ticket_id)
            && let Some(title) = resolved.title
        {
            output.push_str(&handoff_ticket_reference(&resolved.id.to_string(), &title));
            index += ticket_id.len();
            *replacements += 1;
            continue;
        }
        let character = line[index..].chars().next().expect("valid UTF-8");
        output.push(character);
        index += character.len_utf8();
    }
    output
}

pub(super) fn bare_ticket_id_at(value: &str, index: usize) -> Option<&str> {
    let bytes = value.as_bytes();
    if index > 0 && is_ticket_id_word_byte(bytes[index - 1]) {
        return None;
    }
    let remaining = &value[index..];
    let length = if remaining.len() >= 36 && is_uuid_token(&remaining[..36]) {
        36
    } else if remaining.len() >= 8 && remaining[..8].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        8
    } else {
        return None;
    };
    if bytes
        .get(index + length)
        .is_some_and(|byte| is_ticket_id_word_byte(*byte))
    {
        return None;
    }
    Some(&value[index..index + length])
}

pub(super) fn is_uuid_token(value: &str) -> bool {
    value.bytes().enumerate().all(|(index, byte)| {
        matches!(index, 8 | 13 | 18 | 23) && byte == b'-'
            || !matches!(index, 8 | 13 | 18 | 23) && byte.is_ascii_hexdigit()
    })
}

pub(super) fn is_ticket_id_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
