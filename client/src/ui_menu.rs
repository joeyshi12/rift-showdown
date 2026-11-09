//! Out-of-game screens: home (duel / solo / sealed / decks / rooms), the deck
//! editor and the room lobby.

use crate::app::*;
use crate::board::img_url;
use crate::dom::{self, by_id, esc, input_value, set_html};
use crate::icons::icon;
use crate::net;
use rift_shared::cards::{CardDb, CardDef};
use rift_shared::decklist::{starter_deck, Deck, DeckRules};
use rift_shared::protocol::*;
use rift_shared::sealed::{self, BOOSTER_SETS};
use rift_shared::game::{team_name, Mode};
use rift_shared::session::Format;

pub const DOMAINS: &[&str] = &["Fury", "Calm", "Mind", "Body", "Chaos", "Order"];

pub fn domain_color(d: &str) -> &'static str {
    match d {
        "Fury" => "#e8483f",
        "Calm" => "#3fbf7f",
        "Mind" => "#3f8fe8",
        "Body" => "#f0913a",
        "Chaos" => "#a050e0",
        "Order" => "#e8c84a",
        _ => "#9aa6b8",
    }
}

pub fn domain_pips(c: &CardDef) -> String {
    c.domains.iter().filter(|d| *d != "Colorless").map(|d| format!("<i class=\"pip\" style=\"background:{}\" title=\"{}\"></i>", domain_color(d), esc(d))).collect()
}

pub fn type_line(c: &CardDef) -> String {
    let mut s = String::new();
    if !c.sup.is_empty() {
        s += &c.sup;
        s += " ";
    }
    s += &c.kind;
    if !c.tags.is_empty() {
        s += " (";
        s += &c.tags.join(", ");
        s += ")";
    }
    s
}

/// Turn ":rb_might:" style tokens and [Keywords] into readable HTML.
pub fn rules_html(text: &str) -> String {
    let mut t = esc(text);
    t = t.replace(":rb_might:", crate::icons::might()).replace(":rb_exhaust:", crate::icons::exhaust());
    t = t.replace(":rb_rune_rainbow:", "<i class=\"rn any\" title=\"any rune\"></i>");
    for d in DOMAINS {
        t = t.replace(&format!(":rb_rune_{}:", d.to_lowercase()), &format!("<i class=\"rn\" style=\"background:{}\" title=\"{d} rune\"></i>", domain_color(d)));
    }
    for n in 0..13 {
        t = t.replace(&format!(":rb_energy_{n}:"), &format!("<b class=\"nrg\">{n}</b>"));
    }
    // [Keyword] -> highlighted
    let mut out = String::new();
    let mut rest = t.as_str();
    while let Some(i) = rest.find('[') {
        if let Some(j) = rest[i..].find(']') {
            out += &rest[..i];
            out += &format!("<b class=\"kw\">{}</b>", &rest[i + 1..i + j]);
            rest = &rest[i + j + 1..];
        } else {
            break;
        }
    }
    out += rest;
    out.replace(")", ")<wbr>")
}

pub fn card_preview(db: &CardDb, code: &str, extra: &str) -> String {
    let Some(c) = db.get(code) else { return String::new() };
    let stats = {
        let mut v = vec![];
        if let Some(e) = c.energy {
            v.push(format!("<span class=\"stat e\">{e}</span>"));
        }
        if let Some(p) = c.power {
            v.push(format!("<span class=\"stat p\" title=\"power\">{p}</span>"));
        }
        if let Some(m) = c.might {
            v.push(format!("<span class=\"stat m\" title=\"might\">{}{m}</span>", crate::icons::might()));
        }
        v.join("")
    };
    format!(
        "<div class=\"pv {land}\"><img src=\"{img}\" alt=\"\"><div class=\"pv-name\">{name}</div><div class=\"pv-type\">{pips}<span>{ty}</span>{stats}</div><div class=\"pv-text\">{text}</div>{extra}<div class=\"pv-meta\">{code}, {rar}, {set}</div></div>",
        land = if c.land { "land" } else { "" },
        img = img_url(&c.img, 480),
        name = esc(&c.full_or_name()),
        pips = domain_pips(c),
        ty = esc(&type_line(c)),
        text = rules_html(&c.text),
        code = esc(&c.code),
        rar = esc(&c.rarity),
        set = esc(&c.set),
    )
}

trait FullName {
    fn full_or_name(&self) -> String;
}
impl FullName for CardDef {
    fn full_or_name(&self) -> String {
        self.name.clone()
    }
}

fn nav(app: &App) -> String {
    let tab = match app.screen {
        Screen::Home(t) => Some(t),
        _ => None,
    };
    let item = |t: Tab, label: &str, ic: &str| format!("<button class=\"nav-i {}\" data-act=\"tab\" data-arg=\"{label}\"><span class=\"ni\">{}</span><span>{label}</span></button>", if tab == Some(t) { "on" } else { "" }, icon(ic));
    format!(
        "<nav class=\"bottom-nav\">{}{}{}{}{}</nav>",
        item(Tab::Duel, "Duel", "duel"),
        item(Tab::Solo, "Solo", "solo"),
        item(Tab::Sealed, "Sealed", "sealed"),
        item(Tab::Decks, "Decks", "deck"),
        item(Tab::Rooms, "Rooms", "rooms"),
    )
}

/// Shown only while a connection that worked before is down. Nothing while
/// connected, during the first connect, or after another tab took over.
pub fn render_status(app: &App) {
    let lost = !app.connected && app.my_id != 0 && !app.names.replaced;
    set_html("net-lost", if lost { "<div class=\"net-lost\" role=\"status\">Connection lost. Reconnecting…</div>" } else { "" });
}

fn deck_options(app: &App, selected: Option<&str>, include_sealed: bool) -> String {
    let mut s = String::new();
    for d in &app.decks {
        if !include_sealed && d.sealed.is_some() {
            continue;
        }
        let legal = d.deck.validate(&app.db, if d.sealed.is_some() { DeckRules::Sealed } else { DeckRules::Standard }).is_empty();
        s += &format!(
            "<option value=\"{}\" {}>{}{}</option>",
            esc(&d.id),
            if Some(d.id.as_str()) == selected { "selected" } else { "" },
            esc(if d.deck.name.is_empty() { "Unnamed deck" } else { &d.deck.name }),
            if d.sealed.is_some() { " (sealed)" } else if legal { "" } else { " (not legal)" }
        );
    }
    s
}

fn top_bar(app: &App, title: &str) -> String {
    // In a room but looking at another page: one click back.
    let back = match &app.room {
        Some(r) if app.screen != Screen::Lobby => format!("<button class=\"btn sm\" data-act=\"to-room\">Back to room {}</button>", esc(&r.code)),
        _ => String::new(),
    };
    format!(
        "<header class=\"topbar\"><div class=\"logo\" data-act=\"tab\" data-arg=\"Duel\">Rift Showdown</div><div class=\"tb-title\">{}</div>{back}</header>",
        esc(title)
    )
}

pub fn render(app: &mut App, force: bool) {
    let html = match app.screen {
        Screen::Home(Tab::Duel) => format!("{}<main class=\"home play\">{}</main>{}", top_bar(app, "Duel"), duel_tab(app), nav(app)),
        Screen::Home(Tab::Solo) => format!("{}<main class=\"home\">{}</main>{}", top_bar(app, "Solo"), solo_tab(app), nav(app)),
        Screen::Home(Tab::Sealed) => format!("{}<main class=\"home\">{}</main>{}", top_bar(app, "Sealed"), sealed_tab(app), nav(app)),
        Screen::Home(Tab::Decks) => format!("{}<main class=\"home\">{}</main>{}", top_bar(app, "Decks"), decks_tab(app), nav(app)),
        Screen::Home(Tab::Rooms) => format!("{}<main class=\"home\">{}</main>{}", top_bar(app, "Rooms"), rooms_tab(app), nav(app)),
        Screen::Editor => {
            if force || by_id("ed-results").is_none() {
                dom::force_html("screen", &editor_shell(app));
            }
            render_editor_parts(app);
            render_status(app);
            return;
        }
        Screen::Lobby => {
            if force || by_id("lobby-main").is_none() {
                dom::force_html("screen", &format!("{}{}", top_bar(app, "Room"), lobby_shell()));
                app.render_chat_inline();
            }
            set_html("lobby-main", &lobby_main(app));
            render_status(app);
            return;
        }
        Screen::Game => String::new(),
    };
    let html = format!("<div class=\"menu-bg\"></div>{html}");
    dom::set_html("screen", &html);
    render_status(app);
}

