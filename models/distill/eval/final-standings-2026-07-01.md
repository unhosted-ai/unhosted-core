# Final standings — 46-prompt held-out orchestration eval (temp 0, Q4_K_M)

| # | model | score | mean tok | truncated | notes |
|---|---|---|---|---|---|
| 1 | base Qwen3-4B-Instruct-2507 @768 | 42.5 (92%) | 554 | 20/46 | verbose; reasoning-in-prose |
| 2 | base Qwen3-8B no-think @768 | 36.5 (79%) | 553 | 17/46 | premature-EOS quirk; hedges verdicts |
| 3 | **Helmsman 4B v0.2** | **32.5 (71%)** | **103** | **0** | **release pick** |
| 4 | Helmsman 4B v0.1 | 30 (65%) | 90 | 0 | superseded |
| 5 | Helmsman 8B 4-bit QLoRA t2 | 25.5 (55%) | 92 | 0 | quantize-roundtrip damage |
| 6 | Helmsman 8B bf16 LoRA | 24.5 (53%) | 160 | 6/46 | loops persist without roundtrip |
| 7 | Helmsman 8B 4-bit QLoRA t1 | 23 (50%) | 101 | 0 | degeneration loops |
| 8 | base Qwen3-8B thinking @768 | 0 answers | 708 | 39/46 | never exited <think>, 46/46 |

No-think 8B per-row: 1:0.5 2:0.5 3:0.5 4:0.5 5:0.5 6:1 7:1 8:1 9:0.5 10:1 11:1
12:1 13:1 14:0.5 15:0.5 16:0.5 17:1 18:1 19:0.5 20:1 21:1 22:1 23:0.5 24:0.5
25:1 26:1 27:1 28:0.5 29:0.5 30:1 31:1 32:1 33:1 34:0.5 35:0.5 36:0.5 37:1
38:1 39:0.5 40:1 41:0.5 42:1 43:1 44:1 45:1 46:1 = 36.5

## Findings that drive the release

1. **Helmsman 4B v0.2 is the release.** Best tuned model; 5x cheaper/faster than
   any base at acceptable correctness, always finishes, zero think-tokens.
2. **Base lineage beats size**: the dedicated non-thinking 4B-2507 instruct
   (92%) outscores the hybrid 8B with thinking disabled (79%). The 2507 was the
   right base choice all along.
3. **Raw hybrid-8B is unusable as a fast orchestrator**: with thinking on it
   delivered zero answers in a 768-token budget on all 46 prompts.
4. **4-bit QLoRA roundtrip (train-on-4bit → dequant → requant) is a quality
   trap**: both 8B takes scored below every 4B variant despite visible capacity
   advantages on isolated prompts (#21 senior-hire, clean second-order lists).
5. **bf16 8B result closes the 8B question: 24.5/46.** Skipping the quantize
   roundtrip did NOT fix the degeneration loops — three trained 8B configs
   (23 / 25.5 / 24.5) all land below both 4B versions. The instability is the
   under-trained-style + hybrid-thinking-base combination, not quantization.
   Capacity flashes are real (#2 contractor: only tuned model correct; #21
   senior hire: all three 8B takes correct, all 4B wrong) but don't survive
   the average. 8B revisit needs: more data (500+ pairs), a non-thinking 8B
   base if one ships, and repetition-penalty-aware eval.
   bf16-8B per-row: 1:0 2:1 3:0 4:0.5 5:0 6:1 7:1 8:0 9:1 10:0.5 11:1 12:0.5
   13:0 14:1 15:0 16:0 17:1 18:0 19:0.5 20:1 21:1 22:1 23:0.5 24:0.5 25:0
   26:0.5 27:1 28:0.5 29:1 30:1 31:0 32:0 33:1 34:0.5 35:0 36:1 37:0.5 38:0
   39:0.5 40:0.5 41:0 42:0.5 43:0 44:1 45:1 46:1 = 24.5

**RELEASE LINEUP (final): Helmsman 4B v0.2, sole release.** Installed in
LM Studio + Unhosted; model card rewritten with this table; weights private
per provenance ruling; recipe public in unhosted-core.

## Both capacity classes share unsolved failures

Zero-slack pipeline orchestration (#16: every variant either invents an
assumption or muddles the schedule) and committed-verdict consistency under a
fix-or-kill frame (#31) survive all training and all scales tested here —
prime v0.3 data targets.
