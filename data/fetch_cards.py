#!/usr/bin/env python3
"""Fetch the Riftbound card list and write a compact cards.json used by the
server and client.

Cards come from the public Riftcodex API. Sets that Riftcodex doesn't have yet
(e.g. a just-released expansion) are filled in from Riot's official card
gallery at riftbound.leagueoflegends.com.

Usage: python3 data/fetch_cards.py            (downloads both, writes data/cards.json)
       python3 data/fetch_cards.py --offline  (rebuilds from the cached raw files)
"""
import html
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
RAW = os.path.join(HERE, "riftcodex_raw.json")
RIOT_RAW = os.path.join(HERE, "riot_gallery_raw.json")
OUT = os.path.join(HERE, "cards.json")
API = "https://api.riftcodex.com/cards?size=100&page={}"
GALLERY = "https://riftbound.leagueoflegends.com/en-us/card-gallery/"


def fetch_all():
    items, page = [], 1
    while True:
        # The API rejects Python's default user agent, so shell out to curl.
        body = subprocess.check_output(["curl", "-sf", API.format(page)])
        d = json.loads(body)
        items += d["items"]
        if page >= d["pages"]:
            break
        page += 1
    with open(RAW, "w") as f:
        json.dump(items, f)
    return items


def fetch_riot():
    """Card list embedded in the official card gallery page (Next.js page data)."""
    page = subprocess.check_output(["curl", "-sfL", "-A", "Mozilla/5.0", GALLERY]).decode()
    m = re.search(r'<script id="__NEXT_DATA__"[^>]*>(.*?)</script>', page, re.S)
    data = json.loads(m.group(1))
    items = None
    for blade in data["props"]["pageProps"]["page"]["blades"]:
        cards = (blade.get("cards") or {}).get("items")
        if cards and "publicCode" in cards[0]:
            items = cards
    if not items:
        raise SystemExit("card gallery: card list not found")
    with open(RIOT_RAW, "w") as f:
        json.dump(items, f)
    return items


def riot_card(c, champions, set_size):
    """Convert one Riot gallery card to the cards.json shape used for Riftcodex cards."""
    code = c["publicCode"].split("/")[0]
    types = c["cardType"].get("type") or []
    if types:
        kind = types[0]["label"]
    else:
        # A few gallery entries leave the type empty; the image description starts with it.
        m = re.match(r"^Riftbound (\w+):", c["cardImage"].get("accessibilityText", ""))
        kind = m.group(1) if m and m.group(1) != "None" else ""
    sup = (c["cardType"].get("superType") or [{}])[0].get("label", "")
    tags = [html.unescape(t) for t in (c.get("tags") or {}).get("tags") or []]
    name, sub = html.unescape(c["name"]), c.get("subtitle") and html.unescape(c["subtitle"])
    if kind == "Legend" and tags:
        # Legends carry only their title; the champion is the tag naming a champion unit.
        champ = next((t for t in tags if t in champions), tags[0])
        short, full = f"{champ}, {name}", f"{champ} - {name}"
    elif kind == "Unit" and sub:
        short, full = f"{name}, {sub}", f"{name} - {sub}"
    else:
        short, full = name, name
    body = (((c.get("text") or {}).get("richText") or {}).get("body") or "")
    text = html.unescape(re.sub(r"<[^>]+>", "", re.sub(r"<br\s*/?>", "", body))).strip()
    val = lambda k: c[k]["value"]["id"] if c.get(k) else None
    num = re.sub(r"\D", "", code.split("-")[1])
    overnumbered = code.split("-")[1].isdigit() and set_size and int(num) > set_size
    return {
        "code": code,
        "name": short,
        "full": full,
        "type": kind,
        "super": sup,
        "rarity": c["rarity"]["value"]["label"],
        "domains": [d["label"] for d in c["domain"]["values"]],
        "energy": val("energy"),
        "might": val("might"),
        "power": val("power"),
        "text": text,
        "tags": tags,
        "set": c["set"]["value"]["id"],
        "num": c.get("collectorNumber") or 0,
        "img": c["cardImage"]["url"],
        "land": c.get("orientation") == "landscape",
        "alt": code[-1].isalpha() or bool(overnumbered),
        "sig": False,
    }


