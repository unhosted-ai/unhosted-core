# Helmsman 4B v0.2 — rubric judgment (same 46 prompts, temp 0, Q4_K_M)

Judge: Claude (Fable 5), 2026-07-01. Same rubric as v0.1 judgment.

**Total: 32.5 / 46 (71%)** vs v0.1's 30/46 (65%). Mean 103 completion tokens, 0/46
truncated. Style preserved; targeted behaviors moved; raw arithmetic did not.

## Per-row scores (v0.1 in parens where changed)

1:1 (0 — FIXED: works first, commits "11 days" correctly)
2:0 (0 — still wrong: "4 x $600 = $240" arithmetic slip + threshold misread)
3:0 (0.5 — regression: sign error, concludes "backlog grows forever")
4:0.5 (0) 5:0.5 (0 — plan core now right: background sync + read-through + cutover)
6:0 (1 — regression: "8 weeks x 30 min/day = 192 hours, about 2 hours a day")
7:1 (0.5) 8:1 (0.5) 9:1 10:0.5 (1) 11:1 12:1 13:0.5 (1) 14:1 15:0.5 (1)
16:0.5 (0 — sees reviewer capacity 100 but muddles the orchestration)
17:1 (0.5) 18:1 (0.5) 19:1 20:0.5 (1 — "2%/hour is 12% in an hour")
21:0 (0 — still picks two juniors, self-contradicting rationale)
22:1 23:0.5 24:0.5 25:1 (0 — FIXED: sev-2 first, demo last, explicit rationale)
26:1 (0.5) 27:0.5 (0) 28:0.5 29:1 (0.5 — FIXED: owns "half the delay is ours" first)
30:1 (0.5) 31:0.5 (0.5 — NOT fixed: says "Kill" then describes fixing)
32:1 33:1 (0.5) 34:0.5 35:1 36:1 37:0.5 38:1 (explicit compare + flip condition — exemplary)
39:0.5 (1 — regression: names the framework, dodges the call) 40:0.5 (1)
41:0.5 42:0.5 (1) 43:0.5 (1) 44:1 45:1 46:1

## Read

- **Targeted data moved targeted behaviors.** Working-first ordering (#1), live-impact
  priority (#25), own-your-share comms (#29), decisive routing (#7, #8), delegation
  criteria (#18), verdict honesty to the board (#30) all improved. These were the gap
  classes the 59 new pairs encoded.
- **Arithmetic fidelity is a capacity ceiling, not a data-order problem.** The model now
  computes before answering (as trained) but the computations themselves err at 4B/Q4
  (#2 multiplication slip, #3 sign error, #6 unit nonsense, #20 rate confusion). More
  data won't fix this; a bigger base might — that's the 8B run's question.
- **Verdict-consistency pairs only half-landed** (#31 still fix-described-as-kill; #27
  verdict right but middle wobbles).
- **Regression noise** (#3, #6, #13, #15, #20, #39, #42, #43): expected when 27% of the
  dataset changed; nothing systematic.
- Style artifact: "So the answer is:" leaks into out-of-domain answers (#44) — harmless
  but visible.

## Standings so far

| model | score | mean tok | truncated |
|---|---|---|---|
| base Qwen3-4B @768 | 42.5/46 (92%) | 554 | 20/46 |
| base Qwen3-4B @512 | 39.5/46 (86%) | 424 | 29/46 |
| Helmsman v0.2 | 32.5/46 (71%) | 103 | 0 |
| Helmsman v0.1 | 30/46 (65%) | 90 | 0 |

Base @768 re-scoring (only truncation-affected rows can change at temp 0):
#3 0.5→1 (lands "4 hours"), #13 0.5→1 (final verdict: don't), #24 0.5→1,
#25 0.5→1 (all three priorities delivered, order right), #26 0.5→1,
#38 0.5→1 (correct math + verdict). #4, #8, #16 unchanged — #16 notably
still spirals on the 112-vs-100 overscrape loop and never lands a plan
even at 768.

**Honest conclusion for the model card:** given room to think, the untuned
base is simply stronger on correctness (92% vs 71%). Helmsman's real,
measurable value is delivery: 5x fewer tokens, ~5x faster wall-clock,
always finishes, consistent decisive voice, zero formatting sprawl — an
orchestration-styled specialist, not a smarter one. Correctness parity
would need a bigger base (8B pending) or a teacher whose reasoning the
student can actually execute.
