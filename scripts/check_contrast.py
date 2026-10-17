#!/usr/bin/env python3
"""Verify the DESIGN.md colour tokens and contrast claims.

DESIGN.md section 3 presents a table of contrast ratios and says they were
"computed". This script recomputes them from the token table in section 2 and
checks the table in section 3 against the result. It exists because a table of
computed numbers in a document is exactly the kind of claim that rots when a
token is edited, and because the document itself says "this check must be
repeated by CI on the real token file".

Method: WCAG 2.x relative luminance and contrast ratio, per the document's own
description. Self-validated against known reference pairs before it is trusted
to judge anything -- a checker that has never been shown to agree with a known
answer is not evidence.

    python scripts/check_contrast.py            # report
    python scripts/check_contrast.py --strict   # warnings fail too

Exit codes: 0 = no errors, 1 = errors found.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

TOKEN_ROW = re.compile(
    r"^\|\s*`--gtt-color-([a-z-]+)`\s*\|\s*`(#[0-9A-Fa-f]{6})`\s*\|\s*`(#[0-9A-Fa-f]{6})`\s*\|",
    re.MULTILINE,
)
CLAIM_ROW = re.compile(
    r"^\|\s*(light|dark)\s*\|\s*([^|]+?)\s*\|\s*([\d.]+)\s*\|\s*([\d.]+)\s*\|\s*(\w+)\s*\|\s*$",
    re.MULTILINE,
)

# WCAG 2.x thresholds.
AA_TEXT = 4.5
AA_NON_TEXT = 3.0

# Reference pairs the implementation is validated against before use.
REFERENCES = [
    ("#000000", "#FFFFFF", 21.00, 0.01),
    ("#767676", "#FFFFFF", 4.54, 0.01),
    ("#777777", "#FFFFFF", 4.48, 0.01),
]


def srgb_to_linear(channel: float) -> float:
    return channel / 12.92 if channel <= 0.04045 else ((channel + 0.055) / 1.055) ** 2.4


def luminance(hex_colour: str) -> float:
    h = hex_colour.lstrip("#")
    r, g, b = (int(h[i: i + 2], 16) / 255 for i in (0, 2, 4))
    return (
        0.2126 * srgb_to_linear(r)
        + 0.7152 * srgb_to_linear(g)
        + 0.0722 * srgb_to_linear(b)
    )


def ratio(fg: str, bg: str) -> float:
    a, b = luminance(fg), luminance(bg)
    hi, lo = max(a, b), min(a, b)
    return (hi + 0.05) / (lo + 0.05)


def self_test() -> list[str]:
    """Prove the implementation matches known reference values."""
    failures = []
    for fg, bg, expected, tol in REFERENCES:
        got = ratio(fg, bg)
        if abs(got - expected) > tol:
            failures.append(
                f"reference pair {fg} on {bg}: expected {expected}, got {got:.2f}"
            )
    return failures


# Pairs DESIGN.md section 3 claims, mapped to the tokens they compare.
# Written out explicitly rather than parsed from the prose so that a reworded
# table cannot silently change what is checked.
CLAIMED_PAIRS = {
    ("light", "text on bg"): ("text", "bg", AA_TEXT),
    ("light", "muted text on surface"): ("text-muted", "surface", AA_TEXT),
    ("light", "accent text on bg"): ("accent", "bg", AA_TEXT),
    ("light", "on-accent on accent"): ("on-accent", "accent", AA_TEXT),
    ("light", "ok on raised"): ("ok", "raised", AA_TEXT),
    ("light", "warn on raised"): ("warn", "raised", AA_TEXT),
    ("light", "error on raised"): ("error", "raised", AA_TEXT),
    ("light", "info on raised"): ("info", "raised", AA_TEXT),
    ("light", "neutral on raised"): ("neutral", "raised", AA_TEXT),
    ("light", "control border on bg"): ("control-border", "bg", AA_NON_TEXT),
    ("light", "focus ring on surface"): ("focus", "surface", AA_NON_TEXT),
    ("dark", "text on bg"): ("text", "bg", AA_TEXT),
    ("dark", "muted text on surface"): ("text-muted", "surface", AA_TEXT),
    ("dark", "accent text on bg"): ("accent", "bg", AA_TEXT),
    ("dark", "on-accent on accent"): ("on-accent", "accent", AA_TEXT),
    ("dark", "ok on raised"): ("ok", "raised", AA_TEXT),
    ("dark", "warn on raised"): ("warn", "raised", AA_TEXT),
    ("dark", "error on raised"): ("error", "raised", AA_TEXT),
    ("dark", "info on raised"): ("info", "raised", AA_TEXT),
    ("dark", "neutral on raised"): ("neutral", "raised", AA_TEXT),
    ("dark", "control border on bg"): ("control-border", "bg", AA_NON_TEXT),
    ("dark", "focus ring on surface"): ("focus", "surface", AA_NON_TEXT),
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--strict", action="store_true")
    args = ap.parse_args()

    errors: list[str] = []
    warnings: list[str] = []

    # 1. Validate the implementation before trusting it.
    ref_failures = self_test()
    if ref_failures:
        for f in ref_failures:
            errors.append(f"contrast implementation failed its own self-test: {f}")
        print("\n".join(f"ERROR {e}" for e in errors))
        print("\nFAILED: the checker is wrong, so nothing below can be trusted")
        return 1
    print("self-test: contrast formula matches all 3 WCAG reference pairs")

    text = (ROOT / "DESIGN.md").read_text(encoding="utf-8-sig")

    # 2. Read the tokens.
    tokens: dict[str, dict[str, str]] = {}
    for m in TOKEN_ROW.finditer(text):
        name, light, dark = m.group(1), m.group(2), m.group(3)
        tokens[name] = {"light": light.upper(), "dark": dark.upper()}
    if not tokens:
        errors.append("no colour tokens parsed from DESIGN.md section 2")
        print("ERROR " + errors[0])
        return 1
    print(f"tokens parsed: {len(tokens)}")

    # 3. Recompute every claimed pair and compare.
    claimed = {
        (m.group(1), m.group(2).strip().lower()): (m.group(3), m.group(4), m.group(5))
        for m in CLAIM_ROW.finditer(text)
    }
    print(f"claimed pairs: {len(claimed)}")

    recomputed = 0
    for key, (fg_name, bg_name, needed) in CLAIMED_PAIRS.items():
        theme, label = key
        if key not in claimed:
            errors.append(f"pair '{label}' ({theme}) is checked but missing from DESIGN.md section 3")
            continue
        stated_ratio, stated_needed, stated_result = claimed[key]
        if fg_name not in tokens or bg_name not in tokens:
            errors.append(f"pair '{label}' ({theme}) names an unknown token")
            continue
        got = ratio(tokens[fg_name][theme], tokens[bg_name][theme])
        recomputed += 1

        if abs(got - float(stated_ratio)) > 0.02:
            errors.append(
                f"{theme} '{label}': DESIGN.md states {stated_ratio} but the "
                f"computed value is {got:.2f}"
            )
        if abs(float(stated_needed) - needed) > 0.01:
            errors.append(
                f"{theme} '{label}': DESIGN.md states threshold {stated_needed}, "
                f"but {needed} applies to this pair"
            )
        actual_pass = got >= needed
        if (stated_result == "pass") != actual_pass:
            errors.append(
                f"{theme} '{label}': DESIGN.md says '{stated_result}' but "
                f"{got:.2f}:1 against a {needed}:1 threshold is "
                f"{'a pass' if actual_pass else 'a FAILURE'}"
            )
        elif not actual_pass:
            warnings.append(f"{theme} '{label}' is below its threshold ({got:.2f}:1)")

    # 4. Any pair in the document that we do not check is a coverage gap.
    for (theme, label) in claimed:
        if (theme, label) not in CLAIMED_PAIRS:
            errors.append(
                f"{theme} '{label}' appears in DESIGN.md section 3 but is not "
                f"covered by this checker"
            )

    print(f"pairs recomputed: {recomputed}")
    print()
    for w in warnings:
        print(f"WARN  {w}")
    for e in errors:
        print(f"ERROR {e}")

    if errors or (args.strict and warnings):
        print(f"\nFAILED: {len(errors)} error(s), {len(warnings)} warning(s)")
        return 1
    print(f"\nPASSED: all {recomputed} pairs recomputed and matched, 0 errors")
    return 0


if __name__ == "__main__":
    sys.exit(main())