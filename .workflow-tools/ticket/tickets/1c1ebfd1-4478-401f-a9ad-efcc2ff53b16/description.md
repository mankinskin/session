## Goal
Reduce low-value context that reaches GitHub Copilot model APIs by tightening upstream workflow guidance, tool-result handling, and bootstrap or handoff behavior.

## Immediate scope
- add repository guidance that distinguishes diagnostic transcript visibility from upstream request shaping
- update existing workflow prompts and instructions so bootstrap and handoff flows prefer durable findings over raw tool chatter
- document the highest-confidence no-code and low-code reductions identified across multiple captured sessions

## Planned follow-up
- specify coded upstream tool-result guards, duplicate suppression, and compact prompt-facing state views in the linked spec
- leave deeper implementation work to follow-up tickets once the contract is agreed

## Acceptance notes
- guidance identifies high-confidence boilerplate that should not reach the LLM by default
- prompts reference upstream tool-result compression and routine-action discipline as the main cost levers
- advanced implementation work is captured in a spec and follow-up ticket with the corrected architecture boundary