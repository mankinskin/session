## Objective
Author a specification for mcp-toolmon session validation before guarded tool calls.

## Context
Owner decision: “`mcp-toolmon` acts as a pre-tool hook and requires a valid session id to execute specific tool calls. Built-in (non-MCP) tools remain exempt for now, replaceable by own tools later. Rationale: mcp-toolmon is already integral to workflow enforcement via the cost gate and tool restarting.”
Relates to specs `1bef7b3d` and `2860a8db`.

## Acceptance criteria
- Define guarded-call scope and valid-session checks.
- Define built-in exemption and replacement boundary.
- Define failure and testable enforcement behavior.

## Out of scope
- Hook implementation.