impl App {
    pub fn render_chat_inline(&mut self) {
        let html = chat_lines(&self.chat);
        set_html("chat-lines", &html);
    }
}

pub fn chat_lines(lines: &[ChatLine]) -> String {
    lines
        .iter()
        .map(|l| if l.system { format!("<div class=\"cl sys\">{}</div>", esc(&l.text)) } else { format!("<div class=\"cl\"><b>{}</b> {}</div>", esc(&l.from), esc(&l.text)) })
        .collect()
}

/// "A", "A and B", "A, B and C".
fn and_list(xs: &[String]) -> String {
    match xs.len() {
        0 => String::new(),
        1 => xs[0].clone(),
        n => format!("{} and {}", xs[..n - 1].join(", "), xs[n - 1]),
    }
}

/// Data Dragon id of a League champion ("Kai'Sa" -> "Kaisa", "Lee Sin" -> "LeeSin").
fn ddragon_id(champ: &str) -> String {
    match champ {
        "Kai'Sa" => "Kaisa".into(),
        "Kha'Zix" => "Khazix".into(),
        "LeBlanc" => "Leblanc".into(),
        "Renata Glasc" => "Renata".into(),
        "Wukong" => "MonkeyKing".into(),
        "Cho'Gath" => "Chogath".into(),
        "Vel'Koz" => "Velkoz".into(),
        "Bel'Veth" => "Belveth".into(),
        "Nunu & Willump" => "Nunu".into(),
        _ => champ.chars().filter(|c| c.is_ascii_alphanumeric()).collect(),
    }
}

/// Hero image for a legend, sharpest first: the champion's full-size splash painting
/// (League of Legends Wiki copy, at 1280/1920/2560px), then Data Dragon's 1215px splash,
/// then the 2x showcase print of the legend, then the legend card. `onerror` walks the list.
fn hero_art(app: &App, l: &CardDef) -> String {
    let showcase = app.db.cards.iter().find(|c| c.is_legend() && c.name == l.name && c.img.contains("-1488x") && !c.full.contains("(Signature)"));
    let card = img_url(&showcase.unwrap_or(l).img, 1024);
    let Some(champ) = l.tags.iter().find(|t| t.as_str() != "Yordle") else {
        return format!("<img class=\"hero-art card\" src=\"{card}\" alt=\"\">");
    };
    let file = format!("{}_OriginalSkin_HD.jpg", dom::encode_uri(&champ.replace(' ', "_")));
    let hd = |w: u32| img_url(&format!("{WIKI_THUMB}{file}/{w}px-{file}"), w);
    let srcset: Vec<String> = [1280, 1920, 2560].iter().map(|w| format!("{} {w}w", hd(*w))).collect();
    let dd = img_url(&format!("{DDRAGON_SPLASH}{}_0.jpg", ddragon_id(champ)), 1024);
    format!(
        "<img class=\"hero-art splash\" src=\"{}\" srcset=\"{}\" sizes=\"(max-width: 640px) 100vw, 62vw\" data-fb=\"{dd}|{card}\" alt=\"\" onerror=\"heroFallback(this)\">",
        hd(1920),
        srcset.join(", ")
    )
}

const DDRAGON_SPLASH: &str = "https://ddragon.leagueoflegends.com/cdn/img/champion/splash/";
const WIKI_THUMB: &str = "https://wiki.leagueoflegends.com/en-us/images/thumb/";

/// The home deck's legend, large, with the other decks as round legend icons.
fn home_hero(app: &App) -> String {
    let Some(d) = app.home_deck() else {
        return r#"<div class="hero none"><div><h2>No decks yet</h2><p>Import a deck code or add a starter deck to play.</p><button class="btn primary" data-act="tab" data-arg="Decks">Go to decks</button></div></div>"#.into();
    };
    let legend = d.deck.legend.as_ref().and_then(|l| app.db.get(l));
    let art = legend.map(|l| hero_art(app, l)).unwrap_or_default();
    let title = legend.map(|l| esc(&l.name).replacen(", ", ",<br>", 1)).unwrap_or_else(|| esc(if d.deck.name.is_empty() { "Unnamed deck" } else { &d.deck.name }));
    let doms: Vec<String> = legend.map(|l| l.domains.iter().filter(|x| *x != "Colorless").cloned().collect()).unwrap_or_default();
    let dia: String = doms.iter().map(|x| format!("<i class=\"dia\" style=\"background:{}\"></i>", domain_color(x))).collect();
    let errs = d.deck.validate(&app.db, DeckRules::Standard);
    let legal = if errs.is_empty() {
        "legal in Standard.".to_string()
    } else {
        format!("<span class=\"bad\">{} issue{} to fix for Standard.</span>", errs.len(), if errs.len() == 1 { "" } else { "s" })
    };
    let what = if doms.is_empty() { String::new() } else { format!("{}. ", and_list(&doms)) };
    let picks: String = app
        .decks
        .iter()
        .filter(|x| x.sealed.is_none())
        .map(|x| {
            let src = x.deck.legend.as_ref().and_then(|l| app.db.get(l)).map(|l| img_url(&l.img, 128)).unwrap_or_default();
            let name = esc(if x.deck.name.is_empty() { "Unnamed deck" } else { &x.deck.name });
            format!(
                "<button class=\"pick {on}\" data-act=\"home-deck\" data-arg=\"{id}\" title=\"{name}\" aria-label=\"Play with {name}\" aria-pressed=\"{pr}\"><img src=\"{src}\" alt=\"\"></button>",
                on = if x.id == d.id { "on" } else { "" },
                pr = x.id == d.id,
                id = esc(&x.id),
            )
        })
        .collect();
    format!(
        r#"<div class="hero">{art}<div class="hero-deck"><h1>{title}</h1><p>{dia}<span>{name}{what}{main} cards, {legal}</span></p><div class="picks">{picks}<button class="lnk" data-act="tab" data-arg="Decks">All decks</button></div></div></div>"#,
        // Decks named after the legend don't repeat it under the title.
        name = match legend {
            Some(l) if d.deck.name.is_empty() || d.deck.name.starts_with(&l.name) => String::new(),
            _ => format!("{}: ", esc(if d.deck.name.is_empty() { "Unnamed deck" } else { &d.deck.name })),
        },
        main = d.deck.main_count(),
    )
}

fn duel_tab(app: &App) -> String {
    let deck_id = app.home_deck().map(|d| esc(&d.id)).unwrap_or_default();
    format!(
        r#"{hero}<div class="ways">
 <section class="way">
  <div><h2>Find match</h2><p>Play someone new with this deck.</p></div>
  <div class="way-go"><button class="btn primary" data-act="queue" {qdis}>{qlabel}</button></div>
  <div class="way-f"><label><span class="sr">Queue</span><select id="q-mode"><option value="Bo1-Standard">Best of 1, Standard</option><option value="Bo3-Standard">Best of 3, Standard</option><option value="Bo1-Unrestricted">Best of 1, Unrestricted</option><option value="Bo3-Unrestricted">Best of 3, Unrestricted</option></select></label></div>
 </section>
 <section class="way">
  <div><h2>Private room</h2><p>Make a room and share the code with a friend.</p></div>
  <div class="way-go"><button class="btn" data-act="create-room">Create</button></div>
  <div class="way-f">
   <label><span class="sr">Players</span><select id="cr-mode"><option value="duel">1v1</option><option value="teams">2v2</option><option value="ffa3">Free-for-all (3 players)</option><option value="ffa">Free-for-all (4 players)</option></select></label>
   <label><span class="sr">Format</span><select id="cr-format"><option value="Bo1">Best of 1</option><option value="Bo3">Best of 3</option></select></label>
   <label><span class="sr">Deck rules</span><select id="cr-rules"><option value="Standard">Standard</option><option value="Unrestricted">Unrestricted</option><option value="Sealed">Sealed</option></select></label>
   <div class="checks"><label class="chk"><input type="checkbox" id="cr-public"> Public</label><label class="chk"><input type="checkbox" id="cr-spec" checked> Spectators</label></div>
  </div>
 </section>
 <section class="way">
  <div><h2>Join a room</h2><p>Enter the code your friend sent you.</p></div>
  <div class="way-go"><button class="btn" data-act="join-room">Join</button><button class="btn" data-act="spectate-room">Watch</button></div>
  <div class="way-f"><label><span class="sr">Room code</span><input id="join-code" class="code-in" maxlength="6" placeholder="------" data-enter="join-room" autocomplete="off" spellcheck="false"></label></div>
 </section>
 <section class="way">
  <div><h2>Practice</h2><p>Goldfish this deck, or play both sides.</p></div>
  <div class="way-go"><button class="btn" data-act="goldfish-id" data-arg="{deck_id}" {gdis}>Goldfish</button><button class="btn" data-act="tab" data-arg="Solo">Both sides</button></div>
 </section>
</div>"#,
        hero = home_hero(app),
        gdis = if deck_id.is_empty() { "disabled" } else { "" },
        qdis = if app.queue.is_some() { "disabled" } else { "" },
        qlabel = if app.queue.is_some() { "Searching" } else { "Search" },
    )
}

