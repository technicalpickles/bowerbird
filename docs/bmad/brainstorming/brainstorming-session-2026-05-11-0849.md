---
stepsCompleted: [1]
inputDocuments:
  - docs/research/README-draft.md
  - docs/research/19-synthesis.md
session_topic: 'Name the project (placeholders: claude-state-bus, agent-state-bus)'
session_goals: 'Generate a strong shortlist of project name candidates that capture: local daemon + hook shim, observes coding agents, pub/sub substrate for pet/lamp/sprite/dashboard/voice presenters'
selected_approach: 'AI-recommended, categorical divergence informed by technicalpickles repo patterns'
techniques_used: ['portfolio-pattern-extraction', 'category-divergence']
ideas_generated: []
context_file: ''
---

# Brainstorming Session Results

**Facilitator:** pickles
**Date:** 2026-05-11

## Session Overview

**Topic:** Name the project. Current placeholders `claude-state-bus` and `agent-state-bus` both feel mid. Need a name that captures: local daemon + hook shim observing coding agents (Claude Code, Codex, Gemini, Cursor), exposing state via pub/sub WebSocket so pet/lamp/sprite/dashboard/voice tools can subscribe.

**Goals:** A shortlist (5-10 strong candidates) suitable for: GitHub repo, Homebrew tap, `cargo install`, daemon binary, brand/logo. Names should feel coherent with the design's substrate-not-app philosophy.

### Context Guidance

From `19-synthesis.md`, the design's North Star is "**preserve underlying data, resist modeling application-level concepts on top**" and "**constrain the core ferociously**." The name should *not* lean toward any one presenter (no `petbus`, no `lampbridge`). It should evoke: substrate, signal, ingestion-once, observe-don't-control.

Semantic seeds extracted from the docs:
- substrate, bus, conduit, channel, manifold
- shim, hook, tap, probe, sensor, watch
- pub/sub, broadcast, relay, fanout
- agent, daemon, sprite (in the unix sense), sentinel
- observability, telemetry, beacon
- small/opinionated/local

### Session Setup

User invoked /bmad-brainstorming with naming task + context pointers. Skipped heavy ceremony; setup compressed.

### User Constraints

- **Vibe:** look at technicalpickles' existing repos, derive categories, brainstorm within them
- **Namespace:** hard constraint (GitHub + crates.io + Homebrew availability must check out)
- **Cleverness:** lean clever
- **Avoid:** `claude-*` prefix (multi-agent), `agent-*` prefix (generic)
- **Length:** ≤10 chars ideal
- **Phonetics:** pronounceable as one word

### Naming Dialects Observed in technicalpickles' Repos

| Dialect | Examples | Vibe |
|---|---|---|
| Pickle/brine signature | `picklehome`, `pickled-claude-plugins`, `brineworks`, `dotpickles-private`, `pickledkms` | Personal brand, playful |
| Homesick pattern (one word, hidden meaning) | `homesick` (dotfiles), `slowpoke` (deliberately-slow rails), `bktide` (buildkite tide), `fitout` (context-aware plugin manager), `scope`, `envsense`, `the-investigator` (Expanse reference) | Clever, brandable, durable |
| Ultra-compact initials | `cq` (claude query), `sb` (second brain), `cenv` | Memorable, low-friction shell ergonomics |
| Literal compound | `agenticpets`, `agentic-container`, `welcome2u`, `cal2txt`, `town-charter` | Honest, low-creativity tax |
| RPG/lore reference | `pirpg`, `starforged-api`, `pickled-rpg-skills`, `pathfinder-2e-prd` | Personal interest, internal-facing |

## Category-Driven Brainstorm

### Category A — Pickle/brine signature (your house style)

The substrate preserves agent state the way brine preserves cucumbers. The pickle theme already maps to fermentation, preservation, and a vessel that many things share. Strong fit.

