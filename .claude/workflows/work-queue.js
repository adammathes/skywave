export const meta = {
  name: 'work-queue',
  description: 'Work the approved (Queued) cards on the owner\'s board: build, review, PR, merge',
  whenToUse: 'The owner says "run the work queue", "do the approved items", or names Queued cards to build. Never for Proposed cards.',
  phases: [
    { title: 'Read', detail: 'the Queued cards on the board, and queue PRs left open by earlier runs' },
    { title: 'Land', detail: 'merge earlier queue PRs that are green; fix the red ones' },
    { title: 'Build', detail: 'one builder per card, each in its own worktree and branch' },
    { title: 'Review', detail: 'a read-only adversarial reviewer per card; fixes go back to the builder' },
    { title: 'Ship', detail: 'open the PR, wait for CI, merge on green' },
  ],
}

// How it works (docs/HANDOFF.md, "Work queue"):
// - Only cards whose status is `queued` are ever started. Proposed cards are the owner's to
//   approve; this script never reads or writes them except to file follow-ups as new
//   Proposed cards.
// - Every step updates the card, so the board is the record and a later run picks up where
//   this one stopped (cards left at `review` with a PR are landed first next time).
// - args (all optional): {
//     now: ISO time, stamped on card updates (scripts cannot read the clock),
//     items: [card ids or title words] to work only those Queued cards,
//     max: how many cards to build at once (default 3: disk and the shared screenshot box),
//     rounds: review rounds before a card goes to Needs you (default 3),
//     merge: false to stop at a green PR instead of merging,
//     land: false to skip landing earlier PRs, build: false to only land,
//     dryRun: true to list what would run and stop,
//   }

const A = args || {}
const NOW = A.now || 'unknown time'
const MAX = A.max || 3
const ROUNDS = A.rounds || 3
const MERGE = A.merge !== false
const BOARD = 'https://claude.ai/artifact/17baWQ1hwt3R194YbzjpWS'
const REPO = 'adammathes/skywave'

const CARD = {
  type: 'object',
  properties: {
    id: { type: 'string' }, title: { type: 'string' }, what: { type: 'string' },
    area: { type: 'string' }, note: { type: 'string' }, state: { type: 'string' },
    order: { type: 'number' }, status: { type: 'string' }, branch: { type: 'string' },
    pr: { type: 'number' },
  },
  required: ['id', 'title', 'status'],
}
const READ = {
  type: 'object',
  properties: {
    queued: { type: 'array', items: CARD },
    landing: { type: 'array', items: CARD },
  },
  required: ['queued', 'landing'],
}
const BUILT = {
  type: 'object',
  properties: {
    branch: { type: 'string' }, head: { type: 'string' },
    summary: { type: 'string', description: 'what changed, for the PR body and the card' },
    checks: { type: 'string', description: 'each verification command and its result' },
    risky: { type: 'boolean', description: 'touches sim, world generation, traffic/pedestrians or anything that could hurt someone' },
    blocked: { type: 'string', description: 'empty unless the card cannot be done without the owner; then why, plainly' },
  },
  required: ['branch', 'head', 'summary', 'checks', 'risky', 'blocked'],
}
const REVIEW = {
  type: 'object',
  properties: {
    verdict: { type: 'string', enum: ['merge', 'fix', 'owner'] },
    findings: { type: 'array', items: { type: 'string' }, description: 'each: file:line, what is wrong, the failing scenario with numbers' },
    followups: { type: 'array', items: { type: 'string' }, description: 'out-of-scope ideas worth a Proposed card' },
  },
  required: ['verdict', 'findings', 'followups'],
}
const SHIPPED = {
  type: 'object',
  properties: {
    pr: { type: 'number' }, head: { type: 'string' },
    ci: { type: 'string', enum: ['green', 'red', 'pending'] },
    failure: { type: 'string', description: 'the failing step and its key log lines when red' },
    merged: { type: 'boolean' }, sha: { type: 'string' },
  },
  required: ['pr', 'head', 'ci', 'failure', 'merged', 'sha'],
}

