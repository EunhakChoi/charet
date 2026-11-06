---
type: llm
focus: trace
weight: 1
---

The answer returns to the charet checkout and refreshes it after the import (charet pull, and charet branch catch-up first when the checkout is on a branch), and if catch-up names clashing files it settles them (or starts from a clean main checkout) instead of editing the unchanged branch head, before editing .charet/
