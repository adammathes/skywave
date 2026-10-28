---
name: work-queue
description: Work the owner's approved Skywave cards. Use when the owner says "run the work queue", "do the approved items", "do all the approved items in the work queue in new subagents", "land the open queue PRs", "what's queued?", or names cards to build. Runs the saved `work-queue` workflow. Never starts Proposed cards.
---

# Work queue

The owner's board (https://claude.ai/artifact/17baWQ1hwt3R194YbzjpWS) is the queue of record.
**Proposed** cards are ideas: never work them. **Queued** cards are approved: work them top first
without asking again. Only the owner moves a card from Proposed to Queued.

The owner asking for this is the opt-in: run the saved workflow with the Workflow tool, `name:
"work-queue"` (if that name is not found, use `scriptPath: "<repo root>/.claude/workflows/work-queue.js"`),
and `args` built from what they said. Always pass `now` as the current UTC time
(`date -u +%Y-%m-%dT%H:%M:%SZ`), since the script cannot read the clock.

| The owner says | args |
|---|---|
| "run the work queue", "do the approved items (in new subagents)" | `{ now }` |
| "do all of them at once", "run five at a time" | `{ now, max: <n> }`. Default is 3; more than 5 strains disk and the shared screenshot box. |
| "just do <card>", "do the radio one and B7" | `{ now, items: ["<card id or a title word>", ...] }` |
| "don't merge", "stop at PRs" | `{ now, merge: false }` |
| "land what's open", "merge the green ones" | `{ now, build: false }` |
| "what would you run?", "what's queued?" | `{ now, dryRun: true }`, then report the lists |

If the owner names a card that is still Proposed, don't start it: say it needs their Approve on the
board first (the workflow logs this too).

What one run does (`.claude/workflows/work-queue.js`):
1. **Read** the board: Queued cards in order, plus cards left at In review with a queue PR.
2. **Land** those earlier PRs: merge if green, one fix attempt if red.
3. **Build** up to `max` Queued cards in parallel, each in its own worktree on branch `q/<card id>`.
4. **Review** each with an independent read-only reviewer. Risky work (sim, world generation, traffic,
   pedestrians) gets the reviewer's own adversarial probes. Findings go back to the builder for up
   to 3 rounds; after that the card moves to Needs you with the findings.
5. **Ship:** open the PR, wait for CI, merge on green. If CI is still running after ~45 minutes, the
   card stays In review and the next run lands it.

Every step updates the card. Out-of-scope ideas from reviews become new Proposed cards. When the
run ends, give the owner its `report` lines and the cards still waiting (`leftQueued`), and nothing
more.

Resume a stopped run with the Workflow tool's `resumeFromRunId` and the same `scriptPath`.