function board(id, fields, phase) {
  return agent(
    `Update one card on the Skywave owner's board with the ArtifactData tool (load it with ToolSearch "select:ArtifactData" if needed). ` +
    `Artifact url ${BOARD}, collection "items", doc "${id}". Read it with action "get" for its version, then action "update" with if_version and exactly these fields: ` +
    `${JSON.stringify(Object.assign({}, fields, { updated: NOW }))}. Also update doc "_meta" the same way with {"updated": ${JSON.stringify(NOW)}}. ` +
    `If a version check fails, re-read and retry once. Reply with the word done.`,
    { label: `board:${id}`, phase, effort: 'low' })
}

function propose(cardId, ideas, phase) {
  if (!ideas || !ideas.length) return Promise.resolve(null)
  return agent(
    `Add new Proposed cards to the Skywave owner's board with the ArtifactData tool (ToolSearch "select:ArtifactData"). Artifact url ${BOARD}, collection "items". ` +
    `For each idea below, action "set" a new doc whose id is a short kebab-case slug not already in the collection, with ` +
    `{"status":"proposed","order":500,"area":"follow-up","title": a short title,"what": the idea in plain words,"state":"from the work queue's review of ${cardId}","updated":${JSON.stringify(NOW)}}. ` +
    `Never set status to anything but "proposed". Ideas:\n- ${ideas.join('\n- ')}\nReply with the word done.`,
    { label: `propose:${cardId}`, phase, effort: 'low' })
}

const RULES =
  'Rules that bind every step: CLAUDE.md (no real names, zero violence, determinism, offline, minimal deps) and docs/HANDOFF.md ' +
  '("Verification before a PR", "Pitfalls"). Software-rendered screenshots are slow and must not run two at once on this machine: ' +
  'run them only as `flock /tmp/skywave-screenshots.lock node tools/screenshot/shoot.mjs ...` (and the same lock for diff.mjs). ' +
  'Use a CARGO_TARGET_DIR inside your own worktree, never a shared one; run `cargo clean --release` when you finish if disk is tight. ' +
  'Determinism hashes move only on purpose, in their own commit, with the reason. Commit messages: imperative, one line, body says why, ' +
  'no model names. Push only your own branch; never push to main.'

function buildPrompt(c, branch) {
  return `You are a Skywave builder. Do this task end to end on branch \`${branch}\`, created from the latest origin/main ` +
    `(git fetch origin main; git checkout -B ${branch} origin/main).\n\n` +
    `Task (the owner approved this card on the board, id ${c.id}): ${c.title}\n${c.what || ''}\n` +
    `${c.note ? `Owner's note on the card: ${c.note}\n` : ''}${c.state ? `Card state so far: ${c.state}\n` : ''}\n` +
    `Read docs/MILESTONES.md and the backlog or spec doc for this area first (docs/backlog/*), and follow the patterns of the code around you. ` +
    `${RULES}\n\nBefore you finish: run the full verification list from docs/HANDOFF.md (the screenshot selftest and golden diff once if the work is visual), ` +
    `fix everything it finds, commit in clean steps, and push the branch with \`git push -u origin ${branch}\`. Do NOT open a PR. ` +
    `If the work touches the vehicle sim, world generation, traffic or pedestrians, also write probe tests for the risks you can think of ` +
    `(e.g. person-steps inside any vehicle body at zero over a long seeded run) and include their numbers in \`checks\`. ` +
    `If the card cannot be done without a decision only the owner can make, stop, push what you have, and say why in \`blocked\`.`
}

