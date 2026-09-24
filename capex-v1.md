# CapEx v1 contract and acceptance

Status: v1 implementation revalidated after adversarial review fixed
child-isolation, explicit-skill selection, and immediate skill-dependency gaps.
Focused request-level regressions and a fresh pinned native/CapEx shared-session
matrix pass. The user authorized a build, not publication or deployment.
The executable planning definition is `capex.plan.pkl`; observations and decisions
belong in its sibling ledger. The binary evidence and explicit unsupported
cases are recorded in `capex-constraints.md`.

## Destination

Start Codex with a small skill catalog. Synchronous hooks can grant named
capabilities during the session. Each grant reveals capability instructions and
matching skill metadata on the next model request. Compaction and resume preserve
the effective selection without relying on a model-written summary.

Grants accumulate for the lifetime of the session. Automatic applicability decay,
revocation, task detection, asynchronous grants, and tool permission changes are
outside v1. Availability means the agent can consider a skill; capability prose
must state when its guidance applies. Accumulation can become noisy in a long
session; revisit decay after measuring that behavior.

## Selection

Use the existing skill discovery locations and precedence for skills. For the
first implementation, capabilities live in the selected project root's
`.agents/capabilities/<name>/CAPABILITY.md`. Additional capability roots and
`.codex/capabilities` aliases are deferred. A capability's directory name is its
stable ID; YAML frontmatter `tags` is a list of strings. Its remaining Markdown is
the instruction body. Skill tags use `metadata.tags`, a list of strings.

Configuration consists of:

```text
CAPEX_CAPABILITY_MODE=1
CAPEX_INITIAL_CAPABILITIES=baseline,engineering
CAPEX_ALWAYS_EXCLUDE_TAGS=video,design
```

Comma-separated lists trim surrounding whitespace, discard empty entries, and
deduplicate exact case-sensitive values. Tags match exactly, without globbing.
An absent mode preserves normal Codex behavior. Mode `1` enables filtering;
invalid nonempty mode values are configuration errors.

In capability mode, a skill is available iff it is otherwise enabled, has at least
one tag in the union of granted capability tags, and has no excluded tag. Untagged
skills stay hidden. Disabled plugins and skills remain disabled. Initial include
and exclude tag variables from the original sketch are omitted; use a baseline
capability instead. Explicit skill references obey the same availability filter
and explain an unavailable result rather than bypassing it.

Inject a granted capability's body independently of skill filtering. Excluding a
tag hides matching skills; it does not erase that capability's own instructions.
Selection affects guidance and discovery only, and is not a filesystem or tool
access restriction.

## Hook and session behavior

A synchronous `SessionStart` or `UserPromptSubmit` hook can add this field to
its normal JSON result (other hook events are deferred in the minimal build):

```json
{"capex":{"grant":["frontend"]}}
```

Accept grants only through the configured hook runtime. Ordinary tool output and
model prose cannot impersonate the envelope. Resolve each ID against the catalog;
report unknown or invalid IDs without granting them. Valid entries in the same
batch can proceed. Deduplicate repeated grants. Failed, timed-out, or asynchronous
hook executions cannot grant capabilities. Preserve existing hook block/stop
semantics; a grant never overrides them.

Persist each accepted grant before advertising it in a CapEx-owned, per-session
sidecar. Do not add rollout JSONL event variants or fields. Existing-shape
`ResponseItem::Message` lines may hold capability instructions, subject to
cross-runtime proof: upstream native Codex and CapEx must both open and continue
sessions written by the other. The sidecar is ignored by native Codex. Use only
existing rollout item shapes for ordinary model-visible context, and prove that
native Codex can continue after CapEx writes. CapEx must also load and continue
sessions after native Codex has added turns or compacted them. The
sidecar must be optional to ordinary Codex, atomically written, bounded, and
scoped by session ID. A missing ordinary-session sidecar falls back to persisted
configured initial grants; an unreadable sidecar or a missing fork sidecar fails
closed to no grants and reports a diagnostic. A resumed fork copies only
the ancestor grants known at the fork point; prove the cutoff in an integration
test before claiming fork support.

