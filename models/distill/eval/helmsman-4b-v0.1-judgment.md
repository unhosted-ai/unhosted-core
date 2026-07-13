# Helmsman 4B v0.1 — rubric judgment (46 held-out prompts, temp 0, Q4_K_M)

Judge: Claude (Fable 5), 2026-07-01. Scoring: 1 = correct call, coherent; 0.5 = defensible
but flawed (missed key insight, garbled passage, weak coverage); 0 = wrong call or
self-contradictory.

**Total: 30 / 46 (65%)** — mean 90 completion tokens, 0/46 truncated.

## Per-row scores

| # | category | score | note |
|---|---|---|---|
| 1 | logic_math | 0 | headline "10 days" contradicts own working, which correctly derives 11 |
| 2 | logic_math | 0 | charges delay to the 5-day quote (finishes before day 6); picks $8,000 despite computing $10,100 < $10,400 |
| 3 | logic_math | 0.5 | headline "1.5 hours", then computes 4h, says "Wait, that's wrong", ends right |
| 4 | planning | 0 | "Week 4: launch the beta" contradicts the 6-week timeline; plan incoherent |
| 5 | planning | 0 | "sync Sunday 2am to 11pm ... synced by 6am" — nonsense; never finds pre-sync + delta cutover |
| 6 | planning | 1 | realistic scope, right priorities; minor redundancy |
| 7 | routing | 0.5 | security first (right), but press routed to HR (wrong owner) and ordering self-contradicts |
| 8 | routing | 0.5 | CI first (right); treats feature request as do-now; over-delegates intern question |
| 9 | routing | 1 | matches rigor to consequence; missing draft+verify combo but sound |
| 10 | summarizing | 1 | clean exec summary with decision framing |
| 11 | summarizing | 1 | exactly the one-liner |
| 12 | summarizing | 1 | right rec; invents "10x faster" (fabricated precision) — note, not fail |
| 13 | consequence_prediction | 1 | right call, decisive |
| 14 | consequence_prediction | 1 | real second-order effects |
| 15 | consequence_prediction | 1 | rubber-stamping + audit fix; point 4 muddled |
| 16 | multi_agent | 0 | invents "classifier flags 20% for review" to dodge the all-100-reviewed constraint; never sees 20×5=100 zero-slack bottleneck |
| 17 | multi_agent | 0.5 | lock fix works; misses ownership/partition root fix |
| 18 | multi_agent | 0.5 | hand-wavy; no bounded-retry / switch-cost criterion |
| 19 | constraints_tradeoffs | 1 | sacrifice cheap, well justified |
| 20 | constraints_tradeoffs | 1 | right workload classes; misses kill-rate compounding math |
| 21 | constraints_tradeoffs | 0 | picks two juniors; reasoning self-contradicts (senior "adds a person to the problem" but juniors need training from a drowning team) |
| 22 | ambiguity | 1 | asks the right questions incl. goal/decision |
| 23 | ambiguity | 0.5 | misses monitoring-first move; "can't reproduce → not your bug" is wrong-headed |
| 24 | ambiguity | 0.5 | "pause" ignores payment-is-current; misses build-approved-work / default-if-no-answer |
| 25 | prioritization | 0 | puts the live sev-2 LAST; "actively impacting users" then "urgent but not blocking" — self-contradiction |
| 26 | prioritization | 0.5 | sound choosing + honesty; closing aphorism is garbled nonsense |
| 27 | prioritization | 0 | "No." then "Ship it, but make the rollback easy" — self-contradiction on irreversible data corruption |
| 28 | communication_risk | 0.5 | omits what data leaked and user action; "handling this internally" reads evasive |
| 29 | communication_risk | 0.5 | invents causes instead of owning the team's half of the delay |
| 30 | communication_risk | 0.5 | core right (don't say fine) but "low-risk if done right" is itself the hedge, muddled |
| 31 | execution_process | 0.5 | headline "Kill." then describes fixing it — headline/body mismatch |
| 32 | execution_process | 1 | automate-first defensible; misses bus-factor-first but coherent |
| 33 | execution_process | 0.5 | "accept as signal" headline, push-back body; content right, verdict fuzzy |
| 34 | goals_learning | 0.5 | crude metric, thin loop |
| 35 | goals_learning | 1 | "would the plan have met the goal under normal execution" — good test |
| 36 | goals_learning | 1 | exclude-from-objectives-but-budget = right nuance |
| 37 | resource_stakeholder | 0.5 | circular criterion; wishy-washy message to the loser |
| 38 | resource_stakeholder | 1 | quantified: 832 h/yr vs one deal; ignores deal-certainty nuance |
| 39 | resource_stakeholder | 1 | Friday with justification; misses flag/ring third option |
| 40 | negotiation | 1 | real tactics; BATNA implicit |
| 41 | negotiation | 0.5 | options all variants of one; misses sign-on/re-level; garbled close |
| 42 | hiring_team | 1 | trial role + mentor + redirect |
| 43 | hiring_team | 1 | ownership transfer + burnout check |
| 44 | out_of_domain | 1 | Canberra |
| 45 | out_of_domain | 1 | correct, exactly two sentences |
| 46 | out_of_domain | 1 | coherent haiku (not strict 5-7-5) |

## Systematic failure patterns (ranked by frequency × severity)

**A. Headline-first commitment failure — the #1 gap.** The Opus training style states
the answer first, then reasons. Opus can back-solve; a 4B cannot. Result: wrong or
contradictory headlines over correct working (#1, #3, #25, #27, #31, #33) and
self-contradiction inside one answer (#7, #21, #27, #30). ~8 of 16 lost points trace here.
Fix in training data: for computational/decision prompts, show compact working FIRST,
then the committed answer — or an explicit verify-then-answer pattern.

**B. Quantitative constraint adherence.** Invents assumptions to dodge a binding
constraint (#16: invents "20% flagged" when 100% review is required), produces
schedule nonsense against a fixed window (#5), misapplies a threshold (#2 delay charged
to a finish-before-deadline quote). Fix: training pairs where the constraint is binding,
tight, and must be named and honored (zero-slack capacity, windows, thresholds).

**C. Final-comparison synthesis.** Computes correct subtotals then picks the wrong
winner (#2). Related to A. Fix: pairs ending in explicit "compare: X vs Y vs Z → pick".

**D. Live-incident priority.** Visibility (CEO demo) outranked live user impact (#25).
Fix: pairs encoding "live impact > deadline visibility > anticipated impact".

**E. Owning fault in communication.** Deflects to invented causes instead of owning the
team's stated share (#29). Fix: pairs where the speaker's side caused part of the problem
and the gold answer owns it specifically, first.

**F. Breadth gaps (0.5s):** monitoring-first triage on vague reports (#23), negotiation
option breadth (#41), crisp measurable goals (#34), decisive verdict when the content
already implies it (#31, #33).

## Strengths to preserve
Summarizing (3/3 clean), consequence prediction (3/3), hiring/team (2/2), most tradeoffs,
concision (mean 90 tok, zero truncation), out-of-domain retention fully intact (3/3).