1. **brine** — pub/sub salt water; presenters bathe in it. 5 chars. The substrate as the liquid medium itself. Killer fit with `brineworks`.
2. **crock** — fermenting crock; everything happens inside the crock. 5 chars. Evokes both pickle vessel and "crock pot of state."
3. **cellar** — preservation room where jars (sessions) sit. 6 chars. Maps to "local daemon, lives in your basement."
4. **vat** — pickling vat; many cucumbers, one vessel. 3 chars. Probably too short / collision-prone.
5. **larder** — cold storage of preserved goods. 6 chars. Less common, distinctive.
6. **ferment** — what the substrate does to raw hook events (turns them into state). 7 chars.
7. **cure** — preserves; verb form is clean. 4 chars. Likely heavily taken.
8. **relish** — what presenters extract; everyone enjoys. 6 chars. Punny double meaning ("relish the event").
9. **briny** — adjective. 5 chars. Cute but soft.
10. **jar** — pickle jar; every session is a jar. 3 chars. Too short.
11. **dill** — short, pickle-coded. 4 chars. Risk of cute.
12. **kosher** — clean preservation; tongue-in-cheek "kosher signals." 6 chars. Possibly culturally loaded.
13. **pickld** — daemon-style truncation of "pickled." 6 chars. Reads as `pickled` aloud.
14. **brinepub** — brine + pub/sub. 8 chars. Functional + signature.
15. **crocket** — crock + socket. 7 chars. Punny on cricket. Maybe too cute.
16. **brindle** — brine + handle. 7 chars. Real word (the dog coat). Pronounceable.
17. **dilld** — daemon-style dill. 5 chars. Pun on "drilled."
18. **brinebus** — direct evolution of `state-bus`. 8 chars. Honest.
19. **picklery** — pickle + creamery. 8 chars. Place where pickling happens.
20. **mash** — early pickling step. 4 chars. Probably overused.
21. **gherkin-d** — no, gherkin is Cucumber BDD's territory.
22. **fermd** — fermentation daemon. 5 chars. Reads as "firmed."
23. **brinery** — preservation house. 7 chars. Lesser-used.
24. **pickleway** — too long; passable cousin to `homesick`.
25. **briney** — alt spelling. Skip.

**Strongest in category:** `brine`, `crock`, `cellar`, `brinery`, `pickld`

### Category B — Homesick pattern (one word, hidden tech meaning)

This is your durability sweet spot. Each name reads as English first, reveals the joke on second glance.

26. **earwig** — listens in on everything (literally "earwig" = eavesdropper). 6 chars. The shim has its ear on every hook. Available on GH? Probably collisions but worth checking.
27. **eaves** — eavesdrops on agents. 5 chars. Beautiful and slightly poetic.
28. **bystand** — a bystander observer; doesn't intervene. 7 chars. Maps perfectly to the substrate-not-actor philosophy.
29. **sidelined** — too long.
30. **lurker** — observes without participating. 6 chars. Slightly creepy but on-the-nose.
31. **bunker** — local daemon, hardened, holds state. 6 chars.
32. **bandstand** — too long but evocative (place where many performers play to one audience).
33. **switchboard** — too long but the metaphor is right (one operator, many lines).
34. **switch** — too generic.
35. **patchbay** — audio engineering term; routes signals between sources and destinations. 8 chars. **Strong technical metaphor.**
36. **junction** — too long (8), also generic.
37. **routine** — pun on routing + daemon routine. 7 chars. Too soft.
38. **busybee** — too cute.
39. **bellhop** — receives events, dispatches to subscribers. 7 chars. Punny on "bell" (push notification).
40. **doorbell** — every hook event is a doorbell ring. 8 chars.
41. **landline** — local-only telephone; pub/sub for ye olde localhost. 8 chars. Punny on "local."
42. **switchman** — too long, but the railway switchman is the perfect metaphor (routes trains/events to tracks/subscribers).
43. **dispatcher** — too long.
44. **pidgin** — homing pigeon; carries messages. 6 chars. Conflict: pidgin chat client owns the name.
45. **carrier** — pigeon vibes. 7 chars. Generic.
46. **homer** — homing pigeon for events. 5 chars. Conflict: homer (the website kit, the framework). Too taken.
47. **scry** — to observe distantly. 4 chars. Mystical. Probably available — cool.
48. **omen** — every event is an omen. 4 chars. Mystical, on-vibe with your starforged interests.
49. **belfry** — bell tower; broadcasts state. 6 chars. Underused, gorgeous.
50. **knell** — a bell signaling event. 5 chars. Slightly funereal.
51. **tally** — counts events; running tally of state. 5 chars. Underused.
52. **drift** — events drift through; sessions drift. 5 chars. Common but pretty.
53. **eddy** — quiet whirl where state collects. 4 chars. Beautiful, short, person-name-collision.
54. **pond** — substrate; pets (presenters) come to drink. 4 chars. Has Stripe vibes.
55. **bog** — preserves things naturally (peat bogs preserve bodies). 3 chars. Punny on debugging.
56. **moor** — anchors many; also a wetland. 4 chars.

