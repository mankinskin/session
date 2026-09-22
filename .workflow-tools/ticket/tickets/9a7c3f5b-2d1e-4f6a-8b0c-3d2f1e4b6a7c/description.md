Implementation-ready handoff support: concise target-ticket and session summaries

This task implements the server-side and handoff-schema follow-ups required to
persist and render concise, durable `target-ticket` and `session` summaries for
implementation-ready handoffs. The new summaries are authored (with generated
defaults) and are required only when a handoff is marked implementation-ready.

Evidence: reviewed handoff (do not overwrite) .session/sessions/.../handoffs/276acf70-5af5-45de-8154-5ef9b58357f7/handoff.md

Acceptance criteria (implementation):
- Persist authored `target_ticket.summary` and `session_summary` alongside the
  existing package fields. Defaults are generated but editable by authors.
- No arbitrary character limits are enforced by the schema; summaries must be
  concise by intent and checked in validation logic, not by truncation.
- The `target_ticket` entry must inline the durable `current_state` and
  `acceptance_criteria` alongside the concise `summary`, the ticket reference
  (`{short-id} {title}`), and an author-supplied `why` field.
- The rendered handoff `handoff.md` must render the concise `target_ticket.summary`
  and `session_summary` as persisted markdown, and must not fetch or render the
  ticket's full objective/body text (ticket-store lookups may resolve title/reference only).
- Legacy handoff formats remain deserializable and compatible.
- Validation checks: JSON round-trip, legacy compatibility, rendered concise
  summary present, absence of full objective/body text, required-field
  rejection for implementation-ready creation, exploratory handoffs allowed,
  and session-summary rendering.
