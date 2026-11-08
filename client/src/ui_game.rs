//! In-game HUD (Master Duel inspired): player plates, phase button, hand,
//! card detail panel, context menus, setup overlays, log/chat, badges,
//! pointer drag-and-drop and keyboard shortcuts.

use crate::app::*;
use crate::board::{card_img, img_url, side_class, Layout, CH};
use crate::dom::{self, by_id, esc, set_html};
use crate::icons::{icon, might};
use crate::math::v3;
use crate::ui_menu::{card_preview, chat_lines, rules_html};
use rift_shared::cards::CardDb;
use rift_shared::game::{team_name, Action, CardId, CardInst, GameState, Mode, Phase, Pos, Stage, Zone};
use rift_shared::session::{MatchAction, MatchStage};
use wasm_bindgen::JsCast;
use web_sys::{HtmlElement, KeyboardEvent, MouseEvent, PointerEvent};

fn shell() -> String {
    r#"<div class="hud">
<div id="g-top" class="g-top"></div>
<div id="g-opp" class="plate opp"></div>
<div id="g-others" class="others"></div>
<div id="g-me" class="plate me"></div>
<div id="g-opphand" class="opphand"></div>
<div id="g-reveal" class="reveal"></div>
<div id="g-hand" class="hand"></div>
<aside id="preview" class="g-preview"></aside>
<div id="g-phase" class="phase"></div>
<div id="g-tools" class="tools"></div>
<div id="g-chain" class="chain"></div>
<div id="g-banner" class="banner"></div>
<div id="g-target" class="targetbar"></div>
<aside class="sidepanel" id="g-sidepanel"><div id="g-tabs" class="tabs"></div><div id="g-log" class="log"></div>
<div id="g-chat" class="g-chat"><div id="chat-lines" class="chat-lines"></div><div class="chat-in"><input id="chat-input" maxlength="300" placeholder="Message" data-enter="chat-send" autocomplete="off"><button class="btn sm" data-act="chat-send">Send</button></div></div></aside>
<div id="g-menu"></div>
<div id="g-overlay"></div>
</div>"#
        .to_string()
}

fn name_of(db: &CardDb, c: &CardInst) -> String {
    db.get(&c.code).map(|d| d.name.clone()).unwrap_or_else(|| "Face-down card".into())
}