**Strongest in category:** `earwig`, `eaves`, `patchbay`, `belfry`, `scry`, `bystand`

### Category C — Substrate/plumbing/observability metaphor

Honest functional metaphors. Slightly less clever but rock-solid.

57. **conduit** — the obvious one. 7 chars. Likely heavily taken on crates.io.
58. **manifold** — pub/sub manifold (also exhaust). 8 chars. Beautiful technical metaphor. Likely collisions.
59. **siphon** — extracts data from agents. 6 chars. Strong evocation.
60. **sluice** — channels water; controlled flow. 6 chars. Underused, distinctive.
61. **flume** — water channel. 5 chars. Apache Flume took this.
62. **wick** — capillary action; draws state up. 4 chars.
63. **tap** — hook tap; many subscribers can tap. 3 chars. Almost certainly conflict-heavy.
64. **spigot** — controlled tap. 6 chars. Punny + technical.
65. **gauge** — measures state. 5 chars. Generic.
66. **probe** — observes without interfering. 5 chars. Heavy use.
67. **lookout** — sentinel + verb. 7 chars.
68. **picket** — sentinel; near-pickle phonetic. 6 chars. Punny via pickle adjacency.
69. **crier** — town crier broadcasting. 5 chars. Beautiful, underused, evokes pub/sub.
70. **gossip** — gossip protocols (real CS term) for state sync. 6 chars. Familiar to distributed systems folks.
71. **herald** — announces state. 6 chars. RPG/myth flavor.
72. **echo** — bounce events back. 4 chars. Too taken (Amazon Echo, echo command).
73. **toll** — toll booth events go through; bell toll. 4 chars.
74. **hitch** — every hook is a hitch. 5 chars. Punny on "hook."
75. **latch** — listens for events. 5 chars.
76. **socket** — too literal/generic.
77. **harbour** — sessions dock here. 7 chars (or `harbor` at 6).
78. **wharf** — sessions dock. 5 chars.
79. **berth** — where a session moors. 5 chars.
80. **stevedore** — too long; loads/unloads cargo (events).
81. **dock** — too generic.
82. **lattice** — substrate. 7 chars. Many collisions.
83. **trellis** — substrate that things grow on. 7 chars. Underused.

**Strongest in category:** `siphon`, `sluice`, `crier`, `gossip`, `spigot`, `belfry` (already in B)

### Category D — Multi-agent / fanout / split-and-merge metaphor

Names that emphasize the "many in, many out" property.

