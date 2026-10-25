# Agent workflow

Most code is written by autonomous agents. This is how work is specified, done, reviewed and merged so that quality stays high without a human watching.

## Roles
- **Spec (Fable, this session):** design docs, milestone breakdown, issue writing, gate preparation, architecture decisions and ADRs. Expensive; used for thinking, not typing.
- **Builders (Opus 5.5 or cheaper):** one issue each, in their own session/worktree, on their own branch, open a PR.
- **Reviewer (Opus 5.5):** reviews every PR against the issue's acceptance criteria, `CLAUDE.md`, and the relevant doc; checks screenshots; requests changes or approves. A second independent review for anything touching `vehicle::sim` or `render` passes (feel-critical).
- **Human:** approves plans, plays gates, decides taste.

## Backlog
- After a milestone's gate is approved, Spec turns the milestone into GitHub issues, one per task, labeled `M<n>` and by crate. Each issue has: goal (one sentence), context (which docs sections), scope (in/out), acceptance criteria (tests to add, numbers to hit, screenshot to attach), and dependencies (issue numbers).
- Issues are sized for one agent session: ≤ ~800 lines changed, finishable without asking.
- Order within a milestone is a DAG; independent issues run in parallel (a swarm of builders), dependent ones wait.

## Builder loop
1. Read `CLAUDE.md`, the issue, the referenced docs. Create a branch `m<n>/<issue>-<slug>`.
2. Write tests from the acceptance criteria first where feasible.
3. Implement minimally. Run the fast checks locally: fmt, clippy, tests, and the screenshot tool for visual work; look at the PNG.
4. Open a PR: title = issue title; body = what/why, how verified (paste command summaries), screenshot(s), "Closes #n". No model names in commits or PRs.
5. Respond to review; keep pushing until green and approved. Do not widen scope; file follow-up issues instead.

## Review checklist
- Does it do exactly what the issue says, no more?
- Determinism and `no_std` rules respected? Budgets respected?
- Tests prove the acceptance criteria, not just coverage?
- Any new dependency: license OK, size justified, ADR if large?
- Visual changes: screenshot attached and looked at; goldens updated deliberately?
- Simplicity: could this be smaller? Any speculative abstraction? Delete it.

## Merge
Squash-merge into `main` once CI is green and the reviewer approved. `main` always deploys to Pages. Breakage on `main` is fixed forward within the day or reverted.

## Spawning a swarm (mechanics)
From a Claude Code cloud session: one child session per issue (`create_session` with the repo, branch name, the issue text as the prompt, Opus 5.5), tags `skywave:m<n>`. A coordinator session (cheap model) watches PR events, triggers the reviewer, and reports to the human once per day or at the milestone's end. The human is pinged only for gates and genuine decisions.

## Memory
Decisions live in `docs/` and `docs/adr/`, never in chat. When a gate changes direction, the relevant doc is edited in the same PR as the first code change.