fn solo_tab(app: &App) -> String {
    let opts = deck_options(app, None, true);
    format!(
        r#"<div class="modes">
 <section class="mode">
  <div class="mode-name"><h2>Goldfish</h2><p>Your deck alone on a two-battlefield table.</p></div>
  <div class="mode-fields">
   <label>Deck<select id="gf-deck">{opts}</select></label>
   <div class="checks"><label class="chk"><input type="checkbox" id="gf-skip" checked> Skip setup</label></div>
  </div>
  <div class="mode-go"><button class="btn primary" data-act="goldfish">Start</button></div>
 </section>
 <section class="mode">
  <div class="mode-name"><h2>Both sides</h2><p>You control both players.</p></div>
  <div class="mode-fields">
   <label>Player 1<select id="sp-deck1">{opts}</select></label>
   <label>Player 2<select id="sp-deck2">{opts}</select></label>
   <div class="checks"><label class="chk"><input type="checkbox" id="sp-skip"> Skip setup</label></div>
  </div>
  <div class="mode-go"><button class="btn primary" data-act="solo-play">Start</button></div>
 </section>
</div>"#
    )
}

fn set_select(id: &str, sel: &str) -> String {
    let mut s = format!("<select id=\"{id}\" data-change=\"{id}\">");
    for (code, name) in BOOSTER_SETS {
        s += &format!("<option value=\"{code}\" {}>{name}</option>", if *code == sel { "selected" } else { "" });
    }
    s + "</select>"
}

fn sealed_tab(app: &App) -> String {
    let st = &app.sealed;
    let packs: String = st.sets.iter().enumerate().map(|(i, s)| format!("<div class=\"pack-sel\"><span>Pack {}</span>{}</div>", i + 1, set_select(&format!("pack-{i}"), s))).collect();
    let pool_html = match &st.pool {
        None => "<div class=\"empty\">No pool opened yet. The same seed always opens the same packs.</div>".to_string(),
        Some(p) => {
            let mut h = format!("<div class=\"pool-head\"><div><span class=\"dim\">Seed</span> <b class=\"num\">{}</b><span class=\"dim\">, {} cards</span></div><div class=\"row\"><button class=\"btn primary\" data-act=\"sealed-build\">Build deck</button><button class=\"btn\" data-act=\"sealed-auto\">Auto-build</button><button class=\"btn ghost\" data-act=\"copy-seed\" data-arg=\"{}\">Copy seed</button></div></div>", esc(&p.seed), p.packs.iter().map(|x| x.len()).sum::<usize>(), esc(&p.seed));
            for (i, pk) in p.packs.iter().enumerate() {
                h += &format!("<div class=\"pack\"><div class=\"pack-t\">Pack {}: {}</div><div class=\"pack-cards\">", i + 1, esc(p.sets.get(i).map(|s| s.as_str()).unwrap_or("")));
                for c in pk {
                    if let Some(d) = app.db.get(c) {
                        h += &format!("<div class=\"pc r-{}{}\" data-code=\"{}\"><img loading=\"lazy\" src=\"{}\"></div>", esc(&d.rarity.to_lowercase()), if d.land { " land" } else { "" }, esc(&d.code), img_url(&d.img, 200));
                    }
                }
                h += "</div></div>";
            }
            h
        }
    };
    let saved: String = app
        .decks
        .iter()
        .filter(|d| d.sealed.is_some())
        .map(|d| format!("<div class=\"mini-deck\"><b>{}</b><span class=\"dim\">{} cards</span><button class=\"btn sm\" data-act=\"deck-edit\" data-arg=\"{}\">Edit</button><button class=\"btn sm\" data-act=\"goldfish-id\" data-arg=\"{}\">Goldfish</button></div>", esc(&d.deck.name), d.deck.main_count(), esc(&d.id), esc(&d.id)))
        .collect();
    format!(
        r#"<div class="sealed-wrap">
<section class="panel side">
 <h2>Sealed pool</h2>
 <label>Set for all packs{all}</label>
 <div class="packs-grid">{packs}</div>
 <div class="row"><button class="btn sm" data-act="pack-add">Add pack</button><button class="btn sm" data-act="pack-remove">Remove pack</button></div>
 <label>Seed<input id="sealed-seed" value="{seed}" placeholder="random"></label>
 <button class="btn gold" data-act="sealed-open">Open packs</button>
 <h3>Saved sealed decks</h3>{saved}
</section>
<section class="panel pool">{pool_html}</section>
</div>"#,
        all = set_select("pack-all", st.sets.first().map(|s| s.as_str()).unwrap_or("OGN")),
        seed = esc(&st.seed),
    )
}

fn deck_tile(app: &App, d: &SavedDeck) -> String {
    let rules = if d.sealed.is_some() { DeckRules::Sealed } else { DeckRules::Standard };
    let errs = d.deck.validate(&app.db, rules);
    let legend = d.deck.legend.as_ref().and_then(|l| app.db.get(l));
    let cover = legend.map(|l| img_url(&l.img, 300)).unwrap_or_default();
    let pips = legend.map(domain_pips).unwrap_or_default();
    format!(
        r#"<div class="deck-case"><div class="dc-cover" data-act="deck-edit" data-arg="{id}" style="background-image:url('{cover}')"></div>
<div class="dc-body"><div class="dc-name">{name}</div><div class="dc-sub">{pips}<span>{legend}</span></div>
<div class="dc-stat {cls}">{status}</div>
<div class="dc-btns"><button class="lnk" data-act="deck-edit" data-arg="{id}">Edit</button><button class="lnk" data-act="deck-code" data-arg="{id}">Copy code</button><button class="lnk" data-act="deck-text" data-arg="{id}">Copy list</button><button class="lnk" data-act="deck-dup" data-arg="{id}">Duplicate</button><button class="lnk del" data-act="deck-del" data-arg="{id}">Delete</button></div></div></div>"#,
        id = esc(&d.id),
        cover = cover,
        name = esc(if d.deck.name.is_empty() { "Unnamed deck" } else { &d.deck.name }),
        pips = pips,
        legend = esc(&legend.map(|l| l.name.clone()).unwrap_or_else(|| "No legend".into())),
        cls = if errs.is_empty() { "ok" } else { "bad" },
        status = if errs.is_empty() { format!("Legal, {} cards and {} runes", d.deck.main_count(), d.deck.rune_count()) } else { format!("{} issue{} to fix", errs.len(), if errs.len() == 1 { "" } else { "s" }) } + if d.sealed.is_some() { " (sealed)" } else { "" },
    )
}