84. **prism** — one stream in, many subscriber views out. 5 chars. Heavily taken (prism.js).
85. **lens** — focuses; refracts. 4 chars. Generic.
86. **weft** — the cross-threads of a weave; agent events woven across sessions. 4 chars. Beautiful, underused.
87. **warp** — same loom metaphor. 4 chars.
88. **loom** — weaves events. 4 chars. Loom.com problem.
89. **hub** — too generic.
90. **fanout** — actual CS term. 6 chars. Honest.
91. **fork** — split; also collision with `git fork`. Skip.
92. **mux** — multiplexer. 3 chars. Tmux conflict-zone.
93. **delta** — change-stream. 5 chars. Way overused.
94. **rebus** — collection of clues that resolve to a message; events that resolve to state. 5 chars. Underused, fun.
95. **chorus** — many sources, one heard outcome. 6 chars. Beautiful.
96. **mosaic** — fragments assemble. 6 chars.
97. **cairn** — pile of stones (events) building a meaning. 5 chars. Underused.
98. **strata** — layered substrate. 6 chars.

**Strongest in category:** `weft`, `chorus`, `rebus`, `fanout` (honest)

### Category E — Ultra-compact / initials (your `cq` / `sb` pattern)

99. **csb** — claude state bus. 3 chars. Bland.
100. **asb** — agent state bus. 3 chars. Bland.
101. **hsb** — hook state bus. Bland.
102. **xb** — cross-bus. Too cryptic.
103. **csbd** — claude state bus daemon. Worse.
104. **busb** — bus daemon. Worse.
105. **pop** — publish-once-process. 3 chars. Likely heavily collided.

**Verdict:** initials are weak here. Your `cq` and `sb` work because they're tools you use personally — substrate adoption by third parties is hurt by opaque initials.

### Category F — Surprise wildcards (orthogonal stretches)

106. **bridge** — too generic.
107. **plinth** — the base a statue stands on; substrate. 6 chars. Underused, distinctive.
108. **footing** — substrate; foundation. 7 chars.
109. **stoop** — local porch where agents sit. 5 chars. Charming.
110. **vestibule** — too long, but the entryway concept is right.
111. **antechamber** — too long.
112. **lobby** — central room; agents check in, presenters loiter. 5 chars.
113. **commons** — shared space. 7 chars.
114. **agora** — Greek public square. 5 chars. Beautiful, mythic, gels with the "many tools share one space" thesis.
115. **forum** — same idea. 5 chars. Generic, taken.
116. **kiosk** — broadcasts info; local. 5 chars.
117. **pulse** — signals over time. 5 chars. Generic.
118. **murmur** — soft constant chatter. 6 chars. Pretty.
119. **chatter** — quiet observable noise. 7 chars.

**Strongest in category:** `agora`, `plinth`, `stoop`, `murmur`

## Round 2 — Refined per user feedback

**Constraints adjusted:**
- Hyphenated compounds OK
- `brine` is a prefix, not a standalone
- Earwig *pattern* (quiet listener, hidden meaning) is good; insects are bad

### Category G — `brine-*` hyphenated dialect

The substrate is the brine; the suffix tells you the role.

120. **brine-tap** — subscribers tap the brine. Phone-tap eavesdrop echo. 9 chars.
121. **brine-bus** — direct pickle evolution of state-bus. 9 chars. Honest.
122. **brine-pub** — pub/sub flavor. 9 chars. Pun on bar/pub.
123. **brine-hub** — 9 chars. Less interesting than -tap.
124. **brine-cast** — broadcasts events. 10 chars.
125. **brine-vat** — pickling vat = the vessel. 9 chars. Storage-coded.
126. **brine-jar** — 9 chars. Slightly twee.
127. **brine-feed** — feed of events. 10 chars.
128. **brine-line** — telephone line of brine. 10 chars.
129. **brine-port** — port to plug presenters into. 10 chars. Audio-rack vibe.
130. **brine-wire** — wiretap echo. 10 chars.
131. **brine-shed** — local shed where state ages. 10 chars. Domestic, pickle-cellar coded.
132. **brine-watch** — observer flavor. 11 chars (over budget).
133. **brine-dock** — sessions dock. 10 chars.
134. **brine-loop** — event loop. 10 chars.
135. **brine-d** — daemon-style. 7 chars. Opaque.