At a model request boundary,
fold accepted grants, derive the effective catalog, then deliver new capability
bodies and newly available skill metadata together. The available skill selector
and explicit invocation resolver must agree with this catalog. Skill bodies retain
normal lazy loading.

Pre-tool and post-tool grants are deferred. Requiring new guidance before an
already-issued tool operation would need a separate block-and-replan mechanism.

## Reconstruction

Compaction reconstructs capability instructions and the selected skill metadata
from recorded grants before the immediate continuation, including automatic
compaction inside a turn. Summaries neither create nor revoke grants. Repeated
compaction must not duplicate the current reconstructed block.

Resume restores recorded grants. Fork restores only grants present at its branch
point. A fresh or cleared session starts from configured initial capabilities.
Child agents receive no inherited dynamic grants in v1 and use their own initial
configuration; inherited conversation text must not become child runtime grants.

Core tests now verify the supported fork cutoff, including a prepared
reference-backed fork converted to a self-contained copied child and then
cold-resumed. Initial grants are effective from session creation, including a
fork before the first turn. For dynamic grants, the implementation selects
only harness-classified capability instruction items retained in a root fork's
history and requires a retained completed-turn boundary; it safely drops
dynamic grants without such evidence. Native compaction may discard that
classification, so full fork fidelity across a native-compacted history is
not claimed. Binary native/CapEx interoperability of the copied child remains
an explicit acceptance gate.

For a simple stable session, capture capability body and tags when granting and
persist that snapshot with its identity. Resume and compaction reuse the snapshot.
Refresh ordinary skill metadata through the existing catalog path; deleted or
disabled skills remain unavailable. Current exclusions apply on resume. Editing a
capability definition affects newly created sessions, not an existing grant.

## Delivery increments and evidence

These describe acceptance, not a second progress tracker. Expand them into nodes
with real source footprints and executable oracles after inspecting the chosen
Codex base.

| Increment | Required evidence |
| --- | --- |
| Catalog and selection | Mode off preserves baseline behavior; empty grants and untagged skills hide correctly; any-tag inclusion and deny precedence work across discovered skill sources; selectors and explicit invocation agree. |
| Persisted grants | Duplicate grants are idempotent; invalid grants do not activate; sidecar replay restores accepted snapshots; forks exclude events after the branch point; native Codex and CapEx can each open and continue the other's sessions from the same storage, including after compaction. |
| Hook delivery | A synchronous hook reveals instructions and metadata on the immediate next request; failed and asynchronous hooks cannot activate; normal tool text cannot activate; stop/block behavior is retained. |
| Reconstruction | Manual and mid-turn automatic compaction reconstruct the catalog and instructions; resume agrees; clear and child sessions follow their stated initialization rules. |
| Integrated proof | Start with baseline, grant frontend, inspect the next model request, repeat the grant, compact, resume, and verify the same selection without duplication; excluded and disabled skills stay hidden throughout. |

Use the upstream model-request test harness where available to assert actual
request contents without relying on an LLM to report its own skill list. Run the
selected repository's required checks and a final review against every row.

## Source inspection and compatibility boundary

The original implementation source was pinned to `openai/codex` main at
`8e17909b27875b76b1e9a883a604ed24e969d609`. For the maintained fork's
current upstream base and subsequent verification, see
`CAPEX_UPSTREAM_LEARNINGS_LEDGER.md` and `capex-constraints.md`.
The source inspection covered skill discovery/rendering/invocation, synchronous
hook result parsing, session event persistence/replay, request construction,
compaction, fork/clear, and child-agent initialization. Grant state lives in a
separate CapEx sidecar. Every rollout item CapEx emits must keep an existing
JSONL shape that the pinned native Codex can read and continue; both runtimes
must use one shared session home in the binary acceptance test.

If child history injection or explicit invocation cannot share the proposed
filter without expanding scope, record the measured conflict and revise the
contract before building downstream work. Do not silently weaken acceptance.

References: original `capex-design.md`; Workbench's proposed
[agent protocol](https://github.com/phosphorco/workbench-go/blob/main/docs/agent-protocol.md).