pub fn render(app: &mut App, force: bool) {
    if force || by_id("g-top").is_none() {
        dom::force_html("screen", &shell());
        let lines = chat_lines(&app.chat);
        dom::force_html("chat-lines", &lines);
    }
    let db = app.db.clone();
    let Some(g) = app.game.as_ref() else { return };
    let gs = &g.view.game;
    let me = g.me();
    let lay = g.layout();
    let opp = lay.other();
    let quad = lay.quad();
    let two = gs.num_seats() > 1 && !quad;

    // ---- top bar
    let phase = if gs.stage == Stage::Play { gs.phase.name() } else { "Setup" };
    let series = if g.view.config.format.wins_needed() > 1 { format!("<span class=\"pill\">Game {}, series {}</span>", g.view.game_no, g.view.wins.iter().map(|w| w.to_string()).collect::<Vec<_>>().join("–")) } else { String::new() };
    let acting = if g.is_local() && two {
        format!("<span class=\"pill\">Acting as <button class=\"btn sm {}\" data-act=\"acting\" data-arg=\"0\">{}</button><button class=\"btn sm {}\" data-act=\"acting\" data-arg=\"1\">{}</button></span>", if g.acting == 0 { "primary" } else { "ghost" }, esc(&gs.seats[0].name), if g.acting == 1 { "primary" } else { "ghost" }, esc(&gs.seats[1].name))
    } else {
        String::new()
    };
    let spec = if g.spectator() {
        "<span class=\"pill live\">Spectating</span>".to_string()
    } else if gs.seats.get(me as usize).map(|p| p.conceded).unwrap_or(false) && gs.stage != Stage::Over {
        "<span class=\"pill live\">You conceded</span>".to_string()
    } else {
        String::new()
    };
    let mode_pill = match gs.options.mode {
        Mode::Teams if quad => format!("<span class=\"pill\">2v2, {}</span>", team_name(gs.team(me))),
        Mode::FreeForAll | Mode::Skirmish if quad => "<span class=\"pill\">Free-for-all</span>".to_string(),
        _ => String::new(),
    };
    let top = format!(
        "<button class=\"ib\" data-act=\"game-menu\" title=\"Menu\">{mi}</button><button class=\"ib\" data-act=\"rewind\" title=\"Rewind the last action (Ctrl+Z)\" {rw}>{ui}</button><button class=\"ib\" data-act=\"forward\" title=\"Forward: undo your last Rewind (Ctrl+Y)\" {fw}>{ri}</button><span class=\"turn\">Turn <b>{turn}</b></span><span class=\"pill\">{phase}</span>{mode_pill}{series}{acting}{spec}<button class=\"ib\" data-act=\"help\" title=\"Help\">{hi}</button>",
        mi = icon("menu"),
        ui = icon("undo"),
        ri = icon("redo"),
        hi = icon("help"),
        rw = if g.view.can_rewind && !g.spectator() { "" } else { "disabled" },
        fw = if g.view.can_forward && !g.spectator() { "" } else { "disabled" },
        turn = gs.turn.max(1),
    );
    set_html("g-top", &top);

    // ---- plates
    let plate = |s: u8| -> String {
        let p = &gs.seats[s as usize];
        let runes = gs.pile(Zone::Runes(s));
        let ready = runes.iter().filter(|&&r| !gs.cards[r as usize].exhausted).count();
        let active = gs.stage == Stage::Play && gs.active == s;
        let wins = if g.view.config.format.wins_needed() > 1 { (0..2).map(|i| format!("<i class=\"wpip {}\"></i>", if g.view.wins.get(s as usize).copied().unwrap_or(0) > i { "on" } else { "" })).collect::<String>() } else { String::new() };
        format!(
            "<div class=\"pl-ava {act}\">{ini}</div><div class=\"pl-body\"><div class=\"pl-name\">{name}{wins}</div><div class=\"pl-score\" data-act=\"score-menu\" data-arg=\"{s}\" title=\"Adjust score\"><b>{score}</b><span>/ {target}</span></div><div class=\"pl-stats\"><span title=\"Cards in hand\">Hand <b>{hand}</b></span><span title=\"Main deck\">Deck <b>{deck}</b></span><span title=\"Ready runes / channeled runes\">Runes <b>{ready}/{rn}</b></span><span title=\"Trash\">Trash <b>{trash}</b></span></div></div>",
            act = if active { "active" } else { "" },
            ini = esc(&p.name.chars().next().unwrap_or('?').to_uppercase().to_string()),
            name = esc(&p.name),
            score = p.score,
            target = gs.options.win_points,
            hand = gs.pile(Zone::Hand(s)).len(),
            deck = gs.pile(Zone::Deck(s)).len(),
            rn = runes.len(),
            trash = gs.pile(Zone::Trash(s)).len(),
        )
    };
    set_html("g-me", &plate(me));
    set_html("g-opp", &if two { plate(opp) } else { String::new() });
    // Four players: compact plates for everyone else, in turn order after you.
    let others = if quad {
        let n = gs.num_seats();
        (1..n)
            .map(|k| (me + k) % n)
            .map(|s| {
                let p = &gs.seats[s as usize];
                let cls = side_class(&lay, s);
                let active = gs.stage == Stage::Play && gs.active == s;
                let ally = cls == "ally";
                let runes = gs.pile(Zone::Runes(s));
                let ready = runes.iter().filter(|&&r| !gs.cards[r as usize].exhausted).count();
                let tag = if ally { "<span class=\"tag\">Teammate</span>" } else if p.conceded { "<span class=\"tag\">Conceded</span>" } else { "" };
                // The 1v1 hand strip isn't drawn here, so a teammate's hand, a shown hand or
                // single revealed cards open from the plate instead.
                let hand_ids = gs.pile(Zone::Hand(s));
                let seen = hand_ids.iter().filter(|&&id| !gs.cards[id as usize].code.is_empty()).count();
                let view_hand = if (ally && !g.spectator()) || seen > 0 {
                    let (cls, label) = if p.hand_shown { ("gold", "Hand shown".to_string()) } else if ally { ("", "Hand".to_string()) } else { ("gold", format!("{seen} revealed")) };
                    format!("<button class=\"btn sm {cls}\" data-act=\"z-view\" data-arg=\"{}\">{label}</button>", esc(&zone_key(Zone::Hand(s))))
                } else {
                    String::new()
                };
                // Shown or revealed cards sit right under the plate (hover for details,
                // click for the full hand); a teammate's whole hand stays behind its button.
                let strip: String = if ally {
                    String::new()
                } else {
                    hand_ids
                        .iter()
                        .map(|&id| &gs.cards[id as usize])
                        .filter(|c| !c.code.is_empty())
                        .filter_map(|c| card_img(c, &db, 200).map(|src| format!("<img class=\"mh\" src=\"{src}\" data-code=\"{}\" data-act=\"z-view\" data-arg=\"{}\" alt=\"\">", esc(&c.code), esc(&zone_key(Zone::Hand(s))))))
                        .collect()
                };
                let strip = if strip.is_empty() { String::new() } else { format!("<div class=\"mini-hand\">{strip}</div>") };
                format!(
                    "<div class=\"mini-wrap\"><div class=\"plate mini {cls}\"><div class=\"pl-ava {act}\">{ini}</div><div class=\"pl-body\"><div class=\"pl-name\">{name}{tag}</div><div class=\"pl-score\" data-act=\"score-menu\" data-arg=\"{s}\" title=\"Adjust score\"><b>{score}</b><span>/ {target}</span></div><div class=\"pl-stats\"><span>Hand <b>{hand}</b></span><span>Deck <b>{deck}</b></span><span>Runes <b>{ready}/{rn}</b></span>{view_hand}</div></div></div>{strip}</div>",
                    act = if active { "active" } else { "" },
                    ini = esc(&p.name.chars().next().unwrap_or('?').to_uppercase().to_string()),
                    name = esc(&p.name),
                    score = p.score,
                    target = gs.options.win_points,
                    hand = gs.pile(Zone::Hand(s)).len(),
                    deck = gs.pile(Zone::Deck(s)).len(),
                    rn = runes.len(),
                )
            })
            .collect::<String>()
    } else {
        String::new()
    };
    set_html("g-others", &others);
    if let Some(e) = by_id("g-others") {
        // The plate count lets the CSS put the showdown banner below the plates on phones.
        let strip = if others.contains("class=\"mini-hand\"") { " strip" } else { "" };
        let _ = e.set_attribute("class", &format!("others n{}{strip}", if quad { gs.num_seats() - 1 } else { 0 }));
    }

    // ---- hands
    let hand_ids = gs.pile(Zone::Hand(me)).to_vec();
    let n = hand_ids.len().max(1) as f64;
    let mut hand = String::new();
    for (i, id) in hand_ids.iter().enumerate() {
        let c = &gs.cards[*id as usize];
        let off = i as f64 - (n - 1.0) / 2.0;
        let rot = off * (14.0 / n).min(4.0);
        let lift = off.abs() * off.abs() * (6.0 / n).min(2.0);
        let sel = g.selected == Some(*id) || g.mull_sel.contains(id);
        let img = card_img(c, &db, 300).unwrap_or_default();
        hand += &format!(
            "<div class=\"hc {sel} {shown}\" data-hand=\"{id}\" data-code=\"{code}\" style=\"--r:{rot:.2}deg;--y:{lift:.1}px;--n1:{n1}\">{inner}</div>",
            n1 = (n - 1.0).max(1.0),
            sel = if sel { "sel" } else { "" },
            shown = if c.shown || gs.seats[me as usize].hand_shown { "shown" } else { "" },
            code = esc(&c.code),
            inner = if img.is_empty() { "<div class=\"back\"></div>".to_string() } else { format!("<img src=\"{img}\" draggable=\"false\">") },
        );
    }
    set_html("g-hand", &hand);
    let mut oh = String::new();
    if two {
        for id in gs.pile(Zone::Hand(opp)) {
            let c = &gs.cards[*id as usize];
            match card_img(c, &db, 200) {
                Some(src) if !c.code.is_empty() => oh += &format!("<div class=\"ohc\" data-code=\"{}\"><img src=\"{}\"></div>", esc(&c.code), src),
                _ => oh += "<div class=\"ohc\"><div class=\"back\"></div></div>",
            }
        }
    }
    set_html("g-opphand", &oh);

    // ---- revealed cards
    let mut rv = String::new();
    for s in 0..gs.num_seats() {
        for id in gs.pile(Zone::Reveal(s)) {
            let c = &gs.cards[*id as usize];
            rv += &format!("<div class=\"rvc\" data-act=\"card\" data-arg=\"{id}\" data-code=\"{}\"><img src=\"{}\"><span>{}</span></div>", esc(&c.code), card_img(c, &db, 200).unwrap_or_default(), esc(&gs.seats[s as usize].name));
        }
    }
    set_html("g-reveal", &if rv.is_empty() { String::new() } else { format!("<div class=\"rv-t\">Revealed</div>{rv}") });

    // ---- chain
    let chain = gs.pile(Zone::Chain);
    let ch = if chain.is_empty() {
        String::new()
    } else {
        let items: String = chain.iter().rev().enumerate().map(|(i, id)| {
            let c = &gs.cards[*id as usize];
            format!("<div class=\"chn\" data-act=\"card\" data-arg=\"{id}\" data-code=\"{}\"><span class=\"chn-n\">{}</span><img src=\"{}\"><span class=\"chn-name\">{}</span></div>", esc(&c.code), chain.len() - i, card_img(c, &db, 200).unwrap_or_default(), esc(&name_of(&db, c)))
        }).collect();
        format!("<div class=\"chain-t\">Chain</div>{items}<button class=\"btn gold sm\" data-act=\"approve-chain\">Resolve top</button>")
    };
    set_html("g-chain", &ch);

    // ---- banner (showdown)
    let banner = match &gs.showdown {
        Some(sd) => format!(
            "<div class=\"sd\"><b>{}</b> at Battlefield {}. <b>{}</b> has focus.<button class=\"btn sm\" data-act=\"pass-focus\">Pass focus</button><button class=\"btn sm gold\" data-act=\"end-showdown\">{}</button></div>",
            if sd.combat { "Combat" } else { "Showdown" },
            (b'A' + sd.bf) as char,
            esc(&gs.seats[sd.focus as usize].name),
            if sd.combat { "Resolve combat" } else { "End showdown" }
        ),
        None => String::new(),
    };
    set_html("g-banner", &banner);

    // ---- pending target choice
    let tbar = match &g.targeting {
        Some(t) => {
            let name = gs.card(t.card).map(|c| name_of(&db, c)).unwrap_or_default();
            format!("<div class=\"tg\"><span>Choose where to {} <b>{}</b>. Click a highlighted area.</span><button class=\"btn sm\" data-act=\"target-cancel\">Cancel</button></div>", t.kind.verb(), esc(&name))
        }
        None => String::new(),
    };
    set_html("g-target", &tbar);

    // ---- phase button
    let my_turn = gs.active == me || (g.is_local() && g.view.config.solo);
    let phase_html = if gs.stage == Stage::Play {
        let list: String = Phase::ALL.iter().map(|p| format!("<span class=\"ph {}\">{}</span>", if *p == gs.phase { "on" } else { "" }, p.name())).collect();
        let (label, act) = if !my_turn {
            (match side_class(&lay, gs.active) {
                "ally" => "Teammate's turn".to_string(),
                _ if quad => format!("{}'s turn", gs.seats[gs.active as usize].name),
                _ => "Opponent's turn".to_string(),
            }, "")
        } else if gs.phase == Phase::Action {
            ("End turn".to_string(), "end-turn")
        } else {
            (format!("Next: {}", next_phase(gs.phase).name()), "phase-next")
        };
        format!(
            "<div class=\"ph-list\">{list}</div><button class=\"ph-btn {cls}\" data-act=\"{act}\" {dis}><span class=\"ph-cur\">{cur}</span><span class=\"ph-lbl\">{label}</span></button><div class=\"ph-who\">{who}</div>",
            cls = if my_turn { "mine" } else { "theirs" },
            dis = if my_turn && !g.spectator() { "" } else { "disabled" },
            cur = gs.phase.name(),
            who = esc(&gs.seats[gs.active as usize].name),
        )
    } else {
        String::new()
    };
    set_html("g-phase", &phase_html);

    // ---- tools
    let tools = if g.spectator() {
        String::new()
    } else {
        let t = |act: &str, ic: &str, label: &str, key: &str, on: bool| format!("<button class=\"tool {}\" data-act=\"{act}\" title=\"{label} {key}\"><span class=\"ti\">{}</span><span class=\"tl\">{label}</span></button>", if on { "on" } else { "" }, icon(ic));
        [
            t("z-draw", "draw", "Draw", "(D)", false),
            t("z-channel", "channel", "Channel", "(C)", false),
            t("ready-runes", "ready", "Ready all", "", false),
            t("z-shuffle", "shuffle", "Shuffle", "", false),
            t("roll", "dice", "d20", "(R)", false),
            t("tokens", "token", "Tokens", "(T)", false),
            t("toggle-autopay", "pay", "Auto-pay", "", g.autopay),
            t("toggle-reveal-hand", "eye", "Show hand", "", gs.seats[me as usize].hand_shown),
        ]
        .concat()
    };
    set_html("g-tools", &tools);

    // ---- side panel
    let unread = if g.unread > 0 && g.panel != Panel::Chat { format!(" <b class=\"badge\">{}</b>", g.unread) } else { String::new() };
    let tabs = format!(
        "<button class=\"{}\" data-act=\"panel\" data-arg=\"log\">Duel log</button><button class=\"{}\" data-act=\"panel\" data-arg=\"chat\">Chat{unread}</button><button data-act=\"panel\" data-arg=\"hide\">{}</button>",
        if g.panel == Panel::Log { "on" } else { "" },
        if g.panel == Panel::Chat { "on" } else { "" },
        if g.panel == Panel::Hidden { "Show" } else { "Hide" }
    );
    set_html("g-tabs", &tabs);
    if let Some(p) = by_id("g-sidepanel") {
        let _ = p.set_attribute("class", match g.panel {
            Panel::Log => "sidepanel log-on",
            Panel::Chat => "sidepanel chat-on",
            Panel::Hidden => "sidepanel hidden",
        });
    }
    // Oldest first, newest at the bottom, like the chat.
    let skip = gs.log.len().saturating_sub(150);
    let log: String = gs.log.iter().skip(skip).map(|l| {
        let who = l.seat.map(|s| format!("<b class=\"s-{}\">{}</b> ", side_class(&lay, s), esc(&gs.seats.get(s as usize).map(|p| p.name.clone()).unwrap_or_default()))).unwrap_or_default();
        let text = if who.is_empty() { l.text.clone() } else { lower_first(&l.text) };
        format!("<div class=\"ll\"><span class=\"lt\">T{}</span>{who}{}</div>", l.turn, esc(&text))
    }).collect();
    set_log(&log);

    // ---- preview
    let pv = if matches!(gs.stage, Stage::Play) { preview_html(&db, g) } else { String::new() };
    set_html("preview", &pv);

    // ---- menu + overlays
    let menu = g.menu.as_ref().map(|m| menu_html(&db, g, m)).unwrap_or_default();
    set_html("g-menu", &menu);
    let ov = overlay_html(&db, g);
    set_html("g-overlay", &ov);
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn next_phase(p: Phase) -> Phase {
    match p {
        Phase::Awaken => Phase::Beginning,
        Phase::Beginning => Phase::Channel,
        Phase::Channel => Phase::Draw,
        Phase::Draw => Phase::Action,
        Phase::Action => Phase::End,
        Phase::End => Phase::Awaken,
    }
}

fn preview_html(db: &CardDb, g: &GameUi) -> String {
    let gs = &g.view.game;
    let id = g.hover.or(g.selected);
    if let Some(id) = id {
        let Some(c) = gs.card(id) else { return String::new() };
        if c.code.is_empty() {
            return format!("<div class=\"pv\"><div class=\"pv-back\"></div><div class=\"pv-name\">Hidden card</div><div class=\"pv-text\">{}: {}</div></div>", esc(&gs.seats[c.owner as usize].name), c.zone.label());
        }
        let mut extra = String::new();
        let mut tags = vec![];
        if c.exhausted {
            tags.push("Exhausted".to_string());
        }
        if c.face_down {
            tags.push("Hidden (face down)".into());
        }
        if c.damage > 0 {
            tags.push(format!("{} damage", c.damage));
        }
        if c.buffs > 0 {
            tags.push(format!("+{} buff", c.buffs));
        }
        if c.counters != 0 {
            tags.push(format!("{} counters", c.counters));
        }
        if c.stunned {
            tags.push("Stunned".into());
        }
        if c.token {
            tags.push("Token".into());
        }
        if !tags.is_empty() {
            extra = format!("<div class=\"pv-state\">{}</div>", tags.iter().map(|t| format!("<span>{}</span>", esc(t))).collect::<String>());
        }
        return card_preview(db, &c.code, &extra);
    }
    if let Some(code) = &g.hover_code {
        return card_preview(db, code, "");
    }
    String::new()
}

/// Update the duel log and keep it pinned to the newest line, unless the
/// player scrolled up to read older lines. Opening the log tab starts at the bottom.
fn set_log(html: &str) {
    let Some(el) = by_id("g-log") else { return };
    let visible = el.client_height() > 0;
    if !visible {
        let _ = el.remove_attribute("data-shown");
        set_html("g-log", html);
        return;
    }
    let first_view = el.get_attribute("data-shown").is_none();
    let at_bottom = el.scroll_top() + el.client_height() >= el.scroll_height() - 30;
    set_html("g-log", html);
    if first_view || at_bottom {
        el.set_scroll_top(el.scroll_height());
    }
    let _ = el.set_attribute("data-shown", "1");
}

/// "the other player" in a 1v1, "the other players" with more seats.
fn others_word(gs: &GameState) -> String {
    if gs.num_seats() > 2 { "the other players" } else { "the other player" }.to_string()
}

fn zone_key(z: Zone) -> String {
    serde_json::to_string(&z).unwrap_or_default()
}

pub fn parse_zone(s: &str) -> Option<Zone> {
    serde_json::from_str(s).ok()
}

struct Item {
    label: String,
    act: String,
    arg: String,
    cls: &'static str,
}

fn it(label: &str, act: &str, arg: impl Into<String>) -> Item {
    Item { label: label.into(), act: act.into(), arg: arg.into(), cls: "" }
}

fn card_menu_items(db: &CardDb, g: &GameUi, id: CardId) -> (String, Vec<Item>) {
    let gs = &g.view.game;
    let c = &gs.cards[id as usize];
    let def = db.get(&c.code);
    let mine = c.owner == g.me() || g.is_local();
    let title = if c.code.is_empty() { "Hidden card".to_string() } else { name_of(db, c) };
    let mut v = vec![];
    let nbf = gs.battlefields.len() as u8;
    let o = c.owner;
    let mv = |label: &str, z: Zone, pos: &str| it(label, "c-move", format!("{id}|{}|{pos}", zone_key(z)));
    match c.zone {
        Zone::Hand(_) if mine => {
            v.push(Item { cls: "hi", ..it(if g.autopay { "Play (auto-pay)" } else { "Play" }, "c-play", id.to_string()) });
            if g.autopay {
                v.push(it("Play without paying", "c-play-free", id.to_string()));
            }
            if def.map(|d| d.is_unit()).unwrap_or(false) && nbf > 0 {
                v.push(it("Play to…", "c-target", format!("{id}|play")));
            }
            if nbf > 0 {
                v.push(it("Hide at…", "c-target", format!("{id}|hide")));
            }
            v.push(it(if c.shown { "Stop revealing" } else { "Reveal" }, "c-reveal", id.to_string()));
            v.push(mv("Discard to trash", Zone::Trash(o), "top"));
            v.push(mv("Put on top of deck", Zone::Deck(o), "top"));
            v.push(mv("Put on bottom of deck", Zone::Deck(o), "bottom"));
            v.push(mv("Move to base", Zone::Base(o), "top"));
            v.push(mv("Banish", Zone::Banish(o), "top"));
        }
        Zone::Base(_) | Zone::Bf(_, _) | Zone::Legend(_) | Zone::Champion(_) => {
            if matches!(c.zone, Zone::Champion(_)) && mine {
                v.push(Item { cls: "hi", ..it(if g.autopay { "Play champion (auto-pay)" } else { "Play champion" }, "c-play", id.to_string()) });
                v.push(mv("Put into hand", Zone::Hand(o), "top"));
            }
            if mine {
                v.push(Item { cls: "hi", ..it(if c.exhausted { "Ready" } else { "Exhaust" }, "c-exh", id.to_string()) });
            }
            if matches!(c.zone, Zone::Base(_) | Zone::Bf(_, _)) {
                if mine {
                    v.push(it("Move to…", "c-target", format!("{id}|move")));
                }
                v.push(Item { cls: "row3", ..it("Damage −", "c-dmg", format!("{id}|-1")) });
                v.push(Item { cls: "row3", ..it("Damage +", "c-dmg", format!("{id}|1")) });
                v.push(Item { cls: "row3", ..it("Buff −", "c-buff", format!("{id}|-1")) });
                v.push(Item { cls: "row3", ..it("Buff +", "c-buff", format!("{id}|1")) });
                v.push(Item { cls: "row3", ..it("Counter −", "c-ctr", format!("{id}|-1")) });
                v.push(Item { cls: "row3", ..it("Counter +", "c-ctr", format!("{id}|1")) });
                v.push(it(if c.stunned { "Remove stun" } else { "Stun" }, "c-stun", id.to_string()));
                if mine {
                    v.push(it(if c.face_down { "Reveal (flip face up)" } else { "Flip face down" }, "c-flip", id.to_string()));
                }
                v.push(it("Kill", "c-kill", id.to_string()));
                if mine {
                    v.push(mv("Return to hand", Zone::Hand(o), "top"));
                    v.push(mv("Put on top of deck", Zone::Deck(o), "top"));
                    v.push(mv("Put on bottom of deck", Zone::Deck(o), "bottom"));
                    v.push(it("Banish", "c-banish", id.to_string()));
                }
            }
        }
        Zone::Runes(_) if mine => {
            v.push(Item { cls: "hi", ..it(if c.exhausted { "Ready" } else { "Exhaust" }, "c-exh", id.to_string()) });
            v.push(it("Recycle to rune deck", "c-recycle", id.to_string()));
            v.push(it("Ready all runes", "ready-runes", ""));
        }
        Zone::Deck(s) if mine => {
            return ("Main deck".into(), deck_items(Zone::Deck(s)));
        }
        Zone::RuneDeck(s) if mine => {
            return ("Rune deck".into(), deck_items(Zone::RuneDeck(s)));
        }
        Zone::Trash(s) | Zone::Banish(s) => {
            v.push(Item { cls: "hi", ..it("View pile", "z-view", zone_key(c.zone)) });
            let _ = s;
        }
        Zone::Chain => {
            v.push(Item { cls: "hi", ..it("Resolve (to trash)", "approve-chain", "") });
            if mine {
                v.push(mv("Return to hand", Zone::Hand(o), "top"));
            }
        }
        Zone::Reveal(_) if mine => {
            v.push(mv("Put into hand", Zone::Hand(o), "top"));
            v.push(mv("Put on top of deck", Zone::Deck(o), "top"));
            v.push(mv("Put on bottom of deck", Zone::Deck(o), "bottom"));
            v.push(mv("Trash", Zone::Trash(o), "top"));
            v.push(mv("Play to base", Zone::Base(o), "top"));
        }
        Zone::BfCard(b) => return bf_items(g, b),
        _ => {}
    }
    v.push(it("Ping for opponent", "c-ping", id.to_string()));
    (title, v)
}

/// Where a pending action can go.
fn target_zones(gs: &GameState, card: CardId, kind: TargetKind) -> Vec<Zone> {
    let Some(c) = gs.card(card) else { return vec![] };
    let o = c.owner;
    let lanes = (0..gs.battlefields.len() as u8).map(|b| Zone::Bf(b, o));
    let mut v: Vec<Zone> = match kind {
        TargetKind::Move | TargetKind::Play => std::iter::once(Zone::Base(o)).chain(lanes).collect(),
        TargetKind::Hide => lanes.collect(),
    };
    v.retain(|z| *z != c.zone);
    v
}

/// Picking from a menu or a target in between means the next click on a card
/// is a fresh click, not the second half of a double-click.
fn reset_double_click() {
    LAST_CLICK.with(|l| *l.borrow_mut() = (0.0, u32::MAX));
}

/// Start choosing a destination; with a single choice, just do it.
fn begin_targeting(app: &mut App, card: CardId, kind: TargetKind) {
    reset_double_click();
    let Some(g) = app.game.as_mut() else { return };
    let zones = target_zones(&g.view.game, card, kind);
    g.menu = None;
    match zones.len() {
        0 => app.render(),
        1 => finish_targeting(app, Targeting { card, kind, zones: zones.clone(), hover: None, from: Zone::Void }, zones[0]),
        _ => {
            g.selected = Some(card);
            let from = g.view.game.card(card).map(|c| c.zone).unwrap_or(Zone::Void);
            g.targeting = Some(Targeting { card, kind, zones, hover: None, from });
            app.render();
        }
    }
}

fn finish_targeting(app: &mut App, t: Targeting, to: Zone) {
    reset_double_click();
    let Some(g) = app.game.as_mut() else { return };
    g.targeting = None;
    g.selected = None;
    let autopay = g.autopay;
    match t.kind {
        TargetKind::Move => app.act(Action::Move { card: t.card, to, pos: Pos::Top }),
        TargetKind::Play => app.act(Action::PlayCard { card: t.card, to: Some(to), autopay }),
        TargetKind::Hide => match to {
            Zone::Bf(b, _) => app.act(Action::Hide { card: t.card, bf: b }),
            _ => app.render(),
        },
    }
}

fn cancel_targeting(app: &mut App) {
    reset_double_click();
    if let Some(g) = app.game.as_mut() {
        if g.targeting.take().is_some() {
            g.selected = None;
        }
    }
    app.render();
}

/// The highlighted destination under a table point: a battlefield card or any
/// lane of that battlefield means the card owner's lane there.
fn target_at(app: &App, x: f64, y: f64) -> Option<Zone> {
    let g = app.game.as_ref()?;
    let t = g.targeting.as_ref()?;
    let (px, pz) = app.renderer.as_ref()?.pick_plane(x, y, 0.0)?;
    let owner = g.view.game.card(t.card)?.owner;
    let z = match g.layout().zone_at(px, pz)? {
        Zone::BfCard(b) | Zone::Bf(b, _) => Zone::Bf(b, owner),
        z => z,
    };
    t.zones.contains(&z).then_some(z)
}

fn deck_items(z: Zone) -> Vec<Item> {
    let k = zone_key(z);
    let mut v = vec![];
    match z {
        Zone::Deck(_) => {
            v.push(Item { cls: "hi", ..it("Draw 1", "z-draw", "1") });
            v.push(it("Draw 2", "z-draw", "2"));
            v.push(Item { cls: "row3", ..it("Look at top 1", "z-peek", format!("{k}|1|0")) });
            v.push(Item { cls: "row3", ..it("Top 3", "z-peek", format!("{k}|3|0")) });
            v.push(Item { cls: "row3", ..it("Top 5", "z-peek", format!("{k}|5|0")) });
            v.push(it("Look at bottom card", "z-peek", format!("{k}|1|1")));
            v.push(it("Search deck", "z-search", k.clone()));
            v.push(it("Reveal top card", "z-revealtop", k.clone()));
            v.push(it("Burn top card", "z-burn", "1"));
            v.push(it("Shuffle", "z-shuffle-z", k));
        }
        _ => {
            v.push(Item { cls: "hi", ..it("Channel 1 rune", "z-channel", "1") });
            v.push(it("Channel 2 runes", "z-channel", "2"));
            v.push(it("Look at the top rune", "z-peek", format!("{k}|1|0")));
            v.push(it("Search rune deck", "z-search", k.clone()));
            v.push(it("Shuffle rune deck", "z-shuffle-z", k));
        }
    }
    v
}

fn bf_items(g: &GameUi, b: u8) -> (String, Vec<Item>) {
    let gs = &g.view.game;
    let mut v = vec![Item { cls: "hi", ..it("Start showdown here", "bf-showdown", b.to_string()) }];
    v.push(it("Resolve combat here", "bf-combat", b.to_string()));
    v.push(it("Conquer (score)", "bf-conquer", b.to_string()));
    v.push(it("Hold (score)", "bf-hold", b.to_string()));
    for s in 0..gs.num_seats() {
        v.push(it(&format!("Controller: {}", gs.seats[s as usize].name), "bf-ctrl", format!("{b}|{s}")));
    }
    v.push(it("Controller: none", "bf-ctrl", format!("{b}|-")));
    if let Some(&id) = gs.pile(Zone::BfCard(b)).first() {
        v.push(it("Ping for opponent", "c-ping", id.to_string()));
    }
    (format!("Battlefield {}", (b'A' + b) as char), v)
}

fn menu_html(db: &CardDb, g: &GameUi, m: &Menu) -> String {
    let gs = &g.view.game;
    let (title, items) = match &m.kind {
        MenuKind::Card(id) => card_menu_items(db, g, *id),
        MenuKind::Zone(z) => match z {
            Zone::Deck(_) | Zone::RuneDeck(_) => (z.label().to_string(), deck_items(*z)),
            Zone::Trash(_) | Zone::Banish(_) => (z.label().into(), vec![it("View pile", "z-view", zone_key(*z))]),
            _ => return String::new(),
        },
        MenuKind::Battlefield(b) => bf_items(g, *b),
        MenuKind::Score(s) => (
            format!("{} score", gs.seats[*s as usize].name),
            vec![Item { cls: "hi", ..it("+1 point", "score", format!("{s}|1")) }, it("−1 point", "score", format!("{s}|-1"))],
        ),
        MenuKind::Game => {
            let mut v = vec![];
            if !g.spectator() && !g.is_over() {
                v.push(Item { cls: "danger", ..it("Concede", "concede", "") });
            }
            if !g.is_local() {
                if !g.spectator() {
                    v.push(it("Return room to lobby", "to-lobby", ""));
                }
                v.push(it("Leave room", "exit-game", ""));
                v.push(it("Show room (lobby view)", "show-lobby", ""));
            } else {
                v.push(it("Exit solo game", "exit-game", ""));
                v.push(it("Restart with same deck(s)", "restart-local", ""));
            }
            v.push(it("Keyboard shortcuts", "help", ""));
            ("Menu".into(), v)
        }
    };
    if g.spectator() && !matches!(m.kind, MenuKind::Game) {
        return String::new();
    }
    let body: String = items.iter().map(|i| format!("<button class=\"mi {}\" data-act=\"{}\" data-arg=\"{}\">{}</button>", i.cls, esc(&i.act), esc(&i.arg), esc(&i.label))).collect();
    let (x, y) = (m.x.max(8.0), m.y.max(8.0));
    format!("<div class=\"ctx\" style=\"left:{x:.0}px;top:{y:.0}px\" id=\"ctx\"><div class=\"ctx-t\">{}</div><div class=\"ctx-b\">{body}</div></div>", esc(&title))
}

fn overlay_html(db: &CardDb, g: &GameUi) -> String {
    let gs = &g.view.game;
    let me = g.me();
    let spec = g.spectator();
    // Peek window (own)
    if let Some(pk) = gs.peeks.iter().find(|p| p.seat == me && !p.cards.is_empty()) {
        let cards: String = pk.cards.iter().map(|&id| {
            let c = &gs.cards[id as usize];
            let o = c.owner;
            let opts = [("Hand", Zone::Hand(o), "top"), ("Top", c.zone, "top"), ("Bottom", c.zone, "bottom"), ("Trash", Zone::Trash(o), "top"), ("Base", Zone::Base(o), "top"), ("Channel", Zone::Runes(o), "top")];
            let btns: String = opts.iter().filter(|(l, _, _)| *l != "Channel" || matches!(c.zone, Zone::RuneDeck(_))).map(|(l, z, p)| format!("<button class=\"mini\" data-act=\"c-move\" data-arg=\"{id}|{}|{p}\">{l}</button>", esc(&zone_key(*z)))).collect();
            format!("<div class=\"pk\" data-code=\"{}\"><img src=\"{}\"><div class=\"pk-b\">{btns}</div></div>", esc(&c.code), card_img(c, db, 260).unwrap_or_default())
        }).collect();
        return format!("<div class=\"modal-bg\"><div class=\"dialog wide\"><div class=\"dlg-t\">Looking at {} card{}</div><div class=\"pk-row\">{cards}</div><div class=\"row c\"><button class=\"btn primary\" data-act=\"peek-close\">Done</button><button class=\"btn\" data-act=\"peek-shuffle\">Shuffle &amp; close</button></div></div></div>", pk.cards.len(), if pk.cards.len() == 1 { "" } else { "s" });
    }
    match &g.modal {
        Some(Modal::Pile(z)) => {
            let ids = gs.pile(*z);
            let owner = z.seat().unwrap_or(me);
            let can = !spec && (owner == me || g.is_local());
            let cards: String = ids.iter().rev().map(|&id| {
                let c = &gs.cards[id as usize];
                let o = c.owner;
                let btns: String = if can {
                    [("Hand", Zone::Hand(o), "top"), ("Base", Zone::Base(o), "top"), ("Deck top", Zone::Deck(o), "top"), ("Deck bottom", Zone::Deck(o), "bottom"), ("Trash", Zone::Trash(o), "top"), ("Banish", Zone::Banish(o), "top")]
                        .iter()
                        .filter(|(_, zz, _)| zz != z)
                        .map(|(l, zz, p)| format!("<button class=\"mini\" data-act=\"c-move\" data-arg=\"{id}|{}|{p}\">{l}</button>", esc(&zone_key(*zz))))
                        .collect()
                } else {
                    String::new()
                };
                let face = match card_img(c, db, 260) {
                    Some(src) if !c.code.is_empty() => format!("<img src=\"{src}\">"),
                    _ => "<div class=\"back\"></div>".to_string(),
                };
                format!("<div class=\"pk\" data-code=\"{}\">{face}<div class=\"pk-b\">{btns}</div></div>", esc(&c.code))
            }).collect();
            let title = format!("{}: {} ({})", gs.seats[owner as usize].name, z.label(), ids.len());
            format!("<div class=\"modal-bg\" data-act=\"close-modal\"><div class=\"dialog wide\" data-act=\"noop\"><div class=\"dlg-t\">{}</div><div class=\"pk-row\">{}</div><div class=\"row c\"><button class=\"btn\" data-act=\"close-modal\">Close</button></div></div></div>", esc(&title), if cards.is_empty() { "<div class=\"empty\">Empty</div>".into() } else { cards })
        }
        Some(Modal::Tokens) => {
            let mut toks: Vec<_> = db.tokens().collect();
            toks.sort_by_key(|t| t.name.clone());
            toks.dedup_by_key(|t| t.name.clone());
            let nbf = gs.battlefields.len() as u8;
            let mut dests = format!("<option value=\"{}\">Base</option>", esc(&zone_key(Zone::Base(me))));
            for b in 0..nbf {
                dests += &format!("<option value=\"{}\">Battlefield {}</option>", esc(&zone_key(Zone::Bf(b, me))), (b'A' + b) as char);
            }
            let items: String = toks.iter().map(|t| format!("<div class=\"pk tok\" data-code=\"{}\"><img src=\"{}\"><div class=\"pk-b\"><button class=\"mini\" data-act=\"token-make\" data-arg=\"{}\">Create {}</button></div></div>", esc(&t.code), img_url(&t.img, 260), esc(&t.code), esc(&t.name))).collect();
            format!("<div class=\"modal-bg\"><div class=\"dialog wide\"><div class=\"dlg-t\">Create tokens</div><div class=\"row c\"><label>Count <select id=\"tok-n\"><option>1</option><option>2</option><option>3</option><option>4</option><option>5</option></select></label><label>Where <select id=\"tok-to\">{dests}</select></label></div><div class=\"pk-row\">{items}</div><div class=\"row c\"><button class=\"btn\" data-act=\"close-modal\">Close</button></div></div></div>")
        }
        Some(Modal::Help) => {
            let rows = [
                ("Click a card", "Open its action menu: play, exhaust, move, damage, buff"),
                ("Drag a card", "Move it: hand → base/battlefield plays it (with auto-pay if on); drop on trash, deck, runes or back into hand"),
                ("Double-click", "Exhaust or ready a card on the board, or play a card from your hand"),
                ("D / C", "Draw a card / channel a rune"),
                ("Space or E", "Advance phase / end turn"),
                ("Ctrl+Z / Ctrl+Y", "Rewind / forward"),
                ("R", "Roll a d20"),
                ("T", "Create tokens"),
                ("Esc", "Close menus and dialogs"),
            ];
            let body: String = rows.iter().map(|(k, v)| format!("<div class=\"kb\"><kbd>{k}</kbd><span>{v}</span></div>")).collect();
            format!("<div class=\"modal-bg\" data-act=\"close-modal\"><div class=\"dialog\"><div class=\"dlg-t\">Controls</div>{body}<p class=\"dim\">Card effects are applied by hand. Click a battlefield for showdowns, combat and scoring.</p><div class=\"row c\"><button class=\"btn primary\" data-act=\"close-modal\">Got it</button></div></div></div>")
        }
        Some(Modal::Sideboard) | None => setup_overlay(db, g),
    }
}

fn setup_overlay(db: &CardDb, g: &GameUi) -> String {
    let gs = &g.view.game;
    let me = g.me();
    let spec = g.spectator();
    if let MatchStage::Sideboarding { locked } = &g.view.stage {
        let mine_locked = locked.get(me as usize).copied().unwrap_or(true);
        if spec || mine_locked {
            return format!("<div class=\"modal-bg\"><div class=\"dialog\"><div class=\"dlg-t\">Sideboarding</div><p>Waiting for {} to lock in.</p></div></div>", others_word(gs));
        }
        let Some(deck) = &g.view.my_deck else { return String::new() };
        let row = |list: &[(String, u32)], act: &str, sel: &Option<String>| -> String {
            list.iter().map(|(c, n)| format!("<div class=\"drow {}\" data-act=\"{act}\" data-arg=\"{}\" data-code=\"{}\"><span class=\"dn\">{}</span><span class=\"dq\">{n}</span></div>", if sel.as_deref() == Some(c) { "sel" } else { "" }, esc(c), esc(c), esc(&db.get(c).map(|d| d.name.clone()).unwrap_or(c.clone())))).collect()
        };
        return format!(
            "<div class=\"modal-bg\"><div class=\"dialog wide\"><div class=\"dlg-t\">Sideboarding for Game {}</div><p class=\"dim\">Select one card on each side, then Swap.</p><div class=\"side-cols\"><div><h3>Main deck</h3>{}</div><div><h3>Sideboard</h3>{}</div></div><div class=\"row c\"><button class=\"btn\" data-act=\"side-swap\" {}>Swap</button><button class=\"btn gold\" data-act=\"side-lock\">Lock in deck</button></div></div></div>",
            g.view.game_no + 1,
            row(&deck.main, "side-out", &g.side_out),
            if deck.side.is_empty() { "<div class=\"dim\">No sideboard cards.</div>".into() } else { row(&deck.side, "side-in", &g.side_in) },
            if g.side_out.is_some() && g.side_in.is_some() { "" } else { "disabled" }
        );
    }
    match &gs.stage {
        Stage::ChooseBattlefield => {
            let p = &gs.seats[me as usize];
            let no_bf = gs.multi() && gs.first == me;
            if spec || p.chosen_bf.is_some() || no_bf || p.conceded {
                let why = if no_bf && !spec { "You go first, so you don't bring a battlefield. " } else { "" };
                let waiting: Vec<String> = gs.seats.iter().enumerate().filter(|(i, s)| s.chosen_bf.is_none() && !s.bf_options.is_empty() && !(gs.multi() && *i as u8 == gs.first)).map(|(_, s)| esc(&s.name)).collect();
                return format!("<div class=\"modal-bg\"><div class=\"dialog\"><div class=\"dlg-t\">Battlefields</div><p>{why}Waiting for {}.</p></div></div>", if waiting.is_empty() { others_word(gs) } else { waiting.join(", ") });
            }
            let opts: String = p.bf_options.iter().map(|c| {
                let d = db.get(c);
                format!("<div class=\"bfopt\" data-act=\"setup-bf\" data-arg=\"{}\" data-code=\"{}\"><img src=\"{}\"><span>{}</span></div>", esc(c), esc(c), d.map(|d| img_url(&d.img, 480)).unwrap_or_default(), esc(&d.map(|d| d.name.clone()).unwrap_or(c.clone())))
            }).collect();
            format!("<div class=\"modal-bg\"><div class=\"dialog wide\"><div class=\"dlg-t\">Choose Your Battlefield{}</div><div class=\"bf-row\">{opts}</div></div></div>", if gs.num_seats() > 1 { "" } else { " (the second one is picked for you)" })
        }
        Stage::ChooseOrder { chooser } => {
            let why = if gs.seats.iter().any(|s| s.roll.is_some()) { "won the roll and chooses" } else { "lost the last game and chooses" };
            let rolls: String = gs.seats.iter().filter_map(|s| s.roll.map(|r| format!("<div class=\"roll\"><span>{}</span><b>{r}</b></div>", esc(&s.name)))).collect();
            if !spec && (*chooser == me || g.is_local()) {
                format!("<div class=\"modal-bg\"><div class=\"dialog\"><div class=\"dlg-t\">Turn order</div><div class=\"rolls\">{rolls}</div><p>{} {why}.</p><div class=\"row c big\"><button class=\"btn gold\" data-act=\"setup-order\" data-arg=\"first\">Go first</button><button class=\"btn\" data-act=\"setup-order\" data-arg=\"second\">Go second</button></div></div></div>", esc(&gs.seats[*chooser as usize].name))
            } else {
                format!("<div class=\"modal-bg\"><div class=\"dialog\"><div class=\"dlg-t\">Turn order</div><div class=\"rolls\">{rolls}</div><p>{} is choosing.</p></div></div>", esc(&gs.seats[*chooser as usize].name))
            }
        }
        Stage::Mulligan => {
            if spec || gs.seats[me as usize].mulligan_done {
                return format!("<div class=\"modal-bg light\"><div class=\"dialog\"><div class=\"dlg-t\">Mulligan</div><p>Waiting for {}.</p></div></div>", others_word(gs));
            }
            let hand: String = gs.pile(Zone::Hand(me)).iter().map(|&id| {
                let c = &gs.cards[id as usize];
                format!("<div class=\"mull {}\" data-act=\"mull-toggle\" data-arg=\"{id}\" data-code=\"{}\"><img src=\"{}\"><span class=\"x\">Bottom</span></div>", if g.mull_sel.contains(&id) { "sel" } else { "" }, esc(&c.code), card_img(c, db, 300).unwrap_or_default())
            }).collect();
            let n = g.mull_sel.len();
            let who = if g.is_local() && gs.num_seats() > 1 { format!(": {}", esc(&gs.seats[me as usize].name)) } else { String::new() };
            let first = if !gs.multi() {
                if gs.first == me { "You go first." } else { "You go second and channel 3 runes on your first turn." }.to_string()
            } else if gs.first == me {
                "You go first and skip your first draw.".to_string()
            } else if gs.last_seat() == me {
                "You go last and channel 3 runes on your first turn.".to_string()
            } else {
                let n = gs.num_seats();
                format!("You take turn {} of each round.", (me + n - gs.first) % n + 1)
            };
            format!("<div class=\"modal-bg\"><div class=\"dialog wide\"><div class=\"dlg-t\">Mulligan{who}</div><p>{first} Pick up to 2 cards to put on the bottom and redraw.</p><div class=\"mull-row\">{hand}</div><div class=\"row c big\"><button class=\"btn gold\" data-act=\"mull-confirm\">{}</button></div></div></div>", if n == 0 { "Keep hand".to_string() } else { format!("Mulligan {n} card{}", if n == 1 { "" } else { "s" }) })
        }
        Stage::Over => {
            let w = gs.winner;
            let local_solo = g.is_local() && gs.num_seats() > 1;
            let (title, cls) = match w {
                None => ("GAME OVER".to_string(), "draw"),
                Some(x) if spec || local_solo => (format!("{} {}", gs.side_name(x).to_uppercase(), if gs.options.mode == Mode::Teams && gs.multi() { "WIN" } else { "WINS" }), "win"),
                Some(_) if gs.won(me) => ("VICTORY".to_string(), "win"),
                Some(_) => ("DEFEAT".to_string(), "lose"),
            };
            let series = if g.view.config.format.wins_needed() > 1 { format!("<div class=\"series\">Series: {}{}</div>", gs.seats.iter().zip(g.view.wins.iter()).map(|(s, x)| format!("{} {x}", esc(&s.name))).collect::<Vec<_>>().join(", "), if g.view.match_winner.is_some() { ". Match complete." } else { "" }) } else { String::new() };
            let mut btns = String::new();
            if !spec {
                if g.view.match_winner.is_none() && g.view.config.format.wins_needed() > 1 {
                    let waiting = g.view.next_ready.get(me as usize).copied().unwrap_or(false);
                    btns += &format!("<button class=\"btn gold\" data-act=\"next-game\" {}>{}</button>", if waiting { "disabled" } else { "" }, if waiting { "Waiting for opponent" } else { "Next game" });
                }
                btns += "<button class=\"btn\" data-act=\"rewind\">Rewind last action</button>";
                if g.is_local() {
                    btns += "<button class=\"btn\" data-act=\"restart-local\">Play again</button><button class=\"btn ghost\" data-act=\"exit-game\">Exit</button>";
                } else {
                    btns += "<button class=\"btn\" data-act=\"to-lobby\">Back to lobby</button><button class=\"btn ghost\" data-act=\"exit-game\">Leave room</button>";
                }
            } else {
                btns += "<button class=\"btn ghost\" data-act=\"exit-game\">Leave</button>";
            }
            let reason = gs.log.iter().rev().find(|l| l.text.contains(" wins the game (")).and_then(|l| l.text.split_once(" (").map(|(_, r)| r.trim_end_matches(").").to_string())).unwrap_or_default();
            let scores: String = if gs.options.mode == Mode::Teams && gs.multi() {
                (0..2u8).map(|t| format!("<div class=\"roll\"><span>{}: {}</span><b>{}</b></div>", team_name(t), esc(&gs.side_name(t)), gs.seats[t as usize].score)).collect()
            } else {
                gs.seats.iter().map(|s| format!("<div class=\"roll\"><span>{}</span><b>{}</b></div>", esc(&s.name), s.score)).collect()
            };
            format!("<div class=\"modal-bg dark\"><div class=\"result {cls}\"><div class=\"res-t\">{}</div>{series}{}<div class=\"res-sub\">Game points</div><div class=\"rolls\">{scores}</div><div class=\"row c big\">{btns}</div></div></div>", esc(&title), if reason.is_empty() { String::new() } else { let mut r = esc(&reason); if let Some(f) = r.get(0..1) { r.replace_range(0..1, &f.to_uppercase()); } format!("<div class=\"series\">{r}.</div>") })
        }
        Stage::Play => String::new(),
    }
}

// ------------------------------------------------------------------ badges

pub fn update_badges(app: &mut App) {
    let db = app.db.clone();
    let Some(g) = app.game.as_ref() else { return };
    let Some(r) = app.renderer.as_ref() else { return };
    let gs = &g.view.game;
    let lay = g.layout();
    let mut want: Vec<(String, String, f64, f64)> = vec![];
    for (&id, a) in &app.scene.anims {
        let c = &gs.cards[id as usize];
        let unit_area = c.zone.is_unit_area();
        let mut parts = String::new();
        if unit_area && !c.face_down {
            if let Some(d) = db.get(&c.code) {
                if let Some(m) = d.might {
                    let cur = m + c.buffs;
                    parts += &format!("<span class=\"b-m {}\">{}{cur}</span>", if c.buffs > 0 { "up" } else { "" }, might());
                }
            }
        }
        if c.damage > 0 {
            parts += &format!("<span class=\"b-d\" title=\"damage\">{}</span>", c.damage);
        }
        if c.buffs > 0 {
            parts += &format!("<span class=\"b-b\">+{}</span>", c.buffs);
        }
        if c.counters != 0 {
            parts += &format!("<span class=\"b-c\" title=\"counters\">{}</span>", c.counters);
        }
        if c.stunned {
            parts += "<span class=\"b-s\">Stunned</span>";
        }
        if parts.is_empty() {
            continue;
        }
        let rot_h = if c.exhausted { a.w } else { a.h };
        if let Some((x, y)) = r.to_screen(v3(a.pos.x, a.pos.y, a.pos.z + rot_h / 2.0 - 0.1)) {
            want.push((format!("c{id}"), parts, x, y));
        }
    }
    // pile counts
    for s in 0..gs.num_seats() {
        for z in [Zone::Deck(s), Zone::RuneDeck(s), Zone::Trash(s), Zone::Banish(s)] {
            let n = gs.pile(z).len();
            if let Some(rect) = lay.rect_for_zone(z) {
                if lay.seat_pos(s).is_none() {
                    continue;
                }
                if let Some((x, y)) = r.to_screen(v3(rect.cx, 0.6, rect.cz + CH / 2.0)) {
                    want.push((format!("p{}", z.key()), format!("<span class=\"b-p\">{n}</span>"), x, y));
                }
            }
        }
    }
    // battlefield controllers
    for (b, bf) in gs.battlefields.iter().enumerate() {
        let rect = lay.bf_rect(b as u8);
        if let Some((x, y)) = r.to_screen(v3(rect.cx, 0.05, rect.cz + rect.h / 2.0 + 0.05)) {
            let label = match bf.controller {
                Some(s) => format!("<span class=\"b-ctl {}\">{}</span>", side_class(&lay, s), esc(&gs.seats[s as usize].name)),
                None => "<span class=\"b-ctl\">Uncontrolled</span>".into(),
            };
            want.push((format!("bf{b}"), label, x, y));
        }
    }
    if let Some((cid, until)) = g.ping {
        if until > dom::now() {
            if let Some(a) = app.scene.anims.get(&cid) {
                if let Some((x, y)) = r.to_screen(v3(a.pos.x, a.pos.y + 0.3, a.pos.z - a.h / 2.0)) {
                    want.push(("ping".into(), "<span class=\"b-ping\">!</span>".into(), x, y));
                }
            }
        }
    }
    let Some(host) = by_id("overlay") else { return };
    let keys: Vec<String> = want.iter().map(|w| w.0.clone()).collect();
    app.badges.retain(|k, el| {
        if keys.contains(k) {
            true
        } else {
            el.remove();
            false
        }
    });
    for (k, html, x, y) in want {
        let el = app.badges.entry(k.clone()).or_insert_with(|| {
            let e: HtmlElement = dom::document().create_element("div").unwrap().dyn_into().unwrap();
            e.set_class_name("badge3d");
            let _ = host.append_child(&e);
            e
        });
        if el.get_attribute("data-h").as_deref() != Some(html.as_str()) {
            el.set_inner_html(&html);
            let _ = el.set_attribute("data-h", &html);
        }
        dom::set_style(el, "transform", &format!("translate({x:.1}px,{y:.1}px) translate(-50%,-50%)"));
    }
}

// ------------------------------------------------------------------ input

fn open_menu(app: &mut App, kind: MenuKind, x: f64, y: f64) {
    let Some(g) = app.game.as_mut() else { return };
    if let MenuKind::Card(id) = kind {
        g.selected = Some(id);
    }
    let w = dom::window().inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1200.0);
    let h = dom::window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(800.0);
    g.menu = Some(Menu { kind, x: (x + 12.0).min(w - 250.0), y: (y - 20.0).min(h - 420.0) });
    app.render();
}