ID_RE = re.compile(r"^([a-z]+)-((?:r|sp|t)?)(\d+)([a-z*]?)(?:-\d+)?$")


def card_code(rid):
    m = ID_RE.match(rid)
    if not m:
        return None
    s, prefix, num, var = m.groups()
    if var == "*":
        var = "s"
    if prefix == "r":
        n = "R" + num.zfill(2)
    elif prefix == "sp":
        n = "SP" + str(int(num))
    elif prefix == "t":
        n = "T" + num.zfill(2)
    else:
        n = num.zfill(3)
    return f"{s.upper()}-{n}{var}"


def clean_name(name):
    # "Vi - Piltover Enforcer (Signature)" -> base "Vi, Piltover Enforcer"
    base = re.sub(r"\s*\((Alternate Art|Signature|Overnumbered|Metal|Showcase|Promo|\d+)\)\s*$", "", name)
    base = re.sub(r"\s*\((Alternate Art|Signature|Overnumbered|Metal|Showcase|Promo)\)\s*", " ", base).strip()
    base = base.replace(" - ", ", ")
    return base


def main():
    items = json.load(open(RAW)) if "--offline" in sys.argv else fetch_all()
    out, seen = [], set()
    for i in items:
        code = card_code(i["riftbound_id"])
        if not code or code in seen:
            continue
        seen.add(code)
        c = i["classification"]
        a = i["attributes"]
        md = i.get("metadata") or {}
        out.append({
            "code": code,
            "name": clean_name(html.unescape(i["name"])),
            "full": html.unescape(i["name"]),
            "type": c["type"],
            "super": c["supertype"] or "",
            "rarity": c["rarity"] or "",
            "domains": [d for d in (c["domain"] or []) if d],
            "energy": a.get("energy"),
            "might": a.get("might"),
            "power": a.get("power"),
            # Riftcodex's plain text keeps HTML escapes like "&gt;" and "&quot;".
            "text": html.unescape((i["text"] or {}).get("plain") or ""),
            "tags": [html.unescape(t) for t in i.get("tags") or []],
            "set": i["set"]["set_id"],
            "num": i.get("collector_number") or 0,
            "img": (i["media"] or {}).get("image_url") or "",
            "land": i.get("orientation") == "landscape",
            "alt": bool(md.get("alternate_art") or md.get("overnumbered") or md.get("signature")) or code[-1].isalpha() and not code[-1].isdigit(),
            "sig": bool(md.get("signature")),
        })
    # Sets Riftcodex doesn't carry yet come from Riot's gallery.
    offline = "--offline" in sys.argv and os.path.exists(RIOT_RAW)
    riot = json.load(open(RIOT_RAW)) if offline else fetch_riot()
    have = {c["set"] for c in out}
    champions = {c["name"].split(", ")[0] for c in out if c["super"] == "Champion"}
    champions |= {c["name"] for c in riot if any(s.get("id") == "champion" for s in c["cardType"].get("superType") or [])}
    added = 0
    for c in riot:
        set_id = c["set"]["value"]["id"]
        if set_id in have:
            continue
        size = int(c["publicCode"].split("/")[1]) if "/" in c["publicCode"] else 0
        card = riot_card(c, champions, size)
        if card["code"] in seen:
            continue
        seen.add(card["code"])
        out.append(card)
        added += 1
    if added:
        print(f"added {added} cards from Riot's card gallery for sets Riftcodex lacks")
    out.sort(key=lambda x: x["code"])
    with open(OUT, "w") as f:
        json.dump(out, f, separators=(",", ":"), ensure_ascii=False)
    print(f"wrote {len(out)} cards to {OUT}")


if __name__ == "__main__":
    main()