fn decks_tab(app: &App) -> String {
    let tiles: String = app.decks.iter().map(|d| deck_tile(app, d)).collect();
    // Only Riot's own preconstructed decks are offered.
    let legends: String = rift_shared::precons::all().iter().map(|p| format!("<option value=\"{}\">{} ({})</option>", esc(&p.legend), esc(&p.name), esc(&p.product.split(' ').next().unwrap_or("")))).collect();
    let import = if app.import_open {
        r#"<section class="panel import"><h2>Import a deck</h2><p class="dim">Deck code from Piltover Archive or RiftAtlas, or a text list.</p>
<label>Name<input id="imp-name" placeholder="My deck"></label><textarea id="imp-text" rows="10" placeholder="Deck code or list…"></textarea><div class="row"><button class="btn primary" data-act="import-deck">Import</button><button class="btn ghost" data-act="import-toggle">Cancel</button></div></section>"#
            .to_string()
    } else {
        String::new()
    };
    format!(
        r#"<div class="deck-toolbar"><button class="btn primary" data-act="deck-new">New deck</button><button class="btn" data-act="import-toggle">Import</button><span class="sep"></span><select id="starter-legend" title="Riot's preconstructed decks">{legends}</select><button class="btn" data-act="deck-starter">Add Riot deck</button></div>{import}<div class="deck-grid">{tiles}</div>"#
    )
}

fn rooms_tab(app: &App) -> String {
    let rows: String = if app.rooms.is_empty() {
        "<div class=\"empty\">No public rooms or live matches right now.</div>".into()
    } else {
        app.rooms
            .iter()
            .map(|r| {
                format!(
                    "<div class=\"room-row\"><code class=\"rc\">{}</code><span>{}</span><span class=\"dim\">{}, {}, {:?}</span><span>{}/{} players, {} watching</span><span class=\"tag {}\">{}</span>{}{}</div>",
                    esc(&r.code),
                    esc(&r.host_name),
                    r.settings.mode.label(),
                    r.settings.format.label(),
                    r.settings.rules,
                    r.players,
                    r.settings.mode.seats(),
                    r.spectators,
                    if r.in_game { "live" } else if r.players >= r.settings.mode.seats() { "" } else { "open" },
                    if r.in_game { "In game" } else if r.players >= r.settings.mode.seats() { "Full" } else { "Open seat" },
                    if r.in_game || r.players >= r.settings.mode.seats() { "<span></span>".to_string() } else { format!("<button class=\"btn sm\" data-act=\"join-code\" data-arg=\"{}\">Join</button>", esc(&r.code)) },
                    if r.settings.allow_spectators { format!("<button class=\"btn sm\" data-act=\"spec-code\" data-arg=\"{}\">Watch</button>", esc(&r.code)) } else { "<span class=\"dim sm\">No spectators</span>".to_string() },
                )
            })
            .collect()
    };
    format!("<section class=\"panel wide\"><div class=\"ph-row\"><h2>Public rooms and live matches</h2><button class=\"btn sm\" data-act=\"rooms-refresh\">Refresh</button></div>{rows}</section>")
}

// ------------------------------------------------------------------ editor

fn editor_shell(app: &App) -> String {
    let e = &app.editor;
    let sel = |id: &str, opts: &[(&str, &str)], cur: &str| -> String {
        let mut s = format!("<select id=\"{id}\" data-change=\"{id}\">");
        for (v, l) in opts {
            s += &format!("<option value=\"{v}\" {}>{l}</option>", if *v == cur { "selected" } else { "" });
        }
        s + "</select>"
    };
    let types = [("", "All types"), ("Unit", "Units"), ("Champion", "Champions"), ("Spell", "Spells"), ("Gear", "Gear"), ("Legend", "Legends"), ("Battlefield", "Battlefields"), ("Rune", "Runes"), ("Signature", "Signature")];
    let mut doms: Vec<(&str, &str)> = vec![("", "All domains")];
    doms.extend(DOMAINS.iter().map(|d| (*d, *d)));
    doms.push(("Colorless", "Colorless"));
    let sets = [("", "All sets"), ("OGN", "Origins"), ("OGS", "Proving Grounds"), ("SFD", "Spiritforged"), ("UNL", "Unleashed"), ("VEN", "Vendetta"), ("OPP", "Promos (OPP)"), ("PR", "Promos (PR)")];
    let costs = [("", "Any cost"), ("0", "0"), ("1", "1"), ("2", "2"), ("3", "3"), ("4", "4"), ("5", "5"), ("6", "6"), ("7", "7+")];
    format!(
        r#"<div class="menu-bg"></div>{top}<div class="editor">
<section class="ed-left">
 <div class="ed-filters"><input id="ed-q" placeholder="Search name, text or tag" value="{q}" data-input="ed-q" autocomplete="off">{t}{d}{s}{c}<label class="chk"><input type="checkbox" id="ed-side" data-change="ed-side" {side}> To sideboard</label></div>
 <div id="ed-results" class="ed-results"></div>
</section>
<aside class="ed-preview" id="preview"><div class="dim pad">Click a card to add it. Shift+click adds it to the sideboard.</div></aside>
<section class="ed-right">
 <div class="ed-head"><input id="ed-name" value="{name}" placeholder="Deck name" data-input="ed-name"><div class="row"><button class="btn primary" data-act="ed-save">Save</button><button class="btn" data-act="ed-code">Copy code</button><button class="btn" data-act="ed-text">Copy list</button><button class="btn ghost" data-act="ed-back">Back</button></div></div>
 <div id="ed-deck" class="ed-deck"></div>
</section></div>"#,
        top = top_bar(app, if e.pool.is_some() { "Sealed deck" } else { "Edit deck" }),
        q = esc(&e.query),
        t = sel("ed-type", &types, &e.ftype),
        d = sel("ed-dom", &doms, &e.fdomain),
        s = sel("ed-set", &sets, &e.fset),
        c = sel("ed-cost", &costs, &e.fcost),
        side = if e.side_mode { "checked" } else { "" },
        name = esc(&e.deck.name),
    )
}

fn matches_filter(e: &EditorState, c: &CardDef) -> bool {
    if c.is_token() {
        return false;
    }
    if let Some(pool) = &e.pool {
        if !pool.contains_key(&c.code) && !(c.is_rune() && !c.alt && c.set == "OGN") {
            return false;
        }
    }
    match e.ftype.as_str() {
        "" => {}
        "Champion" => {
            if !c.is_champion_unit() {
                return false;
            }
        }
        "Signature" => {
            if !c.is_signature() {
                return false;
            }
        }
        t => {
            if c.kind != t {
                return false;
            }
        }
    }
    if !e.fdomain.is_empty() {
        if e.fdomain == "Colorless" {
            if !c.is_colorless() {
                return false;
            }
        } else if !c.domains.contains(&e.fdomain) {
            return false;
        }
    }
    if !e.fset.is_empty() && c.set != e.fset {
        return false;
    }
    if !e.fcost.is_empty() {
        let want: i32 = e.fcost.parse().unwrap_or(0);
        let have = c.energy.unwrap_or(-1);
        if (want == 7 && have < 7) || (want < 7 && have != want) {
            return false;
        }
    }
    if !e.query.is_empty() {
        let q = e.query.to_lowercase();
        let hay = format!("{} {} {} {}", c.name, c.text, c.tags.join(" "), c.code).to_lowercase();
        if !q.split_whitespace().all(|w| hay.contains(w)) {
            return false;
        }
    }
    true
}

fn count_in_deck(d: &Deck, code: &str) -> u32 {
    let m: u32 = d.main.iter().chain(d.runes.iter()).filter(|e| e.0 == code).map(|e| e.1).sum();
    m + (d.champion.as_deref() == Some(code)) as u32 + (d.legend.as_deref() == Some(code)) as u32 + d.battlefields.iter().filter(|b| *b == code).count() as u32
}

pub fn render_editor_parts(app: &mut App) {
    let e = &app.editor;
    let mut res: Vec<&CardDef> = app.db.cards.iter().filter(|c| matches_filter(e, c)).collect();
    res.sort_by_key(|c| (c.kind != "Legend", c.energy.unwrap_or(-1), c.name.clone(), c.code.clone()));
    let total = res.len();
    let mut html = String::new();
    for c in res.iter().take(e.limit) {
        let n = count_in_deck(&e.deck, &c.code) + e.deck.side.iter().filter(|s| s.0 == c.code).map(|s| s.1).sum::<u32>();
        let avail = e.pool.as_ref().and_then(|p| p.get(&c.code)).copied();
        html += &format!(
            "<div class=\"ctile {land}\" data-act=\"ed-add\" data-arg=\"{code}\" data-code=\"{code}\"><img loading=\"lazy\" src=\"{img}\" alt=\"{name}\">{cnt}{av}</div>",
            land = if c.land { "land" } else { "" },
            code = esc(&c.code),
            img = img_url(&c.img, 260),
            name = esc(&c.name),
            cnt = if n > 0 { format!("<span class=\"cnt\">{n}</span>") } else { String::new() },
            av = avail.map(|a| format!("<span class=\"avail\">×{a}</span>")).unwrap_or_default(),
        );
    }
    if total > e.limit {
        html += &format!("<button class=\"btn more\" data-act=\"ed-more\">Show more ({} more)</button>", total - e.limit);
    }
    if total == 0 {
        html = "<div class=\"empty\">No cards match those filters.</div>".into();
    }
    set_html("ed-results", &html);
    let deck = deck_panel(app);
    set_html("ed-deck", &deck);
}