function reviewPrompt(c, b, round) {
  return `You are an independent, adversarial, read-only reviewer for Skywave. Do NOT push, commit or edit the main checkout. ` +
    `Review \`git diff origin/main...origin/${b.branch}\` (head ${b.head}) for the card "${c.title}": ${c.what || ''}\n` +
    `Builder's summary: ${b.summary}\nBuilder's checks: ${b.checks}\n\n` +
    `Check: it does what the card asks and nothing more; CLAUDE.md rules (no real brand/place/street/station/model names, zero violence, ` +
    `determinism: no mul_add, no HashMap order, no platform RNG/clock in no_std crates; coast hashes and goldens unchanged unless the card is about them); ` +
    `correctness bugs with a realistic failing path; tests that prove the claims (a sweep that never exercises the risk proves nothing). ` +
    `${b.risky ? 'This change is risky (sim, world gen, traffic or pedestrians): write your OWN adversarial probe tests in a throwaway worktree ' +
      `(\`git worktree add <scratch>/wt-${c.id}-${round} origin/${b.branch}\`, its own CARGO_TARGET_DIR, removed afterwards) and try to break the guarantees; report measured numbers. ` : ''}` +
    `Skip style nits. verdict "merge" when nothing blocking is left; "fix" with concrete findings (file:line, scenario, numbers) when the builder must change something; ` +
    `"owner" only when the right fix is a product decision for the owner. Put good out-of-scope ideas in followups, not findings. This is review round ${round}.`
}

function fixPrompt(c, b, v) {
  return `You are the Skywave builder for card "${c.title}" (id ${c.id}). Your branch \`${b.branch}\` (head ${b.head}) came back from review with these blocking findings:\n- ` +
    `${v.findings.join('\n- ')}\n\nCheck out the branch (git fetch origin; git checkout -B ${b.branch} origin/${b.branch}), reproduce each finding first, fix it at its cause, ` +
    `add a test that fails without the fix, re-run the verification list from docs/HANDOFF.md, and push the branch (no force-push; merge commits are fine). ` +
    `${RULES} If a finding is wrong, say so with evidence in \`summary\` instead of changing code for it. Do NOT open a PR.`
}

function shipPrompt(c, b) {
  return `Ship branch \`${b.branch}\` of ${REPO} for the board card "${c.title}" (id ${c.id}). Use the GitHub MCP tools (ToolSearch "+github pull request").\n` +
    `1. If no open PR exists for the branch, open one into main: a plain title, and a body made of what changed (${b.summary}) and the verification (${b.checks}). ` +
    `Follow any PR template in the repo, end the body with the repo's usual attribution line, and attach screenshots for visual work as the builder's commits link them.\n` +
    `2. If the branch is behind main, merge origin/main into it (no rebase, no force-push), re-run cargo test for the crates it touches, and push.\n` +
    `3. Wait for the \`ci\` check run on the PR's current head to complete: call get_check_runs, and between calls do useful local work (re-run the crate tests) ` +
    `rather than idle. Give up after about 45 minutes and return ci "pending".\n` +
    `4. ${MERGE ? 'If ci is green and the branch is not behind main, merge with merge_method "merge" and expectedHeadSha set to the full head SHA; ' +
      'if main moved while you waited, merge main in again and wait once more. ' : 'Do not merge; stop at a green PR. '}` +
    `If ci is red, do not merge: put the failing step and its key log lines (get_job_logs) in \`failure\`. Return merged and sha accordingly.`
}

// One card from start to finish. Returns a line for the final report.
async function work(c) {
  const branch = c.branch || `q/${c.id}`
  await board(c.id, { status: 'building', branch, state: 'work queue: building' }, 'Build')
  let b = await agent(buildPrompt(c, branch), { label: `build:${c.id}`, phase: 'Build', schema: BUILT, isolation: 'worktree' })
  if (!b) {
    await board(c.id, { status: 'blocked', state: 'work queue: the builder died; run the queue again to retry' }, 'Build')
    return `${c.id}: builder failed`
  }
  if (b.blocked) {
    await board(c.id, { status: 'blocked', state: `needs you: ${b.blocked}` }, 'Build')
    return `${c.id}: needs the owner (${b.blocked})`
  }
  let v = null
  for (let r = 1; r <= ROUNDS; r++) {
    v = await agent(reviewPrompt(c, b, r), { label: `review:${c.id}:${r}`, phase: 'Review', schema: REVIEW })
    if (!v) { v = { verdict: 'fix', findings: ['the reviewer died before reporting; review again'], followups: [] }; continue }
    await propose(c.id, v.followups, 'Review')
    if (v.verdict !== 'fix') break
    await board(c.id, { state: `work queue: review round ${r} sent ${v.findings.length} finding(s) back` }, 'Review')
    const f = await agent(fixPrompt(c, b, v), { label: `fix:${c.id}:${r}`, phase: 'Review', schema: BUILT, isolation: 'worktree' })
    if (f) b = Object.assign({}, b, { head: f.head, summary: `${b.summary}\nReview round ${r}: ${f.summary}`, checks: f.checks })
  }
  if (!v || v.verdict !== 'merge') {
    const why = v && v.verdict === 'owner' ? 'a decision for you' : `still failing review after ${ROUNDS} rounds`
    await board(c.id, { status: 'blocked', state: `needs you: ${why}. ${(v && v.findings || []).join(' | ')}`.slice(0, 900) }, 'Review')
    return `${c.id}: ${why}`
  }
  return ship(c, b)
}

