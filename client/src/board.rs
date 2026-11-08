//! Table layout, the animated 3D card scene, picking and the table texture.
//! Layout uses a viewer frame: the viewing seat sits at +z (near side).

use crate::gfx::{canvas_2d, Renderer};
use crate::math::*;
use rift_shared::cards::CardDb;
use rift_shared::game::{CardId, CardInst, GameState, Mode, Zone};
use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI};
use web_sys::HtmlCanvasElement;

pub const CW: f32 = 1.0;
pub const CH: f32 = 1.4;
pub const RUNE_SCALE: f32 = 0.78;
pub const TABLE_X: f32 = 8.8;
pub const TABLE_Z: f32 = 6.7;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub cx: f32,
    pub cz: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, z: f32) -> bool {
        (x - self.cx).abs() <= self.w / 2.0 && (z - self.cz).abs() <= self.h / 2.0
    }
    fn mirror(self) -> Rect {
        Rect { cx: -self.cx, cz: -self.cz, ..self }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Base,
    Runes,
    Legend,
    Champion,
    Deck,
    Trash,
    RuneDeck,
    Banish,
    Lane(u8),
}

pub struct Layout {
    pub nbf: usize,
    /// Seat drawn at the near side.
    pub me: u8,
    pub seats: u8,
    /// 2v2 / free-for-all: how the boards sit around the table.
    pub mode: Mode,
}

/// Where a seat's board sits. `col` is in the board's own frame
/// (-1 left, +1 right, 0 full width); far boards are turned 180°.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeatPos {
    pub near: bool,
    pub col: f32,
}

/// Four-seat boards: half the table each, centred at x = ±QUAD_BOARD_X.
const QUAD_BOARD_X: f32 = 6.0;
const QUAD_TABLE_X: f32 = 12.0;