/// A tap fires `click` after `pointerup`, at the same spot. When the tap just opened a
/// menu (a bottom sheet on phones), that click would land on a menu item: ignore it.
/// Only touch taps start this window, so fast mouse users are not affected.
pub fn menu_just_opened() -> bool {
    MENU_OPENED.with(|t| dom::now() - t.get() < 400.0)
}

fn card_arg(arg: &str) -> (CardId, &str, &str) {
    let mut it = arg.splitn(3, '|');
    let id = it.next().and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);
    (id, it.next().unwrap_or(""), it.next().unwrap_or(""))
}

pub fn handle(app: &mut App, act: &str, arg: &str, ev: &MouseEvent) {
    let db = app.db.clone();
    let Some(g) = app.game.as_mut() else { return };
    let me = g.me();
    let autopay = g.autopay;
    let (id, p1, p2) = card_arg(arg);
    match act {
        "noop" => {}
        "card" => {
            if id != u32::MAX {
                let (x, y) = (ev.client_x() as f64, ev.client_y() as f64);
                open_menu(app, MenuKind::Card(id), x, y);
            }
        }
        "game-menu" => open_menu(app, MenuKind::Game, 60.0, 60.0),
        "score-menu" => {
            let s: u8 = arg.parse().unwrap_or(0);
            let (x, y) = (ev.client_x() as f64, ev.client_y() as f64);
            open_menu(app, MenuKind::Score(s), x, y);
        }
        "target-cancel" => cancel_targeting(app),
        "close-modal" | "close-menu" => {
            g.modal = None;
            g.menu = None;
            app.render();
        }
        "help" => {
            g.menu = None;
            g.modal = Some(Modal::Help);
            app.render();
        }
        "tokens" => {
            g.menu = None;
            g.modal = Some(Modal::Tokens);
            app.render();
        }
        "token-make" => {
            let to = crate::ui_game::parse_zone(&dom::input_value("tok-to")).unwrap_or(Zone::Base(me));
            let n: u32 = dom::input_value("tok-n").parse().unwrap_or(1);
            g.modal = None;
            app.act(Action::CreateToken { code: arg.to_string(), to, count: n });
        }
        "panel" => {
            g.panel = match arg {
                "chat" => Panel::Chat,
                "hide" => {
                    if g.panel == Panel::Hidden {
                        Panel::Log
                    } else {
                        Panel::Hidden
                    }
                }
                _ => Panel::Log,
            };
            if g.panel == Panel::Chat {
                g.unread = 0;
            }
            app.render();
            if let Some(el) = by_id("chat-lines") {
                el.set_scroll_top(el.scroll_height());
            }
        }
        "acting" => {
            g.acting = arg.parse().unwrap_or(0);
            g.selected = None;
            g.menu = None;
            if let Source::Local(m) = &g.src {
                g.view = m.view(Some(g.acting), true);
            }
            app.render();
        }
        "toggle-autopay" => {
            g.autopay = !g.autopay;
            set_autopay(g.autopay);
            let msg = if g.autopay { "Auto-pay on: playing a card exhausts/recycles runes for its cost." } else { "Auto-pay off." };
            app.toast(msg, false);
            app.render();
        }
        "toggle-reveal-hand" => {
            let shown = !g.view.game.seats[me as usize].hand_shown;
            app.act(Action::RevealHand { shown });
        }
        "rewind" => app.match_act(MatchAction::Rewind),
        "forward" => app.match_act(MatchAction::Forward),
        "phase-next" => app.act(Action::AdvancePhase),
        "end-turn" => app.act(Action::EndTurn),
        "roll" => app.act(Action::RollD20),
        "concede" => app.act(Action::Concede),
        "next-game" => app.match_act(MatchAction::NextGame),
        "to-lobby" => {
            crate::net::send(&rift_shared::protocol::ClientMsg::ReturnToLobby);
        }
        "show-lobby" => {
            g.menu = None;
            app.go(Screen::Lobby);
        }
        "exit-game" => app.leave_game(),
        "restart-local" => {
            if let Source::Local(m) = &g.src {
                let decks = m.decks.clone();
                let names = m.names.clone();
                let sref = g.sealed_ref.clone();
                app.start_local(decks, names, true, sref);
            }
        }
        "approve-chain" => app.act(Action::ApproveChain),
        "pass-focus" => app.act(Action::PassFocus),
        "end-showdown" => app.act(Action::EndShowdown),
        "setup-bf" => app.act(Action::ChooseBattlefield { code: arg.to_string() }),
        "setup-order" => app.act(Action::ChooseOrder { go_first: arg == "first" }),
        "mull-toggle" => {
            if g.mull_sel.contains(&id) {
                g.mull_sel.retain(|&x| x != id);
            } else if g.mull_sel.len() < 2 {
                g.mull_sel.push(id);
            } else {
                app.toast("You may mulligan at most 2 cards.", true);
                return;
            }
            app.render();
        }
        "mull-confirm" => {
            let cards = std::mem::take(&mut g.mull_sel);
            app.act(Action::Mulligan { cards });
        }
        "side-out" => {
            g.side_out = Some(arg.to_string());
            app.render();
        }
        "side-in" => {
            g.side_in = Some(arg.to_string());
            app.render();
        }
        "side-swap" => {
            if let (Some(o), Some(i)) = (g.side_out.take(), g.side_in.take()) {
                app.match_act(MatchAction::SideSwap { out_code: o, in_code: i });
            }
        }
        "side-lock" => app.match_act(MatchAction::LockSideboard),
        "peek-close" => app.act(Action::ClosePeek),
        "peek-shuffle" => {
            let z = g.view.game.peeks.iter().find(|p| p.seat == me).and_then(|p| p.cards.first()).and_then(|&c| g.card_zone(c)).unwrap_or(Zone::Deck(me));
            app.act(Action::ClosePeek);
            app.act(Action::Shuffle { zone: z });
        }
        // card actions
        "c-play" => app.act(Action::PlayCard { card: id, to: None, autopay }),
        "c-play-free" => app.act(Action::PlayCard { card: id, to: None, autopay: false }),
        "c-target" => {
            let kind = match p1 {
                "play" => TargetKind::Play,
                "hide" => TargetKind::Hide,
                _ => TargetKind::Move,
            };
            begin_targeting(app, id, kind);
        }
        "c-move" => {
            if let Some(z) = parse_zone(p1) {
                let pos = if p2 == "bottom" { Pos::Bottom } else { Pos::Top };
                app.act(Action::Move { card: id, to: z, pos });
            }
        }
        "c-exh" => app.act(Action::ToggleExhausted { card: id }),
        "c-dmg" => app.act(Action::Damage { card: id, delta: p1.parse().unwrap_or(1) }),
        "c-buff" => app.act(Action::Buff { card: id, delta: p1.parse().unwrap_or(1) }),
        "c-ctr" => app.act(Action::Counter { card: id, delta: p1.parse().unwrap_or(1) }),
        "c-stun" => app.act(Action::Stun { card: id }),
        "c-kill" => app.act(Action::Kill { card: id }),
        "c-banish" => app.act(Action::Banish { card: id }),
        "c-flip" => app.act(Action::Flip { card: id }),
        "c-reveal" => app.act(Action::Reveal { card: id }),
        "c-ping" => app.act(Action::Ping { card: id }),
        "c-recycle" => app.act(Action::RecycleRune { card: id }),
        "ready-runes" => app.act(Action::ReadyRunes),
        "z-draw" => app.act(Action::Draw { n: arg.parse().unwrap_or(1) }),
        "z-channel" => app.act(Action::Channel { n: arg.parse().unwrap_or(1) }),
        "z-burn" => app.act(Action::Burn { n: arg.parse().unwrap_or(1) }),
        "z-shuffle" => app.act(Action::Shuffle { zone: Zone::Deck(me) }),
        "z-shuffle-z" => {
            if let Some(z) = parse_zone(arg) {
                app.act(Action::Shuffle { zone: z });
            }
        }
        "z-search" => {
            if let Some(z) = parse_zone(arg) {
                app.act(Action::Search { zone: z });
            }
        }
        "z-revealtop" => {
            if let Some(z) = parse_zone(arg) {
                app.act(Action::RevealTop { zone: z });
            }
        }
        "z-peek" => {
            let mut it = arg.split('|');
            let z = it.next().and_then(parse_zone);
            let n: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            let bottom = it.next() == Some("1");
            if let Some(z) = z {
                app.act(Action::Peek { zone: z, n, bottom });
            }
        }
        "z-view" => {
            if let Some(z) = parse_zone(arg) {
                g.menu = None;
                g.modal = Some(Modal::Pile(z));
                app.render();
            }
        }
        "bf-showdown" => app.act(Action::StartShowdown { bf: arg.parse().unwrap_or(0) }),
        "bf-combat" => app.act(Action::ResolveCombat { bf: arg.parse().unwrap_or(0) }),
        "bf-conquer" => app.act(Action::Conquer { bf: arg.parse().unwrap_or(0) }),
        "bf-hold" => app.act(Action::Hold { bf: arg.parse().unwrap_or(0) }),
        "bf-ctrl" => {
            let (b, s) = arg.split_once('|').unwrap_or(("0", "-"));
            app.act(Action::SetController { bf: b.parse().unwrap_or(0), seat: s.parse().ok() });
        }
        "score" => {
            let (s, d) = arg.split_once('|').unwrap_or(("0", "1"));
            app.act(Action::Score { seat: s.parse().unwrap_or(0), delta: d.parse().unwrap_or(1) });
        }
        _ => {
            let _ = db;
        }
    }
}

