+++
title = "Documentation"
description = "How mediation models telecom usage: local-time arithmetic, integer quantities, validated identifiers, deduplication, completeness, correlation, the rule catalogue and the conservation law."
sort_by = "weight"
template = "docs/section.html"
page_template = "docs/page.html"
+++

Everything below is written for someone who has to make a billing run correct,
not for someone browsing an API. Each page states the failure it exists to
prevent, then shows the code that prevents it.

**Every Rust block on this site is compiled and run as a doctest** by the
crate's own test suite. A page that claims an API the crate does not have fails
the build, so what you read here is what the current release does — not what an
earlier one did.

## Where to start

If you are evaluating the crate, read [Getting started](@/docs/getting-started.md)
and then [The conservation law](@/docs/conservation.md) — the second is the claim
that separates a mediation layer from a data pipeline.

If you are integrating it, read [One pass, and the order it runs
in](@/docs/pipeline.md), which is the assembly every other page is a piece of.

If you are here because something is billing wrong, the three pages that most
often hold the answer are [Time is local, not UTC](@/docs/time.md),
[Completeness and sequence gaps](@/docs/completeness.md) and
[Deduplication](@/docs/deduplication.md).