impl Layout {
    /// Three or four boards (2v2 or free-for-all): two boards share a half of the table.
    pub fn quad(&self) -> bool {
        self.seats > 2
    }
    pub fn table_x(&self) -> f32 {
        if self.quad() {
            QUAD_TABLE_X
        } else {
            TABLE_X
        }
    }
    /// Camera distance factor so the wider four-player table fits.
    pub fn zoom(&self) -> f32 {
        self.table_x() / TABLE_X
    }
    pub fn bf_x(&self, b: u8) -> f32 {
        let span = if self.quad() { 14.4 } else { 9.0 };
        match self.nbf {
            0 | 1 => 0.0,
            2 if !self.quad() => [-2.55, 2.55][b as usize % 2],
            _ => -span / 2.0 + span * (b as f32) / ((self.nbf - 1) as f32),
        }
    }
    fn lane_w(&self) -> f32 {
        if self.quad() {
            return 3.4;
        }
        match self.nbf {
            0 | 1 => 9.0,
            2 => 4.8,
            _ => 9.4 / self.nbf as f32,
        }
    }
    /// Width of the showdown highlight over a battlefield column.
    pub fn column_w(&self) -> f32 {
        if self.quad() {
            self.lane_w() * 2.0 + 0.2
        } else {
            self.lane_w() + 0.2
        }
    }
    pub fn bf_rect(&self, b: u8) -> Rect {
        Rect { cx: self.bf_x(b), cz: 0.0, w: 1.85, h: 1.35 }
    }
    /// Board position of a seat, or None if it isn't drawn (goldfish).
    pub fn seat_pos(&self, s: u8) -> Option<SeatPos> {
        if s >= self.seats {
            return None;
        }
        if !self.quad() {
            if self.seats < 2 && s != self.me {
                return None;
            }
            return Some(SeatPos { near: s == self.me, col: 0.0 });
        }
        let n = self.seats;
        let r = (s + n - self.me % n) % n;
        if n == 3 {
            // Three players: your board is centred, the next player top-left, the last top-right.
            return Some(match r {
                0 => SeatPos { near: true, col: 0.0 },
                1 => SeatPos { near: false, col: 1.0 },
                _ => SeatPos { near: false, col: -1.0 },
            });
        }
        // 2v2: your team on the near half, the next opponent across from you.
        // Free-for-all: seats run clockwise around the table.
        let (near, col) = match (self.mode, r) {
            (_, 0) => (true, -1.0),
            (Mode::Teams, 2) => (true, 1.0),
            (Mode::Teams, 3) => (false, -1.0),
            (_, 1) => (false, 1.0),
            (_, 2) => (false, -1.0),
            _ => (true, 1.0),
        };
        Some(SeatPos { near, col })
    }
    pub fn slot_rect(&self, slot: Slot, p: SeatPos) -> Rect {
        let r = if self.quad() {
            let bx = p.col * QUAD_BOARD_X;
            match slot {
                Slot::Base => Rect { cx: bx - 0.6, cz: 3.8, w: 7.0, h: 1.75 },
                Slot::Runes => Rect { cx: bx - 0.6, cz: 5.6, w: 7.0, h: 1.35 },
                Slot::Legend => Rect { cx: bx - 5.0, cz: 3.8, w: 1.3, h: 1.7 },
                Slot::Champion => Rect { cx: bx - 5.0, cz: 5.6, w: 1.3, h: 1.7 },
                Slot::Deck => Rect { cx: bx + 3.75, cz: 3.8, w: 1.25, h: 1.65 },
                Slot::Trash => Rect { cx: bx + 5.1, cz: 3.8, w: 1.25, h: 1.65 },
                Slot::RuneDeck => Rect { cx: bx + 3.75, cz: 5.6, w: 1.25, h: 1.65 },
                Slot::Banish => Rect { cx: bx + 5.1, cz: 5.6, w: 1.25, h: 1.65 },
                // A centred board (three players) has the whole battlefield column.
                Slot::Lane(b) if p.col == 0.0 => Rect { cx: self.bf_x(b), cz: 1.78, w: self.lane_w() * 2.0, h: 1.7 },
                Slot::Lane(b) => Rect { cx: self.bf_x(b) + p.col * 1.75, cz: 1.78, w: self.lane_w(), h: 1.7 },
            }
        } else {
            match slot {
                Slot::Base => Rect { cx: 0.0, cz: 3.8, w: 9.8, h: 1.75 },
                Slot::Runes => Rect { cx: 0.0, cz: 5.6, w: 9.8, h: 1.35 },
                Slot::Legend => Rect { cx: -6.45, cz: 3.8, w: 1.3, h: 1.7 },
                Slot::Champion => Rect { cx: -6.45, cz: 5.6, w: 1.3, h: 1.7 },
                Slot::Deck => Rect { cx: 6.2, cz: 3.8, w: 1.25, h: 1.65 },
                Slot::Trash => Rect { cx: 7.65, cz: 3.8, w: 1.25, h: 1.65 },
                Slot::RuneDeck => Rect { cx: 6.2, cz: 5.6, w: 1.25, h: 1.65 },
                Slot::Banish => Rect { cx: 7.65, cz: 5.6, w: 1.25, h: 1.65 },
                Slot::Lane(b) => Rect { cx: self.bf_x(b), cz: 1.78, w: self.lane_w(), h: 1.7 },
            }
        };
        if p.near {
            return r;
        }
        match slot {
            // Lanes stay beside their battlefield; only the side offset turns around.
            Slot::Lane(b) => Rect { cx: 2.0 * self.bf_x(b) - r.cx, cz: -r.cz, ..r },
            _ => r.mirror(),
        }
    }
    /// The seat whose plate and hand sit opposite in a 1v1.
    pub fn other(&self) -> u8 {
        if self.seats > 1 {
            1 - self.me.min(1)
        } else {
            self.me
        }
    }
    pub fn seat_near(&self, s: u8) -> bool {
        self.seat_pos(s).map(|p| p.near).unwrap_or(false)
    }
    fn zone_for(slot: Slot, s: u8) -> Zone {
        match slot {
            Slot::Base => Zone::Base(s),
            Slot::Runes => Zone::Runes(s),
            Slot::Legend => Zone::Legend(s),
            Slot::Champion => Zone::Champion(s),
            Slot::Deck => Zone::Deck(s),
            Slot::Trash => Zone::Trash(s),
            Slot::RuneDeck => Zone::RuneDeck(s),
            Slot::Banish => Zone::Banish(s),
            Slot::Lane(b) => Zone::Bf(b, s),
        }
    }
    pub fn slots(&self) -> Vec<Slot> {
        let mut v = vec![Slot::Base, Slot::Runes, Slot::Legend, Slot::Champion, Slot::Deck, Slot::Trash, Slot::RuneDeck, Slot::Banish];
        for b in 0..self.nbf {
            v.push(Slot::Lane(b as u8));
        }
        v
    }
    /// Seats with a board on the table, viewer first.
    pub fn shown_seats(&self) -> Vec<(u8, SeatPos)> {
        let n = self.seats.max(1);
        (0..n).map(|k| (self.me + k) % n).filter_map(|s| self.seat_pos(s).map(|p| (s, p))).collect()
    }
    /// Zone under a table point (for drops / clicks on empty zones).
    pub fn zone_at(&self, x: f32, z: f32) -> Option<Zone> {
        for b in 0..self.nbf as u8 {
            if self.bf_rect(b).contains(x, z) {
                return Some(Zone::BfCard(b));
            }
        }
        for (s, p) in self.shown_seats() {
            for slot in self.slots() {
                if self.slot_rect(slot, p).contains(x, z) {
                    return Some(Self::zone_for(slot, s));
                }
            }
        }
        None
    }
    pub fn rect_for_zone(&self, z: Zone) -> Option<Rect> {
        let slot = match z {
            Zone::Base(_) => Slot::Base,
            Zone::Runes(_) => Slot::Runes,
            Zone::Legend(_) => Slot::Legend,
            Zone::Champion(_) => Slot::Champion,
            Zone::Deck(_) => Slot::Deck,
            Zone::Trash(_) => Slot::Trash,
            Zone::RuneDeck(_) => Slot::RuneDeck,
            Zone::Banish(_) => Slot::Banish,
            Zone::Bf(b, _) => Slot::Lane(b),
            Zone::BfCard(b) => return Some(self.bf_rect(b)),
            _ => return None,
        };
        let p = self.seat_pos(z.seat()?)?;
        Some(self.slot_rect(slot, p))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub pos: V3,
    pub rot: f32,
    pub face_up: bool,
    pub w: f32,
    pub h: f32,
}

/// Target placement for every card that lives on the 3D table.
pub fn layout_cards(g: &GameState, l: &Layout) -> HashMap<CardId, Place> {
    let mut out = HashMap::new();
    let row = |out: &mut HashMap<CardId, Place>, ids: &[CardId], r: Rect, scale: f32, near: bool| {
        let n = ids.len();
        if n == 0 {
            return;
        }
        let (w, h) = (CW * scale, CH * scale);
        let widths: Vec<f32> = ids.iter().map(|&id| if g.cards[id as usize].exhausted { h } else { w }).collect();
        let gap = 0.1 * scale;
        let total: f32 = widths.iter().sum::<f32>() + gap * (n as f32 - 1.0);
        let squeeze = if total > r.w { (r.w - widths[0]) / (total - widths[0]).max(0.01) } else { 1.0 };
        let total_s = if total > r.w { r.w } else { total };
        let dir = if near { 1.0 } else { -1.0 };
        let mut x = r.cx - dir * total_s / 2.0;
        for (i, &id) in ids.iter().enumerate() {
            let c = &g.cards[id as usize];
            let cx = x + dir * widths[i] / 2.0 * if i == 0 { 1.0 } else { 1.0 };
            let rot = if c.exhausted { -FRAC_PI_2 } else { 0.0 };
            out.insert(id, Place { pos: v3(cx, 0.02 + i as f32 * 0.004, r.cz), rot, face_up: !c.code.is_empty() && !c.face_down, w, h });
            x += dir * (widths[i] + gap) * squeeze;
        }
    };
    let stack = |out: &mut HashMap<CardId, Place>, ids: &[CardId], r: Rect, face: bool| {
        for (i, &id) in ids.iter().enumerate() {
            let c = &g.cards[id as usize];
            let y = 0.02 + (i as f32 * 0.012).min(0.5);
            let up = face && !c.code.is_empty() && !c.face_down || (c.shown && !c.code.is_empty());
            out.insert(id, Place { pos: v3(r.cx, y, r.cz), rot: if c.exhausted { -FRAC_PI_2 } else { 0.0 }, face_up: up, w: CW, h: CH });
        }
    };
    for (s, p) in l.shown_seats() {
        let near = p.near;
        row(&mut out, g.pile(Zone::Base(s)), l.slot_rect(Slot::Base, p), 1.0, near);
        row(&mut out, g.pile(Zone::Runes(s)), l.slot_rect(Slot::Runes, p), RUNE_SCALE, near);
        stack(&mut out, g.pile(Zone::Legend(s)), l.slot_rect(Slot::Legend, p), true);
        stack(&mut out, g.pile(Zone::Champion(s)), l.slot_rect(Slot::Champion, p), true);
        stack(&mut out, g.pile(Zone::Deck(s)), l.slot_rect(Slot::Deck, p), false);
        stack(&mut out, g.pile(Zone::RuneDeck(s)), l.slot_rect(Slot::RuneDeck, p), false);
        stack(&mut out, g.pile(Zone::Trash(s)), l.slot_rect(Slot::Trash, p), true);
        stack(&mut out, g.pile(Zone::Banish(s)), l.slot_rect(Slot::Banish, p), true);
        for b in 0..l.nbf as u8 {
            row(&mut out, g.pile(Zone::Bf(b, s)), l.slot_rect(Slot::Lane(b), p), 0.92, near);
        }
    }
    for b in 0..l.nbf as u8 {
        let r = l.bf_rect(b);
        for &id in g.pile(Zone::BfCard(b)) {
            out.insert(id, Place { pos: v3(r.cx, 0.02, r.cz), rot: 0.0, face_up: true, w: 1.74, h: 1.24 });
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
pub struct Anim {
    pub pos: V3,
    pub rot: f32,
    pub flip: f32,
    pub w: f32,
    pub h: f32,
    pub alive: bool,
}

#[derive(Default)]
pub struct Scene {
    pub anims: HashMap<CardId, Anim>,
    pub order: Vec<CardId>,
    pub table_canvas: Option<HtmlCanvasElement>,
    pub table_key: String,
}

pub struct DrawOpts<'a> {
    pub hover: Option<CardId>,
    pub selected: Option<CardId>,
    pub drag: Option<(CardId, f32, f32)>,
    pub drop_zone: Option<Zone>,
    pub ping: Option<CardId>,
    pub focus_bf: Option<u8>,
    pub controllers: &'a [Option<u8>],
    /// Destinations to pick from for a pending action, and the one under the pointer.
    pub targets: &'a [Zone],
    pub target_hover: Option<Zone>,
    pub db: &'a CardDb,
}

impl Scene {
    pub fn update(&mut self, g: &GameState, l: &Layout, dt: f32) {
        let targets = layout_cards(g, l);
        // Cards ease into place; with reduced motion they jump there.
        let t = if crate::dom::reduced_motion() { 1.0 } else { 1.0 - (-dt * 11.0).exp() };
        for a in self.anims.values_mut() {
            a.alive = false;
        }
        for (&id, p) in &targets {
            let flip_t = if p.face_up { 0.0 } else { PI };
            let entry = self.anims.entry(id).or_insert_with(|| {
                // New on the table: fly in from the owner's hand side.
                let owner_near = l.seat_near(g.cards[id as usize].owner);
                let from = if matches!(g.cards[id as usize].zone, Zone::BfCard(_)) { p.pos.add(v3(0.0, 2.0, 0.0)) } else { v3(if l.quad() { p.pos.x } else { 0.0 }, 2.5, if owner_near { 8.0 } else { -8.0 }) };
                Anim { pos: from, rot: p.rot, flip: flip_t, w: p.w, h: p.h, alive: true }
            });
            entry.alive = true;
            entry.pos = entry.pos.lerp(p.pos, t);
            entry.rot = lerp_angle(entry.rot, p.rot, t);
            entry.flip += (flip_t - entry.flip) * t;
            entry.w += (p.w - entry.w) * t;
            entry.h += (p.h - entry.h) * t;
        }
        self.anims.retain(|_, a| a.alive);
        let mut order: Vec<CardId> = self.anims.keys().copied().collect();
        order.sort_by(|a, b| self.anims[a].pos.y.partial_cmp(&self.anims[b].pos.y).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b)));
        self.order = order;
    }

    pub fn ensure_table(&mut self, r: &mut Renderer, l: &Layout) {
        let key = format!("{}-{}-{}-{:?}", l.nbf, l.seats, l.me, l.mode);
        if self.table_key != key {
            self.table_canvas = Some(paint_table(l));
            self.table_key = key.clone();
        }
        if let Some(c) = &self.table_canvas {
            r.set_table(&key, c);
        }
    }

    pub fn card_model(a: &Anim, lift: f32) -> M4 {
        let flip_scale = a.flip.cos().abs().max(0.02);
        let y = a.pos.y + lift + a.flip.sin().abs() * 0.7;
        let m = mul(&translate(v3(a.pos.x, y, a.pos.z)), &rot_y(a.rot));
        mul(&m, &scale(a.w * flip_scale, 1.0, a.h))
    }

    pub fn draw(&self, r: &Renderer, g: &GameState, l: &Layout, o: &DrawOpts) {
        let db = o.db;
        // table
        if let Some(t) = &r.table {
            let tx = l.table_x();
            let m = mul(&translate(v3(0.0, 0.0, 0.0)), &scale(tx * 2.0, 1.0, TABLE_Z * 2.0));
            r.draw(&m, t, 2, [1.0, 1.0, 1.0, 1.0], (tx * 2.0, TABLE_Z * 2.0));
        }
        // battlefield controller tint + showdown focus
        for b in 0..l.nbf as u8 {
            let rect = l.bf_rect(b);
            if let Some(Some(c)) = o.controllers.get(b as usize) {
                let col = side_color(l, *c);
                let m = mul(&translate(v3(rect.cx, 0.006, rect.cz)), &scale(rect.w + 0.35, 1.0, rect.h + 0.35));
                r.draw(&m, &r.white, 4, col, (rect.w + 0.35, rect.h + 0.35));
            }
            if o.focus_bf == Some(b) {
                let w = l.column_w();
                let m = mul(&translate(v3(rect.cx, 0.008, 0.0)), &scale(w, 1.0, 5.4));
                r.draw(&m, &r.white, 4, [0.85, 0.7, 0.36, 0.45], (w, 5.4));
            }
        }
        for &z in o.targets {
            if let Some(rect) = l.rect_for_zone(z) {
                // Pulsing lime frames; the one under the pointer is solid.
                let on = o.target_hover == Some(z);
                let a = if on { 1.0 } else { 0.55 + 0.3 * (r.time * 4.0).sin().abs() };
                let col = if on { [0.8, 1.0, 0.0, 1.0] } else { [0.8, 1.0, 0.0, a] };
                let m = mul(&translate(v3(rect.cx, 0.012, rect.cz)), &scale(rect.w + 0.12, 1.0, rect.h + 0.12));
                r.draw(&m, &r.white, 4, col, (rect.w + 0.12, rect.h + 0.12));
                if on {
                    r.draw(&m, &r.white, 4, [1.0, 1.0, 1.0, 0.35], (rect.w + 0.12, rect.h + 0.12));
                }
            }
        }
        if let Some(z) = o.drop_zone {
            if let Some(rect) = l.rect_for_zone(z) {
                let m = mul(&translate(v3(rect.cx, 0.01, rect.cz)), &scale(rect.w + 0.1, 1.0, rect.h + 0.1));
                r.draw(&m, &r.white, 4, [0.8, 1.0, 0.0, 1.0], (rect.w + 0.1, rect.h + 0.1));
            }
        }
        let dragging = o.drag.map(|d| d.0);
        for &id in &self.order {
            if Some(id) == dragging {
                continue;
            }
            self.draw_card(r, g, db, id, &self.anims[&id], o, 0.0);
        }
        if let Some((id, x, z)) = o.drag {
            if let Some(a) = self.anims.get(&id) {
                let mut a = *a;
                a.pos = v3(x, 0.9, z);
                a.rot = 0.0;
                self.draw_card(r, g, db, id, &a, o, 0.0);
            }
        }
    }

    fn draw_card(&self, r: &Renderer, g: &GameState, db: &CardDb, id: CardId, a: &Anim, o: &DrawOpts, lift0: f32) {
        let c = &g.cards[id as usize];
        let hovered = o.hover == Some(id);
        let lift = lift0 + if hovered { 0.12 } else { 0.0 };
        // shadow
        let sm = mul(&mul(&translate(v3(a.pos.x + 0.05, 0.004, a.pos.z + 0.08)), &rot_y(a.rot)), &scale(a.w + 0.3, 1.0, a.h + 0.3));
        r.draw(&sm, &r.white, 3, [0.0, 0.0, 0.0, if a.pos.y > 0.3 { 0.5 } else { 1.0 }], (a.w + 0.3, a.h + 0.3));
        let sel = o.selected == Some(id);
        let pinged = o.ping == Some(id);
        if sel || hovered || pinged {
            let col = if pinged { [1.0, 0.8, 0.2, 1.0] } else if sel { [0.8, 1.0, 0.0, 1.0] } else { [1.0, 1.0, 1.0, 0.45] };
            let gm = mul(&mul(&translate(v3(a.pos.x, a.pos.y + lift - 0.002, a.pos.z)), &rot_y(a.rot)), &scale(a.w + 0.5, 1.0, a.h + 0.5));
            r.draw(&gm, &r.white, 1, col, (a.w + 0.5, a.h + 0.5));
        }
        let front = a.flip < FRAC_PI_2;
        let m = Self::card_model(a, lift);
        let def = db.get(&c.code);
        let tex = if front && !c.code.is_empty() {
            def.and_then(|d| r.texture(&d.code, &img_url(&d.img, 360), &d.name, d.land))
        } else {
            None
        };
        let is_rune = def.map(|d| d.is_rune()).unwrap_or(false) || matches!(c.zone, Zone::RuneDeck(_) | Zone::Runes(_));
        let back = if is_rune { &r.rune_back } else { &r.back };
        let dim = if c.exhausted && c.zone.is_board() { 0.72 } else { 1.0 };
        let tint = if c.stunned { [0.7 * dim, 0.75 * dim, 1.0 * dim, 1.0] } else { [dim, dim, dim, 1.0] };
        match tex {
            Some(t) => r.draw(&m, &t, 0, tint, (a.w, a.h)),
            None => r.draw(&m, back, 0, tint, (a.w, a.h)),
        }
    }

    /// Topmost card under the pointer.
    pub fn pick(&self, r: &Renderer, sx: f64, sy: f64) -> Option<CardId> {
        for &id in self.order.iter().rev() {
            let a = &self.anims[&id];
            let Some((x, z)) = r.pick_plane(sx, sy, a.pos.y) else { continue };
            let (dx, dz) = (x - a.pos.x, z - a.pos.z);
            let (s, c) = (-a.rot).sin_cos();
            let lx = dx * c + dz * s;
            let lz = -dx * s + dz * c;
            if lx.abs() <= a.w / 2.0 && lz.abs() <= a.h / 2.0 {
                return Some(id);
            }
        }
        None
    }
}

/// Controller ring colour: blue for you (and a 2v2 teammate), red for opponents.
/// Free-for-all opponents get their own colours so you can tell them apart.
pub fn side_color(l: &Layout, s: u8) -> [f32; 4] {
    match side_class(l, s) {
        "me" | "ally" => [0.24, 0.61, 1.0, 0.9],
        "op2" => [0.95, 0.62, 0.2, 0.9],
        "op3" => [0.72, 0.42, 1.0, 0.9],
        _ => [1.0, 0.35, 0.35, 0.9],
    }
}

/// CSS class for a seat relative to the viewer: me, ally, op, op2, op3.
pub fn side_class(l: &Layout, s: u8) -> &'static str {
    if s == l.me {
        return "me";
    }
    if l.mode == Mode::Teams && s % 2 == l.me % 2 {
        return "ally";
    }
    if l.mode.is_ffa() && l.quad() {
        let n = l.seats.max(1);
        return match (s + n - l.me % n) % n {
            1 => "op",
            2 => "op2",
            _ => "op3",
        };
    }
    "op"
}