// Open (or find) the PR, wait for CI, merge on green; one fix attempt on red.
async function ship(c, b) {
  let s = await agent(shipPrompt(c, b), { label: `ship:${c.id}`, phase: 'Ship', schema: SHIPPED })
  if (s && s.ci === 'red') {
    await board(c.id, { status: 'review', pr: s.pr, state: 'work queue: CI red, fixing' }, 'Ship')
    const f = await agent(fixPrompt(c, b, { findings: [`CI failed on the PR: ${s.failure}`] }),
      { label: `ci-fix:${c.id}`, phase: 'Ship', schema: BUILT, isolation: 'worktree' })
    if (f) s = await agent(shipPrompt(c, Object.assign({}, b, { head: f.head })), { label: `reship:${c.id}`, phase: 'Ship', schema: SHIPPED })
  }
  if (!s) {
    await board(c.id, { status: 'review', state: 'work queue: shipping failed; run the queue again to land it' }, 'Ship')
    return `${c.id}: ship step failed`
  }
  if (s.merged) {
    await board(c.id, { status: 'merged', pr: s.pr, state: `merged (${s.sha.slice(0, 7)}) by the work queue` }, 'Ship')
    return `${c.id}: merged #${s.pr}`
  }
  const st = s.ci === 'red' ? 'blocked' : 'review'
  const msg = s.ci === 'red' ? `needs you: CI still red after one fix. ${s.failure}` : s.ci === 'pending' ? 'work queue: CI still running; the next run lands it' : 'work queue: green, waiting for your merge'
  await board(c.id, { status: st, pr: s.pr, state: msg.slice(0, 900) }, 'Ship')
  return `${c.id}: PR #${s.pr} ${s.ci}`
}

phase('Read')
const read = await agent(
  `Read the Skywave owner's board with the ArtifactData tool (ToolSearch "select:ArtifactData"): artifact url ${BOARD}, collection "items", ` +
  `action "list" with query.limit 200. Return two lists of cards (id, title, what, area, note, state, order, status, branch, pr): ` +
  `"queued" = every card whose status is exactly "queued", sorted by order ascending; ` +
  `"landing" = every card whose status is "review" and whose state starts with "work queue" and that has a pr number. ` +
  `Ignore every other status, especially "proposed". Do not change anything.`,
  { label: 'read-board', phase: 'Read', schema: READ, effort: 'low' })
if (!read) return { error: 'could not read the board' }

let queue = read.queued
if (A.items && A.items.length) {
  const want = A.items.map(s => String(s).toLowerCase())
  queue = queue.filter(c => want.some(w => c.id.toLowerCase() === w || c.title.toLowerCase().includes(w)))
  const missing = want.filter(w => !read.queued.some(c => c.id.toLowerCase() === w || c.title.toLowerCase().includes(w)))
  if (missing.length) log(`Not in Queued, so not started (approve them on the board first): ${missing.join(', ')}`)
}
const now = queue.slice(0, MAX)
if (queue.length > now.length) log(`Building ${now.length} of ${queue.length} queued cards this run (max ${MAX}); left for the next run: ${queue.slice(MAX).map(c => c.id).join(', ')}`)

if (A.dryRun) return { wouldLand: read.landing.map(c => `${c.id} (#${c.pr})`), wouldBuild: now.map(c => c.id), leftQueued: queue.slice(MAX).map(c => c.id) }

const report = []
if (A.land !== false && read.landing.length) {
  phase('Land')
  const landed = await pipeline(read.landing, c => ship(c, { branch: c.branch, head: '', summary: c.what || c.title, checks: 'see the PR' }))
  report.push(...landed.filter(Boolean))
}
if (A.build !== false && now.length) {
  const built = await pipeline(now, c => work(c))
  report.push(...built.filter(Boolean))
} else if (!now.length) {
  log('Nothing is Queued. Approve cards on the board to have them worked.')
}
return { report, leftQueued: queue.slice(MAX).map(c => c.id) }