**Strongest:** `brine-tap`, `brine-bus`, `brine-port`, `brine-shed`

### Category H — Earwig pattern, without the bug (eavesdropping place/observer)

Same vibe — quiet listener, hidden meaning — but the metaphor isn't an insect.

136. **earshot** — within earshot = local daemon, by definition. 7 chars. Strong.
137. **earful** — gets an earful from every hook. 6 chars. Slightly chatty connotation.
138. **earmark** — tags specific events for subscribers. 7 chars. Punny on event filtering.
139. **eaves** — eavesdrops on agents; from "eavesdropper." 5 chars. Pure.
140. **eavesdrop** — direct verb. 9 chars. Maybe too literal.
141. **peephole** — observes through. 8 chars. Slightly creepy.
142. **bystand** — bystander observer; doesn't intervene (substrate-not-actor!). 7 chars. **Very on-philosophy.**
143. **lurker** — observes without participating. 6 chars. Some creep factor.
144. **vigil** — watchful staying-awake; daemon-coded. 5 chars. **Strong.**
145. **outpost** — local listening post. 7 chars.
146. **picket** — sentry/sentinel; phonetic pickle adjacency. 6 chars. Bonus brand fit.
147. **belfry** — bell tower; broadcasts and observes. 6 chars.
148. **crow's-foot** — too long.
149. **crow** — observes, collects. 4 chars. Possibly too short.
150. **magpie** — collects every shiny event (matches the "preserve native payload verbatim" principle!). 6 chars. **Conceptually exact.**
151. **nightjar** — bird, nocturnal observer, contains "jar" (pickle adjacency by accident). 8 chars. Pretty.
152. **owl** — 3 chars; classic observer. Probably too short / common.
153. **chime** — bell on events. 5 chars.
154. **knell** — funereal but evocative bell. 5 chars.
155. **toll** — bell toll. 4 chars.

**Strongest:** `earshot`, `bystand`, `vigil`, `magpie`, `belfry`, `nightjar`

### Category I — Wiretap / surveillance family (lean into the eavesdrop)

156. **wiretap** — direct eavesdropping metaphor; sounds like a Unix tool. 7 chars. **Strong.**
157. **tapline** — line you tap. 7 chars.
158. **tipline** — events as tips coming in. 7 chars.
159. **sleuth** — detective. 6 chars.
160. **gumshoe** — detective. 7 chars. Slightly nostalgic.
161. **stakeout** — observes a target over time. 8 chars. Punny on `stake` (deps).
162. **dossier** — file kept on a session. 7 chars. Slightly heavy.
163. **bugnet** — too on-the-nose, also insect.
164. **snitch** — informs; observes and reports. 6 chars. Cute but loaded.
165. **dragnet** — collects everything. 7 chars. Strong, slightly noir.

**Strongest:** `wiretap`, `stakeout`, `dragnet`

### Category J — Patchbay / audio routing family (your locked-in winner)

If `patchbay` is hitting, names in the same neighborhood are worth a look.

166. **patchbay** — locked. 8 chars.
167. **crossbar** — telephone switching matrix. 8 chars. Same era of tech metaphor.
168. **pinboard** — pin events; visual board. 8 chars.
169. **aux-bus** — auxiliary send; audio engineering. 7 chars. Niche, technical.
170. **switchbay** — combines switchboard + patchbay. 9 chars. Maybe too compound.
171. **loopback** — networking metaphor for localhost-only. 8 chars. Slightly too literal.
172. **mainline** — telephone trunk. 8 chars.
173. **trunkline** — same. 9 chars.

**Strongest:** `patchbay` (locked), `crossbar`, `pinboard`

## Refined shortlist

After two rounds, the names that survive *all* your constraints (≤10 chars OR hyphenated, pronounceable, no claude-/agent-, lean clever, no bugs):