fn deck_panel(app: &App) -> String {
    let db = &app.db;
    let e = &app.editor;
    let d = &e.deck;
    let rules = if e.pool.is_some() { DeckRules::Sealed } else { DeckRules::Standard };
    let errs = d.validate(db, rules);
    let mut errs2 = errs.clone();
    if let (Some(s), true) = (&e.sealed, e.pool.is_some()) {
        let pool = sealed::open_pool(db, &s.sets, &s.seed);
        errs2.extend(sealed::check_pool(db, d, &pool));
    }
    let row = |code: &str, n: u32, section: &str| -> String {
        let Some(c) = db.get(code) else { return String::new() };
        let star = if section == "main" && c.is_champion_unit() { format!("<button class=\"mini\" title=\"Make chosen champion\" data-act=\"ed-champ\" data-arg=\"{}\">Champion</button>", esc(code)) } else { String::new() };
        format!(
            "<div class=\"drow\" data-code=\"{code}\">{cost}{pips}<span class=\"dn\">{name}</span>{star}<span class=\"dq\">{n}</span><button class=\"mini\" data-act=\"ed-inc\" data-arg=\"{section}|{code}\">+</button><button class=\"mini\" data-act=\"ed-dec\" data-arg=\"{section}|{code}\">−</button></div>",
            code = esc(code),
            cost = c.energy.map(|x| format!("<span class=\"cost\">{x}</span>")).unwrap_or_else(|| "<span class=\"cost none\"></span>".into()),
            pips = domain_pips(c),
            name = esc(&c.name),
        )
    };
    let mut h = String::new();
    h += &format!("<div class=\"dsec\"><div class=\"dsh\">Legend</div>{}</div>", d.legend.as_ref().map(|l| row(l, 1, "legend")).unwrap_or_else(|| "<div class=\"dim pad\">Add a legend</div>".into()));
    h += &format!("<div class=\"dsec\"><div class=\"dsh\">Chosen champion</div>{}</div>", d.champion.as_ref().map(|l| row(l, 1, "champ")).unwrap_or_else(|| "<div class=\"dim pad\">Add a champion unit matching your legend</div>".into()));
    for (title, pred) in [("Units", "Unit"), ("Spells", "Spell"), ("Gear", "Gear")] {
        let mut items: Vec<&(String, u32)> = d.main.iter().filter(|(c, _)| db.get(c).map(|x| x.kind == pred).unwrap_or(false)).collect();
        if items.is_empty() {
            continue;
        }
        items.sort_by_key(|(c, _)| (db.get(c).and_then(|x| x.energy).unwrap_or(0), c.clone()));
        let n: u32 = items.iter().map(|e| e.1).sum();
        h += &format!("<div class=\"dsec\"><div class=\"dsh\">{title} <b>{n}</b></div>{}</div>", items.iter().map(|(c, n)| row(c, *n, "main")).collect::<String>());
    }
    h += &format!("<div class=\"dsec\"><div class=\"dsh\">Battlefields <b>{}/3</b></div>{}</div>", d.battlefields.len(), d.battlefields.iter().map(|b| row(b, 1, "bf")).collect::<String>());
    h += &format!("<div class=\"dsec\"><div class=\"dsh\">Runes <b>{}/12</b></div>{}</div>", d.rune_count(), d.runes.iter().map(|(c, n)| row(c, *n, "rune")).collect::<String>());
    if !d.side.is_empty() || e.side_mode {
        h += &format!("<div class=\"dsec\"><div class=\"dsh\">Sideboard <b>{}/8</b></div>{}</div>", d.side_count(), d.side.iter().map(|(c, n)| row(c, *n, "side")).collect::<String>());
    }
    let curve = {
        let mut buckets = [0u32; 8];
        for (c, n) in &d.main {
            if let Some(x) = db.get(c).and_then(|x| x.energy) {
                buckets[(x.max(0) as usize).min(7)] += n;
            }
        }
        let max = *buckets.iter().max().unwrap_or(&1).max(&1);
        buckets.iter().enumerate().map(|(i, b)| format!("<div class=\"bar\" title=\"{b}\"><div style=\"height:{}%\"></div><span>{}</span></div>", b * 100 / max, if i == 7 { "7+".into() } else { i.to_string() })).collect::<String>()
    };
    let status = if errs2.is_empty() { "<div class=\"valid ok\">Deck is legal</div>".to_string() } else { format!("<div class=\"valid bad\">{}</div>", errs2.iter().map(|x| format!("<div>{}</div>", esc(x))).collect::<String>()) };
    format!("<div class=\"dsum\"><span><b>{}</b> main</span><span><b>{}</b> runes</span><span><b>{}</b> side</span></div><div class=\"curve\">{curve}</div>{status}{h}", d.main_count(), d.rune_count(), d.side_count())
}

// ------------------------------------------------------------------ lobby

fn lobby_shell() -> String {
    r#"<div class="menu-bg"></div><div class="lobby"><div id="lobby-main" class="lobby-main"></div>
<aside class="panel chat"><h2>Chat</h2><div id="chat-lines" class="chat-lines"></div><div class="chat-in"><input id="chat-input" maxlength="300" placeholder="Message" data-enter="chat-send" autocomplete="off"><button class="btn sm" data-act="chat-send">Send</button></div></aside></div>"#
        .to_string()
}