pub fn on_key(app: &mut App, e: &KeyboardEvent) {
    let Some(g) = app.game.as_mut() else { return };
    let k = e.key();
    let ctrl = e.ctrl_key() || e.meta_key();
    if k == "Escape" {
        g.menu = None;
        g.modal = None;
        g.selected = None;
        if g.targeting.take().is_some() {
            reset_double_click();
        }
        app.render();
        return;
    }
    if g.spectator() || g.view.game.stage != Stage::Play {
        if ctrl && (k == "z" || k == "Z") && !g.spectator() {
            e.prevent_default();
            app.match_act(MatchAction::Rewind);
        }
        return;
    }
    match (ctrl, k.as_str()) {
        (true, "z") | (true, "Z") => {
            e.prevent_default();
            app.match_act(MatchAction::Rewind)
        }
        (true, "y") | (true, "Y") => {
            e.prevent_default();
            app.match_act(MatchAction::Forward)
        }
        (false, "d") | (false, "D") => app.act(Action::Draw { n: 1 }),
        (false, "c") | (false, "C") => app.act(Action::Channel { n: 1 }),
        (false, "r") | (false, "R") => app.act(Action::RollD20),
        (false, "t") | (false, "T") => {
            g.modal = Some(Modal::Tokens);
            app.render();
        }
        (false, " ") | (false, "e") | (false, "E") => {
            e.prevent_default();
            if g.view.game.phase == Phase::Action {
                app.act(Action::EndTurn)
            } else {
                app.act(Action::AdvancePhase)
            }
        }
        _ => {}
    }
}

