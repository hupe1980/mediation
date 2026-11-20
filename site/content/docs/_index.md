+++
title = "Documentation"
description = "How mediation models telecom usage, one failure per page, with compiled examples."
sort_by = "weight"
template = "docs/section.html"
page_template = "docs/page.html"
+++

Each page states the failure it prevents and shows the code that prevents it.
Every Rust block on this site is compiled and run as a doctest.

- **Evaluating:** [Getting started](@/docs/getting-started.md), then
  [The conservation law](@/docs/conservation.md).
- **Integrating:** [One pass, and the order it runs in](@/docs/pipeline.md).
- **Fixed-line voice:** [Fixed-line voice](@/docs/fixed-voice.md).
- **Something bills wrong:** [Time](@/docs/time.md),
  [Completeness](@/docs/completeness.md), [Deduplication](@/docs/deduplication.md).