fn lobby_main(app: &App) -> String {
    let Some(r) = &app.room else { return "<div class=\"empty\">Not in a room.</div>".into() };
    let is_host = r.host == app.my_id;
    let me_player = matches!(r.you, Role::Player(_));
    let me = match r.you {
        Role::Player(i) => r.players.get(i as usize),
        _ => None,
    };
    let seat = |i: usize| -> String {
        match r.players.get(i) {
            Some(p) => {
                let legend = p.legend.as_ref().and_then(|l| app.db.get(l));
                let cover = legend.map(|l| img_url(&l.img, 300)).unwrap_or_default();
                format!(
                    r#"<div class="seat {ready}"><div class="seat-art" style="background-image:url('{cover}')"></div><div class="seat-info"><div class="seat-name">{name}{host}{off}{left}</div><div class="seat-deck">{deck}</div><div class="seat-state">{state}</div>{errs}</div></div>"#,
                    ready = if p.ready { "ready" } else { "" },
                    host = if p.id == r.host { "<span class=\"tag\">Host</span>" } else { "" },
                    name = esc(&p.name),
                    off = if p.connected || p.left { "" } else { " <span class=\"dim\">(disconnected)</span>" },
                    left = if p.left { " <span class=\"dim\">(left, conceded)</span>" } else { "" },
                    deck = esc(p.deck_name.as_deref().unwrap_or("No deck selected")),
                    state = if p.ready { "<b class=\"ok\">Ready</b>" } else if p.deck_ok { "Deck loaded" } else if p.deck_name.is_some() { "<span class=\"bad\">Deck not legal</span>" } else { "Choosing deck…" },
                    errs = if !p.deck_errors.is_empty() && Some(p.id) == me.map(|m| m.id) { format!("<div class=\"seat-errs\">{}</div>", p.deck_errors.iter().take(4).map(|e| format!("<div>{}</div>", esc(e))).collect::<String>()) } else { String::new() },
                )
            }
            None => "<div class=\"seat empty-seat\"><div class=\"seat-info\"><div class=\"seat-name dim\">Open seat</div><div class=\"dim\">Waiting for a player</div></div></div>".to_string(),
        }
    };
    let s = &r.settings;
    let empty_seat = || "<div class=\"seat empty-seat\"><div class=\"seat-info\"><div class=\"seat-name dim\">Open seat</div><div class=\"dim\">Waiting for a player</div></div></div>".to_string();
    let seats_html = if s.mode == Mode::Teams {
        // Two team columns; a player can move to the other team while it has room.
        let my_team = me.map(|m| m.team);
        let col = |t: u8| -> String {
            let idx: Vec<usize> = (0..r.players.len()).filter(|&i| r.players[i].team == t).collect();
            let cards: String = (0..2).map(|k| idx.get(k).map(|&i| seat(i)).unwrap_or_else(empty_seat)).collect();
            let join = if me_player && !r.in_game && my_team != Some(t) && idx.len() < 2 { format!("<button class=\"btn sm\" data-act=\"set-team\" data-arg=\"{t}\">Join {}</button>", team_name(t)) } else { String::new() };
            format!("<div class=\"team\"><div class=\"team-h\"><h2>{}</h2>{join}</div>{cards}</div>", team_name(t))
        };
        format!("<div class=\"seats teams\">{}{}</div>", col(0), col(1))
    } else {
        let cls = if s.mode.seats() > 2 { "seats four" } else { "seats" };
        format!("<div class=\"{cls}\">{}</div>", (0..s.mode.seats()).map(|i| seat(i)).collect::<String>())
    };
    let multi = s.mode != Mode::Duel;
    let dis = if is_host { "" } else { "disabled" };
    let opt = |v: &str, cur: bool, label: &str| format!("<option value=\"{v}\" {}>{label}</option>", if cur { "selected" } else { "" });
    let settings = format!(
        r#"<section class="panel settings"><h2>Room settings {hosttag}</h2>
<label>Players<select id="rs-mode" data-change="room-settings" {dis}>{m1}{m2}{m3}</select></label>
<label>Match format<select id="rs-format" data-change="room-settings" {dis} {fdis}>{f1}{f2}</select></label>
<label>Deck rules<select id="rs-rules" data-change="room-settings" {dis}>{r1}{r2}{r3}</select></label>
<label>Points to win<input type="number" id="rs-points" min="1" max="30" value="{pts}" data-change="room-settings" {dis}></label>
<label class="chk"><input type="checkbox" id="rs-public" data-change="room-settings" {pub_} {dis}> Public room</label>
<label class="chk"><input type="checkbox" id="rs-spec" data-change="room-settings" {spec} {dis}> Allow spectators</label>
<label class="chk"><input type="checkbox" id="rs-hands" data-change="room-settings" {hands} {dis}> Spectators can see hands</label>
<label class="chk"><input type="checkbox" id="rs-auto" data-change="room-settings" {auto} {dis}> Automate start of turn (awaken, score, channel, draw)</label>
</section>"#,
        hosttag = if is_host { "" } else { "<span class=\"dim sm\">Only the host can change these</span>" },
        m1 = opt("duel", s.mode == Mode::Duel, "1v1"),
        m2 = opt("teams", s.mode == Mode::Teams, "2v2 (teams)"),
        m3 = format!("{}{}", opt("ffa3", s.mode == Mode::Skirmish, "Free-for-all (3 players)"), opt("ffa", s.mode == Mode::FreeForAll, "Free-for-all (4 players)")),
        fdis = if multi { "disabled title=\"2v2 and free-for-all are single games\"" } else { "" },
        f1 = opt("Bo1", s.format == Format::Bo1, "Best of 1"),
        f2 = opt("Bo3", s.format == Format::Bo3, "Best of 3"),
        r1 = opt("Standard", s.rules == DeckRules::Standard, "Standard"),
        r2 = opt("Unrestricted", s.rules == DeckRules::Unrestricted, "Unrestricted"),
        r3 = opt("Sealed", s.rules == DeckRules::Sealed, "Sealed"),
        pts = s.win_points,
        pub_ = if s.public { "checked" } else { "" },
        spec = if s.allow_spectators { "checked" } else { "" },
        hands = if s.spectators_see_hands { "checked" } else { "" },
        auto = if s.auto_steps { "checked" } else { "" },
    );
    let specs: String = if r.spectators.is_empty() { String::new() } else { format!("<span class=\"dim\">Watching:</span> {}", r.spectators.iter().map(|m| format!("<span class=\"spec\">{}</span>", esc(&m.name))).collect::<String>()) };
    let selected_deck = app.decks.iter().find(|d| me.and_then(|m| m.deck_name.as_ref()).map(|n| *n == d.deck.name).unwrap_or(false)).map(|d| d.id.clone());
    let controls = if me_player {
        format!(
            r#"<section class="panel deckpick"><h2>Your deck</h2><div class="row"><select id="lobby-deck">{opts}</select><button class="btn" data-act="lobby-use-deck">Use deck</button></div>
<details><summary>Paste a deck code or list</summary><textarea id="lobby-paste" rows="5"></textarea><button class="btn sm" data-act="lobby-paste">Use pasted deck</button></details>
<div class="row big"><button class="btn {rcls}" data-act="ready">{rlabel}</button><button class="btn ghost" data-act="toggle-spec">Switch to spectator</button><button class="btn ghost" data-act="lobby-goldfish">Goldfish this deck</button></div></section>"#,
            opts = deck_options(app, selected_deck.as_deref(), s.rules == DeckRules::Sealed),
            rcls = if me.map(|m| m.ready).unwrap_or(false) { "ghost" } else { "gold" },
            rlabel = if me.map(|m| m.ready).unwrap_or(false) { "Not ready" } else { "Ready" },
        )
    } else {
        format!("<section class=\"panel deckpick\"><h2>Spectating</h2><p class=\"dim\">{}</p><div class=\"row\"><button class=\"btn\" data-act=\"toggle-spec\" {}>Take a player seat</button></div></section>", if r.in_game { "The match is in progress." } else if multi { if s.mode.seats() == 3 { "The match starts when all three players are ready." } else { "The match starts when all four players are ready." } } else { "The match starts when both players are ready." }, if r.players.len() >= s.mode.seats() || r.in_game { "disabled" } else { "" })
    };
    format!(
        r#"<div class="room-head"><div><div class="dim">Room code</div><div class="room-code">{code}</div></div><div class="row"><button class="btn" data-act="copy" data-arg="{code}">Copy code</button><button class="btn" data-act="copy-invite" data-arg="{code}">Invite link</button><button class="btn" data-act="copy-spec" data-arg="{code}">Spectator link</button>{back}<button class="btn danger" data-act="leave-room">Leave</button></div></div>
{seats_html}<div class="specs">{specs}</div>
<div class="lobby-cols">{controls}{settings}</div>"#,
        code = esc(&r.code),
        back = if r.in_game { "<button class=\"btn gold\" data-act=\"to-game\">Return to match</button>" } else { "" },
    )
}

fn room_settings_from_inputs() -> RoomSettings {
    let mode = Mode::from_key(&input_value("rs-mode"));
    RoomSettings {
        mode,
        format: if input_value("rs-format") == "Bo3" && mode == Mode::Duel { Format::Bo3 } else { Format::Bo1 },
        rules: match input_value("rs-rules").as_str() {
            "Unrestricted" => DeckRules::Unrestricted,
            "Sealed" => DeckRules::Sealed,
            _ => DeckRules::Standard,
        },
        public: dom::checked("rs-public"),
        allow_spectators: dom::checked("rs-spec"),
        spectators_see_hands: dom::checked("rs-hands"),
        auto_steps: dom::checked("rs-auto"),
        win_points: input_value("rs-points").parse().unwrap_or(8),
    }
}

// ------------------------------------------------------------------ handlers

pub fn open_editor(app: &mut App, saved: Option<SavedDeck>, pool: Option<(SealedRef, std::collections::HashMap<String, u32>)>) {
    let (id, deck, sealed_ref) = match saved {
        Some(s) => (Some(s.id), s.deck, s.sealed),
        None => (None, Deck { name: "New deck".into(), ..Default::default() }, None),
    };
    let pool_map = match (&pool, &sealed_ref) {
        (Some((_, m)), _) => Some(m.clone()),
        (None, Some(s)) => Some(sealed::open_pool(&app.db, &s.sets, &s.seed).cards()),
        _ => None,
    };
    let sealed_ref = pool.map(|p| p.0).or(sealed_ref);
    app.editor = EditorState { deck_id: id, deck, sealed: sealed_ref, pool: pool_map, limit: 120, ..Default::default() };
    app.go(Screen::Editor);
}

