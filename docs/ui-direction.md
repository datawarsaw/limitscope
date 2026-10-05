# LimitScope UI Direction

Approved design direction: Variant A — Halo.

## Priority

When guidance conflicts, use this order:

1. Current task
2. This document
3. Current production behavior and architecture
4. Local approved Halo reference source
5. Other design experiments

## Approved

- Semantic remaining-quota color scale
- Halo meter treatment
- Dark glass visual language
- Restrained, non-neon visual effects
- Motion polish between providers and detail-card states (active-provider selector glide, card open/close fades, card glide between segments, per-provider content crossfade, eased meter width and semantic-color band changes)

## Planned next

- Resting Dock exploration after the base Halo interaction is polished

## Halo reference

Do not assume direct Figma MCP access is available.

When implementation needs exact Halo visual or interaction details, use the locally available Figma Make source/reference files when provided or available.

Treat generated Figma Make code as design reference, not production architecture.

Harvest specific approved patterns and adapt them to the existing LimitScope implementation.

Do not copy generated components wholesale.

## Do not copy from design prototypes

- Placeholder providers or placeholder quota data
- Prototype-only logos
- Generated architecture wholesale
- Unrelated layout changes
- Typography replacements unless explicitly approved
- Experimental variants that have not been approved

Production providers remain:

- OpenAI / Codex
- Z.ai
- OpenCode Go
- Antigravity / Gemini
- Grok

## Implementation rule

Preserve existing production behavior and architecture unless the current task explicitly changes them.

Keep every implementation slice narrow.

Do not mix multiple experimental design directions in one task.

## Human Acceptance

Every UI implementation task must finish with:

- targeted validation
- a local runnable artifact suitable for Human Acceptance
- exact executable or installer path
- version/build identifier
- concise visual checks

Do not push, merge, tag, publish, or release before Human Acceptance.

## Task prompting

Keep UI task prompts short.

The task prompt should normally specify only the next implementation slice.

Do not require the task prompt to repeat the full Halo specification when it is already captured here or available in the local approved reference source.