fn target_el(e: &PointerEvent) -> Option<web_sys::Element> {
    e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok())
}

fn screen_h() -> f64 {
    dom::window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(800.0)
}

fn drop_target(app: &App, x: f64, y: f64, from_hand: bool) -> Option<Zone> {
    let g = app.game.as_ref()?;
    if y > screen_h() - 140.0 {
        return Some(Zone::Hand(g.me()));
    }
    let r = app.renderer.as_ref()?;
    let (px, pz) = r.pick_plane(x, y, 0.0)?;
    let _ = from_hand;
    g.layout().zone_at(px, pz)
}

fn do_drop(app: &mut App, card: CardId, target: Zone, from_hand: bool) {
    let Some(g) = app.game.as_ref() else { return };
    let gs = &g.view.game;
    let Some(c) = gs.card(card) else { return };
    let owner = c.owner;
    let from = c.zone;
    // Units, gear and runes always land in their owner's zones.
    let target = match target {
        Zone::BfCard(b) => Zone::Bf(b, owner),
        Zone::Bf(b, _) => Zone::Bf(b, owner),
        Zone::Base(_) => Zone::Base(owner),
        Zone::Hand(_) => Zone::Hand(owner),
        Zone::Deck(_) => Zone::Deck(owner),
        Zone::RuneDeck(_) => Zone::RuneDeck(owner),
        Zone::Runes(_) => Zone::Runes(owner),
        Zone::Trash(_) => Zone::Trash(owner),
        Zone::Banish(_) => Zone::Banish(owner),
        Zone::Legend(_) => Zone::Legend(owner),
        Zone::Champion(_) => Zone::Champion(owner),
        z => z,
    };
    if target == from {
        return;
    }
    let autopay = g.autopay;
    let is_spell = app.db.get(&c.code).map(|d| d.is_spell()).unwrap_or(false);
    let playing = (from_hand || matches!(from, Zone::Champion(_))) && matches!(target, Zone::Base(_) | Zone::Bf(_, _));
    if playing {
        let to = if is_spell { None } else { Some(target) };
        app.act(Action::PlayCard { card: card, to, autopay });
    } else if matches!(from, Zone::Deck(_)) && matches!(target, Zone::Hand(_)) && gs.pile(from).last() == Some(&card) {
        app.act(Action::Draw { n: 1 });
    } else if matches!(from, Zone::RuneDeck(_)) && matches!(target, Zone::Runes(_)) {
        app.act(Action::Channel { n: 1 });
    } else {
        let pos = if matches!(target, Zone::RuneDeck(_)) { Pos::Bottom } else { Pos::Top };
        app.act(Action::Move { card, to: target, pos });
    }
}