| # | Name | Reasoning |
|---|---|---|
| 1 | **patchbay** | Locked. Audio rack = literal substrate. 8 chars. |
| 2 | **brine-tap** | Pickle dialect + tap metaphor. Echoes wiretap. 9 chars. |
| 3 | **earshot** | "Within earshot" = local daemon. Listening metaphor without an insect. 7 chars. |
| 4 | **bystand** | Bystander observer = substrate-not-actor philosophy literally. 7 chars. |
| 5 | **magpie** | Collects every shiny event verbatim = preserve-native-payload principle. 6 chars. |
| 6 | **wiretap** | Direct eavesdropping; Unix-tool-sounding. 7 chars. |
| 7 | **vigil** | Watchful staying-awake; daemon-coded. 5 chars. |
| 8 | **brine-bus** | Direct evolution of `state-bus` with pickle dialect. 9 chars. |
| 9 | **belfry** | Bell tower; broadcasts; quiet observer. 6 chars. |
| 10 | **nightjar** | Nocturnal bird + accidental "jar" pickle adjacency. 8 chars. |

## Round 3 — Namespace verification + magpie-adjacent push

User feedback:
- `bystand` corrected to `bystander` (real word)
- `pickled-magpie` rejected (reads as dead bird in jar)
- Like the magpie energy, want adjacents

### Namespace findings (crates.io + GitHub exact-name)

| Name | crates.io | Top GH | Verdict |
|---|---|---|---|
| magpie | TAKEN (Othello, 23K dl) | Programming lang, ML paper | Bare name DEAD |
| patchbay | TAKEN (Linux netns, 6K dl) | (contested) | Bare name DEAD |
| earshot | TAKEN (VAD, 21K dl) | — | Bare name DEAD |
| wiretap | TAKEN (9K dl) | sandialabs/wiretap (1096★) | Bare name DEAD |
| vigil | TAKEN (20K dl) | munificent/vigil (3032★), valeriansaliou/vigil (1918★) | Bare name DEAD |
| jackdaw | TAKEN (Bevy editor) | skelsec/jackdaw (586★), FundingCircle/jackdaw (376★) | Bare name DEAD |
| **bystander** | **FREE** | jonhoo/bystander (30★, Jon Gjengset) | Clean, minor neighbor |
| **bowerbird** | **FREE** | ara3d (64★), ropensci (52★) | Clean, mild conflicts |
| **treepie** | **FREE** | 5★ max | Genuinely clean |
| **chough** | **FREE** | 15★ max | Clean, hard pronunciation |
| **corvid** | **FREE** | 69★ (Roblox plugin) | Decent |
| **corvidae** | **FREE** | unchecked | Clean, clinical-sounding |
| **magpie-d** | **FREE** | 0 | Clean, daemon convention |
| **magpie-bus** | **FREE** | 0 | Clean, honest evolution |
| **magpie-tap** | **FREE** | 0 | Clean, stacked metaphors |
| **magpie-hook** | **FREE** | 0 | Clean, function-clear |
| **magpie-pub** | **FREE** | 0 | Clean, pub/sub flavor |
| **brine-tap** | **FREE** | 0 | Clean, pickle dialect |
| **brine-bus** | **FREE** | 0 | Clean, pickle dialect |
| **belfry** | FREE | All 0★ | Clean enough |

### Three viable lanes after verification

**Lane A — Clean bare-name with corvid/collector concept**

- `bowerbird` (9 chars) — Bowerbirds collect bright objects and arrange them in their bower for display. That's `preserve-native-payload + presenter-display` literally encoded in a bird. Strongest concept-fit of all options. Namespace coexists with ara3d (3D library, 64★) and ropensci/bowerbird (R data-fetching package, 52★); different domains, not blocking.
- `treepie` (7 chars) — Asian corvid, magpie cousin, genuinely clean namespace. Unfamiliar word though; readers won't know it without explanation.

**Lane B — Salvage `magpie` with a hyphen**

