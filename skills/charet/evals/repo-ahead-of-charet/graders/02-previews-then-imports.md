---
type: llm
focus: trace
weight: 1
---

The answer checks the import scope with charet import --update --dry-run (without claiming it shows conflicts) and, only when drift was reported, then brings the changes in with charet import --update, expecting a both-sides-changed file to be refused there
