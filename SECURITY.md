# Security policy

If you find a security issue, **do not open a public issue.**

Email **security@unhosted.dev** with:

- A description of the issue
- Steps to reproduce
- Affected versions
- Your proposed fix, if any

We aim to acknowledge within 72 hours and to ship a fix or coordinated disclosure plan within 14 days for confirmed issues.

## In scope

- The Unhosted runtime and CLI
- Anything that can leak prompts, model weights, or node identifiers
- Anything that lets a public-swarm node compromise a local-mode user
- Pairing/auth flows for trusted-peer mode

## Out of scope

- Vulnerabilities in upstream projects we depend on (llama.cpp, MLX, model files). Report those upstream — we'll track and pin.
- Issues that require physical access to a user's hardware.
- Theoretical attacks on stablecoin networks. Not our threat model.

## Credit

Researchers who report valid issues will be credited in release notes (with their permission). No bounty program at this stage — we're pre-revenue.
