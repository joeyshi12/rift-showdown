#!/usr/bin/env python3
"""Turn Riot's published preconstructed deck lists into data/precons.json.

The deck list sections of Riot's announcements on playriftbound.com are saved
as text in data/precons/*.txt. Card names are matched to codes in data/cards.json; any
name that doesn't match exactly stops the build so nothing is guessed.

Usage: python3 data/build_precons.py
"""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "precons")
OUT = os.path.join(HERE, "precons.json")
NEWS = "https://playriftbound.com/en-us/news/announcements/"

cards = json.load(open(os.path.join(HERE, "cards.json")))
base = [c for c in cards if not c["alt"] and c["set"] not in ("OPP", "PR", "JDG")]


def norm(s):
    return s.replace("\u2019", "'").replace("\u00a0", " ").strip().lower()


problems = []

# Spelling slips in Riot's published lists. Each has exactly one card that matches,
# in the deck's domains, in both Riot's card gallery and Riftcodex.
CORRECTIONS = {
    "Evelyn, Entrancing": "Evelynn, Entrancing",  # Vex deck, UNL-141
    "Carion Dredger": "Carrion Dredger",  # Vi deck, UNL-153
    "Mageseeker, Investigator": "Mageseeker Investigator",  # Vi deck, UNL-163
}

# Decks left out because a listed card can't be identified.
SKIP = {
    "Zed": "lists 3 'Shadowcloaked Assassin', which is not a card in Riot's card gallery",
    "Seraphine": "lists Burn Away (RAD-124, Body), outside Seraphine's Mind/Order domains",
    "Evelynn": "lists Bushwhack Trap, Demon Shade and Elnuk Herd (Fury), outside Evelynn's Body/Chaos domains",
}


def by_name(name, kind=None):
    name = CORRECTIONS.get(name.strip(), name)
    hits = [c for c in base if norm(c["name"]) == norm(name) and (kind is None or c["type"] == kind)]
    if not hits:
        near = sorted({c["name"] for c in base if norm(c["name"]).split(",")[0][:4] == norm(name)[:4] and (kind is None or c["type"] == kind)})[:6]
        problems.append(f"no card named {name!r}" + (f" ({kind})" if kind else "") + f"; similar: {near}")
        return {"code": "?", "name": name, "type": kind or "", "super": "", "tags": [], "domains": []}
    # Same card printed in several sets: the earliest printing.
    hits.sort(key=lambda c: (c["code"].split("-")[0] not in ("OGN", "SFD", "UNL", "VEN", "RAD"), c["code"]))
    return hits[0]


def legend_by_title(title):
    t = norm(title)
    hits = [c for c in base if c["type"] == "Legend" and (norm(c["name"]) == t or norm(c["name"]).endswith(", " + t))]
    # Extra printings of one legend (same name and domains) count as one card.
    if len({(c["name"], tuple(c["domains"])) for c in hits}) != 1:
        raise SystemExit(f"legend {title!r}: {len(hits)} matches")
    return min(hits, key=lambda c: c["code"])


def rune(domain):
    hits = sorted((c for c in base if c["type"] == "Rune" and c["domains"] == [domain]), key=lambda c: (c["set"] != "OGN", c["code"]))
    return hits[0]


def parse_named(text, title, product, source):
    """Lists published as 'Legend: / Champion Options: / Main Deck: / Battlefields: / Runes:'."""
    deck = {"name": f"{title} {product.split(' ', 1)[1]}", "product": product, "source": NEWS + source, "champion_options": [], "main": [], "battlefields": [], "runes": []}
    section = None
    for line in text:
        low = line.lower().rstrip(":")
        if low.startswith("legend"):
            section = "legend"
            rest = line.split(":", 1)[1].strip() if ":" in line else ""
            if rest:
                deck["legend"] = legend_by_title(rest.split(", ", 1)[-1])["code"]
            continue
        if low in ("champion options", "champion unit options"):
            section = "champ"
            continue
        if low in ("main deck", "cards"):
            section = "main"
            continue
        if low == "battlefields":
            section = "bf"
            continue
        if low in ("rune pool", "runes"):
            section = "runes"
            continue
        if low == "tokens":
            section = "tokens"
            continue
        if section == "legend":
            deck["legend"] = legend_by_title(line)["code"]
            section = None
        elif section in ("champ", "main"):
            m = re.match(r"^(\d+)\s+(.+?)\s*$", line)
            n, name = int(m.group(1)), m.group(2)
            c = by_name(name, "Unit" if section == "champ" else None)
            deck["champion_options" if section == "champ" else "main"].append([c["code"], n])
        elif section == "bf":
            deck["battlefields"].append(by_name(line, "Battlefield")["code"])
        elif section == "runes":
            m = re.match(r"^(\d+)\s+(\w+)", line)
            deck["runes"].append([rune(m.group(2))["code"], int(m.group(1))])
    return deck