- `magpie-d` (8 chars) — Reads as "magpie daemon" in unix style. Project = `magpie-d` repo, binary = `magpie-d`. Tight, dialect-respectful.
- `magpie-bus` (10 chars) — Honest evolution of `state-bus`. Reads as "magpie's bus."
- `magpie-tap` (10 chars) — Combines magpie + wiretap eavesdrop + audio tap. Densely metaphored.

**Lane C — Philosophy-first**

- `bystander` (9 chars) — Free on crates.io. Encodes substrate-not-actor principle literally. Coexists with Jon Gjengset's 30★ repo (visible Rust educator, mild namespace neighbor).

### Recommendation

If forced to pick one: **bowerbird**. It's the only name where the metaphor *exactly* matches the design's load-bearing principles (collects everything bright = preserves payloads verbatim; arranges in bower = presenters render). And the namespace, while not virgin, is uncrowded enough that you'd be the most prominent `bowerbird` in the dev-tooling space.

Runner-up: **magpie-d**. If you prefer keeping `magpie` as the through-line and the unix-daemon convention reads as on-brand for "a thing that ships as a Homebrew formula."

## Round 4 — Deep namespace verification for bowerbird

User signal: leaning toward `bowerbird`. Verified across all major registries.

### Verdict: CLEAN

| Registry | Status |
|---|---|
| crates.io (`bowerbird`) | FREE |
| crates.io (`bowerbird-shim`, `bowerbird-daemon`, `bowerbird-protocol`, `bowerbird-adapter`, `bowerbird-cli`, `bowerbird-core`, `bowerbird-rs`) | All FREE |
| npm | FREE |
| PyPI | FREE |
| Homebrew core formula | FREE |
| GitHub | 23 exact-name repos but none in AI/agent/observability/dev-tools space |

### GitHub neighborhood (top exact-name repos)

| Repo | Stars | Lang | Domain |
|---|---|---|---|
| ara3d/bowerbird | 64★ | C# | Revit (architecture CAD) plug-in framework |
| ropensci/bowerbird | 52★ | R | Scientific dataset collection |
| gever/bowerbird | 5★ | HTML | Pilot status tracking |
| bartekbrak/bowerbird | 2★ | Python | Logging formatters |
| nthloop/bowerbird | 2★ | JS | Chrome color extension |
| clatworthylab/bowerbird | 1★ | R | Single-cell genomics |
| Klithik/bowerbird | 1★ | Go | File organizer CLI |

**None overlap with this project's domain.** You'd be the most prominent dev-tools/AI `bowerbird` from launch.

### Project naming implications if committed to bowerbird

- Repo: `github.com/technicalpickles/bowerbird`
- Binary: `bowerbird` (full name); optional `bb` shell alias documented in install
- Crates: `bowerbird-protocol`, `bowerbird-shim`, `bowerbird-daemon`, `bowerbird-adapter-claude`
- Homebrew: `brew install bowerbird`
- Cargo: `cargo install bowerbird`
- Hook install command: `bowerbird install` (vs `claude-state-bus install`)
- Daemon command: `bowerbird daemon`
- Auth/state directory: `~/.bowerbird/`
- Default bind: unchanged, `127.0.0.1:9876`

### Metaphor extension (free vocabulary)

The bowerbird metaphor extends naturally without forcing:

- The SQLite event log = the **bower** (`~/.bowerbird/bower.db`)
- Subscribers = **visitors** (bowerbirds attract mates to their bowers)
- The 11-value reaction enum could be visualized as "what the bowerbird is doing right now"
- Adapters could be the bird's **collection sources** (where it gathers shiny things from)

None of this needs to be adopted — it's available if you want it.

### Final shortlist

| Rank | Name | Note |
|---|---|---|
| 1 | **bowerbird** | Clean namespace, concept-perfect, brandable |
| 2 | magpie-d | Salvages magpie via unix daemon convention |
| 3 | bystander | Philosophy-first, encodes substrate-not-actor |
| 4 | brine-tap | Pickle dialect winner |
| 5 | belfry | Bell tower observer, clean enough namespace |