fn editor_add(app: &mut App, code: &str, to_side: bool) {
    let db = app.db.clone();
    let Some(c) = db.get(code) else { return };
    let e = &mut app.editor;
    if let Some(pool) = &e.pool {
        if !(c.is_rune() && !c.alt) {
            let have = pool.get(code).copied().unwrap_or(0);
            let used = count_in_deck(&e.deck, code) + e.deck.side.iter().filter(|s| s.0 == code).map(|s| s.1).sum::<u32>();
            if used >= have {
                app.toast("No more copies of that card in your pool.", true);
                return;
            }
        }
    }
    if to_side && c.is_main_deck_card() {
        e.deck.add_side(code, 1);
    } else {
        e.deck.add_card(&db, code, 1);
    }
    e.dirty = true;
    render_editor_parts(app);
}

fn save_editor(app: &mut App) {
    let name = input_value("ed-name");
    let e = &mut app.editor;
    e.deck.name = if name.trim().is_empty() { "Unnamed deck".into() } else { name.trim().to_string() };
    let id = e.deck_id.clone().unwrap_or_else(new_id);
    e.deck_id = Some(id.clone());
    let sd = SavedDeck { id: id.clone(), deck: e.deck.clone(), sealed: e.sealed.clone(), updated: js_sys::Date::now() };
    match app.decks.iter_mut().find(|d| d.id == id) {
        Some(d) => *d = sd,
        None => app.decks.insert(0, sd),
    }
    app.editor.dirty = false;
    app.save_decks();
    app.toast("Deck saved.", false);
}

fn find_deck_choice(app: &App, select_id: &str) -> Option<SavedDeck> {
    let id = input_value(select_id);
    app.deck_by_id(&id).cloned().or_else(|| app.decks.first().cloned())
}