def parse_coded(text, title, product, source):
    """Lists published as ITEM / QTY / Card Number triples (Spiritforged)."""
    deck = {"name": f"{title} {product.split(' ', 1)[1]}", "product": product, "source": NEWS + source, "champion_options": [], "main": [], "battlefields": [], "runes": []}
    for name, qty, number in zip(text[0::3], text[1::3], text[2::3]):
        code = number.split("/")[0]
        c = next((x for x in base if x["code"] == code), None)
        if c is None:
            if "-T" in code:
                continue  # tokens aren't part of the deck list
            raise SystemExit(f"{title}: unknown card number {code}")
        if norm(c["name"].split(", ")[-1]) != norm(name.split(", ")[-1]) and norm(c["name"]) != norm(name):
            raise SystemExit(f"{title}: {code} is {c['name']!r}, list says {name!r}")
        n = int(qty)
        if c["type"] == "Legend":
            deck["legend"] = code
        elif c["type"] == "Battlefield":
            deck["battlefields"].append(code)
        elif c["type"] == "Rune":
            deck["runes"].append([code, n])
        elif c["super"] == "Champion" and c["tags"] and title in c["tags"]:
            deck["champion_options"].append([code, n])
        else:
            deck["main"].append([code, n])
    return deck


def lines(path):
    return [l.strip() for l in open(path) if l.strip()]


def between(ls, start, stop):
    i = ls.index(start) + 1
    j = next((k for k in range(i, len(ls)) if ls[k] in stop), len(ls))
    return ls[i:j]


decks = []
sf = lines(os.path.join(SRC, "spiritforged-precons-fiora-rumble.txt"))
rumble = between(sf, "Card Number", ["Fiora"])
fiora = between(sf[sf.index("Fiora") :], "Card Number", [])
decks.append(parse_coded(rumble, "Rumble", "Spiritforged Champion Deck", "spiritforged-precons-fiora-rumble/"))
decks.append(parse_coded(fiora, "Fiora", "Spiritforged Champion Deck", "spiritforged-precons-fiora-rumble/"))
vv = lines(os.path.join(SRC, "vi--vex-champion-decks.txt"))
vex_i = next(i for i, l in enumerate(vv) if l.startswith("Legend: Vex"))
vi_i = next(i for i, l in enumerate(vv) if l.startswith("Legend: Vi,"))
decks.append(parse_named(vv[vex_i : vi_i - 1], "Vex", "Unleashed Champion Deck", "vi--vex-champion-decks/"))
decks.append(parse_named(vv[vi_i:], "Vi", "Unleashed Champion Deck", "vi--vex-champion-decks/"))
for f, product, pairs in [("the-vendetta-overview.txt", "Vendetta Showdown Deck", ["Shen", "Zed"]), ("the-radiance-overview.txt", "Radiance Showdown Deck", ["Seraphine", "Evelynn"])]:
    ls = lines(os.path.join(SRC, f))
    for who in pairs:
        if who in SKIP:
            print(f"skipped {who}: {SKIP[who]}")
            continue
        i = ls.index(f"{who} Preconstructed Decklist") + 1
        j = next((k for k in range(i, len(ls)) if ls[k].endswith("Preconstructed Decklist")), len(ls))
        decks.append(parse_named(ls[i:j], who, product, f[:-4] + "/"))

if problems:
    raise SystemExit("\n".join(problems))
# Checks against the published product: 1 legend + 40 main (champions included) + 12 runes + 3 battlefields = 56.
for d in decks:
    main = sum(n for _, n in d["main"]) + sum(n for _, n in d["champion_options"])
    runes = sum(n for _, n in d["runes"])
    if "legend" not in d or main != 40 or runes != 12 or len(d["battlefields"]) != 3:
        raise SystemExit(f"{d['name']}: legend={d.get('legend')} main={main} runes={runes} battlefields={len(d['battlefields'])}")
# Every card must be in the legend's domains (or colorless), as the deckbuilding rules require.
code_of = {c["code"]: c for c in base}
for d in decks:
    allowed = set(code_of[d["legend"]]["domains"])
    for code, _ in d["champion_options"] + d["main"]:
        doms = [x for x in code_of[code]["domains"] if x != "Colorless"]
        if not set(doms) <= allowed:
            raise SystemExit(f"{d['name']}: {code_of[code]['name']} ({'/'.join(doms)}) is outside {'/'.join(sorted(allowed))}")
with open(OUT, "w") as f:
    json.dump(decks, f, indent=1)
print(f"wrote {len(decks)} decks to {OUT}")