pub fn img_url(src: &str, w: u32) -> String {
    if src.is_empty() {
        return String::new();
    }
    format!("/img?u={}&w={w}", crate::dom::encode_uri(src))
}

pub fn card_img(c: &CardInst, db: &CardDb, w: u32) -> Option<String> {
    db.get(&c.code).map(|d| img_url(&d.img, w))
}

/// Paint the play mat: zone frames and labels, Master Duel style glow lines.
fn paint_table(l: &Layout) -> HtmlCanvasElement {
    let s = 120.0f64;
    let tx = l.table_x();
    let (w, h) = ((tx as f64 * 2.0 * s) as u32, (TABLE_Z as f64 * 2.0 * s) as u32);
    let (c, x) = canvas_2d(w, h);
    let (fw, fh) = (w as f64, h as f64);
    let px = |v: f32| (v as f64 + tx as f64) * s;
    let pz = |v: f32| (v as f64 + TABLE_Z as f64) * s;
    // Deep blue cloth mat (the menu field colour): soft centre light plus fine noise.
    let g = x.create_radial_gradient(fw / 2.0, fh / 2.0, 40.0, fw / 2.0, fh / 2.0, fw * 0.6).unwrap();
    let _ = g.add_color_stop(0.0, "#1a2c4c");
    let _ = g.add_color_stop(0.7, "#0c1830");
    let _ = g.add_color_stop(1.0, "#040a16");
    x.set_fill_style_canvas_gradient(&g);
    x.fill_rect(0.0, 0.0, fw, fh);
    let mut seed: u32 = 0x9e37_79b9;
    for _ in 0..26000 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let px_ = (seed % w) as f64;
        let py_ = ((seed / w) % h) as f64;
        let a = if seed & 1 == 0 { "rgba(255,255,255,0.025)" } else { "rgba(0,0,0,0.12)" };
        x.set_fill_style_str(a);
        x.fill_rect(px_, py_, 2.0, 2.0);
    }
    // centre line between the two halves
    x.set_stroke_style_str("rgba(255,255,255,0.10)");
    x.set_line_width(2.0);
    x.begin_path();
    x.move_to(0.0, fh / 2.0);
    x.line_to(fw, fh / 2.0);
    x.stroke();
    let frame = |rect: Rect, col: &str, label: &str, big: bool| {
        let (x0, y0, ww, hh) = (px(rect.cx - rect.w / 2.0), pz(rect.cz - rect.h / 2.0), rect.w as f64 * s, rect.h as f64 * s);
        let r = 14.0f64.min(hh / 4.0);
        x.begin_path();
        x.move_to(x0 + r, y0);
        x.line_to(x0 + ww - r, y0);
        x.quadratic_curve_to(x0 + ww, y0, x0 + ww, y0 + r);
        x.line_to(x0 + ww, y0 + hh - r);
        x.quadratic_curve_to(x0 + ww, y0 + hh, x0 + ww - r, y0 + hh);
        x.line_to(x0 + r, y0 + hh);
        x.quadratic_curve_to(x0, y0 + hh, x0, y0 + hh - r);
        x.line_to(x0, y0 + r);
        x.quadratic_curve_to(x0, y0, x0 + r, y0);
        x.set_fill_style_str("rgba(0,0,0,0.28)");
        x.fill();
        x.set_stroke_style_str(col);
        x.set_line_width(if big { 3.0 } else { 2.5 });
        x.stroke();
        if !label.is_empty() {
            x.set_fill_style_str("rgba(220,228,240,0.45)");
            x.set_font(if big { "600 34px 'Barlow Condensed', sans-serif" } else { "600 24px 'Barlow Condensed', sans-serif" });
            x.set_text_align("center");
            x.set_text_baseline("middle");
            let _ = x.fill_text(label, x0 + ww / 2.0, y0 + hh / 2.0);
        }
    };
    let names = |slot: Slot| match slot {
        Slot::Base => "Base".to_string(),
        Slot::Runes => "Rune pool".into(),
        Slot::Legend => "Legend".into(),
        Slot::Champion => "Champion".into(),
        Slot::Deck => "Main deck".into(),
        Slot::Trash => "Trash".into(),
        Slot::RuneDeck => "Rune deck".into(),
        Slot::Banish => "Banishment".into(),
        Slot::Lane(_) => String::new(),
    };
    if l.quad() {
        // split line between the two boards on a half (the near half has one board with three players)
        x.set_stroke_style_str("rgba(255,255,255,0.07)");
        x.begin_path();
        x.move_to(fw / 2.0, 0.0);
        x.line_to(fw / 2.0, pz(-1.0 * 2.7));
        if l.seats > 3 {
            x.move_to(fw / 2.0, pz(2.7));
            x.line_to(fw / 2.0, fh);
        }
        x.stroke();
    }
    for (_, p) in l.shown_seats() {
        let col = if p.near { "rgba(200,212,230,0.55)" } else { "rgba(200,212,230,0.40)" };
        for slot in l.slots() {
            let big = matches!(slot, Slot::Base | Slot::Runes | Slot::Lane(_));
            frame(l.slot_rect(slot, p), col, &names(slot), big);
        }
    }
    for b in 0..l.nbf as u8 {
        let rect = l.bf_rect(b);
        frame(rect, "rgba(216,180,92,0.75)", "", true);
        x.set_fill_style_str("rgba(216,180,92,0.7)");
        x.set_font("700 44px 'Barlow Condensed', sans-serif");
        x.set_text_align("center");
        let _ = x.fill_text(&((b'A' + b) as char).to_string(), px(rect.cx - rect.w / 2.0 - 0.35), pz(0.0));
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zones_resolve_and_mirror() {
        let l = Layout { nbf: 2, me: 0, seats: 2, mode: Mode::Duel };
        assert_eq!(l.zone_at(0.0, 3.8), Some(Zone::Base(0)));
        assert_eq!(l.zone_at(0.0, -3.8), Some(Zone::Base(1)));
        assert_eq!(l.zone_at(-2.55, 1.8), Some(Zone::Bf(0, 0)));
        assert_eq!(l.zone_at(-2.55, -1.8), Some(Zone::Bf(0, 1)));
        assert_eq!(l.zone_at(2.55, 0.0), Some(Zone::BfCard(1)));
        assert_eq!(l.zone_at(6.2, 3.8), Some(Zone::Deck(0)));
        assert_eq!(l.zone_at(-6.2, -3.8), Some(Zone::Deck(1)));
        let l1 = Layout { nbf: 2, me: 1, seats: 2, mode: Mode::Duel };
        assert_eq!(l1.zone_at(0.0, 3.8), Some(Zone::Base(1)));
    }

    #[test]
    fn four_seat_boards() {
        // 2v2 from seat 1: teammate (seat 3) beside me, opponents across.
        let l = Layout { nbf: 3, me: 1, seats: 4, mode: Mode::Teams };
        assert_eq!(l.zone_at(-6.6, 3.8), Some(Zone::Base(1)));
        assert_eq!(l.zone_at(5.4, 3.8), Some(Zone::Base(3)));
        assert_eq!(l.zone_at(-5.4, -3.8), Some(Zone::Base(2)), "next opponent sits across from me");
        assert_eq!(l.zone_at(6.6, -3.8), Some(Zone::Base(0)));
        assert_eq!(side_class(&l, 3), "ally");
        assert_eq!(side_class(&l, 0), "op");
        // Lanes stay under their battlefield; each seat has its own half of the column.
        for b in 0..3u8 {
            let x = l.bf_x(b);
            assert_eq!(l.zone_at(x, 0.0), Some(Zone::BfCard(b)));
            assert_eq!(l.zone_at(x - 1.75, 1.8), Some(Zone::Bf(b, 1)));
            assert_eq!(l.zone_at(x + 1.75, 1.8), Some(Zone::Bf(b, 3)));
            assert_eq!(l.zone_at(x - 1.75, -1.8), Some(Zone::Bf(b, 2)));
            assert_eq!(l.zone_at(x + 1.75, -1.8), Some(Zone::Bf(b, 0)));
        }
        // Free-for-all runs clockwise: next seat top-left, then top-right, then bottom-right.
        let f = Layout { nbf: 3, me: 0, seats: 4, mode: Mode::FreeForAll };
        assert_eq!(f.zone_at(-5.4, -3.8), Some(Zone::Base(1)));
        assert_eq!(f.zone_at(6.6, -3.8), Some(Zone::Base(2)));
        assert_eq!(f.zone_at(5.4, 3.8), Some(Zone::Base(3)));
        assert_eq!(f.shown_seats().len(), 4);
        // Three players: my board centred, the other two across.
        let t = Layout { nbf: 2, me: 2, seats: 3, mode: Mode::Skirmish };
        assert_eq!(t.zone_at(-0.6, 3.8), Some(Zone::Base(2)));
        assert_eq!(t.zone_at(-5.4, -3.8), Some(Zone::Base(0)));
        assert_eq!(t.zone_at(6.6, -3.8), Some(Zone::Base(1)));
        assert_eq!(t.zone_at(t.bf_x(0) - 3.0, 1.8), Some(Zone::Bf(0, 2)), "a centred board has the whole lane");
        assert_eq!(side_class(&t, 0), "op");
        assert_eq!(side_class(&t, 1), "op2");
        for (s, p) in t.shown_seats() {
            for slot in t.slots() {
                let r = t.slot_rect(slot, p);
                assert_eq!(t.zone_at(r.cx, r.cz), Some(Layout::zone_for(slot, s)), "{slot:?} of seat {s}");
            }
        }
        // Every zone of every seat resolves back to itself.
        for (s, p) in f.shown_seats() {
            for slot in f.slots() {
                let r = f.slot_rect(slot, p);
                assert_eq!(f.zone_at(r.cx, r.cz), Some(Layout::zone_for(slot, s)), "{slot:?} of seat {s}");
                assert!(r.cx.abs() + r.w / 2.0 <= f.table_x());
            }
        }
    }
}