thread_local! {
    static LAST_CLICK: std::cell::RefCell<(f64, u32)> = const { std::cell::RefCell::new((0.0, u32::MAX)) };
    static MENU_OPENED: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

fn is_double(id: CardId) -> bool {
    let now = dom::now();
    LAST_CLICK.with(|l| {
        let mut l = l.borrow_mut();
        let d = now - l.0 < 350.0 && l.1 == id;
        *l = (now, id);
        d
    })
}

pub fn on_pointer(app: &mut App, kind: &str, e: &PointerEvent) {
    let (x, y) = (e.client_x() as f64, e.client_y() as f64);
    let el = target_el(e);
    let on_canvas = el.as_ref().map(|t| t.id() == "table" || t.id() == "overlay" || t.id() == "screen" || t.get_attribute("class").map(|c| c.contains("hud")).unwrap_or(false)).unwrap_or(false);
    let hand_id: Option<CardId> = el.as_ref().and_then(|t| t.closest("[data-hand]").ok().flatten()).and_then(|h| h.get_attribute("data-hand")).and_then(|s| s.parse().ok());
    // Choosing a destination: hovering lights the area, a click picks it,
    // a click anywhere else (or a right-click) cancels.
    if app.game.as_ref().map(|g| g.targeting.is_some()).unwrap_or(false) {
        match kind {
            "move" => {
                let z = if on_canvas { target_at(app, x, y) } else { None };
                if let Some(t) = app.game.as_mut().and_then(|g| g.targeting.as_mut()) {
                    t.hover = z;
                }
                if let Some(c) = by_id("table") {
                    let _ = c.set_attribute("style", if z.is_some() { "cursor:pointer" } else { "" });
                }
            }
            "up" if on_canvas => {
                let z = if e.button() == 0 { target_at(app, x, y) } else { None };
                let t = app.game.as_ref().and_then(|g| g.targeting.clone());
                match (t, z) {
                    (Some(t), Some(z)) => finish_targeting(app, t, z),
                    _ => cancel_targeting(app),
                }
            }
            _ => {}
        }
        return;
    }
    match kind {
        "down" => {
            if e.button() != 0 {
                return;
            }
            let picked = if on_canvas { app.renderer.as_ref().and_then(|r| app.scene.pick(r, x, y)) } else { None };
            let Some(g) = app.game.as_mut() else { return };
            if g.spectator() {
                return;
            }
            // Touch has no hover: a tap shows the card in the detail panel.
            if e.pointer_type() == "touch" {
                let tapped = hand_id.or(picked);
                if tapped.is_some() && g.hover != tapped {
                    g.hover = tapped;
                    g.hover_code = None;
                    let pv = if matches!(g.view.game.stage, Stage::Play) { preview_html(&app.db, g) } else { String::new() };
                    set_html("preview", &pv);
                }
            }
            if let Some(id) = hand_id {
                g.drag = Some(Drag { card: id, sx: x, sy: y, x, y, active: false, from_hand: true });
            } else if let Some(id) = picked {
                g.drag = Some(Drag { card: id, sx: x, sy: y, x, y, active: false, from_hand: false });
            } else if on_canvas {
                g.drag = None;
            }
        }
        "move" => {
            let mut need_render = false;
            let slop = if e.pointer_type() == "touch" { 14.0 } else { 8.0 };
            let drop = {
                let Some(g) = app.game.as_ref() else { return };
                g.drag.as_ref().map(|d| (d.from_hand, d.active || (x - d.sx).abs() + (y - d.sy).abs() > slop))
            };
            if let Some((from_hand, active)) = drop {
                let dz = if active { drop_target(app, x, y, from_hand) } else { None };
                let g = app.game.as_mut().unwrap();
                if let Some(d) = g.drag.as_mut() {
                    if active && !d.active {
                        need_render = true;
                        g.menu = None;
                    }
                    d.active = active;
                    d.x = x;
                    d.y = y;
                }
                g.drop_zone = dz;
                if let Some(el) = by_id("drag-ghost") {
                    if let Ok(h) = el.dyn_into::<HtmlElement>() {
                        dom::set_style(&h, "transform", &format!("translate({x}px,{y}px) translate(-50%,-60%)"));
                    }
                }
                if from_hand && active {
                    ensure_ghost(app, x, y);
                }
            } else {
                let picked = if on_canvas { app.renderer.as_ref().and_then(|r| app.scene.pick(r, x, y)) } else { None };
                let code = el.as_ref().and_then(|t| t.closest("[data-code]").ok().flatten()).and_then(|h| h.get_attribute("data-code"));
                let g = app.game.as_mut().unwrap();
                let new_hover = picked.or(hand_id);
                let new_code = if new_hover.is_none() { code.filter(|c| !c.is_empty()) } else { None };
                if g.hover != new_hover || g.hover_code != new_code {
                    g.hover = new_hover;
                    g.hover_code = new_code;
                    let pv = if matches!(g.view.game.stage, Stage::Play) { preview_html(&app.db, g) } else { String::new() };
                    set_html("preview", &pv);
                }
                if let Some(c) = by_id("table") {
                    let _ = c.set_attribute("style", if picked.is_some() { "cursor:pointer" } else { "" });
                }
            }
            if need_render {
                app.render();
            }
        }
        "up" => {
            if e.pointer_type() == "touch" {
                MENU_OPENED.with(|t| t.set(dom::now()));
            }
            let Some(g) = app.game.as_mut() else { return };
            let drag = g.drag.take();
            g.drop_zone = None;
            if let Some(gh) = by_id("drag-ghost") {
                gh.remove();
            }
            if let Some(d) = drag {
                if d.active {
                    if let Some(t) = drop_target(app, x, y, d.from_hand) {
                        do_drop(app, d.card, t, d.from_hand);
                    } else {
                        app.render();
                    }
                    return;
                }
                // click
                let double = is_double(d.card);
                let zone = g.card_zone(d.card);
                if double {
                    g.menu = None;
                    match zone {
                        Some(Zone::Hand(_)) => {
                            let a = g.autopay;
                            app.act(Action::PlayCard { card: d.card, to: None, autopay: a });
                        }
                        Some(Zone::Deck(_)) => app.act(Action::Draw { n: 1 }),
                        Some(Zone::RuneDeck(_)) => app.act(Action::Channel { n: 1 }),
                        Some(z) if z.is_board() && !matches!(z, Zone::BfCard(_)) => app.act(Action::ToggleExhausted { card: d.card }),
                        _ => {}
                    }
                    return;
                }
                if g.view.game.stage == Stage::Mulligan && zone.map(|z| matches!(z, Zone::Hand(_))).unwrap_or(false) {
                    return handle(app, "mull-toggle", &d.card.to_string(), e);
                }
                open_menu(app, MenuKind::Card(d.card), x, y);
            } else if on_canvas {
                // click on empty table: zone menus / battlefield menu / close menu
                let z = app.renderer.as_ref().and_then(|r| r.pick_plane(x, y, 0.0)).and_then(|(px, pz)| g.layout().zone_at(px, pz));
                match z {
                    Some(Zone::BfCard(b)) => open_menu(app, MenuKind::Battlefield(b), x, y),
                    Some(z @ (Zone::Deck(_) | Zone::RuneDeck(_) | Zone::Trash(_) | Zone::Banish(_))) => open_menu(app, MenuKind::Zone(z), x, y),
                    _ => {
                        g.menu = None;
                        g.selected = None;
                        if e.pointer_type() == "touch" {
                            g.hover = None;
                            set_html("preview", "");
                        }
                        app.render();
                    }
                }
            }
        }
        // The browser took the pointer (scroll, system gesture): drop nothing.
        "cancel" => {
            let Some(g) = app.game.as_mut() else { return };
            let was_active = g.drag.take().is_some_and(|d| d.active);
            g.drop_zone = None;
            if let Some(gh) = by_id("drag-ghost") {
                gh.remove();
            }
            if was_active {
                app.render();
            }
        }
        _ => {}
    }
}

fn ensure_ghost(app: &App, x: f64, y: f64) {
    if by_id("drag-ghost").is_some() {
        return;
    }
    let Some(g) = app.game.as_ref() else { return };
    let Some(d) = &g.drag else { return };
    let Some(c) = g.view.game.card(d.card) else { return };
    let src = card_img(c, &app.db, 300).unwrap_or_default();
    let e = dom::document().create_element("div").unwrap();
    e.set_id("drag-ghost");
    e.set_class_name("drag-ghost");
    e.set_inner_html(&format!("<img src=\"{src}\">"));
    let _ = e.set_attribute("style", &format!("transform:translate({x}px,{y}px) translate(-50%,-60%)"));
    let _ = dom::document().body().unwrap().append_child(&e);
}

#[allow(dead_code)]
fn layout_dbg(l: &Layout, g: &GameState) -> usize {
    crate::board::layout_cards(g, l).len()
}

#[allow(dead_code)]
fn text(db: &CardDb, code: &str) -> String {
    db.get(code).map(|c| rules_html(&c.text)).unwrap_or_default()
}