/// Returns true when the action was handled here.
pub fn handle(app: &mut App, act: &str, arg: &str, shift: bool) -> bool {
    if crate::name::handle(app, act) {
        return true;
    }
    let db = app.db.clone();
    match act {
        "tab" => {
            let t = match arg {
                "Solo" => Tab::Solo,
                "Sealed" => Tab::Sealed,
                "Deck" | "Decks" => Tab::Decks,
                "Rooms" => Tab::Rooms,
                _ => Tab::Duel,
            };
            if app.screen == Screen::Lobby && app.room.is_some() {
                // keep room membership; user can return through the Duel tab banner
            }
            app.go(Screen::Home(t));
        }
        "copy" | "copy-seed" => {
            dom::copy_text(arg);
            app.toast(if act == "copy" { "Room code copied." } else { "Seed copied." }, false);
        }
        "copy-invite" => {
            dom::copy_text(&format!("{}/room/{arg}", dom::origin()));
            app.toast("Invite link copied.", false);
        }
        "copy-spec" => {
            dom::copy_text(&format!("{}/room/{arg}?spectate=1", dom::origin()));
            app.toast("Spectator link copied.", false);
        }
        "close-modal" => {
            if let Some(g) = app.game.as_mut() {
                g.modal = None;
                g.menu = None;
            }
            app.import_open = false;
            app.render();
        }
        // ---- duel
        "create-room" => {
            let mode = Mode::from_key(&input_value("cr-mode"));
            let settings = RoomSettings {
                mode,
                win_points: mode.default_points(),
                format: if input_value("cr-format") == "Bo3" && mode == Mode::Duel { Format::Bo3 } else { Format::Bo1 },
                rules: match input_value("cr-rules").as_str() {
                    "Unrestricted" => DeckRules::Unrestricted,
                    "Sealed" => DeckRules::Sealed,
                    _ => DeckRules::Standard,
                },
                public: dom::checked("cr-public"),
                allow_spectators: dom::checked("cr-spec"),
                ..Default::default()
            };
            crate::name::ask(app, ClientMsg::CreateRoom { settings });
        }
        "join-room" | "spectate-room" => {
            let code = input_value("join-code").trim().to_uppercase();
            if code.len() != 6 {
                app.toast("Room codes are 6 characters.", true);
            } else {
                crate::name::ask(app, ClientMsg::JoinRoom { code, spectate: act == "spectate-room" });
            }
        }
        "join-code" => crate::name::ask(app, ClientMsg::JoinRoom { code: arg.into(), spectate: false }),
        "spec-code" => crate::name::ask(app, ClientMsg::JoinRoom { code: arg.into(), spectate: true }),
        "rooms-refresh" => net::send(&ClientMsg::ListRooms),
        "queue" => {
            let mode = input_value("q-mode");
            let (f, r) = mode.split_once('-').unwrap_or(("Bo1", "Standard"));
            let mode = QueueMode { format: if f == "Bo3" { Format::Bo3 } else { Format::Bo1 }, rules: if r == "Unrestricted" { DeckRules::Unrestricted } else { DeckRules::Standard } };
            let Some(d) = app.home_deck().cloned() else {
                app.toast("Choose or import a deck first.", true);
                return true;
            };
            let errs = d.deck.validate(&db, mode.rules);
            if !errs.is_empty() {
                app.toast(&format!("Paste a valid deck before searching for a match. {}", errs[0]), true);
                return true;
            }
            crate::name::ask(app, ClientMsg::Queue { mode, deck: d.deck, sealed: None });
        }
        "cancel-queue" => net::send(&ClientMsg::CancelQueue),
        "home-deck" => {
            app.set_home_deck(arg);
            app.render();
        }
        // ---- solo
        "goldfish" | "goldfish-id" | "lobby-goldfish" => {
            let d = match act {
                "goldfish-id" => app.deck_by_id(arg).cloned(),
                "lobby-goldfish" => find_deck_choice(app, "lobby-deck"),
                _ => find_deck_choice(app, "gf-deck"),
            };
            let Some(d) = d else {
                app.toast("Choose or import a deck first.", true);
                return true;
            };
            let errs = d.deck.validate(&db, DeckRules::Unrestricted);
            if !errs.is_empty() {
                app.toast(&format!("Load a valid deck before starting solo play. {}", errs[0]), true);
                return true;
            }
            let skip = act != "goldfish" || dom::checked("gf-skip");
            app.start_local(vec![d.deck], vec![app.name.clone()], skip, d.sealed);
        }
        "solo-play" => {
            let (Some(a), Some(b)) = (find_deck_choice(app, "sp-deck1"), find_deck_choice(app, "sp-deck2")) else {
                app.toast("Choose two decks.", true);
                return true;
            };
            for d in [&a, &b] {
                let errs = d.deck.validate(&db, DeckRules::Unrestricted);
                if !errs.is_empty() {
                    app.toast(&format!("{}: {}", d.deck.name, errs[0]), true);
                    return true;
                }
            }
            app.start_local(vec![a.deck, b.deck], vec!["Player 1".into(), "Player 2".into()], dom::checked("sp-skip"), None);
        }
        // ---- sealed
        "pack-add" => {
            if app.sealed.sets.len() < 12 {
                let s = app.sealed.sets.last().cloned().unwrap_or("OGN".into());
                app.sealed.sets.push(s);
            }
            app.render();
        }
        "pack-remove" => {
            if app.sealed.sets.len() > 1 {
                app.sealed.sets.pop();
            }
            app.render();
        }
        "sealed-open" => {
            let mut seed = input_value("sealed-seed").trim().to_string();
            if seed.is_empty() {
                seed = format!("{:06X}", (js_sys::Math::random() * 16777215.0) as u32);
            }
            if app.sealed.sets.is_empty() {
                app.toast("A sealed mix must open at least one pack.", true);
                return true;
            }
            app.sealed.seed = seed.clone();
            app.sealed.pool = Some(sealed::open_pool(&db, &app.sealed.sets, &seed));
            app.render();
        }
        "sealed-build" | "sealed-auto" => {
            let Some(p) = app.sealed.pool.clone() else { return true };
            let r = SealedRef { sets: p.sets.clone(), seed: p.seed.clone() };
            if act == "sealed-auto" {
                if let Some(mut d) = sealed::auto_build(&db, &p) {
                    d.name = format!("Sealed {}", p.seed);
                    let sd = SavedDeck { id: new_id(), deck: d, sealed: Some(r.clone()), updated: js_sys::Date::now() };
                    open_editor(app, Some(sd), Some((r, p.cards())));
                }
            } else {
                let deck = Deck { name: format!("Sealed {}", p.seed), ..Default::default() };
                let sd = SavedDeck { id: new_id(), deck, sealed: Some(r.clone()), updated: 0.0 };
                open_editor(app, Some(sd), Some((r, p.cards())));
            }
        }
        // ---- decks
        "import-toggle" => {
            app.import_open = !app.import_open;
            app.render();
            dom::focus("imp-text");
        }
        "import-deck" => {
            let text = input_value("imp-text");
            match Deck::import(&db, &text) {
                Ok(mut d) => {
                    let n = input_value("imp-name");
                    d.name = if n.trim().is_empty() { d.legend.as_ref().and_then(|l| db.get(l)).map(|l| format!("{} (imported)", l.name)).unwrap_or("Imported deck".into()) } else { n.trim().into() };
                    app.decks.insert(0, SavedDeck { id: new_id(), deck: d, sealed: None, updated: js_sys::Date::now() });
                    app.save_decks();
                    app.import_open = false;
                    app.toast("Deck imported.", false);
                    app.render();
                }
                Err(e) => app.toast(&e, true),
            }
        }
        "deck-new" => open_editor(app, None, None),
        "deck-edit" => {
            if let Some(d) = app.deck_by_id(arg).cloned() {
                open_editor(app, Some(d), None);
            }
        }
        "deck-code" => {
            if let Some(d) = app.deck_by_id(arg) {
                match d.deck.to_code() {
                    Ok(c) => {
                        dom::copy_text(&c);
                        app.toast("Deck code copied.", false);
                    }
                    Err(e) => app.toast(&e, true),
                }
            }
        }
        "deck-text" => {
            if let Some(d) = app.deck_by_id(arg) {
                dom::copy_text(&d.deck.to_text(&db));
                app.toast("Deck list copied.", false);
            }
        }
        "deck-dup" => {
            if let Some(mut d) = app.deck_by_id(arg).cloned() {
                d.id = new_id();
                d.deck.name = format!("{} (copy)", d.deck.name);
                app.decks.insert(0, d);
                app.save_decks();
                app.render();
            }
        }
        "deck-del" => {
            app.decks.retain(|d| d.id != arg);
            app.save_decks();
            app.render();
        }
        "deck-starter" => {
            let l = input_value("starter-legend");
            match starter_deck(&db, &l) {
                Some(d) => {
                    app.decks.insert(0, SavedDeck { id: new_id(), deck: d, sealed: None, updated: js_sys::Date::now() });
                    app.save_decks();
                    app.render();
                }
                None => app.toast("Riot has no preconstructed deck for that legend.", true),
            }
        }
        // ---- editor
        "ed-add" => editor_add(app, arg, shift || app.editor.side_mode),
        "ed-more" => {
            app.editor.limit += 120;
            render_editor_parts(app);
        }
        "ed-inc" | "ed-dec" => {
            let (sec, code) = arg.split_once('|').unwrap_or(("main", arg));
            if act == "ed-inc" {
                editor_add(app, code, sec == "side");
            } else {
                if sec == "side" {
                    app.editor.deck.remove_side(code);
                } else {
                    app.editor.deck.remove_card(code);
                }
                render_editor_parts(app);
            }
        }
        "ed-champ" => {
            let d = &mut app.editor.deck;
            if let Some(old) = d.champion.take() {
                d.main.push((old, 1));
            }
            d.remove_card(arg);
            d.champion = Some(arg.to_string());
            // merge duplicate entries created by the swap
            let mut merged: Vec<(String, u32)> = vec![];
            for (c, n) in d.main.drain(..) {
                match merged.iter_mut().find(|e| e.0 == c) {
                    Some(e) => e.1 += n,
                    None => merged.push((c, n)),
                }
            }
            d.main = merged;
            render_editor_parts(app);
        }
        "ed-save" => {
            save_editor(app);
            render_editor_parts(app);
        }
        "ed-code" => match app.editor.deck.to_code() {
            Ok(c) => {
                dom::copy_text(&c);
                app.toast("Deck code copied.", false);
            }
            Err(e) => app.toast(&e, true),
        },
        "ed-text" => {
            dom::copy_text(&app.editor.deck.to_text(&db));
            app.toast("Deck list copied.", false);
        }
        "ed-back" => {
            if app.editor.dirty {
                save_editor(app);
            }
            let to = if app.room.is_some() { Screen::Lobby } else if app.editor.pool.is_some() { Screen::Home(Tab::Sealed) } else { Screen::Home(Tab::Decks) };
            app.go(to);
        }
        // ---- lobby
        "leave-room" => {
            net::send(&ClientMsg::LeaveRoom);
        }
        "to-room" => {
            let live = app.room.as_ref().is_some_and(|r| r.in_game) && app.game.as_ref().is_some_and(|g| !g.is_local());
            app.go(if live { Screen::Game } else { Screen::Lobby });
        }
        "to-game" => {
            if app.game.is_some() {
                app.go(Screen::Game);
            }
        }
        "lobby-use-deck" => {
            if let Some(d) = find_deck_choice(app, "lobby-deck") {
                net::send(&ClientMsg::SetDeck { deck: d.deck, sealed: d.sealed });
            }
        }
        "lobby-paste" => match Deck::import(&db, &input_value("lobby-paste")) {
            Ok(mut d) => {
                if d.name.is_empty() {
                    d.name = "Pasted deck".into();
                }
                net::send(&ClientMsg::SetDeck { deck: d, sealed: None });
            }
            Err(e) => app.toast(&e, true),
        },
        "ready" => {
            let ready = app.room.as_ref().and_then(|r| match r.you {
                Role::Player(i) => r.players.get(i as usize).map(|p| p.ready),
                _ => None,
            });
            net::send(&ClientMsg::SetReady { ready: !ready.unwrap_or(false) });
        }
        "toggle-spec" => net::send(&ClientMsg::ToggleSpectator),
        "set-team" => net::send(&ClientMsg::SetTeam { team: arg.parse().unwrap_or(0) }),
        "chat-send" => {
            let t = input_value("chat-input");
            if !t.trim().is_empty() {
                net::send(&ClientMsg::Chat { text: t });
                dom::set_input_value("chat-input", "");
            }
        }
        _ => return false,
    }
    true
}

pub fn on_change(app: &mut App, key: &str, val: &str, _el: &web_sys::Element) {
    match key {
        "room-settings" => {
            let mut settings = room_settings_from_inputs();
            // Switching modes moves an untouched points target to the new mode's default (8 or 11).
            if let Some(cur) = app.room.as_ref().map(|r| r.settings.clone()) {
                if cur.mode != settings.mode && cur.win_points == cur.mode.default_points() {
                    settings.win_points = settings.mode.default_points();
                }
            }
            net::send(&ClientMsg::UpdateSettings { settings });
        }
        "ed-type" => {
            app.editor.ftype = val.into();
            render_editor_parts(app);
        }
        "ed-dom" => {
            app.editor.fdomain = val.into();
            render_editor_parts(app);
        }
        "ed-set" => {
            app.editor.fset = val.into();
            render_editor_parts(app);
        }
        "ed-cost" => {
            app.editor.fcost = val.into();
            render_editor_parts(app);
        }
        "ed-side" => {
            app.editor.side_mode = val == "1";
            render_editor_parts(app);
        }
        "pack-all" => {
            for s in app.sealed.sets.iter_mut() {
                *s = val.to_string();
            }
            app.render();
        }
        k if k.starts_with("pack-") => {
            if let Ok(i) = k[5..].parse::<usize>() {
                if let Some(s) = app.sealed.sets.get_mut(i) {
                    *s = val.to_string();
                }
            }
        }
        _ => {}
    }
}

pub fn on_input(app: &mut App, key: &str, val: &str) {
    match key {
        "ed-q" => {
            app.editor.query = val.to_string();
            app.editor.limit = 120;
            render_editor_parts(app);
        }
        "ed-name" => {
            app.editor.deck.name = val.to_string();
            app.editor.dirty = true;
        }
        _ => {}
    }
}
